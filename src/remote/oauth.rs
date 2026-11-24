//! Minimal OAuth 2.1 authorization server for the desktop MCP endpoint:
//! dynamic client registration, authorization code + PKCE (S256 only), refresh
//! tokens, and a PIN pairing step that proves the person at the PC approved it.
//!
//! Why the PIN: redirect URIs are limited to the known chat hosts, so a stranger
//! cannot steal a code, but they could register a client of their *own* ChatGPT or
//! Claude account and try to pair it with this PC. Approval therefore needs a PIN
//! that is shown only inside this program, works once and expires quickly.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs, io,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

const PIN_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const MAX_CLIENTS: usize = 200;
const MAX_PENDING: usize = 100;
pub const SCOPE: &str = "desktop";

#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub allowed_redirect_hosts: Vec<String>,
    pub allow_localhost: bool,
    pub access_ttl: u64,
    pub refresh_ttl: u64,
    pub code_ttl: u64,
    pub request_ttl: u64,
    pub pairing_ttl: u64,
    pub max_pin_attempts: u32,
}
impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            allowed_redirect_hosts: [
                "claude.ai",
                "claude.com",
                "chatgpt.com",
                "chat.openai.com",
                "platform.openai.com",
            ]
            .map(String::from)
            .to_vec(),
            allow_localhost: false,
            access_ttl: 3600,
            refresh_ttl: 14 * 24 * 3600,
            code_ttl: 60,
            request_ttl: 600,
            pairing_ttl: 300,
            max_pin_attempts: 5,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct OAuthError {
    pub status: u16,
    pub code: &'static str,
    pub description: String,
}
impl OAuthError {
    fn new(status: u16, code: &'static str, description: impl Into<String>) -> Self {
        Self {
            status,
            code,
            description: description.into(),
        }
    }
    pub fn to_json(&self) -> Value {
        json!({ "error": self.code, "error_description": self.description })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApproveError {
    UnknownRequest,
    PairingClosed,
    WrongPin { attempts_left: u32 },
    Locked,
}

#[derive(Clone, Serialize, Deserialize)]
struct Client {
    name: String,
    redirect_uris: Vec<String>,
    created: u64,
}
#[derive(Clone)]
struct PendingRequest {
    client_id: String,
    redirect_uri: String,
    state: Option<String>,
    code_challenge: String,
    created: u64,
}
struct AuthCode {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    expires: u64,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum TokenKind {
    Access,
    Refresh,
}
#[derive(Clone, Serialize, Deserialize)]
struct TokenRecord {
    client_id: String,
    expires: u64,
    kind: TokenKind,
}
struct Pairing {
    pin: String,
    expires: u64,
    attempts: u32,
}
#[derive(Default)]
struct Inner {
    clients: HashMap<String, Client>,
    pending: HashMap<String, PendingRequest>,
    codes: HashMap<String, AuthCode>,
    tokens: HashMap<String, TokenRecord>,
    pairing: Option<Pairing>,
}
/// Only hashes of tokens are written to disk, never the tokens themselves.
#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    clients: HashMap<String, Client>,
    tokens: HashMap<String, TokenRecord>,
}

pub struct AuthServer {
    inner: Mutex<Inner>,
    config: AuthConfig,
    store: Option<PathBuf>,
}

pub struct AuthorizePage {
    pub request_id: String,
    pub client_name: String,
    pub redirect_host: String,
}
pub struct TokenInfo {
    pub client_name: String,
}
pub struct PairingView {
    pub pin: String,
    pub remaining_secs: u64,
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    // A broken system RNG must never fall back to predictable tokens.
    getrandom::fill(&mut bytes).expect("system random number generator unavailable");
    bytes
}
fn random_token<const N: usize>() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<N>())
}
fn hash_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    ct_eq(computed.as_bytes(), challenge.as_bytes())
}
fn normalize_pin(pin: &str) -> String {
    pin.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}
fn new_pin() -> String {
    let raw: String = random_bytes::<8>()
        .iter()
        .map(|byte| char::from(PIN_ALPHABET[usize::from(byte & 31)]))
        .collect();
    format!("{}-{}", &raw[..4], &raw[4..])
}

pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    index += 2;
                } else {
                    out.push(b'%');
                }
            }
            other => out.push(other),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
/// Query strings and `application/x-www-form-urlencoded` bodies.
pub fn parse_form(text: &str) -> HashMap<String, String> {
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn authority_of(uri: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = uri.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    Some((scheme, authority))
}
pub fn redirect_host(uri: &str) -> String {
    authority_of(uri)
        .map(|(_, authority)| authority.to_owned())
        .unwrap_or_default()
}
fn redirect_allowed(uri: &str, config: &AuthConfig) -> bool {
    let Some((scheme, authority)) = authority_of(uri) else {
        return false;
    };
    if authority.contains('@') || uri.contains('#') || uri.len() > 2000 {
        return false;
    }
    match scheme {
        "https" => {
            !authority.contains(':')
                && config
                    .allowed_redirect_hosts
                    .iter()
                    .any(|host| host.eq_ignore_ascii_case(authority))
        }
        "http" => {
            let host = authority.split(':').next().unwrap_or_default();
            config.allow_localhost && matches!(host, "localhost" | "127.0.0.1")
        }
        _ => false,
    }
}
fn append_query(uri: &str, query: &str) -> String {
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{query}")
}

impl AuthServer {
    pub fn new(config: AuthConfig, store: Option<PathBuf>, now: u64) -> Self {
        let mut inner = Inner::default();
        if let Some(persisted) = store
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|raw| serde_json::from_str::<Persisted>(&raw).ok())
        {
            inner.clients = persisted.clients;
            inner.tokens = persisted.tokens;
            inner.tokens.retain(|_, record| record.expires > now);
        }
        Self {
            inner: Mutex::new(inner),
            config,
            store,
        }
    }
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A poisoned lock only means another request panicked; the data is still valid.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn persist(&self, inner: &Inner) {
        let Some(path) = &self.store else {
            return;
        };
        let persisted = Persisted {
            clients: inner.clients.clone(),
            tokens: inner.tokens.clone(),
        };
        let write = || -> io::Result<()> {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let temp = path.with_extension("json.tmp");
            fs::write(&temp, serde_json::to_vec(&persisted)?)?;
            fs::rename(temp, path)
        };
        // Losing persistence only means chats must reconnect after a restart.
        let _ = write();
    }
    fn purge(&self, inner: &mut Inner, now: u64) {
        let request_ttl = self.config.request_ttl;
        inner
            .pending
            .retain(|_, request| now.saturating_sub(request.created) < request_ttl);
        inner.codes.retain(|_, code| code.expires > now);
        inner.tokens.retain(|_, record| record.expires > now);
        if inner
            .pairing
            .as_ref()
            .is_some_and(|pairing| pairing.expires <= now)
        {
            inner.pairing = None;
        }
    }

    pub fn register_client(&self, body: &Value, now: u64) -> Result<Value, OAuthError> {
        let uris: Vec<String> = body
            .get("redirect_uris")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if uris.is_empty() || uris.len() > 5 {
            return Err(OAuthError::new(
                400,
                "invalid_redirect_uri",
                "Informe de 1 a 5 redirect_uris.",
            ));
        }
        if let Some(bad) = uris.iter().find(|uri| !redirect_allowed(uri, &self.config)) {
            return Err(OAuthError::new(
                400,
                "invalid_redirect_uri",
                format!("redirect_uri não permitido: {}", redirect_host(bad)),
            ));
        }
        let name: String = body
            .get("client_name")
            .and_then(Value::as_str)
            .unwrap_or("Cliente MCP")
            .chars()
            .filter(|c| !c.is_control())
            .take(60)
            .collect();
        let mut inner = self.lock();
        self.purge(&mut inner, now);
        if inner.clients.len() >= MAX_CLIENTS {
            return Err(OAuthError::new(
                429,
                "temporarily_unavailable",
                "Clientes demais.",
            ));
        }
        let client_id = random_token::<16>();
        inner.clients.insert(
            client_id.clone(),
            Client {
                name: name.clone(),
                redirect_uris: uris.clone(),
                created: now,
            },
        );
        self.persist(&inner);
        Ok(json!({
            "client_id": client_id,
            "client_id_issued_at": now,
            "client_name": name,
            "redirect_uris": uris,
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "scope": SCOPE,
        }))
    }

    pub fn begin_authorize(
        &self,
        params: &HashMap<String, String>,
        now: u64,
    ) -> Result<AuthorizePage, OAuthError> {
        let get = |key: &str| params.get(key).map(String::as_str).unwrap_or_default();
        let mut inner = self.lock();
        self.purge(&mut inner, now);
        let client = inner
            .clients
            .get(get("client_id"))
            .cloned()
            .ok_or_else(|| OAuthError::new(400, "invalid_client", "Cliente desconhecido."))?;
        let redirect_uri = get("redirect_uri");
        if !client.redirect_uris.iter().any(|uri| uri == redirect_uri) {
            return Err(OAuthError::new(
                400,
                "invalid_request",
                "redirect_uri não registrado.",
            ));
        }
        if get("response_type") != "code" {
            return Err(OAuthError::new(
                400,
                "unsupported_response_type",
                "Use response_type=code.",
            ));
        }
        let challenge = get("code_challenge");
        if get("code_challenge_method") != "S256" || !(43..=128).contains(&challenge.len()) {
            return Err(OAuthError::new(
                400,
                "invalid_request",
                "PKCE com S256 é obrigatório.",
            ));
        }
        if inner.pending.len() >= MAX_PENDING {
            return Err(OAuthError::new(
                429,
                "temporarily_unavailable",
                "Pedidos demais.",
            ));
        }
        let request_id = random_token::<16>();
        inner.pending.insert(
            request_id.clone(),
            PendingRequest {
                client_id: get("client_id").to_owned(),
                redirect_uri: redirect_uri.to_owned(),
                state: params
                    .get("state")
                    .filter(|state| !state.is_empty())
                    .cloned(),
                code_challenge: challenge.to_owned(),
                created: now,
            },
        );
        Ok(AuthorizePage {
            request_id,
            client_name: client.name,
            redirect_host: redirect_host(redirect_uri),
        })
    }

    /// What the approval page shows for a request that is still waiting.
    pub fn request_info(&self, request_id: &str, now: u64) -> Option<AuthorizePage> {
        let mut inner = self.lock();
        self.purge(&mut inner, now);
        let request = inner.pending.get(request_id)?;
        let client = inner.clients.get(&request.client_id)?;
        Some(AuthorizePage {
            request_id: request_id.to_owned(),
            client_name: client.name.clone(),
            redirect_host: redirect_host(&request.redirect_uri),
        })
    }

    pub fn open_pairing(&self, now: u64) -> PairingView {
        let mut inner = self.lock();
        let pairing = Pairing {
            pin: new_pin(),
            expires: now + self.config.pairing_ttl,
            attempts: 0,
        };
        let view = PairingView {
            pin: pairing.pin.clone(),
            remaining_secs: self.config.pairing_ttl,
        };
        inner.pairing = Some(pairing);
        view
    }
    pub fn close_pairing(&self) {
        self.lock().pairing = None;
    }
    pub fn pairing(&self, now: u64) -> Option<PairingView> {
        let mut inner = self.lock();
        self.purge(&mut inner, now);
        inner.pairing.as_ref().map(|pairing| PairingView {
            pin: pairing.pin.clone(),
            remaining_secs: pairing.expires.saturating_sub(now),
        })
    }

    /// Checks the PIN for a pending request and, if right, returns the URL to send
    /// the browser to (the chat's callback with the authorization code).
    pub fn approve(&self, request_id: &str, pin: &str, now: u64) -> Result<String, ApproveError> {
        let mut inner = self.lock();
        self.purge(&mut inner, now);
        if !inner.pending.contains_key(request_id) {
            return Err(ApproveError::UnknownRequest);
        }
        let max_attempts = self.config.max_pin_attempts;
        let Some(pairing) = inner.pairing.as_mut() else {
            return Err(ApproveError::PairingClosed);
        };
        if !ct_eq(
            normalize_pin(pin).as_bytes(),
            normalize_pin(&pairing.pin).as_bytes(),
        ) {
            pairing.attempts += 1;
            if pairing.attempts >= max_attempts {
                inner.pairing = None;
                return Err(ApproveError::Locked);
            }
            return Err(ApproveError::WrongPin {
                attempts_left: max_attempts - pairing.attempts,
            });
        }
        inner.pairing = None;
        let request = inner
            .pending
            .remove(request_id)
            .ok_or(ApproveError::UnknownRequest)?;
        let code = random_token::<32>();
        inner.codes.insert(
            hash_hex(&code),
            AuthCode {
                client_id: request.client_id,
                redirect_uri: request.redirect_uri.clone(),
                code_challenge: request.code_challenge,
                expires: now + self.config.code_ttl,
            },
        );
        let mut query = format!("code={code}");
        if let Some(state) = &request.state {
            query.push_str(&format!("&state={}", percent_encode(state)));
        }
        Ok(append_query(&request.redirect_uri, &query))
    }

    /// The user declined on the approval page: send the browser back with an error.
    pub fn deny(&self, request_id: &str) -> Option<String> {
        let request = self.lock().pending.remove(request_id)?;
        let mut query = "error=access_denied".to_owned();
        if let Some(state) = &request.state {
            query.push_str(&format!("&state={}", percent_encode(state)));
        }
        Some(append_query(&request.redirect_uri, &query))
    }

    pub fn token(&self, form: &HashMap<String, String>, now: u64) -> Result<Value, OAuthError> {
        let get = |key: &str| form.get(key).map(String::as_str).unwrap_or_default();
        let mut inner = self.lock();
        self.purge(&mut inner, now);
        let client_id = match get("grant_type") {
            "authorization_code" => {
                let code = inner.codes.remove(&hash_hex(get("code"))).ok_or_else(|| {
                    OAuthError::new(400, "invalid_grant", "Código inválido ou expirado.")
                })?;
                let verifier = get("code_verifier");
                let verifier_ok = (43..=128).contains(&verifier.len())
                    && verifier
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'));
                if code.client_id != get("client_id")
                    || code.redirect_uri != get("redirect_uri")
                    || !verifier_ok
                    || !pkce_matches(verifier, &code.code_challenge)
                {
                    return Err(OAuthError::new(
                        400,
                        "invalid_grant",
                        "Verificação PKCE falhou.",
                    ));
                }
                code.client_id
            }
            "refresh_token" => {
                let key = hash_hex(get("refresh_token"));
                let record = inner
                    .tokens
                    .get(&key)
                    .filter(|record| record.kind == TokenKind::Refresh)
                    .cloned()
                    .ok_or_else(|| {
                        OAuthError::new(400, "invalid_grant", "Refresh token inválido.")
                    })?;
                if !get("client_id").is_empty() && get("client_id") != record.client_id {
                    return Err(OAuthError::new(
                        400,
                        "invalid_grant",
                        "Refresh token de outro cliente.",
                    ));
                }
                // Rotation: a used refresh token never works twice.
                inner.tokens.remove(&key);
                record.client_id
            }
            _ => {
                return Err(OAuthError::new(
                    400,
                    "unsupported_grant_type",
                    "Use authorization_code ou refresh_token.",
                ))
            }
        };
        let access = random_token::<32>();
        let refresh = random_token::<32>();
        inner.tokens.insert(
            hash_hex(&access),
            TokenRecord {
                client_id: client_id.clone(),
                expires: now + self.config.access_ttl,
                kind: TokenKind::Access,
            },
        );
        inner.tokens.insert(
            hash_hex(&refresh),
            TokenRecord {
                client_id,
                expires: now + self.config.refresh_ttl,
                kind: TokenKind::Refresh,
            },
        );
        self.persist(&inner);
        Ok(json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": self.config.access_ttl,
            "refresh_token": refresh,
            "scope": SCOPE,
        }))
    }

    pub fn validate(&self, bearer: &str, now: u64) -> Option<TokenInfo> {
        let inner = self.lock();
        let record = inner.tokens.get(&hash_hex(bearer))?;
        if record.kind != TokenKind::Access || record.expires <= now {
            return None;
        }
        let client = inner.clients.get(&record.client_id)?;
        Some(TokenInfo {
            client_name: client.name.clone(),
        })
    }

    /// Names of chats that currently hold a usable refresh token.
    pub fn connected_clients(&self, now: u64) -> Vec<String> {
        let inner = self.lock();
        let mut names: Vec<String> = inner
            .tokens
            .values()
            .filter(|record| record.kind == TokenKind::Refresh && record.expires > now)
            .filter_map(|record| inner.clients.get(&record.client_id))
            .map(|client| client.name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Disconnects every chat: all tokens, codes, pending requests and clients.
    pub fn revoke_all(&self) {
        let mut inner = self.lock();
        inner.clients.clear();
        inner.tokens.clear();
        inner.codes.clear();
        inner.pending.clear();
        inner.pairing = None;
        self.persist(&inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    const REDIRECT: &str = "https://chatgpt.com/connector_platform_oauth_redirect";
    const NOW: u64 = 10_000;

    fn server() -> AuthServer {
        AuthServer::new(AuthConfig::default(), None, NOW)
    }
    fn register(server: &AuthServer) -> String {
        let reply = server
            .register_client(
                &json!({"client_name": "ChatGPT", "redirect_uris": [REDIRECT]}),
                NOW,
            )
            .expect("register");
        reply["client_id"].as_str().unwrap().to_owned()
    }
    fn authorize_params(client_id: &str) -> HashMap<String, String> {
        parse_form(&format!(
            "response_type=code&client_id={client_id}&redirect_uri={}&code_challenge={CHALLENGE}&code_challenge_method=S256&state=abc%20123",
            percent_encode(REDIRECT)
        ))
    }
    fn code_from(redirect: &str) -> String {
        parse_form(redirect.split_once('?').unwrap().1)["code"].clone()
    }
    fn token_form(client_id: &str, code: &str) -> HashMap<String, String> {
        parse_form(&format!(
            "grant_type=authorization_code&code={code}&client_id={client_id}&redirect_uri={}&code_verifier={VERIFIER}",
            percent_encode(REDIRECT)
        ))
    }
    /// Runs the whole flow and returns the token reply.
    fn connect(server: &AuthServer, client_id: &str) -> Value {
        let page = server
            .begin_authorize(&authorize_params(client_id), NOW)
            .unwrap();
        let pin = server.open_pairing(NOW).pin;
        let redirect = server.approve(&page.request_id, &pin, NOW + 1).unwrap();
        server
            .token(&token_form(client_id, &code_from(&redirect)), NOW + 2)
            .unwrap()
    }

    #[test]
    fn pkce_follows_the_rfc_example() {
        assert!(pkce_matches(VERIFIER, CHALLENGE));
        assert!(!pkce_matches("x", CHALLENGE));
    }
    #[test]
    fn full_flow_issues_a_working_token() {
        let server = server();
        let client_id = register(&server);
        let tokens = connect(&server, &client_id);
        let access = tokens["access_token"].as_str().unwrap();
        let info = server.validate(access, NOW + 3).expect("valid token");
        assert_eq!(info.client_name, "ChatGPT");
        assert_eq!(tokens["token_type"], "Bearer");
        assert_eq!(server.connected_clients(NOW + 3), ["ChatGPT"]);
    }
    #[test]
    fn redirect_is_limited_to_known_chat_hosts() {
        let server = server();
        for bad in [
            "https://evil.example/cb",
            "https://chatgpt.com.evil.example/cb",
            "https://user@chatgpt.com/cb",
            "https://chatgpt.com:8443/cb",
            "http://chatgpt.com/cb",
            "http://localhost:3000/cb",
            "https://chatgpt.com/cb#frag",
            "javascript:alert(1)",
        ] {
            let reply = server.register_client(&json!({"redirect_uris": [bad]}), NOW);
            assert_eq!(reply.unwrap_err().code, "invalid_redirect_uri", "{bad}");
        }
        for good in [
            "https://claude.ai/api/mcp/auth_callback",
            "https://chatgpt.com/connector_platform_oauth_redirect",
        ] {
            assert!(server
                .register_client(&json!({"redirect_uris": [good]}), NOW)
                .is_ok());
        }
        let local = AuthServer::new(
            AuthConfig {
                allow_localhost: true,
                ..AuthConfig::default()
            },
            None,
            NOW,
        );
        assert!(local
            .register_client(&json!({"redirect_uris": ["http://localhost:6274/cb"]}), NOW)
            .is_ok());
    }
    #[test]
    fn registration_needs_redirect_uris() {
        let server = server();
        assert!(server.register_client(&json!({}), NOW).is_err());
        assert!(server
            .register_client(&json!({"redirect_uris": []}), NOW)
            .is_err());
    }
    #[test]
    fn authorize_checks_client_redirect_and_pkce() {
        let server = server();
        let client_id = register(&server);
        let mut params = authorize_params(&client_id);
        assert!(server.begin_authorize(&params, NOW).is_ok());
        params.insert("redirect_uri".into(), "https://chatgpt.com/other".into());
        assert_eq!(
            server.begin_authorize(&params, NOW).err().unwrap().code,
            "invalid_request"
        );
        let mut unknown = authorize_params("nope");
        assert_eq!(
            server.begin_authorize(&unknown, NOW).err().unwrap().code,
            "invalid_client"
        );
        unknown = authorize_params(&client_id);
        unknown.insert("code_challenge_method".into(), "plain".into());
        assert!(server.begin_authorize(&unknown, NOW).is_err());
        let mut no_pkce = authorize_params(&client_id);
        no_pkce.remove("code_challenge");
        assert!(server.begin_authorize(&no_pkce, NOW).is_err());
    }
    #[test]
    fn approval_needs_an_open_pairing_and_the_right_pin() {
        let server = server();
        let client_id = register(&server);
        let page = server
            .begin_authorize(&authorize_params(&client_id), NOW)
            .unwrap();
        assert_eq!(
            server.approve(&page.request_id, "AAAA-AAAA", NOW),
            Err(ApproveError::PairingClosed)
        );
        let pin = server.open_pairing(NOW).pin;
        assert!(matches!(
            server.approve(&page.request_id, "ZZZZ-ZZZZ", NOW),
            Err(ApproveError::WrongPin { attempts_left: 4 })
        ));
        assert_eq!(
            server.approve("unknown", &pin, NOW),
            Err(ApproveError::UnknownRequest)
        );
        // The PIN may be typed in lowercase and without the dash.
        let typed = pin.replace('-', "").to_lowercase();
        assert!(server.approve(&page.request_id, &typed, NOW).is_ok());
    }
    #[test]
    fn a_pin_works_once_and_locks_after_too_many_mistakes() {
        let server = server();
        let client_id = register(&server);
        let first = server
            .begin_authorize(&authorize_params(&client_id), NOW)
            .unwrap();
        let second = server
            .begin_authorize(&authorize_params(&client_id), NOW)
            .unwrap();
        let pin = server.open_pairing(NOW).pin;
        assert!(server.approve(&first.request_id, &pin, NOW).is_ok());
        assert_eq!(
            server.approve(&second.request_id, &pin, NOW),
            Err(ApproveError::PairingClosed)
        );

        let pin = server.open_pairing(NOW).pin;
        for _ in 0..4 {
            assert!(matches!(
                server.approve(&second.request_id, "WRONG-PIN", NOW),
                Err(ApproveError::WrongPin { .. })
            ));
        }
        assert_eq!(
            server.approve(&second.request_id, "WRONG-PIN", NOW),
            Err(ApproveError::Locked)
        );
        // Even the right PIN no longer works: the pairing was closed.
        assert_eq!(
            server.approve(&second.request_id, &pin, NOW),
            Err(ApproveError::PairingClosed)
        );
    }
    #[test]
    fn pairing_and_requests_expire() {
        let server = server();
        let client_id = register(&server);
        let page = server
            .begin_authorize(&authorize_params(&client_id), NOW)
            .unwrap();
        let pin = server.open_pairing(NOW).pin;
        assert!(server.pairing(NOW + 299).is_some());
        assert!(server.pairing(NOW + 300).is_none());
        server.open_pairing(NOW);
        assert_eq!(
            server.approve(&page.request_id, &pin, NOW + 400),
            Err(ApproveError::PairingClosed)
        );
        let stale_pin = server.open_pairing(NOW + 650).pin;
        assert_eq!(
            server.approve(&page.request_id, &stale_pin, NOW + 650),
            Err(ApproveError::UnknownRequest)
        );
    }
    #[test]
    fn the_callback_carries_code_and_state() {
        let server = server();
        let client_id = register(&server);
        let page = server
            .begin_authorize(&authorize_params(&client_id), NOW)
            .unwrap();
        let pin = server.open_pairing(NOW).pin;
        let redirect = server.approve(&page.request_id, &pin, NOW).unwrap();
        assert!(redirect.starts_with(&format!("{REDIRECT}?code=")));
        assert!(redirect.ends_with("&state=abc%20123"));
        assert_eq!(page.redirect_host, "chatgpt.com");
    }
    #[test]
    fn a_code_works_once_only() {
        let server = server();
        let client_id = register(&server);
        let page = server
            .begin_authorize(&authorize_params(&client_id), NOW)
            .unwrap();
        let pin = server.open_pairing(NOW).pin;
        let code = code_from(&server.approve(&page.request_id, &pin, NOW).unwrap());
        assert!(server
            .token(&token_form(&client_id, &code), NOW + 1)
            .is_ok());
        let replay = server.token(&token_form(&client_id, &code), NOW + 2);
        assert_eq!(replay.unwrap_err().code, "invalid_grant");
    }
    #[test]
    fn a_code_expires_and_is_bound_to_its_client_redirect_and_verifier() {
        let server = server();
        let client_id = register(&server);
        let issue = || {
            let page = server
                .begin_authorize(&authorize_params(&client_id), NOW)
                .unwrap();
            let pin = server.open_pairing(NOW).pin;
            code_from(&server.approve(&page.request_id, &pin, NOW).unwrap())
        };
        let late = issue();
        assert!(server
            .token(&token_form(&client_id, &late), NOW + 61)
            .is_err());
        let other_client = issue();
        assert!(server
            .token(&token_form("someone-else", &other_client), NOW + 1)
            .is_err());
        let wrong_verifier = issue();
        let mut form = token_form(&client_id, &wrong_verifier);
        form.insert("code_verifier".into(), "a".repeat(43));
        assert!(server.token(&form, NOW + 1).is_err());
        let wrong_redirect = issue();
        let mut form = token_form(&client_id, &wrong_redirect);
        form.insert(
            "redirect_uri".into(),
            "https://chatgpt.com/elsewhere".into(),
        );
        assert!(server.token(&form, NOW + 1).is_err());
    }
    #[test]
    fn refresh_tokens_rotate() {
        let server = server();
        let client_id = register(&server);
        let tokens = connect(&server, &client_id);
        let refresh = tokens["refresh_token"].as_str().unwrap().to_owned();
        let form = parse_form(&format!(
            "grant_type=refresh_token&refresh_token={refresh}&client_id={client_id}"
        ));
        let renewed = server.token(&form, NOW + 100).unwrap();
        let new_access = renewed["access_token"].as_str().unwrap();
        assert!(server.validate(new_access, NOW + 101).is_some());
        assert_eq!(
            server.token(&form, NOW + 102).unwrap_err().code,
            "invalid_grant"
        );
        let stranger = parse_form(&format!(
            "grant_type=refresh_token&refresh_token={}&client_id=other",
            renewed["refresh_token"].as_str().unwrap()
        ));
        assert!(server.token(&stranger, NOW + 103).is_err());
    }
    #[test]
    fn access_tokens_expire() {
        let server = server();
        let client_id = register(&server);
        let tokens = connect(&server, &client_id);
        let access = tokens["access_token"].as_str().unwrap();
        assert!(server.validate(access, NOW + 3599).is_some());
        assert!(server.validate(access, NOW + 3603).is_none());
        assert!(server.validate("garbage", NOW).is_none());
    }
    #[test]
    fn a_refresh_token_is_not_an_access_token() {
        let server = server();
        let client_id = register(&server);
        let tokens = connect(&server, &client_id);
        assert!(server
            .validate(tokens["refresh_token"].as_str().unwrap(), NOW + 3)
            .is_none());
    }
    #[test]
    fn revoking_disconnects_everyone() {
        let server = server();
        let client_id = register(&server);
        let tokens = connect(&server, &client_id);
        let access = tokens["access_token"].as_str().unwrap();
        server.revoke_all();
        assert!(server.validate(access, NOW + 3).is_none());
        assert!(server.connected_clients(NOW + 3).is_empty());
        assert!(server
            .begin_authorize(&authorize_params(&client_id), NOW + 3)
            .is_err());
    }
    #[test]
    fn tokens_survive_a_restart_and_are_stored_hashed() {
        let dir = std::env::temp_dir().join(format!("bridge-oauth-{}", crate::clock::now_nanos()));
        let store = dir.join("oauth.json");
        let first = AuthServer::new(AuthConfig::default(), Some(store.clone()), NOW);
        let client_id = register(&first);
        let tokens = connect(&first, &client_id);
        let access = tokens["access_token"].as_str().unwrap().to_owned();
        let saved = fs::read_to_string(&store).unwrap();
        assert!(
            !saved.contains(&access),
            "the plaintext token must never be written"
        );
        let second = AuthServer::new(AuthConfig::default(), Some(store), NOW + 10);
        assert!(second.validate(&access, NOW + 11).is_some());
        let expired = AuthServer::new(AuthConfig::default(), None, NOW + 10);
        assert!(expired.validate(&access, NOW + 11).is_none());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn denying_sends_the_browser_back_with_an_error() {
        let server = server();
        let client_id = register(&server);
        let page = server
            .begin_authorize(&authorize_params(&client_id), NOW)
            .unwrap();
        let back = server.deny(&page.request_id).unwrap();
        assert!(back.contains("error=access_denied") && back.contains("state=abc%20123"));
        assert!(server.deny(&page.request_id).is_none());
    }
    #[test]
    fn unknown_grant_types_are_refused() {
        let server = server();
        let form = parse_form("grant_type=password&username=a&password=b");
        assert_eq!(
            server.token(&form, NOW).unwrap_err().code,
            "unsupported_grant_type"
        );
    }
    #[test]
    fn pins_use_an_unambiguous_alphabet() {
        for _ in 0..50 {
            let pin = new_pin();
            assert_eq!(pin.len(), 9);
            assert!(pin
                .chars()
                .all(|c| c == '-' || PIN_ALPHABET.contains(&(c as u8))));
            assert!(!pin.contains(['0', 'O', '1', 'I']));
        }
    }
    #[test]
    fn form_parsing_decodes_escapes() {
        let form = parse_form("a=b%20c&d=e+f&g=%C3%A7&empty=&novalue");
        assert_eq!(form["a"], "b c");
        assert_eq!(form["d"], "e f");
        assert_eq!(form["g"], "ç");
        assert_eq!(form["empty"], "");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_encode("a b&c/ç"), "a%20b%26c%2F%C3%A7");
    }
}
