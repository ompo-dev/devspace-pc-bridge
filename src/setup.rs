use crate::storage;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc::Sender,
    thread,
};

const INSTALL: &str = include_str!("../install.ps1");
const SUPERVISOR: &str = include_str!("../scripts/bridge.ps1");
const DOCTOR: &str = include_str!("../scripts/doctor.ps1");
const UNINSTALL: &str = include_str!("../scripts/uninstall.ps1");

#[derive(Debug, Clone, Copy)]
pub enum Task {
    InstallBridge,
    Diagnose,
    InstallCodex,
    InstallClaude,
    LoginCodex,
    LoginClaude,
    LinkCodexMcp,
    LinkClaudeMcp,
    AuthCodexMcp,
    AuthClaudeMcp,
}

impl Task {
    pub fn label(self) -> &'static str {
        match self {
            Self::InstallBridge => "Preparando DevSpace e Tailscale",
            Self::Diagnose => "Verificando instalacao",
            Self::InstallCodex => "Instalando Codex CLI",
            Self::InstallClaude => "Instalando Claude Code CLI",
            Self::LoginCodex => "Autorizando conta ChatGPT / Codex",
            Self::LoginClaude => "Autorizando Claude Code",
            Self::LinkCodexMcp => "Registrando DevSpace no Codex",
            Self::LinkClaudeMcp => "Registrando DevSpace no Claude Code",
            Self::AuthCodexMcp => "Autorizando DevSpace MCP no Codex",
            Self::AuthClaudeMcp => "Autorizando DevSpace MCP no Claude Code",
        }
    }
}

pub enum TaskEvent {
    ProcessStarted(u32),
    Line(String),
    Finished(Result<(), String>),
}

fn bundled_scripts() -> Result<PathBuf, String> {
    let base = std::env::temp_dir().join(format!("devspace-pc-bridge-{}", std::process::id()));
    fs::create_dir_all(base.join("scripts")).map_err(|error| error.to_string())?;
    for (name, value) in [
        ("install.ps1", INSTALL),
        ("scripts/bridge.ps1", SUPERVISOR),
        ("scripts/doctor.ps1", DOCTOR),
        ("scripts/uninstall.ps1", UNINSTALL),
    ] {
        fs::write(base.join(name), value).map_err(|error| error.to_string())?;
    }
    Ok(base)
}

fn hidden_command(name: &str) -> Command {
    #[cfg(windows)]
    let executable = match name {
        "codex" => "codex.cmd",
        "claude" => "claude.cmd",
        other => other,
    };
    #[cfg(not(windows))]
    let executable = name;
    let mut command = Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

fn run_process(mut command: Command, tx: &Sender<TaskEvent>) -> Result<(), String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    let mut child = command.spawn().map_err(|error| format!("Falha ao iniciar: {error}"))?;
    let _ = tx.send(TaskEvent::ProcessStarted(child.id()));
    let stdout = child.stdout.take().ok_or("stdout indisponivel")?;
    let stderr = child.stderr.take().ok_or("stderr indisponivel")?;

    let out_sender = tx.clone();
    let output = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = out_sender.send(TaskEvent::Line(line));
        }
    });
    let error_sender = tx.clone();
    let error_output = thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = error_sender.send(TaskEvent::Line(format!("! {line}")));
        }
    });
    let code = child.wait().map_err(|error| format!("Falha aguardando comando: {error}"))?;
    let _ = output.join();
    let _ = error_output.join();
    if code.success() {
        Ok(())
    } else {
        Err(format!("Comando terminou com codigo {code}"))
    }
}

pub fn start_task(task: Task, root: PathBuf, tx: Sender<TaskEvent>) {
    thread::spawn(move || {
        let _ = tx.send(TaskEvent::Line(format!("> {}", task.label())));
        let result = execute(task, root, &tx);
        let _ = tx.send(TaskEvent::Finished(result));
    });
}

fn execute(task: Task, root: PathBuf, tx: &Sender<TaskEvent>) -> Result<(), String> {
    let command = match task {
        Task::InstallBridge | Task::Diagnose => {
            let path = bundled_scripts()?;
            let script = if matches!(task, Task::InstallBridge) {
                if !root.is_dir() {
                    return Err("Pasta de projetos inexistente".into());
                }
                path.join("install.ps1")
            } else {
                path.join("scripts").join("doctor.ps1")
            };
            let mut command = hidden_command("powershell.exe");
            command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
            command.arg(script);
            if matches!(task, Task::InstallBridge) {
                command.arg("-WorkspacesRoot").arg(root);
            }
            command
        }
        Task::InstallCodex | Task::InstallClaude => {
            let mut command = hidden_command("npm.cmd");
            command.args(["install", "--global"]);
            command.arg(if matches!(task, Task::InstallCodex) {
                "@openai/codex"
            } else {
                "@anthropic-ai/claude-code"
            });
            command
        }
        Task::LoginCodex => {
            let mut command = hidden_command("codex");
            command.arg("login");
            command
        }
        Task::LoginClaude => {
            let mut command = hidden_command("claude");
            command.args(["auth", "login"]);
            command
        }
        Task::LinkCodexMcp | Task::LinkClaudeMcp | Task::AuthCodexMcp | Task::AuthClaudeMcp => {
            let url = storage::devspace_mcp_url()
                .ok_or("DevSpace ainda nao foi configurado. Configure primeiro.")?;
            if matches!(task, Task::LinkCodexMcp) {
                let mut command = hidden_command("codex");
                command.args(["mcp", "add", "devspace", "--url", &url]);
                command
            } else if matches!(task, Task::AuthCodexMcp) {
                let mut command = hidden_command("codex");
                command.args(["mcp", "login", "devspace", "--oauth-client-registration", "DCR"]);
                command
            } else if matches!(task, Task::AuthClaudeMcp) {
                let mut command = hidden_command("claude");
                command.args(["mcp", "login", "devspace"]);
                command
            } else {
                let mut command = hidden_command("claude");
                command.args(["mcp", "add", "--transport", "http", "--scope", "user", "devspace", &url]);
                command
            }
        }
    };
    run_process(command, tx)
}

pub fn copy_url() -> Result<String, String> {
    let url = storage::devspace_mcp_url().ok_or("Nenhuma URL MCP configurada")?;
    let mut child = hidden_command("clip.exe")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    use std::io::Write;
    child
        .stdin
        .take()
        .ok_or("Clipboard indisponivel")?
        .write_all(url.as_bytes())
        .map_err(|error| error.to_string())?;
    let status = child.wait().map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("Falha ao copiar para o clipboard".into());
    }
    Ok(url)
}

pub fn open_panel() -> Result<(), String> {
    let url = storage::devspace_mcp_url().ok_or("Nenhum MCP configurado")?;
    let panel = url.trim_end_matches("/mcp").to_owned() + "/web/";
    // Default browser can open a GUI window, but no extra terminal/CMD window.
    hidden_command("explorer.exe")
        .arg(panel)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn test_mcp_endpoint() -> Result<String, String> {
    let url = storage::devspace_mcp_url().ok_or("URL MCP ausente")?;
    let origin = url.trim_end_matches("/mcp");
    let response = reqwest::blocking::Client::new()
        .get(format!("{origin}/healthz"))
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .map_err(|error| format!("Health publico indisponivel: {error}"))?;
    Ok(format!(
        "Health HTTPS: HTTP {} (autenticacao MCP deve ser validada no cliente)",
        response.status()
    ))
}

pub fn ensure_scripts_have_no_keys() -> bool {
    !INSTALL.contains("gho_") && !SUPERVISOR.contains("gho_") && !DOCTOR.contains("gho_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_scripts_exist() {
        assert!(INSTALL.contains("DevSpacePCBridge"));
        assert!(SUPERVISOR.contains("settings.json"));
        assert!(UNINSTALL.contains("Unregister-ScheduledTask"));
        assert!(ensure_scripts_have_no_keys());
    }
}
