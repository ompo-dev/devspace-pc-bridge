use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatClient {
    ChatGpt,
    Claude,
}

impl ChatClient {
    pub fn label(self) -> &'static str {
        match self {
            Self::ChatGpt => "ChatGPT (chat normal)",
            Self::Claude => "Claude (chat normal)",
        }
    }
    pub fn link(self) -> &'static str {
        match self {
            Self::ChatGpt => "https://chatgpt.com/plugins",
            Self::Claude => "https://claude.ai/customize/connectors",
        }
    }
    pub fn instructions(self) -> &'static [&'static str] {
        match self {
            Self::ChatGpt => &[
                "1. Abra ChatGPT > Plugins > + > Adicionar MCP personalizado.",
                "2. Insira a URL HTTPS /mcp copiada do aplicativo.",
                "3. Configure OAuth e autorize com a conta Owner do DevSpace.",
                "4. Revise ferramentas e permissões do plugin.",
                "5. No chat, use @ para selecionar o plugin DevSpace.",
                "Obs.: suporte a escrita depende do plano e das políticas do ChatGPT.",
            ],
            Self::Claude => &[
                "1. Abra Claude > Personalizar > Conectores > Adicionar.",
                "2. Escolha Conector personalizado e cole a URL /mcp.",
                "3. Escolha OAuth, autentique e autorize o DevSpace.",
                "4. Habilite o conector na conversa com o Claude.",
                "5. Solicite a leitura de um arquivo e confirme as permissões.",
                "Obs.: são os chats normais; não usa Claude Code.",
            ],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub workspace: PathBuf,
    pub client: ChatClient,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            workspace: default_workspace(),
            client: ChatClient::ChatGpt,
        }
    }
}
impl Settings {
    pub fn load() -> Self {
        if let Some(config) = fs::read_to_string(settings_path())
            .ok()
            .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
        {
            return config;
        }
        // v0.3 stored model chats in config.json. Never overwrite or delete it.
        let mut settings = Self::default();
        let old = settings_path().with_file_name("config.json");
        if let Some(root) = read_json(&old)
            .as_ref()
            .and_then(|value| value.get("root"))
            .and_then(Value::as_str)
        {
            if Path::new(root).is_dir() {
                settings.workspace = PathBuf::from(root);
            }
        }
        settings
    }
    pub fn save(&self) -> io::Result<()> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temp, path)?;
        Ok(())
    }
}

pub fn home() -> PathBuf {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
pub fn default_workspace() -> PathBuf {
    let user = home();
    [
        user.join("OneDrive").join("Documents").join("GitHub"),
        user.join("Documents").join("GitHub"),
        user.join("Documents"),
        user.clone(),
    ]
    .into_iter()
    .find(|p| p.is_dir())
    .unwrap_or(user)
}
pub fn settings_path() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Roaming"))
        .join("DevSpacePCBridge")
        .join("bridge-ui.json")
}

/// `fs::canonicalize` on Windows returns `\\?\C:\...`, which PowerShell and
/// people handle badly. UNC verbatim paths are left untouched.
pub fn plain_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

const SENSITIVE_DIRS: [&str; 8] = [
    ".ssh",
    ".aws",
    ".gnupg",
    ".kube",
    ".claude",
    ".codex",
    ".devspace",
    "appdata",
];

fn normalized(path: &Path) -> String {
    plain_path(path)
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// Why `path` is too broad to hand to a publicly reachable MCP server, if it is.
/// Purely lexical: callers canonicalize first.
pub fn root_risk_with_home(path: &Path, home: &Path) -> Option<&'static str> {
    let text = normalized(path);
    let home_text = normalized(home);
    // After trimming the trailing separator a drive root is just "c:".
    if text.is_empty() || (text.len() == 2 && text.ends_with(':')) {
        return Some("raiz de um disco");
    }
    if text == home_text {
        return Some("pasta pessoal inteira");
    }
    if home_text.starts_with(&format!("{text}\\")) {
        return Some("pasta que contém a pasta pessoal");
    }
    if text.split('\\').any(|part| SENSITIVE_DIRS.contains(&part)) {
        return Some("pasta com credenciais de ferramentas");
    }
    None
}
pub fn root_risk(path: &Path) -> Option<&'static str> {
    root_risk_with_home(path, &home())
}

/// Removes `//` and `/* */` comments and trailing commas so DevSpace's
/// `config.jsonc` can be parsed with a strict JSON parser. String-aware.
pub fn strip_jsonc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        out.push(escaped);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                // The newline itself is kept so error positions stay meaningful.
                while chars.peek().is_some_and(|next| *next != '\n') {
                    chars.next();
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => out.push(c),
        }
    }
    remove_trailing_commas(&out)
}

fn remove_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.get(index + 1) {
                    out.push(*escaped);
                    index += 1;
                }
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == ',' {
            let next_significant = chars[index + 1..]
                .iter()
                .find(|next| !next.is_whitespace())
                .copied();
            if !matches!(next_significant, Some('}' | ']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        index += 1;
    }
    out
}

/// Windows PowerShell 5.1 writes `Set-Content -Encoding UTF8` files with a BOM,
/// which serde_json rejects, so it is stripped before parsing.
fn read_json(path: &Path) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&strip_jsonc(raw.trim_start_matches('\u{feff}'))).ok()
}

pub fn valid_tailscale_host(host: &str) -> bool {
    host.ends_with(".ts.net")
        && host.len() > ".ts.net".len()
        && host.split('.').all(|part| {
            !part.is_empty()
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}
fn valid_origin(origin: &str) -> Option<String> {
    let hostname = origin.strip_prefix("https://")?.trim_end_matches('/');
    valid_tailscale_host(hostname).then(|| format!("https://{hostname}/mcp"))
}

/// Read the existing installation only. No authorization tokens are accessed.
pub fn mcp_url() -> Option<String> {
    mcp_url_in(&home().join(".devspace"))
}

fn mcp_url_in(dir: &Path) -> Option<String> {
    for file in [
        dir.join("pc-bridge").join("settings.json"),
        dir.join("autostart").join("tailscale.json"),
    ] {
        if let Some(host) = read_json(&file)
            .as_ref()
            .and_then(|value| value.get("hostname"))
            .and_then(Value::as_str)
        {
            if valid_tailscale_host(host) {
                return Some(format!("https://{host}/mcp"));
            }
        }
    }
    let json = read_json(&dir.join("config.jsonc"))?;
    valid_origin(json.get("server")?.get("publicBaseUrl")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let dir = env::temp_dir().join(format!(
            "bridge-test-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn known_host_only() {
        assert!(valid_tailscale_host("pc.example.ts.net"));
        assert!(!valid_tailscale_host("pc.ts.net.bad.tld"));
        assert!(!valid_tailscale_host("pC.example.ts.net"));
        assert!(!valid_tailscale_host("pc.example.ts.net/evil"));
        assert_eq!(
            valid_origin("https://pc.example.ts.net"),
            Some("https://pc.example.ts.net/mcp".into())
        );
    }
    #[test]
    fn config_does_not_contain_api_or_chat_state() {
        let content = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!content.contains("api_key"));
        assert!(!content.contains("messages"));
        assert!(!content.contains("codex"));
    }
    #[test]
    fn both_clients_point_to_normal_chats() {
        assert!(ChatClient::ChatGpt
            .link()
            .starts_with("https://chatgpt.com/"));
        assert!(ChatClient::Claude.link().starts_with("https://claude.ai/"));
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_are_removed() {
        let input = "{\n  // line\n  \"a\": \"http://x // not a comment\", /* block */\n  \"b\": [1, 2,],\n}";
        let value: Value = serde_json::from_str(&strip_jsonc(input)).unwrap();
        assert_eq!(value["a"], "http://x // not a comment");
        assert_eq!(value["b"][1], 2);
    }
    #[test]
    fn jsonc_keeps_escaped_quotes_and_commas_in_strings() {
        let input = r#"{"a": "say \"hi\", ]", "b": 1,}"#;
        let value: Value = serde_json::from_str(&strip_jsonc(input)).unwrap();
        assert_eq!(value["a"], "say \"hi\", ]");
    }
    #[test]
    fn unterminated_block_comment_does_not_panic() {
        assert!(serde_json::from_str::<Value>(&strip_jsonc("{\"a\": 1} /* open")).is_ok());
    }

    #[test]
    fn broad_roots_are_flagged() {
        let home = Path::new(r"C:\Users\maico");
        assert!(root_risk_with_home(Path::new(r"C:\"), home).is_some());
        assert!(root_risk_with_home(Path::new(r"C:\Users\maico"), home).is_some());
        assert!(root_risk_with_home(Path::new(r"c:\users\MAICO\"), home).is_some());
        assert!(root_risk_with_home(Path::new(r"C:\Users"), home).is_some());
        assert!(root_risk_with_home(Path::new(r"C:\Users\maico\.ssh"), home).is_some());
        assert!(root_risk_with_home(Path::new(r"C:\Users\maico\AppData\Roaming"), home).is_some());
        assert!(root_risk_with_home(Path::new(r"\\?\C:\Users\maico"), home).is_some());
    }
    #[test]
    fn project_folders_are_allowed() {
        let home = Path::new(r"C:\Users\maico");
        assert!(
            root_risk_with_home(Path::new(r"C:\Users\maico\OneDrive\Documents\GitHub"), home)
                .is_none()
        );
        assert!(root_risk_with_home(Path::new(r"D:\projetos"), home).is_none());
    }
    #[test]
    fn verbatim_prefix_is_stripped_except_unc() {
        assert_eq!(
            plain_path(Path::new(r"\\?\C:\a\b")),
            PathBuf::from(r"C:\a\b")
        );
        assert_eq!(
            plain_path(Path::new(r"\\?\UNC\srv\share")),
            PathBuf::from(r"\\?\UNC\srv\share")
        );
        assert_eq!(plain_path(Path::new(r"C:\a")), PathBuf::from(r"C:\a"));
    }

    #[test]
    fn settings_json_with_bom_is_read() {
        let dir = scratch_dir("bom");
        let bridge = dir.join("pc-bridge");
        fs::create_dir_all(&bridge).unwrap();
        fs::write(
            bridge.join("settings.json"),
            "\u{feff}{\"hostname\":\"pc.example.ts.net\",\"port\":7676}",
        )
        .unwrap();
        assert_eq!(
            mcp_url_in(&dir),
            Some("https://pc.example.ts.net/mcp".into())
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn config_jsonc_with_comments_is_read() {
        let dir = scratch_dir("jsonc");
        fs::write(
            dir.join("config.jsonc"),
            "{\n  // gerado pelo DevSpace\n  \"server\": { \"publicBaseUrl\": \"https://pc.example.ts.net\", },\n}",
        )
        .unwrap();
        assert_eq!(
            mcp_url_in(&dir),
            Some("https://pc.example.ts.net/mcp".into())
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn invalid_hosts_in_files_are_ignored() {
        let dir = scratch_dir("invalid");
        let bridge = dir.join("pc-bridge");
        fs::create_dir_all(&bridge).unwrap();
        fs::write(
            bridge.join("settings.json"),
            "{\"hostname\":\"evil.example.com\"}",
        )
        .unwrap();
        assert_eq!(mcp_url_in(&dir), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
