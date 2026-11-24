//! Decides, for every remote request, whether it may touch a window.
//! Everything is denied until the user switches control on, for a limited time,
//! and only apps the user has explicitly allowed are ever visible or reachable.

use super::WindowInfo;
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, fs, io, path::Path};

pub const DEFAULT_ENABLE_MINUTES: u32 = 30;
pub const MAX_ENABLE_MINUTES: u32 = 240;

/// Never reachable, whatever the user allows. Password managers hold the keys to
/// everything else; terminals and shells could approve a pairing prompt or run
/// anything, which would defeat the other rules.
const HARD_BLOCKED_PROCESSES: [&str; 17] = [
    "1password.exe",
    "bitwarden.exe",
    "keepass.exe",
    "keepassxc.exe",
    "lastpass.exe",
    "dashlane.exe",
    "enpass.exe",
    "windowsterminal.exe",
    "wt.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "conhost.exe",
    "openconsole.exe",
    "mintty.exe",
    "alacritty.exe",
    "wezterm-gui.exe",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PolicyConfig {
    /// Process names, lowercase, e.g. `notepad.exe`.
    pub allowed_apps: Vec<String>,
    pub screenshots: bool,
    pub input: bool,
    /// Windows whose title contains one of these words are refused.
    pub blocked_title_words: Vec<String>,
    pub max_actions_per_minute: u32,
}
impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            allowed_apps: Vec::new(),
            screenshots: true,
            input: false,
            blocked_title_words: [
                "senha", "password", "passwd", "cofre", "vault", "2fa", "otp",
            ]
            .map(String::from)
            .to_vec(),
            max_actions_per_minute: 30,
        }
    }
}
impl PolicyConfig {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temp, path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    View,
    Input,
}

pub struct Policy {
    config: PolicyConfig,
    enabled_until: Option<u64>,
    /// Process ids that are never reachable: this program and its ancestors
    /// (the terminal it runs in).
    protected_pids: Vec<u32>,
    recent_ms: VecDeque<u64>,
}

pub fn normalize_app(name: &str) -> String {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    base.trim().to_ascii_lowercase()
}

impl Policy {
    pub fn new(config: PolicyConfig, protected_pids: Vec<u32>) -> Self {
        Self {
            config,
            enabled_until: None,
            protected_pids,
            recent_ms: VecDeque::new(),
        }
    }
    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }
    pub fn config_mut(&mut self) -> &mut PolicyConfig {
        &mut self.config
    }
    /// Switches control on for `minutes` (clamped) and returns the deadline.
    pub fn enable(&mut self, now_secs: u64, minutes: u32) -> u64 {
        let minutes = minutes.clamp(1, MAX_ENABLE_MINUTES);
        let until = now_secs + u64::from(minutes) * 60;
        self.enabled_until = Some(until);
        until
    }
    pub fn disable(&mut self) {
        self.enabled_until = None;
    }
    pub fn remaining_secs(&self, now_secs: u64) -> Option<u64> {
        self.enabled_until
            .and_then(|until| until.checked_sub(now_secs))
            .filter(|left| *left > 0)
    }
    pub fn is_enabled(&self, now_secs: u64) -> bool {
        self.remaining_secs(now_secs).is_some()
    }
    pub fn is_app_allowed(&self, process: &str) -> bool {
        let process = normalize_app(process);
        self.config
            .allowed_apps
            .iter()
            .any(|allowed| normalize_app(allowed) == process)
    }
    fn hard_blocked(&self, window: &WindowInfo) -> Option<&'static str> {
        if self.protected_pids.contains(&window.pid) {
            return Some("janela do próprio bridge ou do terminal que o executa");
        }
        let process = normalize_app(&window.process);
        HARD_BLOCKED_PROCESSES
            .contains(&process.as_str())
            .then_some("app bloqueado (gerenciador de senhas ou terminal)")
    }
    fn blocked_title_word(&self, window: &WindowInfo) -> Option<&str> {
        let title = window.title.to_lowercase();
        self.config
            .blocked_title_words
            .iter()
            .find(|word| !word.is_empty() && title.contains(&word.to_lowercase()))
            .map(String::as_str)
    }
    /// Whether the remote side may even know this window exists.
    pub fn visible_to_remote(&self, window: &WindowInfo) -> bool {
        self.hard_blocked(window).is_none()
            && self.is_app_allowed(&window.process)
            && self.blocked_title_word(window).is_none()
    }
    /// For requests that are not about one window (listing windows): control must be
    /// on, and the request counts toward the rate limit like any other.
    pub fn check_session(&mut self, now_ms: u64) -> Result<(), String> {
        self.recent_ms
            .retain(|at| now_ms.saturating_sub(*at) < 60_000);
        self.recent_ms.push_back(now_ms);
        if !self.is_enabled(now_ms / 1000) {
            return Err("O controle remoto está desligado. Ligue-o no DevSpace PC Bridge.".into());
        }
        if self.recent_ms.len() > self.config.max_actions_per_minute as usize {
            return Err("Limite de ações por minuto atingido. Aguarde um pouco.".into());
        }
        Ok(())
    }
    /// Every call counts toward the rate limit, denied ones included, so a client
    /// cannot probe windows or spam requests unnoticed.
    pub fn check(
        &mut self,
        now_ms: u64,
        window: &WindowInfo,
        action: Action,
    ) -> Result<(), String> {
        self.recent_ms
            .retain(|at| now_ms.saturating_sub(*at) < 60_000);
        self.recent_ms.push_back(now_ms);
        if !self.is_enabled(now_ms / 1000) {
            return Err("O controle remoto está desligado. Ligue-o no DevSpace PC Bridge.".into());
        }
        match action {
            Action::View if !self.config.screenshots => {
                return Err("Capturas de tela estão desativadas.".into());
            }
            Action::Input if !self.config.input => {
                return Err("Teclado e mouse remotos estão desativados.".into());
            }
            Action::View | Action::Input => {}
        }
        if let Some(reason) = self.hard_blocked(window) {
            return Err(format!("Janela recusada: {reason}."));
        }
        if !self.is_app_allowed(&window.process) {
            return Err(format!(
                "O app {} não foi liberado para controle remoto.",
                normalize_app(&window.process)
            ));
        }
        if let Some(word) = self.blocked_title_word(window) {
            return Err(format!(
                "Janela recusada: o título contém a palavra bloqueada \"{word}\"."
            ));
        }
        if self.recent_ms.len() > self.config.max_actions_per_minute as usize {
            return Err("Limite de ações por minuto atingido. Aguarde um pouco.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::Rect;

    fn window(process: &str, title: &str, pid: u32) -> WindowInfo {
        WindowInfo {
            id: 1,
            title: title.into(),
            process: process.into(),
            pid,
            rect: Rect::default(),
            minimized: false,
            foreground: false,
        }
    }
    fn policy_for(apps: &[&str], input: bool) -> Policy {
        let config = PolicyConfig {
            allowed_apps: apps.iter().map(|app| (*app).to_owned()).collect(),
            input,
            ..PolicyConfig::default()
        };
        let mut policy = Policy::new(config, vec![4242]);
        policy.enable(1_000, 30);
        policy
    }
    const NOW_MS: u64 = 1_000_000;

    #[test]
    fn everything_is_denied_until_enabled() {
        let mut policy = Policy::new(
            PolicyConfig {
                allowed_apps: vec!["notepad.exe".into()],
                ..PolicyConfig::default()
            },
            vec![],
        );
        let result = policy.check(NOW_MS, &window("notepad.exe", "a", 1), Action::View);
        assert!(result.unwrap_err().contains("desligado"));
    }
    #[test]
    fn control_expires_on_its_own() {
        let mut policy = policy_for(&["notepad.exe"], false);
        assert!(policy.is_enabled(1_000 + 29 * 60));
        assert!(!policy.is_enabled(1_000 + 30 * 60));
        let late = (1_000 + 31 * 60) * 1000;
        assert!(policy
            .check(late, &window("notepad.exe", "a", 1), Action::View)
            .is_err());
    }
    #[test]
    fn enabling_is_clamped() {
        let mut policy = policy_for(&[], false);
        assert_eq!(policy.enable(0, 0), 60);
        assert_eq!(
            policy.enable(0, 100_000),
            u64::from(MAX_ENABLE_MINUTES) * 60
        );
        policy.disable();
        assert!(!policy.is_enabled(1));
    }
    #[test]
    fn only_allowed_apps_are_reachable() {
        let mut policy = policy_for(&["notepad.exe"], false);
        assert!(policy
            .check(
                NOW_MS,
                &window("Notepad.EXE", "Sem título", 1),
                Action::View
            )
            .is_ok());
        let denied = policy
            .check(NOW_MS + 1, &window("chrome.exe", "Mail", 2), Action::View)
            .unwrap_err();
        assert!(denied.contains("chrome.exe"));
        assert!(policy_for(&[], false)
            .check(NOW_MS, &window("notepad.exe", "a", 1), Action::View)
            .is_err());
    }
    #[test]
    fn paths_in_the_allowlist_are_normalized() {
        let mut policy = policy_for(&["C:\\Windows\\System32\\Notepad.exe"], false);
        assert!(policy
            .check(NOW_MS, &window("notepad.exe", "a", 1), Action::View)
            .is_ok());
    }
    #[test]
    fn input_needs_its_own_switch() {
        let mut view_only = policy_for(&["notepad.exe"], false);
        let notepad = window("notepad.exe", "a", 1);
        assert!(view_only.check(NOW_MS, &notepad, Action::View).is_ok());
        assert!(view_only
            .check(NOW_MS + 1, &notepad, Action::Input)
            .unwrap_err()
            .contains("Teclado e mouse"));
        let mut full = policy_for(&["notepad.exe"], true);
        assert!(full.check(NOW_MS, &notepad, Action::Input).is_ok());
    }
    #[test]
    fn screenshots_can_be_switched_off() {
        let mut policy = policy_for(&["notepad.exe"], true);
        policy.config_mut().screenshots = false;
        assert!(policy
            .check(NOW_MS, &window("notepad.exe", "a", 1), Action::View)
            .unwrap_err()
            .contains("Capturas"));
    }
    #[test]
    fn terminals_and_password_managers_can_never_be_allowed() {
        let mut policy = policy_for(&["windowsterminal.exe", "keepassxc.exe", "cmd.exe"], true);
        for process in ["windowsterminal.exe", "keepassxc.exe", "cmd.exe"] {
            let result = policy.check(NOW_MS, &window(process, "x", 1), Action::View);
            assert!(result.is_err(), "{process} must stay blocked");
            assert!(!policy.visible_to_remote(&window(process, "x", 1)));
        }
    }
    #[test]
    fn the_bridges_own_process_tree_is_protected() {
        let mut policy = policy_for(&["customterm.exe"], true);
        // pid 4242 is protected even though its app is on the allowlist.
        assert!(policy
            .check(NOW_MS, &window("customterm.exe", "x", 4242), Action::Input)
            .unwrap_err()
            .contains("próprio bridge"));
        assert!(policy
            .check(NOW_MS + 1, &window("customterm.exe", "x", 7), Action::Input)
            .is_ok());
    }
    #[test]
    fn sensitive_titles_are_refused() {
        let mut policy = policy_for(&["notepad.exe"], false);
        let result = policy.check(
            NOW_MS,
            &window("notepad.exe", "Minhas SENHAS.txt - Bloco de Notas", 1),
            Action::View,
        );
        assert!(result.unwrap_err().contains("senha"));
        assert!(!policy.visible_to_remote(&window("notepad.exe", "vault.txt", 1)));
    }
    #[test]
    fn unlisted_apps_are_invisible_to_the_remote_side() {
        let policy = policy_for(&["notepad.exe"], false);
        assert!(policy.visible_to_remote(&window("notepad.exe", "Notas", 1)));
        assert!(!policy.visible_to_remote(&window("chrome.exe", "Banco", 2)));
    }
    #[test]
    fn rate_limit_counts_denied_attempts_too() {
        let mut policy = policy_for(&["notepad.exe"], false);
        policy.config_mut().max_actions_per_minute = 3;
        let notepad = window("notepad.exe", "a", 1);
        let chrome = window("chrome.exe", "a", 2);
        assert!(policy.check(NOW_MS, &notepad, Action::View).is_ok());
        assert!(policy.check(NOW_MS + 1, &chrome, Action::View).is_err());
        assert!(policy.check(NOW_MS + 2, &chrome, Action::View).is_err());
        let limited = policy.check(NOW_MS + 3, &notepad, Action::View);
        assert!(limited.unwrap_err().contains("Limite"));
        // A minute later the window has slid past.
        assert!(policy
            .check(NOW_MS + 61_000, &notepad, Action::View)
            .is_ok());
    }
    #[test]
    fn listing_needs_control_on_and_is_rate_limited() {
        let mut off = Policy::new(PolicyConfig::default(), vec![]);
        assert!(off.check_session(NOW_MS).is_err());
        let mut on = policy_for(&[], false);
        on.config_mut().max_actions_per_minute = 2;
        assert!(on.check_session(NOW_MS).is_ok());
        assert!(on.check_session(NOW_MS + 1).is_ok());
        assert!(on.check_session(NOW_MS + 2).unwrap_err().contains("Limite"));
    }
    #[test]
    fn config_round_trips_and_survives_garbage() {
        let dir = std::env::temp_dir().join(format!("bridge-policy-{}", crate::clock::now_nanos()));
        let path = dir.join("policy.json");
        assert_eq!(PolicyConfig::load(&path), PolicyConfig::default());
        let config = PolicyConfig {
            allowed_apps: vec!["notepad.exe".into()],
            input: true,
            ..PolicyConfig::default()
        };
        config.save(&path).unwrap();
        assert_eq!(PolicyConfig::load(&path), config);
        fs::write(&path, "not json").unwrap();
        assert_eq!(PolicyConfig::load(&path), PolicyConfig::default());
        fs::remove_dir_all(dir).unwrap();
    }
}
