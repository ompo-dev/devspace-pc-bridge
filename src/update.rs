//! Tells the user when a newer release exists. It never downloads or replaces
//! anything. Set `DEVSPACE_BRIDGE_NO_UPDATE_CHECK=1` to skip the request.

use crate::setup::TaskEvent;
use std::{sync::mpsc::Sender, thread, time::Duration};

const LATEST_URL: &str = "https://api.github.com/repos/ompo-dev/devspace-pc-bridge/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/ompo-dev/devspace-pc-bridge/releases";

pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    // "1-beta" counts as 1.
    let patch = parts
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

fn latest_release_tag() -> Option<String> {
    if std::env::var_os("DEVSPACE_BRIDGE_NO_UPDATE_CHECK").is_some() {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(7))
        .user_agent(concat!("DevSpacePCBridge/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let body = client
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    json.get("tag_name")?.as_str().map(str::to_owned)
}

pub fn spawn_check(tx: Sender<TaskEvent>) {
    thread::spawn(move || {
        let Some(tag) = latest_release_tag() else {
            return;
        };
        if is_newer(env!("CARGO_PKG_VERSION"), &tag) {
            let _ = tx.send(TaskEvent::Update(tag));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_parsed() {
        assert_eq!(parse_version("v0.4.1"), Some((0, 4, 1)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version("1.2"), None);
    }
    #[test]
    fn only_strictly_newer_versions_count() {
        assert!(is_newer("0.4.0", "v0.4.1"));
        assert!(is_newer("0.4.9", "0.10.0"));
        assert!(!is_newer("0.4.0", "v0.4.0"));
        assert!(!is_newer("0.5.0", "v0.4.9"));
        assert!(!is_newer("0.4.0", "garbage"));
    }
}
