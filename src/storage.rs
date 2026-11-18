use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Codex,
    ClaudeCode,
    OpenAi,
    Anthropic,
    OpenRouter,
    Ollama,
}

impl Provider {
    pub const ALL: [Self; 6] = [
        Self::Codex,
        Self::ClaudeCode,
        Self::OpenAi,
        Self::Anthropic,
        Self::OpenRouter,
        Self::Ollama,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex CLI (conta ChatGPT)",
            Self::ClaudeCode => "Claude Code CLI (conta Claude)",
            Self::OpenAi => "OpenAI API",
            Self::Anthropic => "Anthropic API",
            Self::OpenRouter => "OpenRouter API",
            Self::Ollama => "Ollama (local)",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::OpenRouter => "openrouter",
            Self::Ollama => "ollama",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Self::Codex | Self::ClaudeCode => "",
            Self::OpenAi => "gpt-4.1-mini",
            Self::Anthropic => "claude-sonnet-4-5",
            Self::OpenRouter => "openai/gpt-4.1-mini",
            Self::Ollama => "llama3.2",
        }
    }

    pub fn default_endpoint(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Ollama => "http://127.0.0.1:11434",
            _ => "",
        }
    }

    pub fn requires_key(self) -> bool {
        matches!(self, Self::OpenAi | Self::Anthropic | Self::OpenRouter)
    }

    pub fn is_agent(self) -> bool {
        matches!(self, Self::Codex | Self::ClaudeCode)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: Provider,
    pub root: PathBuf,
    pub model: String,
    pub endpoint: String,
    pub allow_edits: bool,
    pub messages: Vec<Message>,
    pub codex_session_id: Option<String>,
    pub claude_session_id: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: Provider::Codex,
            root: default_root(),
            model: String::new(),
            endpoint: String::new(),
            allow_edits: false,
            messages: Vec::new(),
            codex_session_id: None,
            claude_session_id: None,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        fs::read_to_string(settings_path())
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = settings_path();
        let parent = path.parent().ok_or(io::Error::other("invalid settings directory"))?;
        fs::create_dir_all(parent)?;
        let body = serde_json::to_vec_pretty(self)?;
        // Write atomically, so a crash cannot truncate the previous conversation.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, body)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn model(&self) -> &str {
        if self.model.trim().is_empty() {
            self.provider.default_model()
        } else {
            self.model.trim()
        }
    }

    pub fn endpoint(&self) -> &str {
        if self.endpoint.trim().is_empty() {
            self.provider.default_endpoint()
        } else {
            self.endpoint.trim()
        }
    }

    pub fn switch_provider(&mut self, next: Provider) {
        if self.provider != next {
            self.provider = next;
            self.model.clear();
            self.endpoint.clear();
            self.allow_edits = false;
            self.messages.clear();
            self.codex_session_id = None;
            self.claude_session_id = None;
        }
    }

    pub fn new_chat(&mut self) {
        self.messages.clear();
        self.codex_session_id = None;
        self.claude_session_id = None;
    }
}

pub fn settings_path() -> PathBuf {
    // APPDATA is user-specific: no secrets or session data in the Git working tree.
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Roaming"))
        .join("DevSpacePCBridge")
        .join("config.json")
}

pub fn home() -> PathBuf {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_root() -> PathBuf {
    let user = home();
    [
        user.join("OneDrive").join("Documents").join("GitHub"),
        user.join("Documents").join("GitHub"),
        user.join("Documents"),
        user.clone(),
    ]
    .into_iter()
    .find(|path| path.is_dir())
    .unwrap_or(user)
}

pub fn bridge_config_path() -> PathBuf {
    home().join(".devspace").join("pc-bridge").join("settings.json")
}

pub fn devspace_mcp_url() -> Option<String> {
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(bridge_config_path()).ok()?).ok()?;
    let hostname = config.get("hostname")?.as_str()?;
    validate_ts_host(hostname).then(|| format!("https://{hostname}/mcp"))
}

pub fn validate_ts_host(value: &str) -> bool {
    value.ends_with(".ts.net")
        && value.split('.').all(|part| {
            !part.is_empty()
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

pub fn valid_workspace(path: &Path) -> bool {
    path.is_dir()
}

pub fn store_key(provider: Provider, secret: &str) -> Result<(), String> {
    if !provider.requires_key() {
        return Err("Este provedor nao usa chave API".into());
    }
    if secret.is_empty() {
        return Err("Chave vazia".into());
    }
    keyring::Entry::new("DevSpacePCBridge", provider.id())
        .map_err(|error| format!("Credenciais Windows: {error}"))?
        .set_password(secret)
        .map_err(|error| format!("Nao foi possivel armazenar chave: {error}"))
}

pub fn load_key(provider: Provider) -> Option<String> {
    if !provider.requires_key() {
        return None;
    }
    keyring::Entry::new("DevSpacePCBridge", provider.id())
        .ok()?
        .get_password()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_switch_resets_conversation_and_endpoint() {
        let mut s = Settings::default();
        s.messages.push(Message { role: "user".into(), content: "hello".into() });
        s.codex_session_id = Some("abc".into());
        s.switch_provider(Provider::Anthropic);
        assert!(s.messages.is_empty());
        assert_eq!(s.codex_session_id, None);
        assert_eq!(s.model(), "claude-sonnet-4-5");
        assert!(s.provider.requires_key());
    }

    #[test]
    fn hostname_validation() {
        assert!(validate_ts_host("pc.tail123.ts.net"));
        assert!(!validate_ts_host("pc.tail123.ts.net/evil"));
        assert!(!validate_ts_host("a..ts.net"));
        assert!(!validate_ts_host("PC.tail.ts.net"));
    }

    #[test]
    fn no_secrets_in_settings() {
        let settings = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!settings.contains("api_key"));
        assert!(!settings.contains("secret"));
    }
}
