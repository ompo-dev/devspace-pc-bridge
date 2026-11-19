use crate::storage::{self, ChatClient};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::Sender,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const INSTALL: &str = include_str!("../install.ps1");
const SUPERVISOR: &str = include_str!("../scripts/bridge.ps1");
const DOCTOR: &str = include_str!("../scripts/doctor.ps1");
const UNINSTALL: &str = include_str!("../scripts/uninstall.ps1");

const LOCAL_HEALTH: &str = "http://127.0.0.1:7676/healthz";
const STATUS_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    Install,
    Doctor,
    CheckHealth,
    Uninstall { disable_funnel: bool },
}
impl Task {
    pub fn label(self) -> &'static str {
        match self {
            Self::Install => "Instalação do bridge",
            Self::Doctor => "Diagnóstico local",
            Self::CheckHealth => "Teste da conexão local e HTTPS",
            Self::Uninstall {
                disable_funnel: false,
            } => "Remoção da tarefa do bridge",
            Self::Uninstall {
                disable_funnel: true,
            } => "Remoção da tarefa do bridge e do Funnel",
        }
    }
    /// Script inside the temporary bundle, or `None` for tasks done in Rust.
    fn script(self) -> Option<&'static str> {
        match self {
            Self::Install => Some("install.ps1"),
            Self::Doctor => Some("scripts/doctor.ps1"),
            Self::Uninstall { .. } => Some("scripts/uninstall.ps1"),
            Self::CheckHealth => None,
        }
    }
}

/// Result of probing the local and public health endpoints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub configured: bool,
    pub local: Option<u16>,
    pub public: Option<u16>,
}
impl Probe {
    pub fn healthy(&self) -> bool {
        self.local == Some(200) && self.public == Some(200)
    }
}
pub fn describe(status: Option<u16>) -> String {
    status.map_or_else(|| "sem resposta".into(), |code| code.to_string())
}

pub enum TaskEvent {
    Started(u32),
    Line(String),
    Done(Result<(), String>),
    Status(Probe),
}

#[cfg(windows)]
fn hidden_process(name: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(name);
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: logs are shown inside Ratatui.
    cmd
}
#[cfg(not(windows))]
fn hidden_process(name: &str) -> Command {
    Command::new(name)
}

pub fn open_chat(client: ChatClient) -> Result<(), String> {
    hidden_process("explorer.exe")
        .arg(client.link())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Não foi possível abrir o navegador: {e}"))
}
pub fn open_owner_panel() -> Result<(), String> {
    let url = storage::mcp_url().ok_or("Bridge não configurado")?;
    hidden_process("explorer.exe")
        .arg(format!("{}/web/", url.trim_end_matches("/mcp")))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Não foi possível abrir o painel: {e}"))
}
pub fn copy_url() -> Result<String, String> {
    let url = storage::mcp_url().ok_or("URL MCP não encontrada. Configure o DevSpace primeiro.")?;
    let mut child = hidden_process("clip.exe")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Clipboard: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("Clipboard indisponível")?
        .write_all(url.as_bytes())
        .map_err(|e| e.to_string())?;
    if !child.wait().map_err(|e| e.to_string())?.success() {
        return Err("Falha ao copiar para o clipboard".into());
    }
    Ok(url)
}

fn write_bundle(dir: &Path) -> Result<(), String> {
    fs::create_dir(dir.join("scripts")).map_err(|e| e.to_string())?;
    for (name, contents) in [
        ("install.ps1", INSTALL),
        ("scripts/bridge.ps1", SUPERVISOR),
        ("scripts/doctor.ps1", DOCTOR),
        ("scripts/uninstall.ps1", UNINSTALL),
    ] {
        fs::write(dir.join(name), contents).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Writes the embedded scripts to a fresh directory. `create_dir` (not
/// `create_dir_all`) fails if the name already exists, so another process can
/// not pre-create it and swap scripts between writing and running them.
fn script_bundle() -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "devspace-bridge-setup-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir(&dir).map_err(|e| format!("Pasta temporária: {e}"))?;
    if let Err(err) = write_bundle(&dir) {
        let _ = fs::remove_dir_all(&dir);
        return Err(err);
    }
    Ok(dir)
}

fn spawn_command(mut cmd: Command, tx: &Sender<TaskEvent>) -> Result<(), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    let exit = child
        .wait()
        .map_err(|e| format!("Falha aguardando comando: {e}"))?;
    let _ = thread_out.join();
    let _ = thread_err.join();
    if exit.success() {
        Ok(())
    } else {
        Err(format!("Comando terminou com {exit}"))
    }
}

pub fn start(task: Task, root: PathBuf, tx: Sender<TaskEvent>) {
    thread::spawn(move || {
        let _ = tx.send(TaskEvent::Line(format!("> {}", task.label())));
        let result = run_task(task, &root, &tx);
        let _ = tx.send(TaskEvent::Done(result));
    });
}

fn run_task(task: Task, root: &Path, tx: &Sender<TaskEvent>) -> Result<(), String> {
    let Some(script) = task.script() else {
        return health(tx);
    };
    if task == Task::Install {
        if !root.is_dir() {
            return Err("Selecione uma pasta válida".into());
        }
        if let Some(reason) = storage::root_risk(root) {
            return Err(format!(
                "Pasta recusada ({reason}). Escolha a pasta dos seus projetos em Configurar > R."
            ));
        }
    }
    let base = script_bundle()?;
    let mut cmd = hidden_process("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(base.join(script));
    match task {
        Task::Install => {
            cmd.arg("-WorkspacesRoot").arg(storage::plain_path(root));
        }
        Task::Uninstall {
            disable_funnel: true,
        } => {
            cmd.arg("-DisableFunnel");
        }
        _ => {}
    }
    let outcome = spawn_command(cmd, tx);
    // Only the temporary copies are removed, never the installed bridge or its config.
    if let Err(err) = fs::remove_dir_all(&base) {
        let _ = tx.send(TaskEvent::Line(format!(
            "Aviso: não consegui remover a pasta temporária: {err}"
        )));
    }
    outcome
}

fn check_url(url: &str) -> Result<u16, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(7))
        .build()
        .map_err(|e| e.to_string())?;
    client
        .get(url)
        .send()
        .map(|r| r.status().as_u16())
        .map_err(|e| e.to_string())
}
fn public_health(mcp_url: &str) -> String {
    format!("{}/healthz", mcp_url.trim_end_matches("/mcp"))
}

pub fn probe() -> Probe {
    let mcp = storage::mcp_url();
    Probe {
        configured: mcp.is_some(),
        local: check_url(LOCAL_HEALTH).ok(),
        public: mcp
            .as_deref()
            .and_then(|url| check_url(&public_health(url)).ok()),
    }
}

/// Probes in the background so the Home screen can show a live state.
/// Stops by itself once the receiving side is gone.
pub fn watch_status(tx: Sender<TaskEvent>) {
    thread::spawn(move || loop {
        if tx.send(TaskEvent::Status(probe())).is_err() {
            break;
        }
        thread::sleep(STATUS_INTERVAL);
    });
}

fn health(tx: &Sender<TaskEvent>) -> Result<(), String> {
    let local_status = check_url(LOCAL_HEALTH);
    let _ = tx.send(TaskEvent::Line(format!(
        "Local: {LOCAL_HEALTH} => {local_status:?}"
    )));
    let mcp = storage::mcp_url().ok_or("Ainda não há endpoint MCP configurado")?;
    let public = public_health(&mcp);
    let public_status = check_url(&public);
    let _ = tx.send(TaskEvent::Line(format!(
        "HTTPS: {public} => {public_status:?}"
    )));
    let _ = tx.send(TaskEvent::Line(
        "HTTP 200 apenas verifica rede. OAuth e ferramentas requerem teste no chat.".into(),
    ));
    match (local_status, public_status) {
        (Ok(200), Ok(200)) => Ok(()),
        _ => Err("Conexão ainda não está saudável. Veja o diagnóstico e o Tailscale.".into()),
    }
}

pub fn status_summary() -> String {
    match storage::mcp_url() {
        Some(url) => format!(
            "Endpoint identificado: {url}\nOwner/OAuth deve ser autorizado no chat.\nO servidor continua no seu computador."
        ),
        None => "Ainda não foi encontrada uma URL MCP válida.\nUse Configurar para instalar e publicar.".into(),
    }
}

pub fn stop_pid(pid: u32) -> Result<(), String> {
    let mut cmd = hidden_process("taskkill.exe");
    let result = cmd
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()
        .map_err(|e| e.to_string())?;
    if result.status.success() {
        Ok(())
    } else {
        Err("Não foi possível cancelar o comando".into())
    }
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
    #[test]
    fn every_script_task_has_an_embedded_script() {
        let dir = script_bundle().expect("bundle");
        for task in [
            Task::Install,
            Task::Doctor,
            Task::Uninstall {
                disable_funnel: false,
            },
            Task::Uninstall {
                disable_funnel: true,
            },
        ] {
            let script = task.script().expect("script task");
            assert!(dir.join(script).is_file(), "{script} missing from bundle");
        }
        assert!(Task::CheckHealth.script().is_none());
        fs::remove_dir_all(dir).expect("cleanup");
    }
    #[test]
    fn bundle_directories_are_never_reused() {
        let first = script_bundle().expect("first");
        let second = script_bundle().expect("second");
        assert_ne!(first, second);
        fs::remove_dir_all(first).expect("cleanup");
        fs::remove_dir_all(second).expect("cleanup");
    }
    #[test]
    fn probe_is_healthy_only_when_both_answer_200() {
        let ok = Probe {
            configured: true,
            local: Some(200),
            public: Some(200),
        };
        assert!(ok.healthy());
        assert!(!Probe {
            public: Some(502),
            ..ok
        }
        .healthy());
        assert!(!Probe { local: None, ..ok }.healthy());
        assert!(!Probe::default().healthy());
    }
    #[test]
    fn public_health_drops_the_mcp_suffix() {
        assert_eq!(
            public_health("https://pc.example.ts.net/mcp"),
            "https://pc.example.ts.net/healthz"
        );
    }
}
