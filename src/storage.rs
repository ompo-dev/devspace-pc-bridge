use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{env, fs, io, path::{Path, PathBuf}};

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
                "4. Revise ferramentas e permissoes do plugin.",
                "5. No chat, use @ para selecionar o plugin DevSpace.",
                "Obs.: suporte a escrita depende do plano e politicas do ChatGPT.",
            ],
            Self::Claude => &[
                "1. Abra Claude > Personalizar > Conectores > Adicionar.",
                "2. Escolha Conector personalizado e cole a URL /mcp.",
                "3. Escolha OAuth, autentique e autorize o DevSpace.",
                "4. Habilite o conector na conversa com o Claude.",
                "5. Solicite a leitura de um arquivo e confirme as permissoes.",
                "Obs.: sao os chats normais; nao usa Claude Code.",
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
        Self { workspace: default_workspace(), client: ChatClient::ChatGpt }
    }
}
impl Settings {
    pub fn load() -> Self {
        if let Ok(s) = fs::read_to_string(settings_path()) {
            if let Ok(config) = serde_json::from_str::<Self>(&s) { return config; }
        }
        // v0.3 stored model chats in config.json. Never overwrite or delete it.
        let old = settings_path().with_file_name("config.json");
        let mut s = Self::default();
        if let Ok(data) = fs::read_to_string(old) {
            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                if let Some(p) = v.get("root").and_then(Value::as_str) {
                    if Path::new(p).is_dir() { s.workspace = PathBuf::from(p); }
                }
            }
        }
        s
    }
    pub fn save(&self) -> io::Result<()> {
        let path = settings_path();
        if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
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
    ].into_iter().find(|p| p.is_dir()).unwrap_or(user)
}
pub fn settings_path() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Roaming"))
        .join("DevSpacePCBridge")
        .join("bridge-ui.json")
}

pub fn valid_tailscale_host(host: &str) -> bool {
    host.ends_with(".ts.net")
        && host.len() > ".ts.net".len()
        && host.split('.').all(|part| {
            !part.is_empty()
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}
fn valid_origin(origin: &str) -> Option<String> {
    let hostname = origin.strip_prefix("https://")?.trim_end_matches('/');
    valid_tailscale_host(hostname).then(|| format!("https://{hostname}/mcp"))
}

/// Read the existing installation only. No authorization tokens are accessed.
pub fn mcp_url() -> Option<String> {
    let dir = home().join(".devspace");
    for file in [dir.join("pc-bridge").join("settings.json"), dir.join("autostart").join("tailscale.json")] {
        if let Ok(raw) = fs::read_to_string(file) {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                if let Some(host) = v.get("hostname").and_then(Value::as_str) {
                    if valid_tailscale_host(host) { return Some(format!("https://{host}/mcp")); }
                }
            }
        }
    }
    let raw = fs::read_to_string(dir.join("config.jsonc")).ok()?;
    let json: Value = serde_json::from_str(&raw).ok()?;
    valid_origin(json.get("server")?.get("publicBaseUrl")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_host_only() {
        assert!(valid_tailscale_host("pc.example.ts.net"));
        assert!(!valid_tailscale_host("pc.ts.net.bad.tld"));
        assert!(!valid_tailscale_host("pC.example.ts.net"));
        assert!(!valid_tailscale_host("pc.example.ts.net/evil"));
        assert_eq!(valid_origin("https://pc.example.ts.net"), Some("https://pc.example.ts.net/mcp".into()));
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
        assert!(ChatClient::ChatGpt.link().starts_with("https://chatgpt.com/"));
        assert!(ChatClient::Claude.link().starts_with("https://claude.ai/"));
    }
}
