mod setup;
mod storage;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use setup::{Probe, Task, TaskEvent};
use std::{
    collections::VecDeque,
    error::Error,
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};
use storage::{ChatClient, Settings};

const MAX_INPUT_CHARS: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Connect,
    Install,
    Security,
    Logs,
    Help,
}
impl Page {
    const ALL: [Self; 6] = [
        Self::Home,
        Self::Connect,
        Self::Install,
        Self::Security,
        Self::Logs,
        Self::Help,
    ];
    fn next(self) -> Self {
        let i = Self::ALL.iter().position(|x| *x == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
    fn title(self) -> &'static str {
        match self {
            Self::Home => "Início",
            Self::Connect => "Conectar chats",
            Self::Install => "Configurar",
            Self::Security => "Segurança",
            Self::Logs => "Logs",
            Self::Help => "Ajuda",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    None,
    Workspace,
    Publish,
    Uninstall,
}

struct App {
    config: Settings,
    page: Page,
    edit: Edit,
    input: String,
    feedback: String,
    logs: VecDeque<String>,
    logs_offset: u16,
    processing: bool,
    process_pid: Option<u32>,
    status: Option<Probe>,
    sender: Sender<TaskEvent>,
    receiver: Receiver<TaskEvent>,
}
impl App {
    fn new() -> Self {
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
            sender,
            receiver,
        }
    }
    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.feedback = message.clone();
        self.logs.push_back(message);
        while self.logs.len() > 350 {
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
    fn refresh(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                TaskEvent::Started(pid) => self.process_pid = Some(pid),
                TaskEvent::Line(text) => self.log(text),
                TaskEvent::Status(probe) => self.status = Some(probe),
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
    }
    fn copy(&mut self) {
        self.log(match setup::copy_url() {
            Ok(url) => format!("URL MCP copiada: {url}"),
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
    fn begin_edit(&mut self, kind: Edit) {
        self.edit = kind;
        self.input = match kind {
            Edit::Workspace => self.config.workspace.to_string_lossy().into_owned(),
            _ => String::new(),
        };
    }
    fn submit_edit(&mut self) {
        let value = self.input.trim().to_owned();
        match self.edit {
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
                if value != "PUBLICAR" {
                    self.log(
                        "Instalação cancelada. Digite PUBLICAR para confirmar a exposição HTTPS.",
                    );
                } else {
                    self.edit = Edit::None;
                    self.input.clear();
                    self.start(Task::Install);
                    return;
                }
            }
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
                        self.edit = Edit::None;
                        self.input.clear();
                        self.start(task);
                        return;
                    }
                    None => self
                        .log("Remoção cancelada. Digite REMOVER ou REMOVER FUNNEL para confirmar."),
                }
            }
            Edit::None => {}
        }
        self.edit = Edit::None;
        self.input.clear();
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
    fn key(&mut self, key: KeyEvent) -> bool {
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
                    self.edit = Edit::None;
                    self.input.clear();
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

fn status_badge(status: Option<&Probe>) -> &'static str {
    match status {
        None => "verificando...",
        Some(probe) if probe.healthy() => "ONLINE",
        Some(probe) if !probe.configured => "NÃO CONFIGURADO",
        Some(_) => "OFFLINE",
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let full = frame.area();
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(4),
    ])
    .split(full);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  DEVSPACE ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "PC BRIDGE",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   |   ChatGPT + Claude  |  sem Codex/Claude Code CLI",
                Style::default().fg(Color::Gray),
            ),
        ]))
        .block(Block::default().borders(Borders::BOTTOM)),
        rows[0],
    );

    let cols = Layout::horizontal([Constraint::Length(24), Constraint::Min(28)]).split(rows[1]);
    let menu = Page::ALL.iter().map(|page| {
        let active = app.page == *page;
        ListItem::new(format!("  {}", page.title())).style(if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        })
    });
    frame.render_widget(
        List::new(menu).block(
            Block::bordered()
                .title(" Navegação (Tab) ")
                .border_style(Color::DarkGray),
        ),
        cols[0],
    );
    let (title, body) = page_content(app);
    let offset = if app.page == Page::Logs {
        let count = app.logs.len();
        let height = cols[1].height.saturating_sub(2) as usize;
        if app.logs_offset == u16::MAX {
            count.saturating_sub(height).min(u16::MAX as usize) as u16
        } else {
            app.logs_offset
        }
    } else {
        0
    };
    frame.render_widget(
        Paragraph::new(body)
            .scroll((offset, 0))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title).border_style(Color::DarkGray)),
        cols[1],
    );
    let task_state = if app.processing {
        "EM EXECUÇÃO"
    } else {
        "PRONTO"
    };
    frame.render_widget(
        Paragraph::new(format!(
            " {task_state}  |  {}  |  {}\n Tab telas  |  Ctrl+C Conectar  |  Ctrl+S Configurar  |  Ctrl+L Logs  |  Ctrl+X Cancelar  |  Ctrl+Q Sair",
            status_badge(app.status.as_ref()),
            app.feedback
        ))
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::TOP)),
        rows[2],
    );
    if app.edit != Edit::None {
        draw_prompt(frame, app, full);
    }
}

fn page_content(app: &App) -> (&'static str, Text<'static>) {
    let lines = match app.page {
        Page::Home => vec![
            " SEU COMPUTADOR, NOS SEUS CHATS NORMAIS".into(),
            "".into(),
            setup::status_summary(),
            match &app.status {
                Some(probe) => format!(
                    "Estado: {}  (local: {}  |  HTTPS: {})",
                    status_badge(Some(probe)),
                    setup::describe(probe.local),
                    setup::describe(probe.public)
                ),
                None => "Estado: verificando...".into(),
            },
            "".into(),
            "Use Configurar para instalar o bridge (se necessário).".into(),
            "Use Conectar chats para copiar a URL e configurar OAuth.".into(),
            "".into(),
            format!("Pasta de projetos: {}", app.config.workspace.display()),
            "".into(),
            " [C] Copiar MCP   [D] Diagnóstico   [T] Testar HTTPS".into(),
            "".into(),
            "Não é um chatbot nem executa modelos de IA.".into(),
        ],
        Page::Connect => {
            let mut v = vec![
                " SELECIONE O CHAT QUE VOCÊ JÁ USA".into(),
                "".into(),
                format!(
                    " [1] ChatGPT {}",
                    if app.config.client == ChatClient::ChatGpt {
                        "< selecionado"
                    } else {
                        ""
                    }
                ),
                format!(
                    " [2] Claude {}",
                    if app.config.client == ChatClient::Claude {
                        "< selecionado"
                    } else {
                        ""
                    }
                ),
                "".into(),
                format!(
                    "URL MCP: {}",
                    storage::mcp_url().unwrap_or_else(|| "(configure o bridge primeiro)".into())
                ),
                "".into(),
                " [C] Copiar URL /mcp   [O] Abrir chat/conectores".into(),
                " [P] Abrir painel Owner DevSpace".into(),
                "".into(),
            ];
            v.extend(
                app.config
                    .client
                    .instructions()
                    .iter()
                    .map(|s| (*s).to_owned()),
            );
            v
        }
        Page::Install => vec![
            " INSTALAÇÃO AUTOMÁTICA, SEM OUTRO CMD".into(),
            "".into(),
            format!("Pasta permitida: {}", app.config.workspace.display()),
            "".into(),
            " [R] Alterar a pasta de projetos".into(),
            " [I] Instalar/Configurar DevSpace + Tailscale".into(),
            " [D] Executar diagnóstico somente leitura".into(),
            " [T] Testar health local e HTTPS".into(),
            " [U] Remover a tarefa do bridge (desinstalar)".into(),
            "".into(),
            "O instalador prepara Node.js, Git, Tailscale e DevSpace.".into(),
            "A primeira configuração pode solicitar UAC, Tailscale e OAuth.".into(),
            "Não substitui credenciais ou configurações existentes.".into(),
            "Pastas amplas (disco inteiro, pasta pessoal, credenciais) são recusadas.".into(),
            "".into(),
            "IMPORTANTE: o Tailscale Funnel publica a URL na internet.".into(),
            "Iniciar a instalação exige confirmar PUBLICAR.".into(),
        ],
        Page::Security => vec![
            " PERMISSÕES SÃO APLICADAS NO SERVIDOR E NO CHAT".into(),
            "".into(),
            "1. DevSpace: restrinja workspaces/allowedRoots.".into(),
            "2. DevSpace Owner: autorize clientes OAuth confiáveis.".into(),
            "3. ChatGPT/Claude: revise ferramentas e confirme escritas.".into(),
            "4. Não publique auth.json, tokens ou credenciais.".into(),
            "".into(),
            "O bridge não concede escrita por botão falso na TUI.".into(),
            "O servidor MCP decide o que uma ferramenta pode fazer.".into(),
            "Permissão do plano/chat pode restringir ferramentas.".into(),
            "".into(),
            "A publicação HTTPS implica risco: use com critério.".into(),
            "Para revogar: desconecte o app no chat e no DevSpace.".into(),
            "O diagnóstico [D] avisa se allowedRoots está amplo demais.".into(),
        ],
        Page::Logs => {
            let mut v = vec![
                " SAÍDA AO VIVO (sem abrir outros terminais)".into(),
                "".into(),
            ];
            if app.logs.is_empty() {
                v.push("Nenhuma operação iniciada.".into());
            }
            v.extend(app.logs.iter().cloned());
            v
        }
        Page::Help => vec![
            " 1. Escolha a pasta em Configurar [R].".into(),
            " 2. Instale o bridge com [I], confirme PUBLICAR.".into(),
            " 3. Acesse Conectar chats, escolha [1] ou [2].".into(),
            " 4. Copie o endpoint MCP com [C].".into(),
            " 5. Abra o chat normal com [O], adicione o MCP.".into(),
            " 6. Autorize OAuth Owner e teste leitura no chat.".into(),
            "".into(),
            " Tab: próxima tela    Esc: Início    F1: Ajuda".into(),
            " Ctrl+C: Conectar     Ctrl+S: Configurar".into(),
            " Ctrl+L: Logs         Ctrl+X: Cancelar processo".into(),
            " Ctrl+Q: Sair         C: Copiar endpoint".into(),
            "".into(),
            "Não requer Codex CLI, Claude Code, chaves API ou".into(),
            "cópia dos modelos no computador.".into(),
            "Precisa de uma conta compatível no ChatGPT/Claude.".into(),
        ],
    };
    (app.page.title(), Text::from(lines.join("\n")))
}

fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(8).min(78);
    let height = 11.min(area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let (title, text) = match app.edit {
        Edit::Workspace => (
            "Pasta de projetos",
            format!(
                "Digite o caminho de uma pasta existente:\n{}\n\nEnter confirmar | Esc cancelar",
                app.input
            ),
        ),
        Edit::Publish => (
            "CONFIRMAÇÃO DE EXPOSIÇÃO PÚBLICA",
            format!(
                "Tailscale Funnel exporá uma URL HTTPS.\nO OAuth do DevSpace deve proteger o acesso.\nDigite PUBLICAR para continuar:\n{}\n\nEnter confirmar | Esc cancelar",
                app.input
            ),
        ),
        Edit::Uninstall => (
            "CONFIRMAÇÃO DE REMOÇÃO",
            format!(
                "Remove a tarefa agendada do bridge. DevSpace, Node, Git e Tailscale ficam.\nDigite REMOVER para continuar,\nou REMOVER FUNNEL para também desativar o Funnel\n(limpa a configuração de Funnel/Serve do Tailscale):\n{}\n\nEnter confirmar | Esc cancelar",
                app.input
            ),
        ),
        Edit::None => return,
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title).border_style(Color::Yellow)),
        popup,
    );
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut app = App::new();
    setup::watch_status(app.sender.clone());
    let mut terminal = ratatui::init();
    let result = (|| -> Result<(), Box<dyn Error>> {
        loop {
            app.refresh();
            terminal.draw(|frame| draw(frame, &app))?;
            if !event::poll(Duration::from_millis(150))? {
                continue;
            }
            let quit = match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.key(key),
                // Windows delivers pasted text as key events; this covers terminals that send it whole.
                Event::Paste(value) if app.edit != Edit::None => {
                    let remaining = MAX_INPUT_CHARS.saturating_sub(app.input.chars().count());
                    app.input.extend(
                        value
                            .chars()
                            .filter(|c| *c != '\r' && *c != '\0')
                            .take(remaining),
                    );
                    false
                }
                _ => false,
            };
            if quit {
                break;
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

fn doctor() -> i32 {
    println!("{}", setup::status_summary());
    let settings = Settings::load();
    println!("Raiz: {}", settings.workspace.display());
    if let Some(reason) = storage::root_risk(&settings.workspace) {
        println!("ATENÇÃO: a raiz configurada é ampla ({reason}).");
    }
    let probe = setup::probe();
    println!("Local: {}", setup::describe(probe.local));
    if probe.configured {
        println!("HTTPS: {}", setup::describe(probe.public));
    } else {
        println!("HTTPS: sem endpoint MCP configurado");
    }
    println!("HTTP 200 só comprova rede; teste OAuth e ferramentas no chat.");
    if probe.healthy() {
        0
    } else {
        1
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("DevSpace PC Bridge v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--doctor") => std::process::exit(doctor()),
        Some(arg) => Err(format!("Argumento não reconhecido: {arg}").into()),
        None => run(),
    }
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

    #[test]
    fn uppercase_shortcuts_work() {
        let mut app = App::new();
        app.page = Page::Install;
        press(&mut app, KeyCode::Char('I'));
        assert!(app.edit == Edit::Publish);
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
    fn status_badge_reflects_probe() {
        assert_eq!(status_badge(None), "verificando...");
        let ok = Probe {
            configured: true,
            local: Some(200),
            public: Some(200),
        };
        assert_eq!(status_badge(Some(&ok)), "ONLINE");
        assert_eq!(status_badge(Some(&Probe::default())), "NÃO CONFIGURADO");
        let down = Probe {
            configured: true,
            local: None,
            public: None,
        };
        assert_eq!(status_badge(Some(&down)), "OFFLINE");
    }
}
