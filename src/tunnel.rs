//! Publishing control: turn the Tailscale Funnel off/on and honour an optional
//! deadline. The `paused` and `expose-until` markers are shared with
//! `scripts/bridge.ps1`, so the supervisor does not switch the Funnel back on.
//!
//! The only documented way to remove a Funnel rule in the installed Tailscale is
//! `funnel reset`, which clears *every* rule. It is therefore used only when the
//! bridge's own port is the only thing published.

use crate::{
    checks, clock,
    setup::{hidden_process, TaskEvent},
    storage::BridgeState,
};
use std::{path::PathBuf, sync::mpsc::Sender};

pub fn find_tailscale() -> Option<PathBuf> {
    checks::find_in_path("tailscale.exe").or_else(|| {
        let fallback = PathBuf::from(std::env::var_os("ProgramFiles")?)
            .join("Tailscale")
            .join("tailscale.exe");
        fallback.is_file().then_some(fallback)
    })
}

fn tailscale(args: &[&str]) -> Result<String, String> {
    let exe = find_tailscale().ok_or("Tailscale não encontrado")?;
    let output = hidden_process(&exe)
        .args(args)
        .output()
        .map_err(|e| format!("Falha ao executar o Tailscale: {e}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(text)
    } else {
        Err(text.trim().to_owned())
    }
}

/// Local ports that `tailscale funnel status` says are being published.
pub fn ports_in_status(status: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for prefix in ["127.0.0.1:", "localhost:"] {
        for (index, _) in status.match_indices(prefix) {
            let digits: String = status[index + prefix.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(port) = digits.parse() {
                ports.push(port);
            }
        }
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

#[derive(Debug, PartialEq, Eq)]
pub enum PausePlan {
    AlreadyOff,
    Reset,
    Refuse(Vec<u16>),
}

pub fn plan_pause(status: &str, port: u16) -> PausePlan {
    let ports = ports_in_status(status);
    if ports.is_empty() {
        return PausePlan::AlreadyOff;
    }
    let others: Vec<u16> = ports.into_iter().filter(|p| *p != port).collect();
    if others.is_empty() {
        PausePlan::Reset
    } else {
        PausePlan::Refuse(others)
    }
}

fn say(tx: &Sender<TaskEvent>, line: impl Into<String>) {
    let _ = tx.send(TaskEvent::Line(line.into()));
}

pub fn pause(port: u16, state: &BridgeState, tx: &Sender<TaskEvent>) -> Result<(), String> {
    // The marker goes first so the supervisor stops restoring the Funnel.
    state
        .set_paused(true)
        .map_err(|e| format!("Não consegui gravar a marca de pausa: {e}"))?;
    state
        .set_expose_until(None)
        .map_err(|e| format!("Não consegui limpar o prazo de exposição: {e}"))?;
    let status = tailscale(&["funnel", "status"])?;
    match plan_pause(&status, port) {
        PausePlan::AlreadyOff => {
            say(tx, "A publicação já estava desligada. Pausa registrada.");
            Ok(())
        }
        PausePlan::Refuse(others) => Err(format!(
            "O Funnel também publica as portas {others:?}; 'funnel reset' apagaria tudo. \
             A pausa foi marcada, mas o Funnel CONTINUA LIGADO: remova a regra da porta {port} manualmente."
        )),
        PausePlan::Reset => {
            tailscale(&["funnel", "reset"])?;
            let after = tailscale(&["funnel", "status"])?;
            if ports_in_status(&after).is_empty() {
                say(
                    tx,
                    "Publicação pausada. O servidor local continua ativo; a internet não alcança mais o PC.",
                );
                Ok(())
            } else {
                Err("O Funnel ainda aparece ativo depois do reset. Confira com o Tailscale.".into())
            }
        }
    }
}

pub fn resume(
    port: u16,
    minutes: Option<u32>,
    state: &BridgeState,
    tx: &Sender<TaskEvent>,
) -> Result<(), String> {
    let until = minutes.map(|m| clock::now_secs() + u64::from(m) * 60);
    state
        .set_paused(false)
        .and_then(|()| state.set_expose_until(until))
        .map_err(|e| format!("Não consegui atualizar o estado da publicação: {e}"))?;
    if let Err(err) = tailscale(&["funnel", "--bg", "--yes", &port.to_string()]) {
        // Roll back so the marker never claims the Funnel is on when it is not.
        let _ = state.set_paused(true);
        let _ = state.set_expose_until(None);
        return Err(format!("O Funnel não foi ligado: {err}"));
    }
    say(
        tx,
        match minutes {
            Some(m) => format!("Publicação retomada por {m} min."),
            None => "Publicação retomada sem prazo.".into(),
        },
    );
    Ok(())
}

pub fn expire_if_due(state: &BridgeState, now: u64) -> bool {
    matches!(state.expose_until(), Some(until) if now >= until)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: &str = "\n# Funnel on:\n#     - https://pc.example.ts.net\n\nhttps://pc.example.ts.net (Funnel on)\n|-- / proxy http://127.0.0.1:7676\n";

    #[test]
    fn ports_are_read_from_status() {
        assert_eq!(ports_in_status(ON), vec![7676]);
        let many = "|-- / proxy http://127.0.0.1:7676\n|-- /api proxy http://localhost:3000\n|-- /x proxy http://127.0.0.1:7676";
        assert_eq!(ports_in_status(many), vec![3000, 7676]);
        assert!(ports_in_status("No serve config").is_empty());
    }
    #[test]
    fn pause_plan_never_resets_other_rules() {
        assert_eq!(plan_pause("No serve config", 7676), PausePlan::AlreadyOff);
        assert_eq!(plan_pause(ON, 7676), PausePlan::Reset);
        let shared = format!("{ON}|-- /api proxy http://127.0.0.1:3000\n");
        assert_eq!(plan_pause(&shared, 7676), PausePlan::Refuse(vec![3000]));
        // Someone else's rule only: still not ours to reset.
        assert_eq!(
            plan_pause("|-- / proxy http://127.0.0.1:3000", 7676),
            PausePlan::Refuse(vec![3000])
        );
    }
    #[test]
    fn deadline_expires_only_when_reached() {
        let dir = std::env::temp_dir().join(format!("bridge-tunnel-{}", clock::now_nanos()));
        let state = BridgeState::in_dir(dir.clone());
        assert!(!expire_if_due(&state, 1_000));
        state.set_expose_until(Some(1_000)).unwrap();
        assert!(!expire_if_due(&state, 999));
        assert!(expire_if_due(&state, 1_000));
        assert!(expire_if_due(&state, 2_000));
        state.set_expose_until(None).unwrap();
        assert!(!expire_if_due(&state, 2_000));
        std::fs::remove_dir_all(dir).ok();
    }
}
