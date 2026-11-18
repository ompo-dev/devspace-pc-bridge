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
use setup::{Task, TaskEvent};
use std::{
    collections::VecDeque,
    error::Error,
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};
use storage::{ChatClient, Settings};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page { Home, Connect, Install, Security, Logs, Help }
impl Page {
    const ALL: [Self; 6] = [Self::Home, Self::Connect, Self::Install, Self::Security, Self::Logs, Self::Help];
    fn next(self) -> Self {
        let i = Self::ALL.iter().position(|x| *x == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
    fn title(self) -> &'static str {
        match self {
            Self::Home => "Inicio",
            Self::Connect => "Conectar chats",
            Self::Install => "Configurar",
            Self::Security => "Seguranca",
            Self::Logs => "Logs",
            Self::Help => "Ajuda",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit { None, Workspace, Publish }

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
            sender,
            receiver,
        }
    }
    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.feedback = message.clone();
        self.logs.push_back(message);
        while self.logs.len() > 350 { self.logs.pop_front(); }
        self.logs_offset = u16::MAX;
    }
    fn save(&mut self) {
        if let Err(e) = self.config.save() { self.log(format!("Falha salvando preferencias: {e}")); }
    }
    fn start(&mut self, task: Task) {
        if self.processing {
            self.log("Ha uma operacao em andamento. Veja Logs ou Ctrl+X.");
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
                TaskEvent::Done(result) => {
                    self.processing = false;
                    self.process_pid = None;
                    match result {
                        Ok(()) => self.log("Operacao concluida."),
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
                    self.log("A pasta escolhida nao existe. Tente novamente.");
                    return;
                }
                self.config.workspace = fs::canonicalize(&path).unwrap_or(path);
                self.save();
                self.log(format!("Pasta autorizada na proxima instalacao: {}", self.config.workspace.display()));
            }
            Edit::Publish => {
                if value != "PUBLICAR" {
                    self.log("Instalacao cancelada. Digite PUBLICAR para confirmar a exposicao HTTPS.");
                } else {
                    self.edit = Edit::None;
                    self.input.clear();
                    self.start(Task::Install);
                    return;
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
                Ok(()) => self.log("Cancelamento solicitado. Confira o estado da instalacao."),
                Err(e) => self.log(format!("Falha ao cancelar: {e}")),
            }
        } else if self.processing { self.log("Aguarde o inicio do processo para cancelar."); }
    }
    fn key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('q') => {
                    if self.processing { self.log("Finalize ou cancele o processo antes de sair."); }
                    else { return true; }
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
                KeyCode::Esc => { self.edit = Edit::None; self.input.clear(); self.log("Acao cancelada."); }
                KeyCode::Backspace => { self.input.pop(); }
                KeyCode::Enter => self.submit_edit(),
                KeyCode::Char(c) if self.input.chars().count() < 1024 => self.input.push(c),
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::Tab => self.page = self.page.next(),
            KeyCode::Esc => self.page = Page::Home,
            KeyCode::F(1) => self.page = Page::Help,
            KeyCode::Char('1') if self.page == Page::Connect => self.select(ChatClient::ChatGpt),
            KeyCode::Char('2') if self.page == Page::Connect => self.select(ChatClient::Claude),
            KeyCode::Char('c') if matches!(self.page, Page::Home | Page::Connect | Page::Install) => self.copy(),
            KeyCode::Char('o') if self.page == Page::Connect => self.open(),
            KeyCode::Char('p') if self.page == Page::Connect => {
                self.log(match setup::open_owner_panel() { Ok(()) => "Painel Owner aberto.".into(), Err(e) => e });
            }
            KeyCode::Char('r') if self.page == Page::Install => self.begin_edit(Edit::Workspace),
            KeyCode::Char('i') if self.page == Page::Install => self.begin_edit(Edit::Publish),
            KeyCode::Char('d') if matches!(self.page, Page::Install | Page::Home) => self.start(Task::Doctor),
            KeyCode::Char('t') if matches!(self.page, Page::Install | Page::Home) => self.start(Task::CheckHealth),
            KeyCode::Up if self.page == Page::Logs => self.logs_offset = self.logs_offset.saturating_sub(1),
            KeyCode::Down if self.page == Page::Logs => self.logs_offset = self.logs_offset.saturating_add(1),
            KeyCode::End if self.page == Page::Logs => self.logs_offset = u16::MAX,
            _ => {}
        }
        false
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let full = frame.area();
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(8), Constraint::Length(4)]).split(full);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  DEVSPACE ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("PC BRIDGE", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("   |   ChatGPT + Claude  |  sem Codex/Claude Code CLI", Style::default().fg(Color::Gray)),
        ])).block(Block::default().borders(Borders::BOTTOM)),
        rows[0],
    );

    let cols = Layout::horizontal([Constraint::Length(24), Constraint::Min(28)]).split(rows[1]);
    let menu = Page::ALL.iter().map(|page| {
        let active = app.page == *page;
        ListItem::new(format!("  {}", page.title())).style(if active {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else { Style::default().fg(Color::Gray) })
    });
    frame.render_widget(
        List::new(menu).block(Block::bordered().title(" Navegacao (Tab) ").border_style(Color::DarkGray)),
        cols[0],
    );
    let (title, body) = page_content(app);
    let offset = if app.page == Page::Logs {
        let count = app.logs.len();
        let height = cols[1].height.saturating_sub(2) as usize;
        if app.logs_offset == u16::MAX { count.saturating_sub(height).min(u16::MAX as usize) as u16 }
        else { app.logs_offset }
    } else { 0 };
    frame.render_widget(
        Paragraph::new(body)
            .scroll((offset, 0))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title).border_style(Color::DarkGray)),
        cols[1],
    );
    let status = if app.processing { "EM EXECUCAO" } else { "PRONTO" };
    frame.render_widget(
        Paragraph::new(format!(
            " {status}  |  {}\n Tab telas  |  Ctrl+C Conectar  |  Ctrl+S Configurar  |  Ctrl+L Logs  |  Ctrl+X Cancelar  |  Ctrl+Q Sair",
            app.feedback
        )).wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::TOP)),
        rows[2],
    );
    if app.edit != Edit::None { draw_prompt(frame, app, full); }
}

fn page_content(app: &App) -> (&'static str, Text<'static>) {
    let lines = match app.page {
        Page::Home => vec![
            " SEU COMPUTADOR, NOS SEUS CHATS NORMAIS".into(),
            "".into(),
            setup::status_summary(),
            "".into(),
            "Use Configurar para instalar o bridge (se necessario).".into(),
            "Use Conectar chats para copiar a URL e configurar OAuth.".into(),
            "".into(),
            format!("Pasta de projetos: {}", app.config.workspace.display()),
            "".into(),
            " [C] Copiar MCP   [D] Diagnostico   [T] Testar HTTPS".into(),
            "".into(),
            "Nao e um chatbot nem executa modelos de IA.".into(),
        ],
        Page::Connect => {
            let mut v = vec![
                " SELECIONE O CHAT QUE VOCE JA USA".into(),
                "".into(),
                format!(" [1] ChatGPT {}",
                    if app.config.client == ChatClient::ChatGpt { "< selecionado" } else { "" }),
                format!(" [2] Claude {}",
                    if app.config.client == ChatClient::Claude { "< selecionado" } else { "" }),
                "".into(),
                format!("URL MCP: {}", storage::mcp_url().unwrap_or_else(|| "(configure o bridge primeiro)".into())),
                "".into(),
                " [C] Copiar URL /mcp   [O] Abrir chat/conectores".into(),
                " [P] Abrir painel Owner DevSpace".into(),
                "".into(),
            ];
            v.extend(app.config.client.instructions().iter().map(|s| (*s).to_owned()));
            v
        }
        Page::Install => vec![
            " INSTALACAO AUTOMATICA, SEM OUTRO CMD".into(),
            "".into(),
            format!("Pasta permitida: {}", app.config.workspace.display()),
            "".into(),
            " [R] Alterar a pasta de projetos".into(),
            " [I] Instalar/Configurar DevSpace + Tailscale".into(),
            " [D] Executar diagnostico somente leitura".into(),
            " [T] Testar health local e HTTPS".into(),
            "".into(),
            "O instalador prepara Node.js, Git, Tailscale e DevSpace.".into(),
            "A primeira configuracao pode solicitar UAC, Tailscale e OAuth.".into(),
            "Nao substitui credenciais ou configuracoes existentes.".into(),
            "".into(),
            "IMPORTANTE: o Tailscale Funnel publica a URL na internet.".into(),
            "Iniciar instalacao exige confirmar PUBLICAR.".into(),
        ],
        Page::Security => vec![
            " PERMISSOES SAO APLICADAS NO SERVIDOR E NO CHAT".into(),
            "".into(),
            "1. DevSpace: restrinja workspaces/allowedRoots.".into(),
            "2. DevSpace Owner: autorize clientes OAuth confiaveis.".into(),
            "3. ChatGPT/Claude: revise ferramentas e confirme escritas.".into(),
            "4. Nao publique auth.json, tokens ou credenciais.".into(),
            "".into(),
            "O bridge nao concede escrita por botao falso na TUI.".into(),
            "O servidor MCP decide o que uma ferramenta pode fazer.".into(),
            "Permissao do plano/chat pode restringir ferramentas.".into(),
            "".into(),
            "A publicacao HTTPS implica risco: use com criterio.".into(),
            "Para revogar: desconecte o app no chat e no DevSpace.".into(),
        ],
        Page::Logs => {
            let mut v = vec![" SAIDA AO VIVO (sem abrir outros terminais)".into(), "".into()];
            if app.logs.is_empty() { v.push("Nenhuma operacao iniciada.".into()); }
            v.extend(app.logs.iter().cloned());
            v
        },
        Page::Help => vec![
            " 1. Escolha a pasta em Configurar [R].".into(),
            " 2. Instale o bridge com [I], confirme PUBLICAR.".into(),
            " 3. Acesse Conectar chats, escolha [1] ou [2].".into(),
            " 4. Copie o endpoint MCP com [C].".into(),
            " 5. Abra o chat normal com [O], adicione o MCP.".into(),
            " 6. Autorize OAuth Owner e teste leitura no chat.".into(),
            "".into(),
            " Tab: proxima tela    Esc: Inicio    F1: Ajuda".into(),
            " Ctrl+C: Conectar     Ctrl+S: Configurar".into(),
            " Ctrl+L: Logs         Ctrl+X: Cancelar processo".into(),
            " Ctrl+Q: Sair         C: Copiar endpoint".into(),
            "".into(),
            "Nao requer Codex CLI, Claude Code, chaves API ou".into(),
            "copia dos modelos no computador.".into(),
            "Precisa de uma conta compatível no ChatGPT/Claude.".into(),
        ],
    };
    (app.page.title(), Text::from(lines.join("\n")))
}

fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(8).min(78);
    let height = 8.min(area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let (title, text) = match app.edit {
        Edit::Workspace => ("Pasta de projetos", format!(
            "Digite o caminho de uma pasta existente:\n{}\n\nEnter confirmar | Esc cancelar", app.input)),
        Edit::Publish => ("CONFIRMACAO DE EXPOSICAO PUBLICA", format!(
            "Tailscale Funnel expora uma URL HTTPS.\nOAuth do DevSpace deve proteger o acesso.\nDigite PUBLICAR para continuar:\n{}\n\nEnter confirmar | Esc cancelar", app.input)),
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
    let mut terminal = ratatui::init();
    let result = (|| -> Result<(), Box<dyn Error>> {
        loop {
            app.refresh();
            terminal.draw(|frame| draw(frame, &app))?;
            if !event::poll(Duration::from_millis(150))? { continue; }
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.key(key) { break; }
                }
                Event::Paste(value) if app.edit != Edit::None => {
                    let remaining = 1024usize.saturating_sub(app.input.len());
                    app.input.extend(value.chars().filter(|c| *c != '\r' && *c != '\0').take(remaining));
                }
                _ => {}
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

fn main() -> Result<(), Box<dyn Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("DevSpace PC Bridge v{}", env!("CARGO_PKG_VERSION")); Ok(())
        }
        Some("--doctor") => {
            println!("{}", setup::status_summary());
            println!("Raiz: {}", Settings::load().workspace.display());
            println!("Diagnostico de rede: use a TUI > Configurar > T.");
            Ok(())
        }
        Some(arg) => Err(format!("Argumento nao reconhecido: {arg}").into()),
        None => run(),
    }
}
