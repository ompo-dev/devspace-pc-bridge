//! Append-only record of what remote clients did. It never stores typed text,
//! only how long it was, so the log itself cannot leak what was entered.

use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Mutex,
};

const MAX_FIELD_CHARS: usize = 80;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AuditEntry {
    pub ts: u64,
    pub client: String,
    pub tool: String,
    pub window: String,
    pub process: String,
    pub outcome: String,
    pub detail: String,
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_FIELD_CHARS)
        .collect()
}

impl AuditEntry {
    pub fn new(
        ts: u64,
        client: &str,
        tool: &str,
        window: &str,
        process: &str,
        outcome: &str,
        detail: &str,
    ) -> Self {
        Self {
            ts,
            client: clean(client),
            tool: clean(tool),
            window: clean(window),
            process: clean(process),
            outcome: clean(outcome),
            detail: clean(detail),
        }
    }
}

pub trait AuditSink: Send + Sync {
    fn record(&self, entry: &AuditEntry);
}

pub struct FileAudit {
    path: PathBuf,
    max_bytes: u64,
    lock: Mutex<()>,
}
impl FileAudit {
    pub fn new(path: PathBuf, max_bytes: u64) -> Self {
        Self {
            path,
            max_bytes,
            lock: Mutex::new(()),
        }
    }
}
impl AuditSink for FileAudit {
    fn record(&self, entry: &AuditEntry) {
        let Ok(_guard) = self.lock.lock() else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let too_big = fs::metadata(&self.path).is_ok_and(|meta| meta.len() > self.max_bytes);
        if too_big {
            let _ = fs::rename(&self.path, self.path.with_extension("jsonl.1"));
        }
        let Ok(line) = serde_json::to_string(entry) else {
            return;
        };
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryAudit(pub Mutex<Vec<AuditEntry>>);
#[cfg(test)]
impl AuditSink for MemoryAudit {
    fn record(&self, entry: &AuditEntry) {
        if let Ok(mut entries) = self.0.lock() {
            entries.push(entry.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_cleaned_and_bounded() {
        let entry = AuditEntry::new(1, "chat\ngpt", "t", &"x".repeat(500), "p", "ok", "a\u{0}b");
        assert_eq!(entry.client, "chatgpt");
        assert_eq!(entry.window.chars().count(), MAX_FIELD_CHARS);
        assert_eq!(entry.detail, "ab");
    }
    #[test]
    fn entries_are_appended_as_json_lines_and_rotated() {
        let dir = std::env::temp_dir().join(format!("bridge-audit-{}", crate::clock::now_nanos()));
        let path = dir.join("audit.jsonl");
        let audit = FileAudit::new(path.clone(), 200);
        for round in 0..6 {
            audit.record(&AuditEntry::new(
                round,
                "c",
                "desktop_click",
                "w",
                "p",
                "ok",
                "d",
            ));
        }
        let current = fs::read_to_string(&path).unwrap();
        assert!(current
            .lines()
            .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok()));
        assert!(
            path.with_extension("jsonl.1").is_file(),
            "log should rotate past the size cap"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
