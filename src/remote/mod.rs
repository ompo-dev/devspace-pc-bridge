//! The endpoint that lets a normal ChatGPT/Claude chat see and operate allowed
//! windows: an HTTP server (reached through a tunnel) speaking MCP, with its own
//! OAuth server. It runs inside this program, so the on/off switch, the pairing PIN
//! and the allowed-apps list are all under the user's eyes in the terminal UI.

pub mod http;
pub mod mcp;
pub mod oauth;

use crate::{
    clock,
    desktop::{
        audit::{AuditSink, FileAudit},
        policy::{Policy, PolicyConfig},
        Desktop,
    },
    storage::BridgeState,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

pub use http::start;
use mcp::McpContext;
use oauth::{AuthConfig, AuthServer};

/// Local port of the desktop endpoint. The DevSpace server itself uses 7676.
pub const DESKTOP_PORT: u16 = 7677;
const AUDIT_MAX_BYTES: u64 = 2 * 1024 * 1024;

pub struct RemoteState {
    pub auth: AuthServer,
    pub mcp: McpContext,
    pub public_origin: Mutex<Option<String>>,
    policy_path: PathBuf,
}

impl RemoteState {
    pub fn new(
        desktop: Arc<dyn Desktop>,
        protected_pids: Vec<u32>,
        data_dir: PathBuf,
        auth_config: AuthConfig,
    ) -> Self {
        let policy_path = data_dir.join("desktop-policy.json");
        let policy = Policy::new(PolicyConfig::load(&policy_path), protected_pids);
        let audit: Arc<dyn AuditSink> = Arc::new(FileAudit::new(
            data_dir.join("desktop-audit.jsonl"),
            AUDIT_MAX_BYTES,
        ));
        Self {
            auth: AuthServer::new(
                auth_config,
                Some(data_dir.join("desktop-oauth.json")),
                clock::now_secs(),
            ),
            mcp: McpContext {
                desktop,
                policy: Arc::new(Mutex::new(policy)),
                audit,
            },
            public_origin: Mutex::new(None),
            policy_path,
        }
    }
    pub fn default_data_dir() -> PathBuf {
        BridgeState::default().dir().to_path_buf()
    }
    pub fn policy(&self) -> MutexGuard<'_, Policy> {
        self.mcp
            .policy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    /// Saves the allowlist and switches. The on/off state is never saved: after a
    /// restart the control is always off.
    pub fn save_policy(&self) -> std::io::Result<()> {
        self.policy().config().save(&self.policy_path)
    }
    pub fn set_public_origin(&self, origin: Option<String>) {
        *self
            .public_origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = origin;
    }
    pub fn public_origin(&self) -> Option<String> {
        self.public_origin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}
