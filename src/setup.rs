use crate::storage::{self, ChatClient};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc::Sender,
    thread,
    time::Duration,
};

const INSTALL: &str = include_str!("../install.ps1");
const SUPERVISOR: &str = include_str!("../scripts/bridge.ps1");
const DOCTOR: &str = include_str!("../scripts/doctor.ps1");
const UNINSTALL: &str = include_str!("../scripts/uninstall.ps1");

#[derive(Clone, Copy, Debug)]
pub enum Task {
    Install,
    Doctor,
    CheckHealth,
}
impl Task {
    pub fn label(self) -> &'static str {
        match self {
            Self::Install => "Instalacao do bridge",
            Self::Doctor => "Diagnostico local",
            Self::CheckHealth => "Teste da conexao local e HTTPS",
        }
    }
}

pub enum TaskEvent {
    Started(u32),
    Line(String),
    Done(Result<(), String>),
}

#[cfg(windows)]
fn hidden_process(name: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(name);
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: logs are shown inside Ratatui.
    cmd
}
#[cfg(not(windows))]
fn hidden_process(name: &str) -> Command { Command::new(name) }

pub fn open_chat(client: ChatClient) -> Result<(), String> {
    hidden_process("explorer.exe")
        .arg(client.link())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Nao foi possivel abrir navegador: {e}"))
}
pub fn open_owner_panel() -> Result<(), String> {
    let url = storage::mcp_url().ok_or("Bridge nao configurado")?;
    hidden_process("explorer.exe")
        .arg(format!("{}/web/", url.trim_end_matches("/mcp")))
        .spawn().map(|_| ())
        .map_err(|e| format!("Nao foi possivel abrir painel: {e}"))
}
pub fn copy_url() -> Result<String, String> {
    let url = storage::mcp_url().ok_or("URL MCP nao encontrada. Configure o DevSpace primeiro.")?;
    let mut child = hidden_process("clip.exe")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Clipboard: {e}"))?;
    child.stdin.take().ok_or("Clipboard nao disponivel")?
        .write_all(url.as_bytes())
        .map_err(|e| e.to_string())?;
    if !child.wait().map_err(|e| e.to_string())?.success() {
        return Err("Falha ao copiar para o clipboard".into());
    }
    Ok(url)
}

fn script_bundle() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("devspace-bridge-setup-{}", std::process::id()));
    fs::create_dir_all(dir.join("scripts")).map_err(|e| e.to_string())?;
    for (name, contents) in [
        ("install.ps1", INSTALL),
        ("scripts/bridge.ps1", SUPERVISOR),
        ("scripts/doctor.ps1", DOCTOR),
        ("scripts/uninstall.ps1", UNINSTALL),
    ] {
        fs::write(dir.join(name), contents).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

fn spawn_command(mut cmd: Command, tx: &Sender<TaskEvent>) -> Result<(), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Falha ao iniciar: {e}"))?;
    let _ = tx.send(TaskEvent::Started(child.id()));
    let out = child.stdout.take().ok_or("Sem stdout")?;
    let err = child.stderr.take().ok_or("Sem stderr")?;
    let output_tx = tx.clone();
    let thread_out = thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = output_tx.send(TaskEvent::Line(line));
        }
    });
    let error_tx = tx.clone();
    let thread_err = thread::spawn(move || {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            let _ = error_tx.send(TaskEvent::Line(format!("! {line}")));
        }
    });
    let exit = child.wait().map_err(|e| format!("Falha aguardando comando: {e}"))?;
    let _ = thread_out.join();
    let _ = thread_err.join();
    if exit.success() { Ok(()) } else { Err(format!("Comando terminou com {exit}")) }
}

pub fn start(task: Task, root: PathBuf, tx: Sender<TaskEvent>) {
    thread::spawn(move || {
        let _ = tx.send(TaskEvent::Line(format!("> {}", task.label())));
        let result = match task {
            Task::Install | Task::Doctor => {
                let scripts = script_bundle();
                match scripts {
                    Err(err) => Err(err),
                    Ok(base) => {
                        let path = if matches!(task, Task::Install) {
                            base.join("install.ps1")
                        } else {
                            base.join("scripts").join("doctor.ps1")
                        };
                        let mut cmd = hidden_process("powershell.exe");
                        cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(path);
                        if matches!(task, Task::Install) {
                            if !root.is_dir() {
                                let _ = tx.send(TaskEvent::Done(Err("Selecione uma pasta valida".into())));
                                return;
                            }
                            cmd.arg("-WorkspacesRoot").arg(root);
                        }
                        let outcome = spawn_command(cmd, &tx);
                        // Never delete the installed bridge script/config; only temporary copies.
                        let _ = fs::remove_dir_all(base);
                        outcome
                    }
                }
            }
            Task::CheckHealth => health(&tx),
        };
        let _ = tx.send(TaskEvent::Done(result));
    });
}

fn check_url(url: &str) -> Result<u16, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(7))
        .build().map_err(|e| e.to_string())?;
    client.get(url).send().map(|r| r.status().as_u16()).map_err(|e| e.to_string())
}
fn health(tx: &Sender<TaskEvent>) -> Result<(), String> {
    let local = "http://127.0.0.1:7676/healthz";
    let local_status = check_url(local);
    let _ = tx.send(TaskEvent::Line(format!("Local: {local} => {local_status:?}")));
    let mcp = storage::mcp_url().ok_or("Ainda nao ha endpoint MCP configurado")?;
    let public = format!("{}/healthz", mcp.trim_end_matches("/mcp"));
    let public_status = check_url(&public);
    let _ = tx.send(TaskEvent::Line(format!("HTTPS: {public} => {public_status:?}")));
    let _ = tx.send(TaskEvent::Line("HTTP 200 apenas verifica rede. OAuth e ferramentas requerem teste no chat.".into()));
    match (local_status, public_status) {
        (Ok(200), Ok(200)) => Ok(()),
        _ => Err("Conexao ainda nao esta saudavel. Veja diagnostico e Tailscale.".into()),
    }
}

pub fn status_summary() -> String {
    match storage::mcp_url() {
        Some(url) => format!("Endpoint identificado: {url}\nOwner/OAuth deve ser autorizado no chat.\nO servidor continua no seu computador."),
        None => "Ainda nao foi encontrada uma URL MCP valida.\nUse Configurar bridge para instalar e publicar.".into(),
    }
}

pub fn stop_pid(pid: u32) -> Result<(), String> {
    let mut cmd = hidden_process("taskkill.exe");
    let result = cmd.args(["/PID", &pid.to_string(), "/T", "/F"]).output().map_err(|e| e.to_string())?;
    if result.status.success() { Ok(()) } else { Err("Nao foi possivel cancelar o comando".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundle_contains_real_scripts() {
        assert!(INSTALL.contains("DevSpacePCBridge"));
        assert!(DOCTOR.contains("healthz"));
        assert!(SUPERVISOR.contains("settings.json"));
        assert!(UNINSTALL.contains("Unregister-ScheduledTask"));
    }
    #[test]
    fn platform_is_normal_chat() {
        assert!(ChatClient::ChatGpt.instructions()[0].contains("ChatGPT"));
        assert!(ChatClient::Claude.instructions()[0].contains("Claude"));
    }
}
