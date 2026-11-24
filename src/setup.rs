use crate::{
    checkpoint, clock,
    storage::{self, BridgeState, ChatClient},
    tunnel,
};
use std::{
    ffi::OsStr,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::Sender,
    thread,
    time::Duration,
};

const INSTALL: &str = include_str!("../install.ps1");
const SUPERVISOR: &str = include_str!("../scripts/bridge.ps1");
const DOCTOR: &str = include_str!("../scripts/doctor.ps1");
const UNINSTALL: &str = include_str!("../scripts/uninstall.ps1");
const COMPONENT_INSTALL: &str = include_str!("../scripts/install-component.ps1");

pub const PORT: u16 = 7676;
const LOCAL_HEALTH: &str = "http://127.0.0.1:7676/healthz";
const STATUS_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Git,
    Node,
    Tailscale,
    Cloudflared,
    DevSpace,
}
impl Component {
    pub fn name(self) -> &'static str {
        match self {
            Self::Git => "Git",
            Self::Node => "Node",
            Self::Tailscale => "Tailscale",
            Self::Cloudflared => "Cloudflared",
            Self::DevSpace => "DevSpace",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    Install,
    InstallComponent(Component),
    Doctor,
    CheckHealth,
    Uninstall { disable_funnel: bool },
    Pause,
    Resume { minutes: Option<u32> },
    Checkpoint,
}
impl Task {
    pub fn label(self) -> &'static str {
        match self {
            Self::Install => "Instalação do bridge",
            Self::InstallComponent(component) => component.name(),
            Self::Doctor => "Diagnóstico local",
            Self::CheckHealth => "Teste da conexão local, HTTPS e OAuth",
            Self::Uninstall {
                disable_funnel: false,
            } => "Remoção da tarefa do bridge",
            Self::Uninstall {
                disable_funnel: true,
            } => "Remoção da tarefa do bridge e do Funnel",
            Self::Pause => "Pausa da publicação",
            Self::Resume { .. } => "Retomada da publicação",
            Self::Checkpoint => "Checkpoint dos repositórios",
        }
    }
    /// Script inside the temporary bundle, or `None` for tasks done in Rust.
    fn script(self) -> Option<&'static str> {
        match self {
            Self::Install => Some("install.ps1"),
            Self::InstallComponent(_) => Some("scripts/install-component.ps1"),
            Self::Doctor => Some("scripts/doctor.ps1"),
            Self::Uninstall { .. } => Some("scripts/uninstall.ps1"),
            Self::CheckHealth | Self::Pause | Self::Resume { .. } | Self::Checkpoint => None,
        }
    }
}

/// What an unauthenticated request to the public `/mcp` endpoint reveals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    /// 401/403: the server demands credentials.
    Protected(u16),
    /// A success answer without any credentials.
    Open(u16),
    Inconclusive(Option<u16>),
}
pub fn classify_protection(status: u16) -> Protection {
    match status {
        401 | 403 => Protection::Protected(status),
        200..=299 => Protection::Open(status),
        other => Protection::Inconclusive(Some(other)),
    }
}
pub fn describe_protection(protection: Option<Protection>) -> String {
    match protection {
        Some(Protection::Protected(code)) => format!("ativa (HTTP {code} sem token)"),
        Some(Protection::Open(code)) => {
            format!("ABERTO: respondeu HTTP {code} sem nenhum token!")
        }
        Some(Protection::Inconclusive(Some(code))) => format!("inconclusiva (HTTP {code})"),
        Some(Protection::Inconclusive(None)) => "inconclusiva (sem resposta)".into(),
        None => "não verificada".into(),
    }
}

/// Result of probing the local and public endpoints plus the publishing markers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub configured: bool,
    pub local: Option<u16>,
    pub public: Option<u16>,
    pub protection: Option<Protection>,
    pub paused: bool,
    pub expose_until: Option<u64>,
}
impl Probe {
    pub fn healthy(&self) -> bool {
        self.local == Some(200) && self.public == Some(200)
    }
    pub fn open_without_auth(&self) -> bool {
        matches!(self.protection, Some(Protection::Open(_)))
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
    Update(String),
    CloudflareUrl(String),
    TrayDisable,
    TrayQuit,
}

#[cfg(windows)]
pub(crate) fn hidden_process(program: impl AsRef<OsStr>) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(program);
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: logs are shown inside Ratatui.
    cmd
}
#[cfg(not(windows))]
pub(crate) fn hidden_process(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
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

/// `clip.exe` reads stdin in the console code page, so UTF-8 accents come out garbled.
/// It does detect UTF-16LE on its own, but a BOM would leak into the pasted text
/// as a stray U+FEFF, so none is written. Plain ASCII is sent unchanged.
fn clipboard_bytes(text: &str) -> Vec<u8> {
    if text.is_ascii() {
        return text.as_bytes().to_vec();
    }
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}
pub fn copy_text(text: &str) -> Result<(), String> {
    let mut child = hidden_process("clip.exe")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Clipboard: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("Clipboard indisponível")?
        .write_all(&clipboard_bytes(text))
        .map_err(|e| e.to_string())?;
    if !child.wait().map_err(|e| e.to_string())?.success() {
        return Err("Falha ao copiar para o clipboard".into());
    }
    Ok(())
}
pub fn copy_url() -> Result<String, String> {
    let url = storage::mcp_url().ok_or("URL MCP não encontrada. Configure o DevSpace primeiro.")?;
    copy_text(&url)?;
    Ok(url)
}

fn write_bundle(dir: &Path) -> Result<(), String> {
    fs::create_dir(dir.join("scripts")).map_err(|e| e.to_string())?;
    for (name, contents) in [
        ("install.ps1", INSTALL),
        ("scripts/bridge.ps1", SUPERVISOR),
        ("scripts/doctor.ps1", DOCTOR),
        ("scripts/uninstall.ps1", UNINSTALL),
        ("scripts/install-component.ps1", COMPONENT_INSTALL),
    ] {
        fs::write(dir.join(name), contents).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Writes the embedded scripts to a fresh directory. `create_dir` (not
/// `create_dir_all`) fails if the name already exists, so another process can
/// not pre-create it and swap scripts between writing and running them.
fn script_bundle() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!(
        "devspace-bridge-setup-{}-{}",
        std::process::id(),
        clock::now_nanos()
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
    match task {
        Task::CheckHealth => health(tx),
        Task::Pause => tunnel::pause(PORT, &BridgeState::default(), tx),
        Task::Resume { minutes } => tunnel::resume(PORT, minutes, &BridgeState::default(), tx),
        Task::Checkpoint => checkpoint::snapshot_all(root, &mut |line| {
            let _ = tx.send(TaskEvent::Line(line));
        }),
        Task::Install | Task::InstallComponent(_) | Task::Doctor | Task::Uninstall { .. } => {
            run_script(task, root, tx)
        }
    }
}

fn run_script(task: Task, root: &Path, tx: &Sender<TaskEvent>) -> Result<(), String> {
    let script = task.script().ok_or("Esta tarefa não usa script")?;
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
        Task::InstallComponent(component) => {
            cmd.arg("-Component").arg(component.name());
        }
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

/// A plain GET with no credentials. A protected server answers 401/403; a 2xx
/// means anyone on the internet can talk to the MCP endpoint.
fn check_protection(mcp_url: &str) -> Protection {
    match check_url(mcp_url) {
        Ok(status) => classify_protection(status),
        Err(_) => Protection::Inconclusive(None),
    }
}

pub fn probe() -> Probe {
    let state = BridgeState::default();
    let mcp = storage::mcp_url();
    let public = mcp
        .as_deref()
        .and_then(|url| check_url(&public_health(url)).ok());
    let protection = match (&mcp, public) {
        (Some(url), Some(200)) => Some(check_protection(url)),
        _ => None,
    };
    Probe {
        configured: mcp.is_some(),
        local: check_url(LOCAL_HEALTH).ok(),
        public,
        protection,
        paused: state.paused(),
        expose_until: state.expose_until(),
    }
}

/// Probes in the background so the Home screen can show a live state, and ends
/// a timed exposure when its deadline passes. Stops once the receiver is gone.
pub fn watch_status(tx: Sender<TaskEvent>) {
    thread::spawn(move || loop {
        let state = BridgeState::default();
        if tunnel::expire_if_due(&state, clock::now_secs()) {
            let _ = tx.send(TaskEvent::Line(
                "Prazo de exposição encerrado; pausando a publicação.".into(),
            ));
            if let Err(err) = tunnel::pause(PORT, &state, &tx) {
                let _ = tx.send(TaskEvent::Line(format!("Falha ao pausar: {err}")));
            }
        }
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
    let protection = matches!(public_status, Ok(200)).then(|| check_protection(&mcp));
    let _ = tx.send(TaskEvent::Line(format!(
        "Proteção OAuth: {}",
        describe_protection(protection)
    )));
    let _ = tx.send(TaskEvent::Line(
        "HTTP 200 apenas verifica rede. Ferramentas e permissões requerem teste no chat.".into(),
    ));
    if matches!(protection, Some(Protection::Open(_))) {
        return Err(
            "ALERTA: o endpoint MCP respondeu sem autenticação. Pause a publicação (P) e corrija o OAuth do DevSpace."
                .into(),
        );
    }
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
        assert!(COMPONENT_INSTALL.contains("Cloudflare.cloudflared"));
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
            Task::InstallComponent(Component::Cloudflared),
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
        for task in [
            Task::CheckHealth,
            Task::Pause,
            Task::Resume { minutes: None },
            Task::Checkpoint,
        ] {
            assert!(task.script().is_none());
        }
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
            ..Probe::default()
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
    #[test]
    fn unauthenticated_answers_are_classified() {
        assert_eq!(classify_protection(401), Protection::Protected(401));
        assert_eq!(classify_protection(403), Protection::Protected(403));
        assert_eq!(classify_protection(200), Protection::Open(200));
        assert_eq!(classify_protection(204), Protection::Open(204));
        assert_eq!(
            classify_protection(404),
            Protection::Inconclusive(Some(404))
        );
        assert_eq!(
            classify_protection(502),
            Protection::Inconclusive(Some(502))
        );
    }
    #[test]
    fn an_open_endpoint_is_flagged_loudly() {
        let open = Probe {
            protection: Some(Protection::Open(200)),
            ..Probe::default()
        };
        assert!(open.open_without_auth());
        assert!(describe_protection(open.protection).contains("ABERTO"));
        assert!(!Probe::default().open_without_auth());
        assert!(describe_protection(Some(Protection::Protected(401))).contains("ativa"));
    }
    #[test]
    fn clipboard_ascii_is_sent_unchanged() {
        assert_eq!(
            clipboard_bytes("https://x.ts.net/mcp"),
            b"https://x.ts.net/mcp"
        );
    }
    #[test]
    fn clipboard_accents_are_utf16le_without_a_bom() {
        // "ç" is U+00E7; a BOM (FF FE) would paste as a stray character.
        assert_eq!(clipboard_bytes("aç"), [b'a', 0, 0xE7, 0x00]);
    }
}
