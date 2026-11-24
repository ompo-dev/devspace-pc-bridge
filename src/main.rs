mod app;
mod checkpoint;
mod checks;
mod clock;
mod cloudflare;
mod desktop;
mod redact;
mod remote;
mod setup;
mod storage;
#[cfg(windows)]
mod tray;
mod tunnel;
mod ui;
mod update;

use app::{App, Edit, MAX_INPUT_CHARS};
use crossterm::event::{self, Event, KeyEventKind};
use setup::Protection;
#[cfg(windows)]
use std::sync::Arc;
use std::{error::Error, path::PathBuf, time::Duration};
use storage::Settings;

fn run() -> Result<(), Box<dyn Error>> {
    let mut app = App::new();
    if let Some(message) = update::take_outcome() {
        app.log(message);
    }
    #[cfg(windows)]
    let desktop_server = {
        let state = Arc::new(remote::RemoteState::new(
            Arc::new(desktop::windows_backend::WindowsDesktop::new()),
            desktop::windows_backend::protected_process_ids(),
            remote::RemoteState::default_data_dir(),
            remote::oauth::AuthConfig::default(),
        ));
        match remote::start(Arc::clone(&state), remote::DESKTOP_PORT) {
            Ok(handle) => {
                app.attach_desktop(state, handle.port);
                Some(handle)
            }
            Err(err) => {
                app.log(format!("Desktop MCP local indisponivel: {err}"));
                None
            }
        }
    };
    #[cfg(windows)]
    let tray = match tray::Tray::start(app.sender.clone()) {
        Ok(icon) => {
            app.tray_available = true;
            Some(icon)
        }
        Err(err) => {
            app.log(format!("Bandeja do Windows indisponivel: {err}"));
            None
        }
    };
    app.start_background_checks();
    #[cfg(windows)]
    let mut notified_update: Option<String> = None;
    let mut terminal = ratatui::init();
    let result = (|| -> Result<(), Box<dyn Error>> {
        loop {
            app.refresh();
            #[cfg(windows)]
            if let (Some(icon), Some(tag)) = (&tray, &app.update) {
                if notified_update.as_deref() != Some(tag.as_str()) {
                    icon.notify_update(tag);
                    notified_update = Some(tag.clone());
                }
            }
            if app.quit_requested {
                break;
            }
            terminal.draw(|frame| ui::draw(frame, &app))?;
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
    app.stop_desktop_tunnel();
    #[cfg(windows)]
    drop(tray);
    #[cfg(windows)]
    if let Some(handle) = desktop_server {
        handle.stop();
    }
    result
}

/// Exit code: 0 healthy, 1 unavailable or paused, 2 reachable without authentication.
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
    println!(
        "Proteção OAuth: {}",
        setup::describe_protection(probe.protection)
    );
    if probe.paused {
        println!("Publicação: PAUSADA");
    }
    if let Some(until) = probe.expose_until {
        println!("Exposição até: {}", clock::format_utc(until));
    }
    println!("HTTP 200 só comprova rede; teste as ferramentas e permissões no chat.");
    if matches!(probe.protection, Some(Protection::Open(_))) {
        println!(
            "ALERTA: o endpoint MCP respondeu sem autenticação. Não o use até corrigir o OAuth."
        );
        return 2;
    }
    i32::from(!probe.healthy())
}

/// Checks the actual localhost HTTP server without opening access to any app.
#[cfg(windows)]
fn desktop_doctor() -> i32 {
    use std::{fs, time::Duration};
    let data_dir = std::env::temp_dir().join(format!(
        "devspace-desktop-doctor-{}-{}",
        std::process::id(),
        clock::now_nanos()
    ));
    let state = Arc::new(remote::RemoteState::new(
        Arc::new(desktop::windows_backend::WindowsDesktop::new()),
        desktop::windows_backend::protected_process_ids(),
        data_dir.clone(),
        remote::oauth::AuthConfig::default(),
    ));
    let handle = match remote::start(state, 0) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("Desktop: falha iniciando socket local: {err}");
            return 1;
        }
    };
    let root = format!("http://127.0.0.1:{}", handle.port);
    let outcome = (|| -> Result<(), String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|err| err.to_string())?;
        let health = client
            .get(format!("{root}/healthz"))
            .send()
            .map_err(|err| err.to_string())?;
        println!("Local /healthz: {}", health.status());
        if health.status() != 200 {
            return Err("healthz nao respondeu 200".into());
        }
        let metadata = client
            .get(format!("{root}/.well-known/oauth-authorization-server"))
            .send()
            .map_err(|err| err.to_string())?;
        println!("OAuth metadata: {}", metadata.status());
        if metadata.status() != 200 {
            return Err("OAuth metadata indisponivel".into());
        }
        let protected = client
            .post(format!("{root}/mcp"))
            .header("Content-Type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .send()
            .map_err(|err| err.to_string())?;
        println!("MCP sem token: {} (esperado 401)", protected.status());
        if protected.status() != 401 {
            return Err("FALHA: acesso sem token!".into());
        }
        let spoofed = client
            .get(format!("{root}/healthz"))
            .header("Host", "foreign.example")
            .send()
            .map_err(|err| err.to_string())?;
        println!("Host externo: {} (esperado 421)", spoofed.status());
        if spoofed.status() != 421 {
            return Err("FALHA: Host externo aceito!".into());
        }
        Ok(())
    })();
    handle.stop();
    let _ = fs::remove_dir_all(data_dir);
    match outcome {
        Ok(()) => {
            println!("Desktop OAuth/MCP local: OK; nenhuma janela liberada.");
            0
        }
        Err(err) => {
            eprintln!("Desktop: {err}");
            1
        }
    }
}

fn repo_root(path: Option<&str>) -> PathBuf {
    path.map_or_else(|| Settings::load().workspace, PathBuf::from)
}

fn checkpoint_command(path: Option<&str>) -> i32 {
    match checkpoint::snapshot_all(&repo_root(path), &mut |line| println!("{line}")) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn checkpoints_command(path: Option<&str>) -> i32 {
    let repo = repo_root(path);
    match checkpoint::list(&repo) {
        Ok(items) if items.is_empty() => {
            println!("Nenhum checkpoint em {}", repo.display());
            0
        }
        Ok(items) => {
            for item in items {
                println!(
                    "{}  {}  {}",
                    item.name,
                    item.short_id,
                    clock::format_utc(item.created)
                );
            }
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn restore_command(args: &[String]) -> i32 {
    let (Some(repo), Some(name)) = (args.first(), args.get(1)) else {
        eprintln!("Uso: DevSpacePCBridge.exe --restore <repositório> <checkpoint> --yes");
        return 1;
    };
    if !args.iter().any(|arg| arg == "--yes") {
        eprintln!(
            "Isto sobrescreve arquivos de {repo} com o checkpoint {name}. O estado atual é salvo antes.\nRepita com --yes para confirmar."
        );
        return 1;
    }
    match checkpoint::restore(&PathBuf::from(repo), name) {
        Ok(()) => {
            println!("Restaurado. O estado anterior ficou salvo como novo checkpoint.");
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("DevSpace PC Bridge v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--doctor") => std::process::exit(doctor()),
        #[cfg(windows)]
        Some("--apply-update") => std::process::exit(update::apply_update(&args[1..])),
        #[cfg(windows)]
        Some("--desktop-doctor") => std::process::exit(desktop_doctor()),
        #[cfg(windows)]
        Some("--tray-smoke") => {
            let (tx, _rx) = std::sync::mpsc::channel();
            let icon = tray::Tray::start(tx).map_err(|err| format!("Bandeja: {err}"))?;
            println!("Icone da bandeja criado; teste local OK.");
            std::thread::sleep(Duration::from_millis(250));
            drop(icon);
            Ok(())
        }
        Some("--checkpoint") => {
            std::process::exit(checkpoint_command(args.get(1).map(String::as_str)))
        }
        Some("--checkpoints") => {
            std::process::exit(checkpoints_command(args.get(1).map(String::as_str)))
        }
        Some("--restore") => std::process::exit(restore_command(&args[1..])),
        Some(arg) => Err(format!("Argumento não reconhecido: {arg}").into()),
        None => run(),
    }
}
