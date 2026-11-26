use crate::{
    app::{history_bar, uptime_percent, App, Edit, Page},
    checks, clock,
    navigation::{self, Action, DESKTOP_STEPS, SETUP_STEPS},
    setup::{self, Probe, Protection},
    storage,
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Clear, Gauge, List, ListItem, ListState, Padding, Paragraph, Wrap,
    },
    Frame,
};

const BG: Color = Color::Rgb(15, 23, 35);
const PANEL: Color = Color::Rgb(22, 32, 48);
const BORDER: Color = Color::Rgb(51, 65, 85);
const TEXT: Color = Color::Rgb(226, 232, 240);
const MUTED: Color = Color::Rgb(148, 163, 184);
const ACCENT: Color = Color::Rgb(96, 165, 250);
const GREEN: Color = Color::Rgb(74, 222, 128);
const AMBER: Color = Color::Rgb(251, 191, 36);
const RED: Color = Color::Rgb(248, 113, 113);
const ACTIVE: Color = Color::Rgb(30, 58, 100);

fn panel(title: &str) -> Block<'_> {
    Block::bordered()
        .title(format!(" {title} "))
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().fg(TEXT).bg(PANEL))
        .padding(Padding::horizontal(1))
}
fn heading(s: impl Into<String>) -> Line<'static> {
    Line::styled(
        s.into(),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )
}
fn dim(s: impl Into<String>) -> Line<'static> {
    Line::styled(s.into(), Style::default().fg(MUTED))
}
fn plain(s: impl Into<String>) -> Line<'static> {
    Line::raw(s.into())
}
fn marked(ok: bool, label: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            if ok { "✓  " } else { "○  " },
            Style::default().fg(if ok { GREEN } else { MUTED }),
        ),
        Span::raw(label.into()),
    ])
}
pub(crate) fn status_badge(status: Option<&Probe>) -> &'static str {
    match status {
        None => "Verificando",
        Some(p) if p.open_without_auth() => "ALERTA: SEM OAUTH",
        Some(p) if p.paused && p.public == Some(200) => "PAUSA NÃO EFETIVA: AINDA PÚBLICO",
        Some(p) if p.paused => "Pausado",
        Some(p) if p.healthy() && matches!(p.protection, Some(Protection::Protected(_))) => {
            "Online · OAuth ativo"
        }
        Some(p) if p.healthy() => "Online · OAuth não confirmado",
        Some(p) if !p.configured => "Não configurado",
        Some(_) => "Offline",
    }
}
fn status_color(status: Option<&Probe>) -> Color {
    match status {
        Some(p) if p.open_without_auth() || (p.paused && p.public == Some(200)) => RED,
        Some(p)
            if p.healthy()
                && !p.paused
                && matches!(p.protection, Some(Protection::Protected(_))) =>
        {
            GREEN
        }
        _ => AMBER,
    }
}
fn exposure_line(probe: &Probe, now: u64) -> String {
    if probe.paused {
        return "Publicação: pausada".into();
    }
    match probe.expose_until {
        Some(until) if until > now => format!(
            "Publicação: mais {} min (até {})",
            (until - now).div_ceil(60),
            clock::format_utc(until)
        ),
        Some(_) => "Publicação: prazo vencido".into(),
        None if probe.configured => "Publicação: sem prazo configurado".into(),
        None => "Publicação: não configurada".into(),
    }
}
fn nav_name(page: Page) -> &'static str {
    page.title()
}
fn title(app: &App) -> &'static str {
    match app.page {
        Page::Home => "Visão geral",
        Page::Install => "Configuração guiada",
        Page::Connect => "Conectar meu chat",
        Page::Desktop => "Controle de janelas",
        Page::Security => "Segurança e diagnóstico",
        Page::Logs => "Atividade",
        Page::Help => "Como funciona",
    }
}
fn subtitle(app: &App) -> String {
    match app.page {
        Page::Home => "Seu computador, nos chats que você já usa.".into(),
        Page::Install if app.setup_step < 4 => format!(
            "Etapa {} de 4  ·  {}",
            app.setup_step + 1,
            SETUP_STEPS[app.setup_step]
        ),
        Page::Install => "Manutenção avançada".into(),
        Page::Connect => "Use o conector MCP com OAuth no ChatGPT ou Claude.".into(),
        Page::Desktop => format!(
            "{}  ·  {} de 4",
            DESKTOP_STEPS[app.desktop_step.min(3)],
            app.desktop_step + 1
        ),
        Page::Security => "Controle o acesso e verifique a proteção antes de usar.".into(),
        Page::Logs => "Acompanhe a operação sem abrir outro terminal.".into(),
        Page::Help => "Navegue com as setas, confirme com Enter.".into(),
    }
}
fn step_line(index: usize, current: usize, label: &str, done: bool) -> Line<'static> {
    let (symbol, color) = if index == current {
        ("›", ACCENT)
    } else if done {
        ("✓", GREEN)
    } else {
        ("○", MUTED)
    };
    Line::from(vec![
        Span::styled(
            format!(" {symbol}  {}. ", index + 1),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            label.to_owned(),
            Style::default().fg(if index == current { TEXT } else { MUTED }),
        ),
    ])
}
fn context(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match app.page {
        Page::Home => {
            lines.push(heading("CONEXÃO"));
            lines.push(Line::styled(
                status_badge(app.status.as_ref()),
                Style::default()
                    .fg(status_color(app.status.as_ref()))
                    .add_modifier(Modifier::BOLD),
            ));
            if let Some(p) = &app.status {
                lines.push(dim(format!(
                    "Local: {}  ·  HTTPS: {}",
                    setup::describe(p.local),
                    setup::describe(p.public)
                )));
                if p.open_without_auth() || (p.paused && p.public == Some(200)) {
                    lines.push(Line::styled(
                        "Acesso público inseguro. Pause e revise o OAuth.",
                        Style::default().fg(RED),
                    ));
                }
            }
            if let Some(tag) = &app.update {
                lines.push(Line::styled(
                    format!("Atualização disponível: {tag}"),
                    Style::default().fg(AMBER),
                ));
            }
            lines.push(plain(""));
            lines.push(heading("SEU PROGRESSO"));
            let p = app.status.as_ref();
            let done = [
                navigation::core_ready(app),
                navigation::workspace_ready(app),
                p.is_some_and(|p| {
                    p.healthy()
                        && !p.paused
                        && matches!(p.protection, Some(Protection::Protected(_)))
                }),
                false,
            ];
            for (i, name) in SETUP_STEPS[..4].iter().enumerate() {
                lines.push(step_line(i, navigation::next_step(app), name, done[i]));
            }
            lines.push(dim("A autorização do chat é concluída no navegador."));
        }
        Page::Install => {
            lines.push(heading("CONFIGURAÇÃO"));
            for (i, name) in SETUP_STEPS[..4].iter().enumerate() {
                lines.push(step_line(i, app.setup_step, name, false));
            }
            lines.push(plain(""));
            match app.setup_step {
                0 => {
                    lines.push(heading("Componentes detectados"));
                    for check in &app.prereqs {
                        lines.push(marked(check.ok, check.label));
                    }
                    lines.push(dim("Cloudflare é opcional para controle de janelas."));
                    lines.push(dim("Componentes ausentes podem exigir UAC."));
                }
                1 => {
                    lines.push(heading("Pasta autorizada"));
                    lines.push(plain(app.config.workspace.display().to_string()));
                    lines.push(marked(
                        navigation::workspace_ready(app),
                        if navigation::workspace_ready(app) {
                            "Pasta válida e restrita"
                        } else {
                            "Escolha uma pasta existente e segura"
                        },
                    ));
                    lines.push(dim(
                        "Somente projetos; nunca o disco inteiro ou credenciais.",
                    ));
                }
                2 => {
                    lines.push(heading("Servidor e exposição"));
                    lines.push(Line::styled(
                        status_badge(app.status.as_ref()),
                        Style::default().fg(status_color(app.status.as_ref())),
                    ));
                    lines.push(plain(
                        "O instalador configura DevSpace, Node, Git e Tailscale.",
                    ));
                    lines.push(Line::styled(
                        "A publicação HTTPS exige confirmação local e OAuth.",
                        Style::default().fg(AMBER),
                    ));
                }
                3 => {
                    lines.push(heading("Conexão ao chat"));
                    lines.push(plain(
                        "Abra o conector personalizado no navegador e autorize com OAuth.",
                    ));
                    lines.push(dim(
                        "O bridge não consegue confirmar automaticamente a autorização no chat.",
                    ));
                }
                _ => {
                    lines.push(heading("Manutenção"));
                    lines.push(dim("Diagnóstico e atualização não removem seus projetos."));
                    lines.push(dim("A remoção da tarefa exige confirmação explícita."));
                }
            }
        }
        Page::Connect => {
            lines.push(heading("CHAT SELECIONADO"));
            lines.push(plain(app.config.client.label()));
            lines.push(plain(""));
            lines.push(heading("ENDEREÇO MCP"));
            lines.push(plain(storage::mcp_url().unwrap_or_else(|| {
                "Configure o servidor antes de conectar.".into()
            })));
            lines.push(plain(""));
            lines.push(heading("NO NAVEGADOR"));
            for instruction in app.config.client.instructions() {
                lines.push(plain(*instruction));
            }
        }
        Page::Desktop => {
            lines.push(heading("ACESSO A JANELAS"));
            if let Some(remote) = &app.desktop_remote {
                let now = clock::now_secs();
                let policy = remote.policy();
                let remaining = policy.remaining_secs(now);
                let capture = policy.config().screenshots;
                let input = policy.config().input;
                let apps = policy.config().allowed_apps.clone();
                drop(policy);
                lines.push(Line::styled(
                    remaining.map_or_else(
                        || "Controle desligado".into(),
                        |s| format!("Sessão ativa · {} min", s.div_ceil(60)),
                    ),
                    Style::default().fg(if remaining.is_some() { AMBER } else { GREEN }),
                ));
                lines.push(plain(""));
                match app.desktop_step {
                    0 => {
                        lines.push(heading("Endereço e pareamento"));
                        lines.push(plain(
                            app.desktop_public_url
                                .as_deref()
                                .unwrap_or("HTTPS não publicado"),
                        ));
                        lines.push(dim("Local: http://127.0.0.1:7677/mcp"));
                        let clients = remote.auth.connected_clients(now);
                        lines.push(plain(format!(
                            "Clientes OAuth: {}",
                            if clients.is_empty() {
                                "nenhum".into()
                            } else {
                                clients.join(", ")
                            }
                        )));
                        if let Some(pin) = remote.auth.pairing(now) {
                            lines.push(Line::styled(
                                format!("PIN LOCAL: {} · {} s", pin.pin, pin.remaining_secs),
                                Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
                            ));
                            lines.push(dim(
                                "Digite somente na página de autorização. Não compartilhe no chat.",
                            ));
                        }
                    }
                    1 => {
                        lines.push(heading("Aplicativos autorizados"));
                        if apps.is_empty() {
                            lines.push(dim("Nenhum aplicativo autorizado."));
                        }
                        for app in &apps {
                            lines.push(marked(true, app.clone()));
                        }
                    }
                    2 => {
                        lines.push(heading("Sessão temporária"));
                        lines.push(plain(format!("{} aplicativo(s) autorizado(s)", apps.len())));
                        lines.push(dim("A sessão começa desligada e expira automaticamente."));
                        lines.push(dim(
                            "Você pode desativá-la ou revogar OAuth a qualquer momento.",
                        ));
                    }
                    _ => {
                        lines.push(marked(
                            capture,
                            format!(
                                "Captura de tela: {}",
                                if capture { "permitida" } else { "bloqueada" }
                            ),
                        ));
                        lines.push(marked(
                            input,
                            format!(
                                "Teclado e cliques: {}",
                                if input { "permitidos" } else { "bloqueados" }
                            ),
                        ));
                        lines.push(dim("Eventos direcionados não movem o mouse físico. Alguns apps podem ignorá-los."));
                    }
                }
            } else {
                lines.push(dim("Desktop MCP indisponível neste processo."));
            }
        }
        Page::Security => {
            lines.push(heading("PROTEÇÃO"));
            lines.push(Line::styled(
                status_badge(app.status.as_ref()),
                Style::default().fg(status_color(app.status.as_ref())),
            ));
            if let Some(p) = &app.status {
                lines.push(plain(exposure_line(p, clock::now_secs())));
                lines.push(plain(format!(
                    "OAuth: {}",
                    setup::describe_protection(p.protection)
                )));
                lines.push(dim(checks::render(&checks::runtime_checks(p))));
            }
            if let Some(p) = uptime_percent(&app.history) {
                lines.push(dim(format!(
                    "Disponibilidade: {p}% · {}",
                    history_bar(&app.history, 20)
                )));
            }
            lines.push(plain(""));
            lines.push(heading("BOAS PRÁTICAS"));
            lines.push(plain("Restrinja as pastas, autorize somente clientes confiáveis e revise escritas no chat."));
            lines.push(plain("Nunca compartilhe auth.json, tokens ou credenciais."));
            lines.push(dim("Restaurar checkpoint: --restore <repo> <nome> --yes"));
        }
        Page::Logs => {}
        Page::Help => {
            lines.push(heading("O que o Bridge faz"));
            lines.push(plain(
                "Conecta seus chats habituais a ferramentas MCP do computador.",
            ));
            lines.push(plain(
                "Não executa modelos de IA e não precisa de Codex CLI, Claude Code ou chaves API.",
            ));
            lines.push(plain(""));
            lines.push(heading("Navegação"));
            lines.push(plain("← → ou Tab: trocar seção"));
            lines.push(plain("↑ ↓: escolher ação · Enter: confirmar seleção"));
            lines.push(plain("Esc: início / cancelar diálogo"));
            lines.push(plain(
                "Ctrl+Q: sair · Ctrl+X: cancelar tarefa quando possível",
            ));
            lines.push(dim(
                "Acesso às ferramentas depende das permissões e da conta de cada chat.",
            ));
        }
    }
    lines
}
fn draw_actions(frame: &mut Frame, app: &App, area: Rect, actions: &[Action]) {
    let index = app.selection.min(actions.len().saturating_sub(1));
    let mut selected = ListState::default().with_selected(Some(index));
    let items = actions.iter().map(|a| ListItem::new(a.label.clone()));
    frame.render_stateful_widget(
        List::new(items)
            .block(panel("Ações"))
            .highlight_style(
                Style::default()
                    .fg(TEXT)
                    .bg(ACTIVE)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› "),
        area,
        &mut selected,
    );
}
fn draw_logs(frame: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(4), Constraint::Min(2)]).split(area);
    let state = if app.processing {
        "Em execução"
    } else {
        match app.task_result {
            Some(true) => "Concluída",
            Some(false) => "Falhou",
            None => "Nenhuma tarefa",
        }
    };
    let color = if app.processing {
        ACCENT
    } else if app.task_result == Some(false) {
        RED
    } else {
        GREEN
    };
    let name = if app.task_title.is_empty() {
        "Atividade do bridge"
    } else {
        &app.task_title
    };
    if let Some(percent) = app.task_progress {
        frame.render_widget(
            Gauge::default()
                .block(panel(name))
                .gauge_style(Style::default().fg(color).bg(PANEL))
                .percent(percent.min(100))
                .label(format!("{state} · {percent}%")),
            rows[0],
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::styled(state, Style::default().fg(color))).block(panel(name)),
            rows[0],
        );
    }
    let block = panel(if app.logs_offset == u16::MAX {
        "Saída ao vivo · PgUp/PgDn"
    } else {
        "Histórico · End para acompanhar"
    });
    let inner = block.inner(rows[1]);
    let width = inner.width.max(1) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for message in &app.logs {
        for line in message.lines() {
            let clean: Vec<char> = line.chars().filter(|c| !c.is_control()).collect();
            let color = if line.starts_with('!') || line.starts_with("Falha") {
                RED
            } else if line.starts_with('[') {
                ACCENT
            } else {
                TEXT
            };
            if clean.is_empty() {
                lines.push(plain(""));
            }
            for chunk in clean.chunks(width) {
                lines.push(Line::styled(
                    chunk.iter().collect::<String>(),
                    Style::default().fg(color),
                ));
            }
        }
    }
    if lines.is_empty() {
        lines.push(dim("Nenhuma operação iniciada."));
    }
    let height = inner.height as usize;
    let back = if app.logs_offset == u16::MAX {
        0
    } else {
        usize::from(app.logs_offset)
    };
    let start = lines.len().saturating_sub(height).saturating_sub(back);
    frame.render_widget(block, rows[1]);
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(start)
                .take(height)
                .collect::<Vec<_>>(),
        ),
        inner,
    );
}
pub(crate) fn draw(frame: &mut Frame, app: &App) {
    let full = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        full,
    );
    if full.width < 32 || full.height < 12 {
        frame.render_widget(
            Paragraph::new("Aumente a janela (mínimo 32 × 12).\nCtrl+Q para sair.")
                .wrap(Wrap { trim: true }),
            full,
        );
        return;
    }
    let compact = full.width < 82;
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .split(full);
    let header = Line::from(vec![
        Span::styled(
            "  DEVSPACE",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  PC BRIDGE",
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("   v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(MUTED),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header).style(Style::default().bg(PANEL)),
        rows[0],
    );
    let body = if compact {
        rows[1]
    } else {
        let cols = Layout::horizontal([Constraint::Length(20), Constraint::Min(40)]).split(rows[1]);
        let items = Page::ALL.iter().map(|p| {
            ListItem::new(format!("  {}", nav_name(*p)))
                .style(Style::default().fg(if *p == app.page { TEXT } else { MUTED }))
        });
        let mut nav =
            ListState::default().with_selected(Page::ALL.iter().position(|p| *p == app.page));
        frame.render_stateful_widget(
            List::new(items)
                .block(panel("Menu"))
                .highlight_style(
                    Style::default()
                        .bg(ACTIVE)
                        .fg(TEXT)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("›"),
            cols[0],
            &mut nav,
        );
        cols[1]
    };
    let content = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(if body.height > 12 { 2 } else { 1 }),
    ])
    .split(body);
    frame.render_widget(
        Paragraph::new(vec![heading(title(app)), dim(subtitle(app))]),
        content[0],
    );
    let actions = navigation::actions(app);
    let action_area = if compact {
        let list_height =
            (actions.len() as u16 + 2).min(content[1].height.saturating_sub(4).max(3));
        let split = Layout::vertical([Constraint::Length(list_height), Constraint::Min(0)])
            .split(content[1]);
        if app.page == Page::Logs {
            draw_logs(frame, app, split[1]);
        } else {
            frame.render_widget(
                Paragraph::new(context(app))
                    .block(panel("Informações"))
                    .wrap(Wrap { trim: false }),
                split[1],
            );
        }
        split[0]
    } else {
        let split = Layout::horizontal([Constraint::Percentage(43), Constraint::Percentage(57)])
            .split(content[1]);
        if app.page == Page::Logs {
            draw_logs(frame, app, split[1]);
        } else {
            frame.render_widget(
                Paragraph::new(context(app))
                    .block(panel("Informações"))
                    .wrap(Wrap { trim: false }),
                split[1],
            );
        }
        split[0]
    };
    draw_actions(frame, app, action_area, &actions);
    if let Some(action) = actions.get(app.selection.min(actions.len().saturating_sub(1))) {
        frame.render_widget(
            Paragraph::new(action.detail)
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: true }),
            content[2],
        );
    }
    let nav = if compact {
        format!(
            "← → Seções · {} · ↑ ↓ Ações · Enter · Esc",
            nav_name(app.page)
        )
    } else {
        "← → / Tab Seções   ↑ ↓ Selecionar   Enter Abrir   Esc Início".into()
    };
    let state = if app.processing {
        "EM EXECUÇÃO"
    } else {
        status_badge(app.status.as_ref())
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(nav, Style::default().fg(MUTED)),
            Line::from(vec![
                Span::styled(
                    format!(" {state} "),
                    Style::default()
                        .fg(if app.processing {
                            ACCENT
                        } else {
                            status_color(app.status.as_ref())
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" · "),
                Span::raw(app.feedback.as_str()),
            ]),
            dim(if app.processing {
                "Ctrl+X Cancelar tarefa (quando disponível) · Ctrl+L Atividade"
            } else {
                "F2 Detalhes · F1 Ajuda · Ctrl+Q Sair"
            }),
        ])
        .style(Style::default().bg(PANEL)),
        rows[2],
    );
    if app.info_open && app.page != Page::Logs && app.edit == Edit::None {
        let popup = Rect::new(
            full.x + 1,
            full.y + 1,
            full.width.saturating_sub(2),
            full.height.saturating_sub(2),
        );
        frame.render_widget(Clear, popup);
        let block = panel("Detalhes · ↑ ↓ / PgUp PgDn · Esc voltar")
            .border_style(Style::default().fg(ACCENT));
        let inner = block.inner(popup);
        let lines = context(app);
        let total: usize = lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(inner.width.max(1) as usize))
            .sum();
        let max = total
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16;
        frame.render_widget(
            Paragraph::new(lines)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((app.info_offset.min(max), 0)),
            popup,
        );
    }
    if app.edit != Edit::None {
        draw_prompt(frame, app, full);
    }
}
fn draw_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let (title, detail, hint) = match app.edit {
        Edit::Workspace => ("Pasta de projetos", "Escolha uma pasta existente e restrita aos seus projetos.", "Caminho da pasta"),
        Edit::Publish => ("Publicar servidor", "O Tailscale Funnel expõe uma URL HTTPS. O OAuth deve proteger o acesso.", "Digite PUBLICAR"),
        Edit::Resume => ("Retomar publicação", "Use PUBLICAR para manter sem prazo ou PUBLICAR 60 para pausar em 60 minutos (1–1440).", "Digite PUBLICAR ou PUBLICAR <minutos>"),
        Edit::Uninstall => ("Remover tarefa", "Preserva DevSpace, Node, Git e Tailscale. REMOVER FUNNEL também desativa o Funnel e limpa Serve.", "Digite REMOVER ou REMOVER FUNNEL"),
        Edit::DesktopApp => ("Autorizar aplicativo", "Informe o nome exato do executável, por exemplo notepad.exe.", "Nome do executável"),
        Edit::DesktopRemoveApp => ("Retirar aplicativo", "Informe o nome exato do executável a remover.", "Nome do executável"),
        Edit::DesktopEnable => ("Ativar sessão", "Use ATIVAR ou ATIVAR 30 (1–240 minutos). Nunca libera apps automaticamente.", "Digite ATIVAR ou ATIVAR <minutos>"),
        Edit::DesktopPublish => ("Publicar Desktop MCP", "Cloudflare cria uma URL temporária. Pareamento PIN e ativação local continuam necessários.", "Digite PUBLICAR DESKTOP"),
        Edit::InstallComponent(_) => ("Instalar componente", "Instala apenas o componente ausente, usando WinGet ou npm oficial. Pode exigir UAC.", "Confirme o componente"),
        Edit::Update => ("Atualizar aplicativo", "Baixa o EXE oficial e confere SHA-256. Preserva backup e reinicia o bridge.", "Digite ATUALIZAR"),
        Edit::None => return,
    };
    let width = area.width.saturating_sub(4).min(76);
    let height = area.height.saturating_sub(2).min(14);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = panel(title).border_style(Style::default().fg(AMBER));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let sections = Layout::vertical([
        Constraint::Min(2),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(inner);
    let mut intro = vec![plain(detail)];
    match app.edit {
        Edit::InstallComponent(c) => intro.push(Line::styled(
            format!("Digite INSTALAR {}", c.name().to_ascii_uppercase()),
            Style::default().fg(AMBER),
        )),
        Edit::Update => {
            intro.push(dim(format!(
                "{} → {}",
                env!("CARGO_PKG_VERSION"),
                app.update.as_deref().unwrap_or("indisponível")
            )));
            intro.push(Line::styled(hint, Style::default().fg(AMBER)));
        }
        _ => intro.push(Line::styled(hint, Style::default().fg(AMBER))),
    }
    frame.render_widget(Paragraph::new(intro).wrap(Wrap { trim: true }), sections[0]);
    let max_chars = sections[1].width.saturating_sub(4) as usize;
    let chars: Vec<char> = app.input.chars().collect();
    let value: String = chars[chars.len().saturating_sub(max_chars)..]
        .iter()
        .collect();
    frame.render_widget(
        Paragraph::new(format!("{value}▏"))
            .block(panel("Confirmação").border_style(Style::default().fg(ACCENT))),
        sections[1],
    );
    frame.render_widget(
        Paragraph::new("Enter confirmar  ·  Esc cancelar").style(Style::default().fg(MUTED)),
        sections[2],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};
    fn up() -> Probe {
        Probe {
            configured: true,
            local: Some(200),
            public: Some(200),
            protection: Some(Protection::Protected(401)),
            ..Probe::default()
        }
    }
    #[test]
    fn open_endpoint_is_never_green() {
        let open = Probe {
            protection: Some(Protection::Open(200)),
            paused: true,
            ..up()
        };
        assert_eq!(status_badge(Some(&open)), "ALERTA: SEM OAUTH");
        assert_eq!(status_color(Some(&open)), RED);
        let paused = Probe {
            paused: true,
            ..up()
        };
        assert!(status_badge(Some(&paused)).contains("AINDA PÚBLICO"));
    }
    #[test]
    fn unknown_oauth_is_not_protected() {
        let unknown = Probe {
            protection: None,
            ..up()
        };
        assert_ne!(status_color(Some(&unknown)), GREEN);
        assert_eq!(status_color(Some(&up())), GREEN);
    }
    #[test]
    fn expiration_is_reported() {
        let timed = Probe {
            expose_until: Some(1090),
            ..up()
        };
        assert!(exposure_line(&timed, 1000).contains("mais 2 min"));
        assert!(exposure_line(&timed, 1200).contains("vencido"));
    }
    #[test]
    fn all_sections_render_at_multiple_terminal_sizes() {
        let mut app = App::new();
        app.status = Some(up());
        app.update = Some("v0.8.0".into());
        for (width, height) in [(120, 35), (82, 24), (60, 20), (32, 12), (20, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for page in Page::ALL {
                app.navigate(page);
                for step in 0..5 {
                    app.setup_step = step;
                    app.desktop_step = step.min(3);
                    terminal.draw(|f| draw(f, &app)).unwrap();
                }
            }
            app.begin_edit(Edit::Update);
            terminal.draw(|f| draw(f, &app)).unwrap();
            app.edit = Edit::None;
        }
    }
    #[test]
    fn current_selection_has_real_action() {
        let mut app = App::new();
        for page in Page::ALL {
            app.navigate(page);
            for n in 0..5 {
                app.setup_step = n;
                app.desktop_step = n.min(3);
                assert!(!navigation::actions(&app).is_empty());
            }
        }
    }
}
