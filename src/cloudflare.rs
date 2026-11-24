//! Account-free Cloudflare Quick Tunnel for the optional desktop MCP endpoint.
//! The tunnel is never started without a local, typed confirmation in the TUI.
//! It is stopped when the program exits; no credentials are created or stored.

use crate::{checks, setup::TaskEvent};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Stdio},
    sync::mpsc::Sender,
    thread,
};

pub fn find_cloudflared() -> Option<PathBuf> {
    checks::find_in_path("cloudflared.exe").or_else(|| {
        ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .map(|path| path.join("cloudflared").join("cloudflared.exe"))
            .find(|path| path.is_file())
    })
}

/// Extract only the public origin, never arbitrary cloudflared log messages.
pub fn quick_tunnel_origin(line: &str) -> Option<String> {
    for word in line.split_whitespace() {
        let Some(start) = word.find("https://") else {
            continue;
        };
        let host: String = word[start + "https://".len()..]
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || *c == '.')
            .collect();
        if host.ends_with(".trycloudflare.com")
            && host.len() > ".trycloudflare.com".len()
            && host
                .split('.')
                .all(|part| !part.is_empty() && !part.starts_with('-') && !part.ends_with('-'))
        {
            return Some(format!("https://{host}"));
        }
    }
    None
}

pub struct Tunnel {
    child: Child,
}

impl Tunnel {
    pub fn start(tx: Sender<TaskEvent>) -> Result<Self, String> {
        let executable = find_cloudflared()
            .ok_or("cloudflared.exe nao encontrado. Instale Cloudflare primeiro.")?;
        let mut child = crate::setup::hidden_process(executable)
            .args([
                "tunnel",
                "--no-autoupdate",
                "--url",
                "http://127.0.0.1:7677",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("Falha iniciando cloudflared: {err}"))?;
        let stderr = child.stderr.take().ok_or("Cloudflare sem stderr")?;
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(origin) = quick_tunnel_origin(&line) {
                    let _ = tx.send(TaskEvent::CloudflareUrl(origin));
                    return;
                }
            }
        });
        Ok(Self { child })
    }
    pub fn exited(&mut self) -> bool {
        self.child
            .try_wait()
            .map_or(true, |status| status.is_some())
    }
    pub fn stop(&mut self) {
        if !self.exited() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_quick_tunnel_origins() {
        assert_eq!(
            quick_tunnel_origin("Visit https://lone-expression-boats-atm.trycloudflare.com/ now"),
            Some("https://lone-expression-boats-atm.trycloudflare.com".into())
        );
        assert_eq!(quick_tunnel_origin("https://evil.example.com"), None);
        assert_eq!(quick_tunnel_origin("http://foo.trycloudflare.com"), None);
    }
}
