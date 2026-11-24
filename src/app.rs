use crate::{
    checks::{self, Check},
    cloudflare,
    desktop::policy::{normalize_app, DEFAULT_ENABLE_MINUTES},
    redact,
    remote::RemoteState,
    setup::{self, Component, Probe, Task, TaskEvent},
    storage::{self, ChatClient, Settings},
    update,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    collections::VecDeque,
    fs,
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc,
    },
};

pub(crate) const MAX_INPUT_CHARS: usize = 1024;
const HISTORY_LEN: usize = 120;
const MAX_LOG_LINES: usize = 350;
/// Longest timed exposure the prompt accepts: one day.
const MAX_EXPOSE_MINUTES: u32 = 1440;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Home,
    Connect,
    Install,
    Security,
    Desktop,
    Logs,
    Help,
}
impl Page {
    pub(crate) const ALL: [Self; 7] = [
        Self::Home,
        Self::Connect,
        Self::Install,
        Self::Security,
        Self::Desktop,
        Self::Logs,
        Self::Help,
    ];
    fn next(self) -> Self {
        let i = Self::ALL.iter().position(|x| *x == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Home => "Início",
            Self::Connect => "Conectar chats",
            Self::Install => "Configurar",
            Self::Security => "Segurança",
            Self::Desktop => "Desktop remoto",
            Self::Logs => "Logs",
            Self::Help => "Ajuda",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edit {
    None,
    Workspace,
    Publish,
    Uninstall,
    Resume,
    DesktopApp,
    DesktopRemoveApp,
    DesktopEnable,
    DesktopPublish,
    InstallComponent(Component),
}

pub(crate) struct App {
    pub(crate) config: Settings,
    pub(crate) page: Page,
    pub(crate) edit: Edit,
    pub(crate) input: String,
    pub(crate) feedback: String,
    pub(crate) logs: VecDeque<String>,
    pub(crate) logs_offset: u16,
    pub(crate) processing: bool,
    process_pid: Option<u32>,
    pub(crate) status: Option<Probe>,
    pub(crate) prereqs: Vec<Check>,
    pub(crate) history: VecDeque<bool>,
    pub(crate) update: Option<String>,
    pub(crate) sender: Sender<TaskEvent>,
    receiver: Receiver<TaskEvent>,
    pub(crate) desktop_remote: Option<Arc<RemoteState>>,
    pub(crate) desktop_tunnel: Option<cloudflare::Tunnel>,
    pub(crate) desktop_public_url: Option<String>,
    pub(crate) tray_available: bool,
    pub(crate) quit_requested: bool,
}

/// `PUBLICAR` (no deadline) or `PUBLICAR <minutes>`. `None` means invalid input.
pub(crate) fn parse_resume(input: &str) -> Option<Option<u32>> {
    let mut words = input.split_whitespace();
    if words.next()? != "PUBLICAR" {
        return None;
    }
    let Some(minutes) = words.next() else {
        return Some(None);
    };
    if words.next().is_some() {
        return None;
    }
    let minutes: u32 = minutes.parse().ok()?;
    (1..=MAX_EXPOSE_MINUTES)
        .contains(&minutes)
        .then_some(Some(minutes))
}

pub(crate) fn uptime_percent(history: &VecDeque<bool>) -> Option<u32> {
    if history.is_empty() {
        return None;
    }
    let up = history.iter().filter(|ok| **ok).count();
    u32::try_from(up * 100 / history.len()).ok()
}

pub(crate) fn history_bar(history: &VecDeque<bool>, width: usize) -> String {
    let skip = history.len().saturating_sub(width);
    history
        .iter()
        .skip(skip)
        .map(|ok| if *ok { '█' } else { '░' })
        .collect()
}

impl App {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            config: Settings::load(),
            page: Page::Home,
            edit: Edit::None,
            input: String::new(),
            feedback: "Use Tab para navegar. Configure e conecte seu chat habitual.".into(),
            logs: VecDeque::new(),
            logs_offset: u16::MAX,
            processing: false,
            process_pid: None,
            status: None,
            prereqs: checks::prerequisites(),
            history: VecDeque::new(),
            update: None,
            sender,
            receiver,
            desktop_remote: None,
            desktop_tunnel: None,
            desktop_public_url: None,
            tray_available: false,
            quit_requested: false,
        }
    }
    pub(crate) fn attach_desktop(&mut self, state: Arc<RemoteState>, port: u16) {
        self.desktop_remote = Some(state);
        self.log(format!(
            "Desktop MCP local: http://127.0.0.1:{port}/mcp (OAuth ativo; controle desligado)."
        ));
    }
    pub(crate) fn stop_desktop_tunnel(&mut self) {
        if let Some(mut tunnel) = self.desktop_tunnel.take() {
            tunnel.stop();
        }
        self.desktop_public_url = None;
        if let Some(remote) = &self.desktop_remote {
            remote.set_public_origin(None);
            remote.policy().disable();
            remote.auth.close_pairing();
        }
    }
    fn desktop_action(&mut self, action: &str) {
        let Some(remote) = self.desktop_remote.as_ref() else {
            self.log("Desktop MCP nao iniciado neste Windows.");
            return;
        };
        match action {
            "pin" => {
                let view = remote.auth.open_pairing(crate::clock::now_secs());
                self.log(format!(
                    "PIN de pareamento aberto por {} s. Veja na tela Desktop.",
                    view.remaining_secs
                ));
            }
            "off" => {
                remote.policy().disable();
                remote.auth.close_pairing();
                self.log("Controle de janelas desligado imediatamente.");
            }
            "revoke" => {
                remote.policy().disable();
                remote.auth.close_pairing();
                remote.auth.revoke_all();
                self.log("Controle desativado e todas as sessoes OAuth revogadas.");
            }
            "view" | "input" => {
                let mut policy = remote.policy();
                if action == "view" {
                    policy.config_mut().screenshots = !policy.config().screenshots;
                } else {
                    policy.config_mut().input = !policy.config().input;
                }
                drop(policy);
                self.log(match remote.save_policy() {
                    Ok(()) => format!("Permissao {action} atualizada."),
                    Err(err) => format!("Falha salvando permissao: {err}"),
                });
            }
            "stop_tunnel" => {
                self.stop_desktop_tunnel();
                self.log("Cloudflare desligado; sessao de controle encerrada.");
            }
            _ => {}
        }
    }
    pub(crate) fn start_background_checks(&self) {
        setup::watch_status(self.sender.clone());
        update::spawn_check(self.sender.clone());
    }
    pub(crate) fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.feedback = message.clone();
        self.logs.push_back(message);
        while self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs_offset = u16::MAX;
    }
    fn save(&mut self) {
        if let Err(e) = self.config.save() {
            self.log(format!("Falha salvando preferências: {e}"));
        }
    }
    fn start(&mut self, task: Task) {
        if self.processing {
            self.log("Há uma operação em andamento. Veja Logs ou Ctrl+X.");
            return;
        }
        self.processing = true;
        self.process_pid = None;
        self.page = Page::Logs;
        self.log(format!("Iniciando {}...", task.label()));
        setup::start(task, self.config.workspace.clone(), self.sender.clone());
    }
    pub(crate) fn refresh(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                TaskEvent::Started(pid) => self.process_pid = Some(pid),
                TaskEvent::Line(text) => self.log(text),
                TaskEvent::Status(probe) => self.apply_status(probe),
                TaskEvent::Update(tag) => {
                    self.log(format!(
                        "Nova versão disponível: {tag}. Baixe em {}",
                        update::RELEASES_URL
                    ));
                    self.update = Some(tag);
                }
                TaskEvent::CloudflareUrl(url) => {
                    if self.desktop_tunnel.is_some() {
                        if let Some(remote) = &self.desktop_remote {
                            remote.set_public_origin(Some(url.clone()));
                        }
                        self.desktop_public_url = Some(format!("{url}/mcp"));
                        self.log(format!("Desktop HTTPS: {url}/mcp"));
                    }
                }
                TaskEvent::TrayDisable => {
                    self.stop_desktop_tunnel();
                    if let Some(remote) = &self.desktop_remote {
                        remote.auth.revoke_all();
                    }
                    self.log("Bandeja: desktop remoto desligado e clientes revogados.");
                }
                TaskEvent::TrayQuit => {
                    if self.processing {
                        #[cfg(windows)]
                        crate::tray::show_console();
                        self.log("Finalize ou cancele a instalacao em andamento antes de sair.");
                    } else {
                        self.stop_desktop_tunnel();
                        self.quit_requested = true;
                    }
                }
                TaskEvent::Done(result) => {
                    self.processing = false;
                    self.process_pid = None;
                    match result {
                        Ok(()) => self.log("Operação concluída."),
                        Err(err) => self.log(format!("Falha: {err}")),
                    }
                }
            }
        }
        if self
            .desktop_tunnel
            .as_mut()
            .is_some_and(|tunnel| tunnel.exited())
        {
            self.stop_desktop_tunnel();
            self.log("Cloudflare encerrou; URL publica removida.");
        }
    }
    fn apply_status(&mut self, probe: Probe) {
        // A deliberate pause is not downtime, so it stays out of the availability history.
        if !probe.paused {
            self.history.push_back(probe.healthy());
            while self.history.len() > HISTORY_LEN {
                self.history.pop_front();
            }
        }
        self.prereqs = checks::prerequisites();
        self.status = Some(probe);
    }
    fn copy(&mut self) {
        self.log(match setup::copy_url() {
            Ok(url) => format!("URL MCP copiada: {url}"),
            Err(err) => err,
        });
    }
    fn copy_logs(&mut self) {
        let joined = self.logs.iter().cloned().collect::<Vec<_>>().join("\n");
        self.log(match setup::copy_text(&redact::redact(&joined)) {
            Ok(()) => "Logs copiados sem hostname, usuário nem tokens.".to_owned(),
            Err(err) => err,
        });
    }
    fn open(&mut self) {
        self.log(match setup::open_chat(self.config.client) {
            Ok(()) => format!("Abrindo {} no navegador...", self.config.client.label()),
            Err(err) => err,
        });
    }
    fn select(&mut self, client: ChatClient) {
        self.config.client = client;
        self.save();
        self.log(format!("Selecionado: {}", client.label()));
    }
    pub(crate) fn begin_edit(&mut self, kind: Edit) {
        self.edit = kind;
        self.input = match kind {
            Edit::Workspace => self.config.workspace.to_string_lossy().into_owned(),
            _ => String::new(),
        };
    }
    fn close_edit(&mut self) {
        self.edit = Edit::None;
        self.input.clear();
    }
    pub(crate) fn submit_edit(&mut self) {
        let value = self.input.trim().to_owned();
        match self.edit {
            Edit::InstallComponent(component) => {
                if value == format!("INSTALAR {}", component.name().to_ascii_uppercase()) {
                    self.close_edit();
                    self.start(Task::InstallComponent(component));
                    return;
                }
                self.log("Instalacao individual cancelada: palavra de confirmacao incorreta.");
            }
            Edit::Workspace => {
                let path = PathBuf::from(value);
                if !path.is_dir() {
                    self.log("A pasta escolhida não existe. Tente novamente.");
                    return;
                }
                let resolved = storage::plain_path(&fs::canonicalize(&path).unwrap_or(path));
                if let Some(reason) = storage::root_risk(&resolved) {
                    self.log(format!(
                        "Pasta recusada: {reason}. Escolha a pasta dos seus projetos, não uma pasta ampla."
                    ));
                    return;
                }
                self.config.workspace = resolved;
                self.save();
                self.log(format!(
                    "Pasta autorizada na próxima instalação: {}",
                    self.config.workspace.display()
                ));
            }
            Edit::Publish => {
                if value == "PUBLICAR" {
                    self.close_edit();
                    self.start(Task::Install);
                    return;
                }
                self.log("Instalação cancelada. Digite PUBLICAR para confirmar a exposição HTTPS.");
            }
            Edit::Resume => match parse_resume(&value) {
                Some(minutes) => {
                    self.close_edit();
                    self.start(Task::Resume { minutes });
                    return;
                }
                None => self
                    .log("Retomada cancelada. Digite PUBLICAR ou PUBLICAR <minutos> (1 a 1440)."),
            },
            Edit::Uninstall => {
                let task = match value.as_str() {
                    "REMOVER" => Some(Task::Uninstall {
                        disable_funnel: false,
                    }),
                    "REMOVER FUNNEL" => Some(Task::Uninstall {
                        disable_funnel: true,
                    }),
                    _ => None,
                };
                match task {
                    Some(task) => {
                        self.close_edit();
                        self.start(task);
                        return;
                    }
                    None => self
                        .log("Remoção cancelada. Digite REMOVER ou REMOVER FUNNEL para confirmar."),
                }
            }
            Edit::DesktopApp | Edit::DesktopRemoveApp => {
                let app_name = normalize_app(&value);
                if app_name.is_empty()
                    || app_name != value.to_ascii_lowercase()
                    || !app_name.ends_with(".exe")
                    || !app_name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                {
                    self.log("Informe apenas o executavel, por exemplo notepad.exe.");
                    return;
                }
                if let Some(remote) = &self.desktop_remote {
                    let mut policy = remote.policy();
                    if self.edit == Edit::DesktopApp {
                        if !policy.config().allowed_apps.contains(&app_name) {
                            policy.config_mut().allowed_apps.push(app_name.clone());
                        }
                    } else {
                        policy
                            .config_mut()
                            .allowed_apps
                            .retain(|name| name != &app_name);
                    }
                    drop(policy);
                    self.log(match remote.save_policy() {
                        Ok(()) => format!("Lista de apps atualizada: {app_name}"),
                        Err(err) => format!("Falha salvando apps: {err}"),
                    });
                }
            }
            Edit::DesktopEnable => {
                if let Some(minutes) = parse_desktop_enable(&value) {
                    if let Some(remote) = &self.desktop_remote {
                        remote.policy().enable(crate::clock::now_secs(), minutes);
                        self.log(format!("Controle ativado por {minutes} minutos."));
                    }
                } else {
                    self.log("Cancelado. Digite ATIVAR ou ATIVAR 30 (ate 240 minutos).");
                }
            }
            Edit::DesktopPublish => {
                if value == "PUBLICAR DESKTOP" {
                    if self.desktop_tunnel.is_some() {
                        self.log("Cloudflare ja esta em execucao.");
                    } else {
                        match cloudflare::Tunnel::start(self.sender.clone()) {
                            Ok(tunnel) => {
                                self.desktop_tunnel = Some(tunnel);
                                self.log(
                                    "Cloudflare iniciando. A URL aparecera quando disponivel.",
                                );
                            }
                            Err(err) => self.log(format!("Cloudflare: {err}")),
                        }
                    }
                } else {
                    self.log("Publicacao cancelada.");
                }
            }
            Edit::None => {}
        }
        self.close_edit();
    }
    fn cancel_task(&mut self) {
        if let Some(pid) = self.process_pid {
            match setup::stop_pid(pid) {
                Ok(()) => self.log("Cancelamento solicitado. Confira o estado da instalação."),
                Err(e) => self.log(format!("Falha ao cancelar: {e}")),
            }
        } else if self.processing {
            self.log("Aguarde o início do processo para cancelar.");
        }
    }
    pub(crate) fn key(&mut self, key: KeyEvent) -> bool {
        // Shortcuts ignore case so Caps Lock/Shift do not break them; text entry keeps it.
        let code = match key.code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('q') => {
                    if self.processing {
                        self.log("Finalize ou cancele o processo antes de sair.");
                    } else {
                        return true;
                    }
                }
                KeyCode::Char('x') => self.cancel_task(),
                KeyCode::Char('l') => self.page = Page::Logs,
                KeyCode::Char('s') => self.page = Page::Install,
                KeyCode::Char('c') => self.page = Page::Connect,
                _ => {}
            }
            return false;
        }
        if self.edit != Edit::None {
            match key.code {
                KeyCode::Esc => {
                    self.close_edit();
                    self.log("Ação cancelada.");
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Enter => self.submit_edit(),
                KeyCode::Char(c) if self.input.chars().count() < MAX_INPUT_CHARS => {
                    self.input.push(c)
                }
                _ => {}
            }
            return false;
        }
        match code {
            KeyCode::Tab => self.page = self.page.next(),
            KeyCode::Esc => self.page = Page::Home,
            KeyCode::F(1) => self.page = Page::Help,
            KeyCode::Char('1') if self.page == Page::Connect => self.select(ChatClient::ChatGpt),
            KeyCode::Char('2') if self.page == Page::Connect => self.select(ChatClient::Claude),
            KeyCode::Char('c')
                if matches!(self.page, Page::Home | Page::Connect | Page::Install) =>
            {
                self.copy()
            }
            KeyCode::Char('o') if self.page == Page::Connect => self.open(),
            KeyCode::Char('p') if self.page == Page::Connect => {
                self.log(match setup::open_owner_panel() {
                    Ok(()) => "Painel Owner aberto.".into(),
                    Err(e) => e,
                });
            }
            KeyCode::Char('p') if self.page == Page::Home => self.start(Task::Pause),
            KeyCode::Char('r') if self.page == Page::Home => self.begin_edit(Edit::Resume),
            KeyCode::Char('k') if self.page == Page::Home => self.start(Task::Checkpoint),
            KeyCode::Char('y') if self.page == Page::Logs => self.copy_logs(),
            #[cfg(windows)]
            KeyCode::Char('m') if self.page == Page::Home && self.tray_available => {
                if let Err(err) = crate::tray::hide_console() {
                    self.log(err);
                }
            }
            KeyCode::Char('1') if self.page == Page::Install => {
                self.begin_edit(Edit::InstallComponent(Component::Git))
            }
            KeyCode::Char('2') if self.page == Page::Install => {
                self.begin_edit(Edit::InstallComponent(Component::Node))
            }
            KeyCode::Char('3') if self.page == Page::Install => {
                self.begin_edit(Edit::InstallComponent(Component::Tailscale))
            }
            KeyCode::Char('4') if self.page == Page::Install => {
                self.begin_edit(Edit::InstallComponent(Component::Cloudflared))
            }
            KeyCode::Char('5') if self.page == Page::Install => {
                self.begin_edit(Edit::InstallComponent(Component::DevSpace))
            }
            KeyCode::Char('c') if self.page == Page::Desktop => self.desktop_action("pin"),
            KeyCode::Char('e') if self.page == Page::Desktop => {
                self.begin_edit(Edit::DesktopEnable)
            }
            KeyCode::Char('b') if self.page == Page::Desktop => self.desktop_action("off"),
            KeyCode::Char('r') if self.page == Page::Desktop => self.desktop_action("revoke"),
            KeyCode::Char('a') if self.page == Page::Desktop => self.begin_edit(Edit::DesktopApp),
            KeyCode::Char('x') if self.page == Page::Desktop => {
                self.begin_edit(Edit::DesktopRemoveApp)
            }
            KeyCode::Char('v') if self.page == Page::Desktop => self.desktop_action("view"),
            KeyCode::Char('i') if self.page == Page::Desktop => self.desktop_action("input"),
            KeyCode::Char('p') if self.page == Page::Desktop => {
                self.begin_edit(Edit::DesktopPublish)
            }
            KeyCode::Char('s') if self.page == Page::Desktop => self.desktop_action("stop_tunnel"),
            KeyCode::Char('r') if self.page == Page::Install => self.begin_edit(Edit::Workspace),
            KeyCode::Char('i') if self.page == Page::Install => self.begin_edit(Edit::Publish),
            KeyCode::Char('u') if self.page == Page::Install => self.begin_edit(Edit::Uninstall),
            KeyCode::Char('d') if matches!(self.page, Page::Install | Page::Home) => {
                self.start(Task::Doctor)
            }
            KeyCode::Char('t') if matches!(self.page, Page::Install | Page::Home) => {
                self.start(Task::CheckHealth)
            }
            KeyCode::Up if self.page == Page::Logs => {
                self.logs_offset = self.logs_offset.saturating_sub(1)
            }
            KeyCode::Down if self.page == Page::Logs => {
                self.logs_offset = self.logs_offset.saturating_add(1)
            }
            KeyCode::End if self.page == Page::Logs => self.logs_offset = u16::MAX,
            _ => {}
        }
        false
    }
}

fn parse_desktop_enable(value: &str) -> Option<u32> {
    let mut words = value.split_whitespace();
    if words.next()? != "ATIVAR" {
        return None;
    }
    let minutes = words
        .next()
        .map(str::parse::<u32>)
        .transpose()
        .ok()?
        .unwrap_or(DEFAULT_ENABLE_MINUTES);
    (words.next().is_none() && (1..=240).contains(&minutes)).then_some(minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(app: &mut App, code: KeyCode) -> bool {
        app.key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    // Tests must never press P, K, D or T on Home/Install: those start real tasks
    // (pausing the Funnel, running git, calling the network).

    #[test]
    fn uppercase_shortcuts_work() {
        let mut app = App::new();
        app.page = Page::Install;
        press(&mut app, KeyCode::Char('I'));
        assert!(app.edit == Edit::Publish);
    }
    #[test]
    fn installer_requires_exact_confirmation_and_does_not_run_on_typo() {
        let mut app = App::new();
        app.begin_edit(Edit::InstallComponent(Component::Cloudflared));
        app.input = "INSTALAR OUTRO".into();
        press(&mut app, KeyCode::Enter);
        assert!(!app.processing);
        assert!(app.feedback.contains("cancelada"));
    }
    #[test]
    fn timed_desktop_requires_valid_local_confirmation() {
        assert_eq!(parse_desktop_enable("ATIVAR"), Some(30));
        assert_eq!(parse_desktop_enable("ATIVAR 240"), Some(240));
        assert_eq!(parse_desktop_enable("ATIVAR 241"), None);
        assert_eq!(parse_desktop_enable("ATIVAR 0"), None);
        assert_eq!(parse_desktop_enable("ativar 30"), None);
        assert_eq!(parse_desktop_enable("ATIVAR 30 EXTRA"), None);
    }
    #[test]
    fn typed_text_keeps_its_case() {
        let mut app = App::new();
        app.begin_edit(Edit::Publish);
        type_text(&mut app, "PUBLICAR");
        assert_eq!(app.input, "PUBLICAR");
    }
    #[test]
    fn input_is_limited_by_characters_not_bytes() {
        let mut app = App::new();
        app.begin_edit(Edit::Publish);
        for _ in 0..MAX_INPUT_CHARS + 10 {
            press(&mut app, KeyCode::Char('ç'));
        }
        assert_eq!(app.input.chars().count(), MAX_INPUT_CHARS);
    }
    #[test]
    fn ctrl_q_quits_only_when_idle() {
        let mut app = App::new();
        let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert!(app.key(quit));
        app.processing = true;
        assert!(!app.key(quit));
    }
    #[test]
    fn uninstall_prompt_opens_from_install_page_only() {
        let mut app = App::new();
        press(&mut app, KeyCode::Char('u'));
        assert!(app.edit == Edit::None);
        app.page = Page::Install;
        press(&mut app, KeyCode::Char('u'));
        assert!(app.edit == Edit::Uninstall);
    }
    #[test]
    fn wrong_confirmation_word_runs_nothing() {
        let mut app = App::new();
        app.begin_edit(Edit::Uninstall);
        type_text(&mut app, "remover");
        press(&mut app, KeyCode::Enter);
        assert!(app.edit == Edit::None);
        assert!(!app.processing);
    }
    #[test]
    fn resume_prompt_opens_on_home_and_rejects_bad_input() {
        let mut app = App::new();
        press(&mut app, KeyCode::Char('r'));
        assert!(app.edit == Edit::Resume);
        type_text(&mut app, "publicar 60");
        press(&mut app, KeyCode::Enter);
        assert!(app.edit == Edit::None);
        assert!(!app.processing);
        assert!(app.feedback.contains("Retomada cancelada"));
    }
    #[test]
    fn broad_workspace_is_refused() {
        let mut app = App::new();
        let before = app.config.workspace.clone();
        app.begin_edit(Edit::Workspace);
        app.input = storage::home().to_string_lossy().into_owned();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.config.workspace, before);
        assert!(app.feedback.contains("recusada"));
    }
    #[test]
    fn resume_input_is_parsed_strictly() {
        assert_eq!(parse_resume("PUBLICAR"), Some(None));
        assert_eq!(parse_resume("  PUBLICAR  "), Some(None));
        assert_eq!(parse_resume("PUBLICAR 60"), Some(Some(60)));
        assert_eq!(parse_resume("PUBLICAR 1440"), Some(Some(1440)));
        assert_eq!(parse_resume("PUBLICAR 0"), None);
        assert_eq!(parse_resume("PUBLICAR 1441"), None);
        assert_eq!(parse_resume("PUBLICAR -5"), None);
        assert_eq!(parse_resume("PUBLICAR 5 6"), None);
        assert_eq!(parse_resume("publicar"), None);
        assert_eq!(parse_resume(""), None);
    }
    #[test]
    fn paused_samples_stay_out_of_the_history() {
        let mut app = App::new();
        let up = Probe {
            local: Some(200),
            public: Some(200),
            ..Probe::default()
        };
        app.apply_status(up);
        app.apply_status(Probe::default());
        app.apply_status(Probe {
            paused: true,
            ..Probe::default()
        });
        assert_eq!(app.history.len(), 2);
        assert_eq!(uptime_percent(&app.history), Some(50));
    }
    #[test]
    fn history_is_capped() {
        let mut app = App::new();
        for _ in 0..HISTORY_LEN + 25 {
            app.apply_status(Probe::default());
        }
        assert_eq!(app.history.len(), HISTORY_LEN);
    }
    #[test]
    fn uptime_and_bar_follow_the_samples() {
        let samples: VecDeque<bool> = [true, true, false, true].into();
        assert_eq!(uptime_percent(&samples), Some(75));
        assert_eq!(uptime_percent(&VecDeque::new()), None);
        assert_eq!(history_bar(&samples, 10), "██░█");
        assert_eq!(history_bar(&samples, 2), "░█");
    }
}
