use crate::{
    cloudflare,
    setup::{Probe, Protection},
    tunnel,
};
use std::path::PathBuf;

pub struct Check {
    pub label: &'static str,
    pub ok: bool,
}

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Tools the installer relies on. Cheap enough to refresh with every status probe.
pub fn prerequisites() -> Vec<Check> {
    vec![
        Check {
            label: "WinGet",
            ok: find_in_path("winget.exe").is_some(),
        },
        Check {
            label: "Git",
            ok: find_in_path("git.exe").is_some(),
        },
        Check {
            label: "Node.js",
            ok: find_in_path("node.exe").is_some(),
        },
        Check {
            label: "DevSpace",
            ok: find_in_path("devspace.cmd").is_some(),
        },
        Check {
            label: "Tailscale",
            ok: tunnel::find_tailscale().is_some(),
        },
        Check {
            label: "Cloudflare",
            ok: cloudflare::find_cloudflared().is_some(),
        },
    ]
}

pub fn runtime_checks(probe: &Probe) -> Vec<Check> {
    vec![
        Check {
            label: "Servidor local",
            ok: probe.local == Some(200),
        },
        Check {
            label: "Publicação HTTPS",
            ok: probe.public == Some(200),
        },
        Check {
            label: "Proteção OAuth",
            ok: matches!(probe.protection, Some(Protection::Protected(_))),
        },
    ]
}

pub fn render(checks: &[Check]) -> String {
    checks
        .iter()
        .map(|check| format!("{} {}", if check.ok { "✔" } else { "✘" }, check.label))
        .collect::<Vec<_>>()
        .join("   ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_marks_each_item() {
        let checks = [
            Check {
                label: "A",
                ok: true,
            },
            Check {
                label: "B",
                ok: false,
            },
        ];
        assert_eq!(render(&checks), "✔ A   ✘ B");
    }
    #[test]
    fn runtime_checks_follow_the_probe() {
        let healthy = Probe {
            configured: true,
            local: Some(200),
            public: Some(200),
            protection: Some(Protection::Protected(401)),
            ..Probe::default()
        };
        assert!(runtime_checks(&healthy).iter().all(|check| check.ok));
        let open = Probe {
            protection: Some(Protection::Open(200)),
            ..healthy
        };
        assert!(!runtime_checks(&open)[2].ok);
        assert!(runtime_checks(&Probe::default()).iter().all(|c| !c.ok));
    }
    #[cfg(windows)]
    #[test]
    fn finds_system_tools_in_path() {
        assert!(find_in_path("cmd.exe").is_some());
        assert!(find_in_path("definitely-not-installed.exe").is_none());
    }
}
