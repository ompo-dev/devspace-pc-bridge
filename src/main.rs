#![cfg_attr(not(windows), allow(dead_code))]

use std::{
    env,
    error::Error,
    fs,
    io::{self, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    DefaultTerminal, Frame,
};

const ACTIONS: &[&str] = &[
    "Configurar / instalar",
    "Escolher pasta de projetos",
    "Atualizar diagnostico",
    "Abrir painel DevSpace",
    "Copiar URL do MCP",
    "Ajuda",
    "Sair",
];
const PORT: u16 = 7676;

const INSTALL_PS1: &str = include_str!("../install.ps1");
const BRIDGE_PS1: &str = include_str!("../scripts/bridge.ps1");
const DOCTOR_PS1: &str = include_str!("../scripts/doctor.ps1");
const UNINSTALL_PS1: &str = include_str!("../scripts/uninstall.ps1");

#[derive(PartialEq, Eq, Clone, Copy)]
enum Page {
    Home,
    EditRoot,
    Help,
}

struct App {
    selected: usize,
    page: Page,
    root: String,
    installed_host: Option<String>,
    status: String,
    feedback: String,
    last_refresh: Instant,
}

impl App {
    fn new() -> Self {
        let root = default_root().to_string_lossy().into_owned();
        let mut app = Self {
            selected: 0,
            page: Page::Home,
            root,
            installed_host: None,
            status: String::new(),
            feedback: "Setas e Enter para navegar. Pressione I para instalar.".into(),
            last_refresh: Instant::now() - Duration::from_secs(10),
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        let settings = settings_path();
        self.installed_host = read_hostname(&settings);
        let local = TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], PORT)),
            Duration::from_millis(400),
        )
        .is_ok();
        let node = command_exists("node", "--version");
        let devspace = command_exists("devspace", "--version");
        let tailscale = tailscale_executable().is_some();
        self.status = format!(
            "DevSpace local       {}\nNode.js              {}\nDevSpace CLI         {}\nTailscale            {}\nConfiguracao         {}\n\nMCP publico:\n{}",
            indicator(local),
            indicator(node),
            indicator(devspace),
            indicator(tailscale),
            if self.installed_host.is_some() { "encontrada" } else { "pendente" },
            self.mcp_url().unwrap_or_else(|| "Execute Configurar / instalar".into())
        );
        self.last_refresh = Instant::now();
    }

    fn mcp_url(&self) -> Option<String> {
        self.installed_host
            .as_ref()
            .map(|host| format!("https://{host}/mcp"))
    }

    fn panel_url(&self) -> Option<String> {
        self.installed_host
            .as_ref()
            .map(|host| format!("https://{host}/web/"))
    }
}

fn indicator(yes: bool) -> &'static str {
    if yes { "[OK]" } else { "[--]" }
}

fn home() -> PathBuf {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_root() -> PathBuf {
    let base = home();
    [
        base.join("OneDrive").join("Documents").join("GitHub"),
        base.join("Documents").join("GitHub"),
        base.join("Documents"),
        base.clone(),
    ]
    .into_iter()
    .find(|path| path.is_dir())
    .unwrap_or(base)
}

fn settings_path() -> PathBuf {
    home().join(".devspace").join("pc-bridge").join("settings.json")
}

fn valid_hostname(value: &str) -> bool {
    value.ends_with(".ts.net")
        && value.len() > ".ts.net".len()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

fn read_hostname(path: &Path) -> Option<String> {
    let data = fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;
    let host = json.get("hostname")?.as_str()?;
    valid_hostname(host).then(|| host.to_owned())
}

fn command_exists(command: &str, arg: &str) -> bool {
    Command::new(command)
        .arg(arg)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn tailscale_executable() -> Option<PathBuf> {
    if let Some(program_files) = env::var_os("ProgramFiles") {
        let full = PathBuf::from(program_files).join("Tailscale").join("tailscale.exe");
        if full.is_file() {
            return Some(full);
        }
    }
    if command_exists("tailscale", "version") {
        return Some(PathBuf::from("tailscale"));
    }
    None
}

fn draw(frame: &mut Frame, app: &App) {
    let bounds = frame.area();
    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(12), Constraint::Length(4)])
        .split(bounds);
    let header = Paragraph::new(Line::from(vec![
        Span::styled("  DEVSPACE ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled("PC BRIDGE", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled("  v0.2  |  Windows + MCP", Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM).border_style(Color::DarkGray));
    frame.render_widget(header, main[0]);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(31), Constraint::Min(38)])
        .split(main[1]);
    let options: Vec<ListItem> = ACTIONS
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let marker = if app.selected == index { "> " } else { "  " };
            let style = if app.selected == index {
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            ListItem::new(format!("{marker}{title}")).style(style)
        })
        .collect();
    frame.render_widget(
        List::new(options).block(Block::bordered().title(" Menu ").border_style(Color::DarkGray)),
        columns[0],
    );

    let body = match app.page {
        Page::Home => Text::from(vec![
            Line::styled(" CONEXAO", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Line::raw(""),
            Line::raw(app.status.as_str()),
            Line::raw(""),
            Line::styled(" PASTA DE PROJETOS", Style::default().fg(Color::Cyan)),
            Line::raw(app.root.as_str()),
            Line::raw(""),
            Line::styled(" Autorizacao OAuth e Tailscale requerem sua aprovacao.", Style::default().fg(Color::DarkGray)),
        ]),
        Page::EditRoot => Text::from(vec![
            Line::styled(" ESCOLHA A RAIZ DE PROJETOS", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Line::raw(""),
            Line::raw("Digite o caminho completo de uma pasta ja existente:"),
            Line::raw(""),
            Line::styled(format!("{}_", app.root), Style::default().fg(Color::Yellow)),
            Line::raw(""),
            Line::raw("Enter: salvar    Esc: cancelar"),
        ]),
        Page::Help => Text::from(vec![
            Line::styled(" PRIMEIROS PASSOS", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Line::raw(""),
            Line::raw("1. Selecione a pasta dos seus projetos."),
            Line::raw("2. Pressione I para iniciar a configuracao."),
            Line::raw("3. Aprove o Windows (UAC) e entre no Tailscale."),
            Line::raw("4. Guarde sua senha Owner e conecte a URL /mcp no ChatGPT."),
            Line::raw(""),
            Line::raw("Teclas: setas, Enter, I, R, D, O, C, H, Q."),
            Line::raw(""),
            Line::styled("Inspirado na experiencia terminal do OpenClaude.", Style::default().fg(Color::DarkGray)),
            Line::styled("Nao executa modelos de IA nem incorpora o OpenClaude.", Style::default().fg(Color::DarkGray)),
        ]),
    };
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Workspace / status ").border_style(Color::DarkGray)),
        columns[1],
    );
    frame.render_widget(
        Paragraph::new(format!("{}\n  [I] Instalar  [R] Pasta  [D] Diagnostico  [O] Painel  [C] URL  [Q] Sair", app.feedback))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Acoes ").border_style(Color::DarkGray)),
        main[2],
    );
}

enum Action {
    Install,
    Doctor,
    OpenPanel,
    CopyUrl,
    Quit,
}

fn choose(app: &mut App) -> Option<Action> {
    match app.selected {
        0 => Some(Action::Install),
        1 => { app.page = Page::EditRoot; None }
        2 => Some(Action::Doctor),
        3 => Some(Action::OpenPanel),
        4 => Some(Action::CopyUrl),
        5 => { app.page = Page::Help; None }
        _ => Some(Action::Quit),
    }
}

fn ui_loop() -> Result<(), Box<dyn Error>> {
    let mut app = App::new();
    let mut terminal = ratatui::init();
    let outcome = (|| -> Result<(), Box<dyn Error>> {
        loop {
            if app.last_refresh.elapsed() > Duration::from_secs(5) {
                app.refresh();
            }
            terminal.draw(|frame| draw(frame, &app))?;
            if !event::poll(Duration::from_millis(250))? {
                continue;
            }
            let Event::Key(key) = event::read()? else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let action = if app.page == Page::EditRoot {
                match key.code {
                    KeyCode::Esc => { app.page = Page::Home; None }
                    KeyCode::Enter => {
                        let proposed = PathBuf::from(app.root.trim());
                        if proposed.is_dir() {
                            app.root = fs::canonicalize(proposed)?.to_string_lossy().into_owned();
                            app.feedback = "Pasta salva para a configuracao.".into();
                            app.page = Page::Home;
                        } else {
                            app.feedback = "A pasta nao existe. Digite um caminho valido.".into();
                        }
                        None
                    }
                    KeyCode::Backspace => { app.root.pop(); None }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.root.push(c);
                        None
                    }
                    _ => None,
                }
            } else {
                match key.code {
                    KeyCode::Up => { app.selected = app.selected.saturating_sub(1); None }
                    KeyCode::Down => { app.selected = (app.selected + 1).min(ACTIONS.len() - 1); None }
                    KeyCode::Enter => choose(&mut app),
                    KeyCode::Esc => { app.page = Page::Home; None }
                    KeyCode::Char(c) => match c.to_ascii_lowercase() {
                        'i' => Some(Action::Install),
                        'r' => { app.page = Page::EditRoot; None }
                        'd' => Some(Action::Doctor),
                        'o' => Some(Action::OpenPanel),
                        'c' => Some(Action::CopyUrl),
                        'h' => { app.page = Page::Help; None }
                        'q' => Some(Action::Quit),
                        _ => None,
                    },
                    _ => None,
                }
            };
            match action {
                Some(Action::Quit) => break,
                Some(Action::Install) => {
                    if !Path::new(&app.root).is_dir() {
                        app.feedback = "Escolha uma pasta de projetos existente primeiro.".into();
                        continue;
                    }
                    ratatui::restore();
                    let result = request_install(&app.root);
                    terminal = ratatui::init();
                    app.feedback = match result {
                        Ok(()) => "Configuracao encerrada. Verifique diagnostico e conecte o MCP.".into(),
                        Err(err) => format!("Nao foi possivel iniciar: {err}"),
                    };
                    app.refresh();
                    app.page = Page::Home;
                }
                Some(Action::Doctor) => {
                    ratatui::restore();
                    let result = run_diagnostics();
                    terminal = ratatui::init();
                    app.feedback = match result {
                        Ok(()) => "Diagnostico encerrado.".into(),
                        Err(err) => format!("Falha no diagnostico: {err}"),
                    };
                    app.refresh();
                }
                Some(Action::OpenPanel) => {
                    if let Some(url) = app.panel_url() {
                        app.feedback = match Command::new("explorer.exe").arg(url).spawn() {
                            Ok(_) => "Painel aberto no navegador. Faca login Owner.".into(),
                            Err(err) => format!("Nao foi possivel abrir navegador: {err}"),
                        };
                    } else {
                        app.feedback = "Configure o DevSpace primeiro.".into();
                    }
                }
                Some(Action::CopyUrl) => {
                    if let Some(url) = app.mcp_url() {
                        app.feedback = match Command::new("powershell.exe")
                            .args(["-NoProfile", "-Command", "Set-Clipboard -Value $args[0]", url.as_str()])
                            .output()
                        {
                            Ok(output) if output.status.success() => "URL do MCP copiada.".into(),
                            _ => format!("Copie manualmente: {url}"),
                        };
                    } else {
                        app.feedback = "Configure o DevSpace primeiro.".into();
                    }
                }
                None => {}
            }
        }
        Ok(())
    })();
    ratatui::restore();
    outcome
}

fn bundle_dir() -> PathBuf {
    env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("DevSpacePCBridge")
        .join("setup")
}

fn write_bundle() -> io::Result<PathBuf> {
    let base = bundle_dir();
    fs::create_dir_all(base.join("scripts"))?;
    for (path, contents) in [
        ("install.ps1", INSTALL_PS1),
        ("scripts/bridge.ps1", BRIDGE_PS1),
        ("scripts/doctor.ps1", DOCTOR_PS1),
        ("scripts/uninstall.ps1", UNINSTALL_PS1),
    ] {
        fs::write(base.join(path), contents)?;
    }
    Ok(base)
}

fn hex_encode(text: &str) -> String {
    text.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(hex: &str) -> Result<String, Box<dyn Error>> {
    if hex.len() % 2 != 0 || hex.len() > 32768 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Argumento de caminho invalido".into());
    }
    let bytes: Result<Vec<u8>, _> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect();
    Ok(String::from_utf8(bytes?)?)
}

fn ps_quote(text: &str) -> String {
    text.replace('\'', "''")
}

fn request_install(root: &str) -> Result<(), Box<dyn Error>> {
    let exe = env::current_exe()?;
    let root_hex = hex_encode(root);
    let script = format!(
        "Start-Process -FilePath '{}' -ArgumentList '--install-hex','{}' -Verb RunAs -Wait",
        ps_quote(&exe.to_string_lossy()),
        root_hex
    );
    println!("Solicitando permissao do Windows (UAC)...");
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .status()?;
    if !status.success() {
        return Err("Permissao negada ou instalador cancelado".into());
    }
    Ok(())
}

fn execute_installer(root: &str) -> Result<(), Box<dyn Error>> {
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        return Err(format!("Pasta de projetos nao existe: {root}").into());
    }
    let base = write_bundle()?;
    println!("DevSpace PC Bridge - instalacao automatica");
    println!("Projetos: {root}");
    println!("Se solicitado, autorize Tailscale e guarde a senha Owner.\n");
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(base.join("install.ps1"))
        .args(["-WorkspacesRoot", root])
        .status()?;
    println!("\nInstalador finalizado com codigo: {status}");
    println!("Pressione Enter para voltar ao aplicativo...");
    let _ = io::stdin().read_line(&mut String::new());
    if !status.success() {
        return Err(format!("Instalador falhou: {status}").into());
    }
    Ok(())
}

fn run_diagnostics() -> Result<(), Box<dyn Error>> {
    let base = write_bundle()?;
    println!("Diagnostico DevSpace PC Bridge\n");
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(base.join("scripts").join("doctor.ps1"))
        .status()?;
    println!("\nPressione Enter para voltar...");
    let _ = io::stdin().read_line(&mut String::new());
    if !status.success() {
        return Err(format!("Diagnostico encerrou com {status}").into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("--install-hex") => {
            let path = hex_decode(&args.next().ok_or("Caminho ausente")?)?;
            execute_installer(&path)
        }
        Some("--doctor") => run_diagnostics(),
        Some("--version") | Some("-V") => {
            println!("DevSpace PC Bridge v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(flag) => Err(format!("Argumento desconhecido: {flag}").into()),
        None => ui_loop(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_malformed() {
        let path = r"C:\Users\João\Meus Projetos";
        assert_eq!(hex_decode(&hex_encode(path)).unwrap(), path);
        assert!(hex_decode("1").is_err());
        assert!(hex_decode("zz").is_err());
    }

    #[test]
    fn accepts_only_tailscale_hosts() {
        assert!(valid_hostname("computer.tail123.ts.net"));
        assert!(!valid_hostname("google.com"));
        assert!(!valid_hostname("evil.ts.net/attack"));
        assert!(!valid_hostname("X.ts.net"));
    }

    #[test]
    fn scripts_are_embedded() {
        assert!(INSTALL_PS1.contains("DevSpacePCBridge"));
        assert!(BRIDGE_PS1.contains("settings.json"));
        assert!(DOCTOR_PS1.contains("devspace"));
    }
}
