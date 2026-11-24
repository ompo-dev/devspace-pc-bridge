//! HTTP routing for the desktop endpoint, kept independent of the socket library
//! so the whole OAuth + MCP flow can be tested by calling `route` directly.

use super::{
    mcp,
    oauth::{self, ApproveError, AuthorizePage},
    RemoteState,
};
use crate::clock;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Read,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

const MAX_BODY_BYTES: u64 = 1024 * 1024;
const MAX_REGISTER_BYTES: usize = 16 * 1024;
const MAX_FORM_BYTES: usize = 8 * 1024;
const WORKERS: usize = 4;

pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    /// Lowercase header names.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}
impl HttpResponse {
    fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", content_type.to_owned())],
            body,
        }
    }
    fn json(status: u16, value: &Value) -> Self {
        Self::new(status, "application/json", value.to_string().into_bytes())
    }
    fn text(status: u16, text: &str) -> Self {
        Self::new(
            status,
            "text/plain; charset=utf-8",
            text.as_bytes().to_vec(),
        )
    }
    fn html(status: u16, html: String) -> Self {
        let mut response = Self::new(status, "text/html; charset=utf-8", html.into_bytes());
        response.headers.push((
            "Content-Security-Policy",
            "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'".into(),
        ));
        response.headers.push(("X-Frame-Options", "DENY".into()));
        response
    }
    fn redirect(url: &str) -> Self {
        let mut response = Self::new(302, "text/plain; charset=utf-8", Vec::new());
        response.headers.push(("Location", url.to_owned()));
        response
    }
    fn empty(status: u16) -> Self {
        Self::new(status, "text/plain; charset=utf-8", Vec::new())
    }
    fn with_header(mut self, name: &'static str, value: String) -> Self {
        self.headers.push((name, value));
        self
    }
}

fn escape_html(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&#39;".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

fn hostname_of(authority: &str) -> &str {
    authority.split(':').next().unwrap_or_default()
}
fn is_local_host(host: &str) -> bool {
    matches!(hostname_of(host), "127.0.0.1" | "localhost")
}

/// With a public origin configured, only that hostname and localhost are served.
/// This keeps a rebinding page from reaching the local port under another name.
fn host_allowed(state: &RemoteState, req: &HttpRequest) -> bool {
    let host = req
        .headers
        .get("host")
        .map(String::as_str)
        .unwrap_or_default();
    let Some(origin) = state.public_origin() else {
        return is_local_host(host);
    };
    let authority = origin
        .split_once("://")
        .map_or(origin.as_str(), |(_, rest)| rest);
    is_local_host(host) || hostname_of(host).eq_ignore_ascii_case(hostname_of(authority))
}

fn origin_of(state: &RemoteState, req: &HttpRequest) -> String {
    if let Some(origin) = state.public_origin() {
        return origin.trim_end_matches('/').to_owned();
    }
    let host = req
        .headers
        .get("host")
        .map(String::as_str)
        .filter(|host| !host.is_empty())
        .unwrap_or("127.0.0.1");
    let scheme = if is_local_host(host) { "http" } else { "https" };
    format!("{scheme}://{host}")
}

fn bearer_token(req: &HttpRequest) -> Option<&str> {
    let value = req.headers.get("authorization")?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|token| !token.is_empty())
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"pt-BR\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"referrer\" content=\"no-referrer\"><title>{title}</title><style>body{{font-family:system-ui,sans-serif;max-width:30rem;margin:3rem auto;padding:0 1rem;line-height:1.5;color:#1b1b1b}}h1{{font-size:1.3rem}}code{{background:#eee;padding:.1rem .3rem;border-radius:3px}}input[type=text]{{font:inherit;font-size:1.4rem;letter-spacing:.2rem;padding:.4rem;width:11rem;text-transform:uppercase}}button{{font:inherit;padding:.5rem 1rem;margin-right:.5rem;cursor:pointer}}.err{{color:#a30000}}</style></head><body>{body}</body></html>"
    )
}

fn error_page(message: &str) -> HttpResponse {
    HttpResponse::html(
        400,
        page(
            "Pedido inválido",
            &format!(
                "<h1>Não foi possível continuar</h1><p>{}</p>",
                escape_html(message)
            ),
        ),
    )
}

fn approval_page(info: &AuthorizePage, message: Option<&str>) -> HttpResponse {
    let notice = message.map_or(String::new(), |text| {
        format!("<p class=\"err\">{}</p>", escape_html(text))
    });
    let body = format!(
        "<h1>Conectar ao DevSpace PC Bridge</h1>\
         <p><strong>{client}</strong> (retorno para <code>{host}</code>) quer ver e operar janelas do seu PC.</p>\
         {notice}\
         <p>Para confirmar que é você, abra o DevSpace PC Bridge no PC, vá à tela <strong>Desktop</strong>, pressione <strong>C</strong> e digite aqui o PIN mostrado lá.</p>\
         <form method=\"post\" action=\"/authorize\" autocomplete=\"off\">\
         <input type=\"hidden\" name=\"request_id\" value=\"{id}\">\
         <p><input type=\"text\" name=\"pin\" placeholder=\"XXXX-XXXX\" maxlength=\"12\" autofocus autocomplete=\"off\"></p>\
         <button type=\"submit\" name=\"action\" value=\"approve\">Permitir</button>\
         <button type=\"submit\" name=\"action\" value=\"deny\">Negar</button>\
         </form>\
         <p>O acesso continua limitado aos apps que você liberou, desliga sozinho e pode ser cortado a qualquer momento no PC.</p>",
        client = escape_html(&info.client_name),
        host = escape_html(&info.redirect_host),
        id = escape_html(&info.request_id),
    );
    HttpResponse::html(200, page("Conectar ao DevSpace PC Bridge", &body))
}

fn unauthorized(origin: &str, had_token: bool) -> HttpResponse {
    let challenge = if had_token {
        format!(
            "Bearer error=\"invalid_token\", resource_metadata=\"{origin}/.well-known/oauth-protected-resource\""
        )
    } else {
        format!("Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource\"")
    };
    HttpResponse::json(401, &json!({ "error": "unauthorized" }))
        .with_header("WWW-Authenticate", challenge)
}

fn resource_metadata(origin: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({
            "resource": format!("{origin}/mcp"),
            "authorization_servers": [origin],
            "scopes_supported": [oauth::SCOPE],
            "bearer_methods_supported": ["header"],
            "resource_name": "DevSpace PC Bridge Desktop",
        }),
    )
}

fn server_metadata(origin: &str) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({
            "issuer": origin,
            "authorization_endpoint": format!("{origin}/authorize"),
            "token_endpoint": format!("{origin}/token"),
            "registration_endpoint": format!("{origin}/register"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "scopes_supported": [oauth::SCOPE],
        }),
    )
}

fn register(state: &RemoteState, req: &HttpRequest, now: u64) -> HttpResponse {
    if req.body.len() > MAX_REGISTER_BYTES {
        return HttpResponse::json(413, &json!({ "error": "invalid_client_metadata" }));
    }
    let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
        return HttpResponse::json(
            400,
            &json!({ "error": "invalid_client_metadata", "error_description": "JSON inválido." }),
        );
    };
    match state.auth.register_client(&body, now) {
        Ok(client) => HttpResponse::json(201, &client),
        Err(err) => HttpResponse::json(err.status, &err.to_json()),
    }
}

fn authorize_get(state: &RemoteState, req: &HttpRequest, now: u64) -> HttpResponse {
    match state
        .auth
        .begin_authorize(&oauth::parse_form(&req.query), now)
    {
        Ok(info) => approval_page(&info, None),
        Err(err) => error_page(&err.description),
    }
}

fn authorize_post(state: &RemoteState, req: &HttpRequest, now: u64) -> HttpResponse {
    if req.body.len() > MAX_FORM_BYTES {
        return error_page("Pedido grande demais.");
    }
    let form = oauth::parse_form(&String::from_utf8_lossy(&req.body));
    let get = |key: &str| form.get(key).map(String::as_str).unwrap_or_default();
    let request_id = get("request_id");
    if get("action") == "deny" {
        return match state.auth.deny(request_id) {
            Some(url) => HttpResponse::redirect(&url),
            None => error_page("Este pedido já expirou. Volte ao chat e tente de novo."),
        };
    }
    let again = |message: &str| match state.auth.request_info(request_id, now) {
        Some(info) => approval_page(&info, Some(message)),
        None => error_page("Este pedido já expirou. Volte ao chat e tente de novo."),
    };
    match state.auth.approve(request_id, get("pin"), now) {
        Ok(url) => HttpResponse::redirect(&url),
        Err(ApproveError::UnknownRequest) => {
            error_page("Este pedido já expirou. Volte ao chat e tente de novo.")
        }
        Err(ApproveError::PairingClosed) => again(
            "O pareamento não está aberto. No DevSpace PC Bridge, tela Desktop, pressione C para gerar um PIN.",
        ),
        Err(ApproveError::WrongPin { attempts_left }) => {
            again(&format!("PIN incorreto. Tentativas restantes: {attempts_left}."))
        }
        Err(ApproveError::Locked) => error_page(
            "Muitas tentativas erradas: o pareamento foi fechado. Gere um novo PIN no PC.",
        ),
    }
}

fn token(state: &RemoteState, req: &HttpRequest, now: u64) -> HttpResponse {
    if req.body.len() > MAX_FORM_BYTES {
        return HttpResponse::json(413, &json!({ "error": "invalid_request" }));
    }
    let form = oauth::parse_form(&String::from_utf8_lossy(&req.body));
    match state.auth.token(&form, now) {
        Ok(tokens) => HttpResponse::json(200, &tokens),
        Err(err) => HttpResponse::json(err.status, &err.to_json()),
    }
}

fn mcp_post(
    state: &RemoteState,
    req: &HttpRequest,
    origin: &str,
    now: u64,
    now_ms: u64,
) -> HttpResponse {
    let bearer = bearer_token(req);
    let Some(info) = bearer.and_then(|token| state.auth.validate(token, now)) else {
        return unauthorized(origin, bearer.is_some());
    };
    let Ok(request) = serde_json::from_slice::<Value>(&req.body) else {
        return HttpResponse::json(
            400,
            &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } }),
        );
    };
    let reply = match &request {
        Value::Array(items) => {
            let replies: Vec<Value> = items
                .iter()
                .filter_map(|item| mcp::handle(&state.mcp, &info.client_name, item, now_ms))
                .collect();
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        single => mcp::handle(&state.mcp, &info.client_name, single, now_ms),
    };
    match reply {
        Some(reply) => HttpResponse::json(200, &reply),
        None => HttpResponse::empty(202),
    }
}

pub fn route(state: &RemoteState, req: &HttpRequest, now: u64, now_ms: u64) -> HttpResponse {
    if !host_allowed(state, req) {
        return HttpResponse::text(421, "Host não reconhecido.");
    }
    let origin = origin_of(state, req);
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/healthz") => HttpResponse::text(200, "ok"),
        (
            "GET",
            "/.well-known/oauth-protected-resource" | "/.well-known/oauth-protected-resource/mcp",
        ) => resource_metadata(&origin),
        ("GET", "/.well-known/oauth-authorization-server") => server_metadata(&origin),
        ("POST", "/register") => register(state, req, now),
        ("GET", "/authorize") => authorize_get(state, req, now),
        ("POST", "/authorize") => authorize_post(state, req, now),
        ("POST", "/token") => token(state, req, now),
        ("POST", "/mcp") => mcp_post(state, req, &origin, now, now_ms),
        (_, "/mcp") => HttpResponse::empty(405).with_header("Allow", "POST".into()),
        _ => HttpResponse::text(404, "Não encontrado."),
    }
}

pub struct RemoteHandle {
    server: Arc<tiny_http::Server>,
    running: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    pub port: u16,
}
impl RemoteHandle {
    pub fn stop(self) {
        self.running.store(false, Ordering::SeqCst);
        for _ in &self.threads {
            self.server.unblock();
        }
        for thread in self.threads {
            let _ = thread.join();
        }
    }
}

fn to_request(mut request: tiny_http::Request) -> (HttpRequest, tiny_http::Request) {
    let url = request.url().to_owned();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let headers = request
        .headers()
        .iter()
        .map(|header| {
            (
                header.field.to_string().to_ascii_lowercase(),
                header.value.to_string(),
            )
        })
        .collect();
    let mut body = Vec::new();
    // A body over the limit is cut and later rejected by the size checks in `route`.
    let _ = request
        .as_reader()
        .take(MAX_BODY_BYTES)
        .read_to_end(&mut body);
    (
        HttpRequest {
            method: request.method().as_str().to_owned(),
            path: path.to_owned(),
            query: query.to_owned(),
            headers,
            body,
        },
        request,
    )
}

fn respond(request: tiny_http::Request, response: HttpResponse) {
    let mut reply = tiny_http::Response::from_data(response.body).with_status_code(response.status);
    let always = [
        ("X-Content-Type-Options", "nosniff"),
        ("Cache-Control", "no-store"),
        ("Referrer-Policy", "no-referrer"),
    ];
    for (name, value) in response
        .headers
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .chain(always)
    {
        if let Ok(header) = tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            reply.add_header(header);
        }
    }
    let _ = request.respond(reply);
}

/// Listens on 127.0.0.1 only; the tunnel is what makes it reachable from outside.
pub fn start(state: Arc<RemoteState>, port: u16) -> Result<RemoteHandle, String> {
    let server = Arc::new(
        tiny_http::Server::http(("127.0.0.1", port))
            .map_err(|e| format!("Não consegui abrir a porta {port}: {e}"))?,
    );
    let port = server
        .server_addr()
        .to_ip()
        .map_or(port, |addr| addr.port());
    let running = Arc::new(AtomicBool::new(true));
    let threads = (0..WORKERS)
        .map(|_| {
            let (server, state, running) = (server.clone(), state.clone(), running.clone());
            thread::spawn(move || {
                while running.load(Ordering::SeqCst) {
                    let Ok(raw) = server.recv() else {
                        continue;
                    };
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    let (request, raw) = to_request(raw);
                    let response = route(&state, &request, clock::now_secs(), clock::now_millis());
                    respond(raw, response);
                }
            })
        })
        .collect();
    Ok(RemoteHandle {
        server,
        running,
        threads,
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        desktop::{
            ClickKind, ClickTarget, Desktop, DesktopResult, Element, Rect, Screenshot, WindowInfo,
        },
        remote::oauth::{percent_encode, AuthConfig},
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use sha2::{Digest, Sha256};

    struct StubDesktop;
    impl Desktop for StubDesktop {
        fn list_windows(&self) -> DesktopResult<Vec<WindowInfo>> {
            Ok(vec![WindowInfo {
                id: 5,
                title: "Notas".into(),
                process: "notepad.exe".into(),
                pid: 10,
                rect: Rect {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 50,
                },
                minimized: false,
                foreground: false,
            }])
        }
        fn screenshot(&self, _: u64, _: u32, _: bool) -> DesktopResult<Screenshot> {
            Ok(Screenshot {
                data: vec![9, 9],
                mime: "image/jpeg",
                width: 100,
                height: 50,
            })
        }
        fn inspect(&self, _: u64, _: u8, _: usize) -> DesktopResult<Vec<Element>> {
            Ok(vec![])
        }
        fn click(&self, _: u64, _: &ClickTarget, _: ClickKind) -> DesktopResult<String> {
            Ok("ok".into())
        }
        fn type_text(&self, _: u64, _: Option<&str>, _: &str, _: bool) -> DesktopResult<String> {
            Ok("ok".into())
        }
        fn press_key(&self, _: u64, _: &str) -> DesktopResult<String> {
            Ok("ok".into())
        }
        fn scroll(&self, _: u64, _: Option<&str>, _: i32, _: i32) -> DesktopResult<String> {
            Ok("ok".into())
        }
    }

    const NOW: u64 = 2_000_000;
    const REDIRECT: &str = "https://claude.ai/api/mcp/auth_callback";

    fn state() -> Arc<RemoteState> {
        let dir = std::env::temp_dir().join(format!("bridge-remote-{}", clock::now_nanos()));
        let state = RemoteState::new(Arc::new(StubDesktop), vec![], dir, AuthConfig::default());
        {
            let mut policy = state.policy();
            policy.config_mut().allowed_apps = vec!["notepad.exe".into()];
            policy.config_mut().input = true;
            policy.enable(NOW, 30);
        }
        Arc::new(state)
    }
    fn request(
        method: &str,
        path: &str,
        query: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> HttpRequest {
        HttpRequest {
            method: method.into(),
            path: path.into(),
            query: query.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_vec(),
        }
    }
    fn send(state: &RemoteState, req: HttpRequest) -> HttpResponse {
        route(state, &req, NOW, NOW * 1000)
    }
    fn body_json(response: &HttpResponse) -> Value {
        serde_json::from_slice(&response.body).unwrap_or(Value::Null)
    }
    fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
        response
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    const HOST: (&str, &str) = ("host", "127.0.0.1:7677");

    /// Runs registration, approval with the PIN and the token exchange.
    fn connect(state: &RemoteState) -> String {
        let verifier = "v".repeat(60);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let registered = send(
            state,
            request(
                "POST",
                "/register",
                "",
                &[HOST],
                json!({"client_name": "Claude", "redirect_uris": [REDIRECT]})
                    .to_string()
                    .as_bytes(),
            ),
        );
        assert_eq!(registered.status, 201);
        let client_id = body_json(&registered)["client_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let query = format!(
            "response_type=code&client_id={client_id}&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256&state=xyz",
            percent_encode(REDIRECT)
        );
        let form_page = send(state, request("GET", "/authorize", &query, &[HOST], b""));
        assert_eq!(form_page.status, 200);
        let html = String::from_utf8(form_page.body).unwrap();
        assert!(html.contains("Claude") && html.contains("claude.ai"));
        let request_id = html
            .split("name=\"request_id\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_owned();
        let pin = state.auth.open_pairing(NOW).pin;
        let approve = send(
            state,
            request(
                "POST",
                "/authorize",
                "",
                &[HOST],
                format!("request_id={request_id}&pin={pin}&action=approve").as_bytes(),
            ),
        );
        assert_eq!(approve.status, 302);
        let location = header(&approve, "Location").unwrap().to_owned();
        assert!(location.starts_with(REDIRECT) && location.ends_with("&state=xyz"));
        let code = oauth::parse_form(location.split_once('?').unwrap().1)["code"].clone();
        let exchange = send(state, request("POST", "/token", "", &[HOST], format!("grant_type=authorization_code&code={code}&client_id={client_id}&redirect_uri={}&code_verifier={verifier}", percent_encode(REDIRECT)).as_bytes()));
        assert_eq!(exchange.status, 200);
        body_json(&exchange)["access_token"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn mcp_call(state: &RemoteState, token: &str, body: Value) -> HttpResponse {
        send(
            state,
            request(
                "POST",
                "/mcp",
                "",
                &[HOST, ("authorization", &format!("Bearer {token}"))],
                body.to_string().as_bytes(),
            ),
        )
    }

    #[test]
    fn a_chat_can_pair_and_then_use_the_tools_end_to_end() {
        let state = state();
        let token = connect(&state);
        let initialized = mcp_call(
            &state,
            &token,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
        );
        assert_eq!(initialized.status, 200);
        assert_eq!(
            body_json(&initialized)["result"]["serverInfo"]["name"],
            "devspace-pc-bridge-desktop"
        );
        let listing = mcp_call(
            &state,
            &token,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "desktop_list_windows", "arguments": {}}}),
        );
        assert!(body_json(&listing)["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Notas"));
        let shot = mcp_call(
            &state,
            &token,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "desktop_screenshot", "arguments": {"window_id": 5}}}),
        );
        assert_eq!(body_json(&shot)["result"]["content"][0]["type"], "image");
        assert_eq!(
            mcp_call(
                &state,
                &token,
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
            )
            .status,
            202
        );
        let batch = mcp_call(
            &state,
            &token,
            json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}, {"jsonrpc": "2.0", "method": "notifications/initialized"}]),
        );
        assert_eq!(body_json(&batch).as_array().unwrap().len(), 1);
    }
    #[test]
    fn the_endpoint_refuses_callers_without_a_valid_token() {
        let state = state();
        let none = send(&state, request("POST", "/mcp", "", &[HOST], b"{}"));
        assert_eq!(none.status, 401);
        let challenge = header(&none, "WWW-Authenticate").unwrap();
        assert!(challenge.contains(
            "resource_metadata=\"http://127.0.0.1:7677/.well-known/oauth-protected-resource\""
        ));
        assert!(!challenge.contains("invalid_token"));
        let wrong = send(
            &state,
            request(
                "POST",
                "/mcp",
                "",
                &[HOST, ("authorization", "Bearer nope")],
                b"{}",
            ),
        );
        assert_eq!(wrong.status, 401);
        assert!(header(&wrong, "WWW-Authenticate")
            .unwrap()
            .contains("invalid_token"));
        let basic = send(
            &state,
            request(
                "POST",
                "/mcp",
                "",
                &[HOST, ("authorization", "Basic abc")],
                b"{}",
            ),
        );
        assert_eq!(basic.status, 401);
    }
    #[test]
    fn a_revoked_token_stops_working_immediately() {
        let state = state();
        let token = connect(&state);
        assert_eq!(
            mcp_call(
                &state,
                &token,
                json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})
            )
            .status,
            200
        );
        state.auth.revoke_all();
        assert_eq!(
            mcp_call(
                &state,
                &token,
                json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})
            )
            .status,
            401
        );
    }
    #[test]
    fn switching_control_off_blocks_even_a_paired_chat() {
        let state = state();
        let token = connect(&state);
        state.policy().disable();
        let shot = mcp_call(
            &state,
            &token,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "desktop_screenshot", "arguments": {"window_id": 5}}}),
        );
        assert_eq!(body_json(&shot)["result"]["isError"], true);
    }
    #[test]
    fn discovery_documents_point_at_the_right_origin() {
        let state = state();
        state.set_public_origin(Some("https://pc.example.ts.net:8443".into()));
        let host = ("host", "pc.example.ts.net:8443");
        let resource = body_json(&send(
            &state,
            request(
                "GET",
                "/.well-known/oauth-protected-resource/mcp",
                "",
                &[host],
                b"",
            ),
        ));
        assert_eq!(resource["resource"], "https://pc.example.ts.net:8443/mcp");
        assert_eq!(
            resource["authorization_servers"][0],
            "https://pc.example.ts.net:8443"
        );
        let meta = body_json(&send(
            &state,
            request(
                "GET",
                "/.well-known/oauth-authorization-server",
                "",
                &[host],
                b"",
            ),
        ));
        assert_eq!(
            meta["token_endpoint"],
            "https://pc.example.ts.net:8443/token"
        );
        assert_eq!(meta["code_challenge_methods_supported"][0], "S256");
        assert_eq!(
            meta["registration_endpoint"],
            "https://pc.example.ts.net:8443/register"
        );
    }
    #[test]
    fn unknown_hostnames_are_refused_once_published() {
        let state = state();
        state.set_public_origin(Some("https://pc.example.ts.net:8443".into()));
        let evil = send(
            &state,
            request("GET", "/healthz", "", &[("host", "evil.example")], b""),
        );
        assert_eq!(evil.status, 421);
        assert_eq!(
            send(
                &state,
                request("GET", "/healthz", "", &[("host", "pc.example.ts.net")], b"")
            )
            .status,
            200
        );
        assert_eq!(
            send(&state, request("GET", "/healthz", "", &[HOST], b"")).status,
            200
        );
    }
    #[test]
    fn methods_and_paths_are_strict() {
        let state = state();
        let get = send(&state, request("GET", "/mcp", "", &[HOST], b""));
        assert_eq!(get.status, 405);
        assert_eq!(header(&get, "Allow"), Some("POST"));
        assert_eq!(
            send(&state, request("DELETE", "/mcp", "", &[HOST], b"")).status,
            405
        );
        assert_eq!(
            send(&state, request("GET", "/admin", "", &[HOST], b"")).status,
            404
        );
        assert_eq!(
            send(&state, request("GET", "/token", "", &[HOST], b"")).status,
            404
        );
    }
    #[test]
    fn registration_rejects_foreign_redirects_and_junk() {
        let state = state();
        let evil = send(
            &state,
            request(
                "POST",
                "/register",
                "",
                &[HOST],
                json!({"redirect_uris": ["https://evil.example/cb"]})
                    .to_string()
                    .as_bytes(),
            ),
        );
        assert_eq!(evil.status, 400);
        assert_eq!(body_json(&evil)["error"], "invalid_redirect_uri");
        assert_eq!(
            send(
                &state,
                request("POST", "/register", "", &[HOST], b"not json")
            )
            .status,
            400
        );
        assert_eq!(
            send(
                &state,
                request(
                    "POST",
                    "/register",
                    "",
                    &[HOST],
                    &vec![b'a'; MAX_REGISTER_BYTES + 1]
                )
            )
            .status,
            413
        );
    }
    #[test]
    fn the_approval_page_escapes_what_the_client_controls() {
        let state = state();
        let registered = send(
            &state,
            request(
                "POST",
                "/register",
                "",
                &[HOST],
                json!({"client_name": "<script>alert(1)</script>", "redirect_uris": [REDIRECT]})
                    .to_string()
                    .as_bytes(),
            ),
        );
        let client_id = body_json(&registered)["client_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let query = format!("response_type=code&client_id={client_id}&redirect_uri={}&code_challenge={}&code_challenge_method=S256", percent_encode(REDIRECT), "c".repeat(43));
        let page = send(&state, request("GET", "/authorize", &query, &[HOST], b""));
        let html = String::from_utf8(page.body.clone()).unwrap();
        assert!(
            !html.contains("<script>alert"),
            "client name was not escaped"
        );
        assert!(html.contains("&lt;script&gt;"));
        assert!(header(&page, "Content-Security-Policy")
            .unwrap()
            .contains("default-src 'none'"));
        assert_eq!(header(&page, "X-Frame-Options"), Some("DENY"));
    }
    #[test]
    fn a_wrong_pin_shows_the_form_again_and_never_issues_a_code() {
        let state = state();
        let registered = send(
            &state,
            request(
                "POST",
                "/register",
                "",
                &[HOST],
                json!({"client_name": "ChatGPT", "redirect_uris": [REDIRECT]})
                    .to_string()
                    .as_bytes(),
            ),
        );
        let client_id = body_json(&registered)["client_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let query = format!("response_type=code&client_id={client_id}&redirect_uri={}&code_challenge={}&code_challenge_method=S256", percent_encode(REDIRECT), "c".repeat(43));
        let html = String::from_utf8(
            send(&state, request("GET", "/authorize", &query, &[HOST], b"")).body,
        )
        .unwrap();
        let request_id = html
            .split("name=\"request_id\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_owned();
        let closed = send(
            &state,
            request(
                "POST",
                "/authorize",
                "",
                &[HOST],
                format!("request_id={request_id}&pin=AAAA-AAAA&action=approve").as_bytes(),
            ),
        );
        assert_eq!(closed.status, 200);
        assert!(String::from_utf8(closed.body)
            .unwrap()
            .contains("pareamento não está aberto"));
        state.auth.open_pairing(NOW);
        let wrong = send(
            &state,
            request(
                "POST",
                "/authorize",
                "",
                &[HOST],
                format!("request_id={request_id}&pin=ZZZZ-ZZZZ&action=approve").as_bytes(),
            ),
        );
        assert_eq!(wrong.status, 200);
        assert!(header(&wrong, "Location").is_none());
        assert!(String::from_utf8(wrong.body)
            .unwrap()
            .contains("Tentativas restantes: 4"));
        let denied = send(
            &state,
            request(
                "POST",
                "/authorize",
                "",
                &[HOST],
                format!("request_id={request_id}&action=deny").as_bytes(),
            ),
        );
        assert_eq!(denied.status, 302);
        assert!(header(&denied, "Location")
            .unwrap()
            .contains("error=access_denied"));
    }
    #[test]
    fn a_real_socket_serves_requests_and_stops() {
        let state = state();
        let handle = start(state, 0).expect("start");
        let base = format!("http://127.0.0.1:{}", handle.port);
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        assert_eq!(
            client
                .get(format!("{base}/healthz"))
                .send()
                .unwrap()
                .text()
                .unwrap(),
            "ok"
        );
        let unauthorized = client
            .post(format!("{base}/mcp"))
            .body("{}")
            .send()
            .unwrap();
        assert_eq!(unauthorized.status().as_u16(), 401);
        assert!(unauthorized.headers().get("www-authenticate").is_some());
        assert_eq!(
            unauthorized
                .headers()
                .get("x-content-type-options")
                .unwrap(),
            "nosniff"
        );
        let meta_text = client
            .get(format!("{base}/.well-known/oauth-authorization-server"))
            .send()
            .unwrap()
            .text()
            .unwrap();
        let meta: Value = serde_json::from_str(&meta_text).unwrap();
        assert_eq!(meta["issuer"], base);
        handle.stop();
    }
}
