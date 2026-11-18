mod backend;
mod setup;
mod storage;

use backend::{WorkerEvent, spawn_chat};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use setup::{Task, TaskEvent};
use std::{
    collections::VecDeque,
    error::Error,
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};
use storage::{Message, Provider, Settings};
use zeroize::Zeroize;

const SETUP_ITEMS: &[(&str, Option<Task>)] = &[
    ("Configurar DevSpace + Tailscale", Some(Task::InstallBridge)),
    ("Executar diagnostico", Some(Task::Diagnose)),
    ("Instalar Codex CLI", Some(Task::InstallCodex)),
    ("Instalar Claude Code CLI", Some(Task::InstallClaude)),
    ("Login Codex (ChatGPT)", Some(Task::LoginCodex)),
    ("Login Claude", Some(Task::LoginClaude)),
    ("Conectar MCP ao Codex", Some(Task::LinkCodexMcp)),
    ("Autorizar OAuth MCP do Codex", Some(Task::AuthCodexMcp)),
    ("Conectar MCP ao Claude", Some(Task::LinkClaudeMcp)),
    ("Copiar URL MCP", None),
    ("Abrir painel DevSpace", None),
    ("Testar health HTTPS", None),
];

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Page {
    Chat,
    Providers,
    Setup,
    Logs,
    Help,
}

impl Page {
    fn next(self) -> Self {
        match self {
            Self::Chat => Self::Providers,
            Self::Providers => Self::Setup,
            Self::Setup => Self::Logs,
            Self::Logs => Self::Help,
            Self::Help => Self::Chat,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    None,
    ApiKey,
    Root,
    Model,
    Endpoint,
    ConfirmWrite,
}

struct App {
    settings: Settings,
    page: Page,
    editor: Edit,
    input: String,
    feedback: String,
    logs: VecDeque<String>,
    setup_index: usize,
    provider_index: usize,
    chat_scroll: u16,
    follow_chat: bool,
    log_scroll: u16,
    busy_chat: bool,
    busy_setup: bool,
    chat_tx: Sender<WorkerEvent>,
    chat_rx: Receiver<WorkerEvent>,
    task_tx: Sender<TaskEvent>,
    task_rx: Receiver<TaskEvent>,
    last_status: Instant,
}

impl App {
    fn new() -> Self {
        let settings = Settings::load();
        let (chat_tx, chat_rx) = mpsc::channel();
        let (task_tx, task_rx) = mpsc::channel();
        let provider_index = Provider::ALL
            .iter()
            .position(|p| *p == settings.provider)
            .unwrap_or(0);
        Self {
            settings,
            page: Page::Chat,
            editor: Edit::None,
            input: String::new(),
            feedback: "Pronto. Enter envia mensagem | Tab alterna telas | F1 ajuda".into(),
            logs: VecDeque::new(),
            setup_index: 0,
            provider_index,
            chat_scroll: 0,
            follow_chat: true,
            log_scroll: 0,
            busy_chat: false,
            busy_setup: false,
            chat_tx,
            chat_rx,
            task_tx,
            task_rx,
            last_status: Instant::now(),
        }
    }

    fn note(&mut self, message: impl Into<String>) {
        let text = message.into();
        self.feedback = text.clone();
        self.logs.push_back(text);
        while self.logs.len() > 300 {
            self.logs.pop_front();
        }
        self.log_scroll = u16::MAX;
    }

    fn save(&mut self) {
        if let Err(error) = self.settings.save() {
            self.note(format!("Nao foi possivel salvar historico: {error}"));
        }
    }

    fn poll_workers(&mut self) {
        while let Ok(event) = self.chat_rx.try_recv() {
            match event {
                WorkerEvent::Chunk(delta) => {
                    if let Some(message) = self.settings.messages.last_mut() {
                        if message.role == "assistant" {
                            message.content.push_str(&delta);
                        }
                    }
                    self.follow_chat = true;
                }
                WorkerEvent::Info(text) => self.note(text),
                WorkerEvent::Session(provider, id) => match provider {
                    Provider::Codex => self.settings.codex_session_id = Some(id),
                    Provider::ClaudeCode => self.settings.claude_session_id = Some(id),
                    _ => {}
                },
                WorkerEvent::Finished(result) => {
                    self.busy_chat = false;
                    match result {
                        Ok(()) => self.note("Resposta concluida."),
                        Err(error) => {
                            self.note(format!("Erro: {error}"));
                            if let Some(last) = self.settings.messages.last_mut() {
                                if last.role == "assistant" && last.content.is_empty() {
                                    last.content = format!("[Erro: {error}]");
                                }
                            }
                        }
                    }
                    self.save();
                }
            }
        }
        while let Ok(event) = self.task_rx.try_recv() {
            match event {
                TaskEvent::Line(line) => self.note(line),
                TaskEvent::Finished(result) => {
                    self.busy_setup = false;
                    self.note(match result {
                        Ok(()) => "Operacao concluida com sucesso.".to_owned(),
                        Err(error) => format!("Falha: {error}. Confira o log."),
                    });
                }
            }
        }
    }

    fn send(&mut self) {
        if self.busy_chat {
            self.note("A resposta anterior ainda esta em execucao.");
            return;
        }
        let prompt = self.input.trim().to_owned();
        if prompt.is_empty() {
            return;
        }
        if prompt.starts_with('/') {
            self.input.clear();
            self.slash_command(&prompt);
            return;
        }
        if !self.settings.root.is_dir() {
            self.note("Configure uma pasta de projetos existente em Provedores (tecla P).");
            return;
        }
        self.input.clear();
        self.settings.messages.push(Message { role: "user".into(), content: prompt });
        self.settings.messages.push(Message { role: "assistant".into(), content: String::new() });
        self.busy_chat = true;
        self.follow_chat = true;
        self.page = Page::Chat;
        self.note(format!("Consultando {}...", self.settings.provider.label()));
        self.save();
        spawn_chat(self.settings.clone(), self.chat_tx.clone());
    }

    fn slash_command(&mut self, input: &str) {
        let (command, argument) = input.split_once(' ').unwrap_or((input, ""));
        let arg = argument.trim();
        match command.to_ascii_lowercase().as_str() {
            "/help" => self.page = Page::Help,
            "/setup" => self.page = Page::Setup,
            "/providers" => self.page = Page::Providers,
            "/logs" => self.page = Page::Logs,
            "/clear" | "/new" => {
                self.settings.new_chat();
                self.save();
                self.note("Conversa reiniciada.");
            }
            "/model" => {
                self.settings.model = arg.into();
                self.settings.new_chat();
                self.save();
                self.note(format!("Modelo: {}", if arg.is_empty() { "padrao" } else { arg }));
            }
            "/provider" => {
                if let Some(next) = Provider::ALL.iter().copied().find(|p| p.id() == arg) {
                    self.settings.switch_provider(next);
                    self.provider_index = Provider::ALL.iter().position(|p| *p == next).unwrap_or(0);
                    self.save();
                    self.note(format!("Provedor: {}", next.label()));
                } else {
                    self.note("Use: /provider codex|claude_code|openai|anthropic|openrouter|ollama");
                }
            }
            "/cd" => {
                let root = PathBuf::from(arg);
                if root.is_dir() {
                    self.settings.root = fs::canonicalize(&root).unwrap_or(root);
                    self.settings.new_chat();
                    self.save();
                    self.note(format!("Pasta: {}", self.settings.root.display()));
                } else {
                    self.note("Use: /cd C:\\caminho\\existente");
                }
            }
            "/mcp" => self.note(storage::devspace_mcp_url().unwrap_or_else(|| "Conecte o DevSpace na aba Setup.".into())),
            "/write" => self.toggle_write(),
            "/read" => {
                if self.settings.allow_edits {
                    self.toggle_write();
                } else {
                    self.note("Modo somente leitura ja esta ativo.");
                }
            }
            "/status" => self.note(format!(
                "{} | {} | {} mensagens | MCP: {}",
                self.settings.provider.label(),
                self.settings.root.display(),
                self.settings.messages.len(),
                if storage::devspace_mcp_url().is_some() { "configurado" } else { "pendente" },
            )),
            _ => self.note(format!("Comando desconhecido: {command}. Digite /help.")),
        }
    }

    fn start(&mut self, task: Task) {
        if self.busy_setup {
            self.note("Uma instalacao ou comando ja esta em execucao.");
            return;
        }
        self.busy_setup = true;
        self.page = Page::Logs;
        self.note(format!("Iniciando: {}. Acompanhe tudo nesta tela.", task.label()));
        setup::start_task(task, self.settings.root.clone(), self.task_tx.clone());
    }

    fn submit_edit(&mut self) {
        let value = self.input.trim().to_owned();
        match self.editor {
            Edit::Root => {
                let path = PathBuf::from(value);
                if !path.is_dir() {
                    self.note("Pasta invalida: informe o caminho de uma pasta existente.");
                    return;
                }
                self.settings.root = fs::canonicalize(&path).unwrap_or(path);
                self.settings.new_chat();
                self.note("Pasta de projetos alterada. Novo contexto de conversa.");
                self.save();
            }
            Edit::Model => {
                self.settings.model = value;
                self.settings.new_chat();
                self.note("Modelo alterado e nova conversa iniciada.");
                self.save();
            }
            Edit::Endpoint => {
                if !value.is_empty() && !(value.starts_with("https://") || value.starts_with("http://127.0.0.1:") || value.starts_with("http://localhost:")) {
                    self.note("Informe HTTPS ou HTTP local para Ollama.");
                    return;
                }
                self.settings.endpoint = value;
                self.settings.new_chat();
                self.note("Endpoint atualizado.");
                self.save();
            }
            Edit::ApiKey => {
                match storage::store_key(self.settings.provider, &value) {
                    Ok(()) => self.note("Chave salva no Gerenciador de Credenciais do Windows."),
                    Err(error) => {
                        self.note(error);
                        return;
                    }
                }
            }
            Edit::ConfirmWrite => {
                if value == "AUTORIZAR" {
                    self.settings.allow_edits = true;
                    self.settings.new_chat();
                    self.note("Modo edicao ativado para Codex/Claude. Nova conversa iniciada.");
                    self.save();
                } else {
                    self.note("Permissao nao concedida. Modo leitura mantido.");
                }
            }
            Edit::None => return,
        }
        if self.editor == Edit::ApiKey {
            self.input.zeroize();
        } else {
            self.input.clear();
        }
        self.editor = Edit::None;
    }

    fn start_edit(&mut self, mode: Edit) {
        self.editor = mode;
        self.input = match mode {
            Edit::Root => self.settings.root.to_string_lossy().into_owned(),
            Edit::Model => self.settings.model().to_owned(),
            Edit::Endpoint => self.settings.endpoint().to_owned(),
            _ => String::new(),
        };
    }

    fn toggle_write(&mut self) {
        if !self.settings.provider.is_agent() {
            self.note("Modo de edicao pertence aos agentes CLI, nao ao chat API.");
            return;
        }
        if self.settings.allow_edits {
            self.settings.allow_edits = false;
            self.settings.new_chat();
            self.note("Modo somente leitura ativado.");
            self.save();
        } else {
            self.start_edit(Edit::ConfirmWrite);
            self.note("Digite AUTORIZAR para permitir edicoes na pasta de projetos.");
        }
    }

    fn select_provider(&mut self) {
        let next = Provider::ALL[self.provider_index];
        if self.busy_chat {
            self.note("Aguarde o agente terminar antes de trocar de provedor.");
            return;
        }
        self.settings.switch_provider(next);
        self.save();
        self.note(format!("Provedor: {}", next.label()));
    }

    fn select_setup(&mut self) {
        let selected = self.setup_index;
        if let Some(task) = SETUP_ITEMS[selected].1 {
            self.start(task);
        } else {
            match selected {
                9 => self.note(match setup::copy_url() {
                    Ok(url) => format!("MCP copiado: {url}"),
                    Err(err) => err,
                }),
                10 => self.note(match setup::open_panel() {
                    Ok(()) => "Painel aberto no navegador.".into(),
                    Err(err) => err,
                }),
                11 => {
                    self.note("Verificando health publico...");
                    let tx = self.task_tx.clone();
                    self.busy_setup = true;
                    self.page = Page::Logs;
                    std::thread::spawn(move || {
                        let result = setup::test_mcp_endpoint()
                            .map(|message| {
                                let _ = tx.send(TaskEvent::Line(message));
                            });
                        let _ = tx.send(TaskEvent::Finished(result));
                    });
                }
                _ => {}
            }
        }
    }

    fn shortcut(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('q') => {
                    if self.busy_chat || self.busy_setup {
                        self.note("Ha operacoes em andamento. Espere concluirem para sair.");
                        return false;
                    }
                    return true;
                }
                KeyCode::Char('n') => {
                    if !self.busy_chat {
                        self.settings.new_chat();
                        self.save();
                        self.note("Nova conversa criada.");
                        self.follow_chat = true;
                    }
                }
                KeyCode::Char('p') => self.page = Page::Providers,
                KeyCode::Char('s') => self.page = Page::Setup,
                KeyCode::Char('l') => self.page = Page::Logs,
                KeyCode::Char('h') => self.page = Page::Help,
                _ => return false,
            }
            return false;
        }
        if self.editor != Edit::None {
            match key.code {
                KeyCode::Enter => self.submit_edit(),
                KeyCode::Esc => {
                    if self.editor == Edit::ApiKey {
                        self.input.zeroize();
                    }
                    self.editor = Edit::None;
                    self.input.clear();
                    self.note("Edicao cancelada.");
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(c) => {
                    if self.input.chars().count() < 8_000 {
                        self.input.push(c);
                    }
                }
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::F(1) => self.page = Page::Help,
            KeyCode::Tab => self.page = self.page.next(),
            KeyCode::Esc => self.page = Page::Chat,
            KeyCode::Enter => match self.page {
                Page::Chat => self.send(),
                Page::Providers => self.select_provider(),
                Page::Setup => self.select_setup(),
                _ => {}
            },
            KeyCode::Up => match self.page {
                Page::Providers => self.provider_index = self.provider_index.saturating_sub(1),
                Page::Setup => self.setup_index = self.setup_index.saturating_sub(1),
                Page::Chat => {
                    self.follow_chat = false;
                    self.chat_scroll = self.chat_scroll.saturating_sub(1);
                }
                Page::Logs => self.log_scroll = self.log_scroll.saturating_sub(1),
                _ => {}
            },
            KeyCode::Down => match self.page {
                Page::Providers => self.provider_index = (self.provider_index + 1).min(Provider::ALL.len() - 1),
                Page::Setup => self.setup_index = (self.setup_index + 1).min(SETUP_ITEMS.len() - 1),
                Page::Chat => self.chat_scroll = self.chat_scroll.saturating_add(1),
                Page::Logs => self.log_scroll = self.log_scroll.saturating_add(1),
                _ => {}
            },
            KeyCode::PageUp => {
                self.follow_chat = false;
                self.chat_scroll = self.chat_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.chat_scroll = self.chat_scroll.saturating_add(10);
                self.log_scroll = self.log_scroll.saturating_add(10);
            }
            KeyCode::End => {
                self.follow_chat = true;
                self.log_scroll = u16::MAX;
            }
            KeyCode::Backspace if self.page == Page::Chat => { self.input.pop(); }
            KeyCode::Char('p') if self.page == Page::Providers => self.start_edit(Edit::Root),
            KeyCode::Char('m') if self.page == Page::Providers => self.start_edit(Edit::Model),
            KeyCode::Char('e') if self.page == Page::Providers => self.start_edit(Edit::Endpoint),
            KeyCode::Char('k') if self.page == Page::Providers => self.start_edit(Edit::ApiKey),
            KeyCode::Char('w') if self.page == Page::Providers => self.toggle_write(),
            KeyCode::Char(c) if self.page == Page::Chat => {
                if self.input.chars().count() < 8_000 {
                    self.input.push(c);
                }
            }
            _ => {}
        }
        false
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(8), Constraint::Length(3)])
        .split(area);
    let title = Line::from(vec![
        Span::styled("  DEVSPACE ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled("BRIDGE", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(
            "  Chat  |  Provedores  |  Setup  |  Logs  |  Ajuda",
            Style::default().fg(Color::Gray),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::BOTTOM)),
        sections[0],
    );
    match app.page {
        Page::Chat => draw_chat(frame, sections[1], app),
        Page::Providers => draw_providers(frame, sections[1], app),
        Page::Setup => draw_setup(frame, sections[1], app),
        Page::Logs => draw_logs(frame, sections[1], app),
        Page::Help => draw_help(frame, sections[1]),
    }
    let indicator = if app.busy_chat {
        "AGENTE ATIVO"
    } else if app.busy_setup {
        "CONFIGURANDO"
    } else {
        "PRONTO"
    };
    frame.render_widget(
        Paragraph::new(format!(" {} | {}\n Ctrl+Q sair  Tab telas  Ctrl+N novo chat  F1 ajuda", indicator, app.feedback))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::TOP)),
        sections[2],
    );
}

fn draw_chat(frame: &mut Frame, bounds: Rect, app: &App) {
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(4), Constraint::Length(5)]).split(bounds);
    let mode = if app.settings.allow_edits && app.settings.provider.is_agent() { "EDICAO" } else { "LEITURA" };
    frame.render_widget(
        Paragraph::new(format!(
            " {}  |  Modelo: {}  |  {}  |  {}",
            app.settings.provider.label(),
            if app.settings.model().is_empty() { "padrao da conta" } else { app.settings.model() },
            mode,
            app.settings.root.display(),
        ))
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(" Sessao ").border_style(Color::DarkGray)),
        rows[0],
    );
    let mut lines: Vec<Line> = Vec::new();
    if app.settings.messages.is_empty() {
        lines.push(Line::styled("  Converse com Codex, Claude ou um provedor de API.", Style::default().fg(Color::DarkGray)));
        lines.push(Line::styled("  Ctrl+P: escolher provedor | Ctrl+S: conectar DevSpace", Style::default().fg(Color::DarkGray)));
        lines.push(Line::raw(""));
        lines.push(Line::raw("  Codex/Claude usam as ferramentas dos agentes instalados."));
        lines.push(Line::raw("  APIs e Ollama permitem chat, sem ferramentas locais automaticas."));
    }
    for message in &app.settings.messages {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            if message.role == "user" { "  VOCE" } else { "  ASSISTENTE" },
            Style::default()
                .fg(if message.role == "user" { Color::Green } else { Color::Cyan })
                .add_modifier(Modifier::BOLD),
        ));
        for line in message.content.lines() {
            lines.push(Line::raw(format!("  {line}")));
        }
    }
    if app.busy_chat {
        lines.push(Line::styled("  ... recebendo resposta ...", Style::default().fg(Color::Yellow)));
    }
    let display_height = rows[1].height.saturating_sub(2);
    let approx_count = lines
        .iter()
        .map(|line| {
            let width = usize::from(rows[1].width.saturating_sub(5).max(1));
            ((line.width() + width.saturating_sub(1)) / width).max(1)
        })
        .sum::<usize>();
    let last = approx_count.saturating_sub(display_height as usize);
    let offset = if app.follow_chat { last.min(u16::MAX as usize) as u16 } else { app.chat_scroll };
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .scroll((offset, 0))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Conversa ").border_style(Color::DarkGray)),
        rows[1],
    );
    let editor_label = match app.editor {
        Edit::ApiKey => " Chave API (oculta; Enter salva no Windows) ",
        Edit::Root => " Caminho do projeto (Enter salva; Esc volta) ",
        Edit::Model => " Modelo (Enter salva; Esc volta) ",
        Edit::Endpoint => " Endpoint API (Enter salva; Esc volta) ",
        Edit::ConfirmWrite => " Digite AUTORIZAR para permitir edicao ",
        Edit::None => " Sua mensagem (Enter enviar) ",
    };
    let input = if app.editor == Edit::ApiKey {
        "•".repeat(app.input.chars().count())
    } else {
        app.input.clone()
    };
    frame.render_widget(
        Paragraph::new(format!("{}\n{}", input, if app.busy_chat { "Aguardando resposta..." } else { "Digite a mensagem e pressione Enter." }))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(editor_label).border_style(Color::Cyan)),
        rows[2],
    );
}

fn draw_providers(frame: &mut Frame, bounds: Rect, app: &App) {
    let split = Layout::horizontal([Constraint::Length(39), Constraint::Min(20)]).split(bounds);
    let rows = Provider::ALL.iter().enumerate().map(|(index, provider)| {
        let active = if *provider == app.settings.provider { " *" } else { "" };
        let style = if index == app.provider_index {
            Style::default().bg(Color::Cyan).fg(Color::Black)
        } else {
            Style::default().fg(Color::White)
        };
        ListItem::new(format!(" {}{}", provider.label(), active)).style(style)
    });
    frame.render_widget(List::new(rows).block(Block::bordered().title(" Provedores ").border_style(Color::DarkGray)), split[0]);
    let current = Provider::ALL[app.provider_index];
    let body = format!(
        "Selecionado: {}\n\nModelo: {}\nEndpoint: {}\n\nPasta: {}\nPermissao: {}\n\nEnter: ativar provedor\nP: editar pasta\nM: editar modelo\nE: editar endpoint\nK: salvar chave API\nW: alternar permissao de edicao\n\nCodex: usa sua autenticacao ChatGPT no CLI.\nClaude Code: usa autenticacao do Claude CLI.\nAPI: exige chave propria, armazenada no Windows.\nOllama: roda modelos localmente.",
        current.label(),
        if app.settings.model().is_empty() { current.default_model() } else { app.settings.model() },
        if app.settings.endpoint().is_empty() { current.default_endpoint() } else { app.settings.endpoint() },
        app.settings.root.display(),
        if app.settings.allow_edits { "edicao autorizada" } else { "somente leitura" },
    );
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Configuracao ").border_style(Color::DarkGray)),
        split[1],
    );
    if app.editor != Edit::None {
        let box_rect = centered(bounds, 78, 8);
        frame.render_widget(ratatui::widgets::Clear, box_rect);
        let secret = app.editor == Edit::ApiKey;
        let content = if secret { "•".repeat(app.input.chars().count()) } else { app.input.clone() };
        frame.render_widget(
            Paragraph::new(format!("{}\n\nEnter: salvar  |  Esc: cancelar", content))
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(if secret { " Credencial Windows " } else { " Editar configuracao " }).border_style(Color::Yellow)),
            box_rect,
        );
    }
}

fn draw_setup(frame: &mut Frame, bounds: Rect, app: &App) {
    let layout = Layout::horizontal([Constraint::Length(40), Constraint::Min(20)]).split(bounds);
    let items = SETUP_ITEMS.iter().enumerate().map(|(index, (label, _))| {
        let style = if index == app.setup_index {
            Style::default().bg(Color::Cyan).fg(Color::Black)
        } else {
            Style::default().fg(Color::White)
        };
        ListItem::new(format!(" {label}")).style(style)
    });
    frame.render_widget(List::new(items).block(Block::bordered().title(" Setup (Enter executar) ").border_style(Color::DarkGray)), layout[0]);
    let mcp = storage::devspace_mcp_url().unwrap_or_else(|| "(ainda nao configurado)".into());
    let body = format!(
        "STATUS\n{}\n\nMCP\n{}\n\nPASTA\n{}\n\n{}\n\nTodas as etapas sao executadas em segundo plano, com logs visiveis na aba Logs.\nNenhum CMD extra e aberto pelo aplicativo.\n\nTailscale e OAuth podem exigir aprovacao no navegador. A instalacao de pacotes pode solicitar UAC.\n\nApenas o setup altera configuracoes do sistema.",
        if app.busy_setup { "Operacao em andamento" } else { "Pronto para configurar" },
        mcp,
        app.settings.root.display(),
        app.feedback,
    );
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }).block(Block::bordered().title(" Conexao DevSpace ").border_style(Color::DarkGray)), layout[1]);
}

fn draw_logs(frame: &mut Frame, bounds: Rect, app: &App) {
    let body = app.logs.iter().map(|line| Line::raw(line.as_str())).collect::<Vec<_>>();
    let height = bounds.height.saturating_sub(2);
    let max_offset = body.len().saturating_sub(height as usize);
    let offset = if app.log_scroll == u16::MAX { max_offset.min(u16::MAX as usize) as u16 } else { app.log_scroll };
    frame.render_widget(
        Paragraph::new(Text::from(body))
            .wrap(Wrap { trim: false })
            .scroll((offset, 0))
            .block(Block::bordered().title(" Log ao vivo (setas para navegar) ").border_style(Color::DarkGray)),
        bounds,
    );
}

fn draw_help(frame: &mut Frame, bounds: Rect) {
    let help = [
        "DEVSPACE PC BRIDGE — GUIA RAPIDO",
        "",
        "1. Provedores: escolha Codex, Claude, OpenAI, Anthropic, OpenRouter ou Ollama.",
        "2. Codex/Claude Code: instale e autentique seus respectivos CLIs na aba Setup.",
        "3. Para acesso aos seus arquivos via MCP: Setup > Instalar DevSpace + Tailscale.",
        "4. Depois conecte DevSpace via MCP aos agentes; conclua OAuth no navegador.",
        "5. Volte ao Chat: digite e envie com Enter.",
        "",
        "ATALHOS",
        "Tab              Alternar telas, sem abrir outras janelas de terminal.",
        "Ctrl+P / S / L   Provedores / Setup / Logs",
        "Ctrl+N           Nova conversa (nao apaga chats antigos do Codex/Claude).",
        "Comandos /help, /new, /model, /provider, /cd, /mcp, /status, /read, /write",
        "Ctrl+Q           Sair (requer tarefas concluidas).",
        "F1              Ajuda",
        "Esc             Voltar ao Chat / cancelar edicao",
        "Setas            Escolher provedor/acao ou navegar na conversa",
        "End             Ir ao final da conversa",
        "",
        "SEGURANCA",
        "Modo somente leitura por padrao. Edicao exige autorizacao explicita.",
        "Credenciais API ficam no Windows Credential Manager; nao no JSON da conversa.",
        "Historico de conversas fica no APPDATA da sua conta Windows.",
        "O Funnel publica HTTPS na internet: mantenha Owner/OAuth e roots restritos.",
        "O ChatGPT e o Claude nao fornecem API por meio da assinatura web.",
    ].join("\n");
    frame.render_widget(
        Paragraph::new(help)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Ajuda ").border_style(Color::DarkGray)),
        bounds,
    );
}

fn centered(bounds: Rect, percent: u16, height: u16) -> Rect {
    let width = bounds.width.saturating_mul(percent).saturating_div(100).max(15);
    let x = bounds.x + bounds.width.saturating_sub(width) / 2;
    let y = bounds.y + bounds.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(bounds.width), height.min(bounds.height))
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut app = App::new();
    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), EnableBracketedPaste)?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        loop {
            app.poll_workers();
            if app.last_status.elapsed() > Duration::from_secs(5) {
                app.last_status = Instant::now();
            }
            terminal.draw(|frame| draw(frame, &app))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.shortcut(key) {
                        break;
                    }
                }
                Event::Paste(value) if app.page == Page::Chat || app.editor != Edit::None => {
                    let max = 8_000usize.saturating_sub(app.input.chars().count());
                    app.input.extend(value.chars().filter(|c| *c != '\r' && *c != '\0').take(max));
                }
                _ => {}
            }
        }
        Ok(())
    })();
    let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

fn main() -> Result<(), Box<dyn Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("DevSpace PC Bridge v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--doctor") => {
            println!("URL MCP: {}", storage::devspace_mcp_url().unwrap_or_else(|| "nao configurado".into()));
            println!("Codex CLI: {}", backend::is_available("codex"));
            println!("Claude Code CLI: {}", backend::is_available("claude"));
            Ok(())
        }
        Some(other) => Err(format!("Argumento desconhecido: {other}").into()),
        None => run(),
    }
}
