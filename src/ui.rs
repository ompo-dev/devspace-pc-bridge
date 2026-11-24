use crate::{
    app::{history_bar, uptime_percent, App, Edit, Page},
    checks, clock, setup,
    setup::Probe,
    storage::{self, ChatClient},
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

const BAR_WIDTH: usize = 40;

pub(crate) fn status_badge(status: Option<&Probe>) -> &'static str {
    let Some(probe) = status else {
        return "verificando...";
    };
    if probe.open_without_auth() {
        "ALERTA: SEM OAUTH"
    } else if probe.paused && probe.public == Some(200) {
        "PAUSA NÃO EFETIVA: AINDA PÚBLICO"
    } else if probe.paused {
        "PAUSADO"
    } else if probe.healthy() {
        "ONLINE"
    } else if !probe.configured {
        "NÃO CONFIGURADO"
    } else {
        "OFFLINE"
    }
}

fn exposure_line(probe: &Probe, now: u64) -> String {
    match (probe.paused, probe.expose_until) {
        (true, _) => "Publicação: pausada (P pausa, R retoma)".into(),
        (false, Some(until)) if until > now => format!(
            "Publicação: ativa por mais {} min (até {})",
            (until - now).div_ceil(60),
            clock::format_utc(until)
        ),
        (false, Some(_)) => "Publicação: prazo vencido, será pausada em instantes".into(),
        (false, None) => "Publicação: ativa, sem prazo".into(),
    }
}

pub(crate) fn draw(frame: &mut Frame, app: &App) {
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

fn home_lines(app: &App) -> Vec<String> {
    let mut lines: Vec<String> = vec![
        " SEU COMPUTADOR, NOS SEUS CHATS NORMAIS".into(),
        "".into(),
        setup::status_summary(),
        "".into(),
    ];
    match &app.status {
        Some(probe) => {
            lines.push(format!(
                "Estado: {}  (local: {}  |  HTTPS: {})",
                status_badge(Some(probe)),
                setup::describe(probe.local),
                setup::describe(probe.public)
            ));
            lines.push(format!(
                "Proteção OAuth: {}",
                setup::describe_protection(probe.protection)
            ));
            lines.push(exposure_line(probe, clock::now_secs()));
            if probe.open_without_auth() {
                lines.push(
                    "ALERTA: o endpoint respondeu sem token. Pause com [P] e revise o OAuth do DevSpace."
                        .into(),
                );
            }
        }
        None => lines.push("Estado: verificando...".into()),
    }
    if let Some(percent) = uptime_percent(&app.history) {
        lines.push(format!(
            "Disponibilidade: {percent}% ({} verificações)  {}",
            app.history.len(),
            history_bar(&app.history, BAR_WIDTH)
        ));
    }
    lines.push("".into());
    lines.push(checks::render(&app.prereqs));
    if let Some(probe) = &app.status {
        lines.push(checks::render(&checks::runtime_checks(probe)));
    }
    if let Some(tag) = &app.update {
        lines.push("".into());
        lines.push(format!("Nova versão disponível: {tag} (veja Logs)"));
    }
    lines.extend([
        "".into(),
        format!("Pasta de projetos: {}", app.config.workspace.display()),
        "".into(),
        " [C] Copiar MCP   [D] Diagnóstico   [T] Testar HTTPS e OAuth".into(),
        " [P] Pausar publicação   [R] Retomar (PUBLICAR [minutos])".into(),
        " [K] Checkpoint git dos projetos (ponto de restauração)".into(),
        if app.tray_available {
            " [M] Ocultar terminal na bandeja do Windows".into()
        } else {
            " Bandeja do Windows indisponivel neste terminal".into()
        },
        "".into(),
        "Não é um chatbot nem executa modelos de IA.".into(),
    ]);
    lines
}

fn desktop_lines(app: &App) -> Vec<String> {
    let Some(remote) = &app.desktop_remote else {
        return vec!["Desktop MCP indisponivel neste processo.".into()];
    };
    let now = clock::now_secs();
    let policy = remote.policy();
    let remaining = policy.remaining_secs(now);
    let capture = policy.config().screenshots;
    let input = policy.config().input;
    let apps = policy.config().allowed_apps.clone();
    drop(policy);
    let connected = remote.auth.connected_clients(now);
    let mut lines = vec![
        " DESKTOP MCP (OAuth + PIN + lista de apps)".into(),
        format!(
            "Clientes OAuth: {}",
            if connected.is_empty() {
                "nenhum".into()
            } else {
                connected.join(", ")
            }
        ),
        "".into(),
        "Servidor local: http://127.0.0.1:7677/mcp".into(),
        format!(
            "HTTPS: {}",
            app.desktop_public_url.as_deref().unwrap_or("desligado")
        ),
        format!(
            "Controle: {}",
            remaining.map_or_else(
                || "DESLIGADO".into(),
                |secs| format!("ligado por mais {} min", secs.div_ceil(60))
            )
        ),
        format!(
            "Capturar/inspecionar: {}    Teclado/cliques: {}",
            if capture { "sim" } else { "nao" },
            if input { "sim" } else { "nao" }
        ),
        format!(
            "Apps autorizados: {}",
            if apps.is_empty() {
                "nenhum".into()
            } else {
                apps.join(", ")
            }
        ),
        "".into(),
    ];
    if let Some(pairing) = remote.auth.pairing(now) {
        lines.push(format!(
            "PIN LOCAL: {} (restam {} s)",
            pairing.pin, pairing.remaining_secs
        ));
        lines
            .push("Nao compartilhe o PIN no chat; digite-o na pagina de autorizacao OAuth.".into());
    }
    lines.extend([
        " [A] Liberar app (.exe)    [X] Retirar app".into(),
        " [E] Ativar por prazo     [B] Desligar agora    [R] Revogar OAuth".into(),
        " [V] Alternar capturas  [I] Alternar entrada   [C] Novo PIN".into(),
        " [P] Publicar Cloudflare [S] Parar Cloudflare".into(),
        "".into(),
        "O mouse fisico nao se move. Alguns aplicativos ignoram eventos direcionados.".into(),
        "O acesso inicia desligado em toda execucao e expira automaticamente.".into(),
    ]);
    lines
}

fn page_content(app: &App) -> (&'static str, Text<'static>) {
    let lines = match app.page {
        Page::Home => home_lines(app),
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
            " [T] Testar health local, HTTPS e OAuth".into(),
            " [U] Remover a tarefa do bridge (desinstalar)".into(),
            "".into(),
            "INSTALACAO INDIVIDUAL (progresso visivel em Logs)".into(),
            " [1] Git          [2] Node.js     [3] Tailscale".into(),
            " [4] Cloudflared  [5] DevSpace".into(),
            "Cada componente pede confirmacao e verifica se ja existe.".into(),
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
            "FERRAMENTAS DO BRIDGE (tela Início)".into(),
            "[T] chama /mcp sem token: 401 = OAuth ativo; 200 = ABERTO.".into(),
            "[P] pausa a publicação (só se o Funnel for só do bridge).".into(),
            "[R] retoma, com prazo opcional: PUBLICAR 60 = 60 minutos.".into(),
            "[K] guarda um checkpoint git antes de liberar escrita.".into(),
            "    Restaurar: DevSpacePCBridge.exe --restore <repo> <nome> --yes".into(),
            "".into(),
            "A publicação HTTPS implica risco: use com critério.".into(),
            "Para revogar: desconecte o app no chat e no DevSpace.".into(),
            "O diagnóstico [D] avisa se allowedRoots está amplo demais.".into(),
        ],
        Page::Desktop => desktop_lines(app),
        Page::Logs => {
            let mut v = vec![
                " SAÍDA AO VIVO (sem abrir outros terminais)".into(),
                " [Y] copiar logs sem hostname, usuário e tokens".into(),
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
            " Início: P pausar | R retomar | K checkpoint | T testar".into(),
            " Logs: Y copiar logs sem dados sensíveis".into(),
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
        Edit::Resume => (
            "RETOMAR PUBLICAÇÃO",
            format!(
                "O Funnel volta a expor a URL HTTPS na internet.\nDigite PUBLICAR para ficar ativo sem prazo,\nou PUBLICAR 60 para pausar sozinho em 60 minutos (1 a 1440):\n{}\n\nEnter confirmar | Esc cancelar",
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
        Edit::DesktopApp => (
            "Permitir aplicativo",
            format!("Digite o nome exato do executavel (ex.: notepad.exe):\n{}\n\nEnter confirma | Esc cancela", app.input),
        ),
        Edit::DesktopRemoveApp => (
            "Remover permissao do aplicativo",
            format!("Nome do executavel para remover:\n{}\n\nEnter confirma | Esc cancela", app.input),
        ),
        Edit::DesktopEnable => (
            "Ativar desktop por tempo limitado",
            format!("Digite ATIVAR ou ATIVAR 30 (1 a 240 minutos).\nNunca libera apps automaticamente.\n{}\n\nEnter confirma | Esc cancela", app.input),
        ),
        Edit::DesktopPublish => (
            "Publicar controle remoto com OAuth",
            format!("Cloudflare Quick Tunnel publica uma URL HTTPS temporaria.\nSera preciso pareamento PIN e ativacao local.\nDigite PUBLICAR DESKTOP para continuar:\n{}\n\nEnter confirma | Esc cancela", app.input),
        ),
        Edit::InstallComponent(component) => (
            "Confirmar instalacao individual",
            format!(
                "Instalar apenas {} se estiver faltando.\nUsa WinGet ou npm oficial. Pode solicitar UAC.\nDigite INSTALAR {}:\n{}\n\nEnter confirma | Esc cancela",
                component.name(), component.name().to_ascii_uppercase(), app.input
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::Protection;

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
    fn status_badge_reflects_probe() {
        assert_eq!(status_badge(None), "verificando...");
        assert_eq!(status_badge(Some(&up())), "ONLINE");
        assert_eq!(status_badge(Some(&Probe::default())), "NÃO CONFIGURADO");
        let down = Probe {
            local: None,
            public: None,
            protection: None,
            ..up()
        };
        assert_eq!(status_badge(Some(&down)), "OFFLINE");
    }
    #[test]
    fn an_open_endpoint_outranks_every_other_state() {
        let open = Probe {
            protection: Some(Protection::Open(200)),
            ..up()
        };
        assert_eq!(status_badge(Some(&open)), "ALERTA: SEM OAUTH");
        let open_and_paused = Probe {
            paused: true,
            ..open
        };
        assert_eq!(status_badge(Some(&open_and_paused)), "ALERTA: SEM OAUTH");
    }
    #[test]
    fn a_pause_that_did_not_take_effect_is_called_out() {
        let paused_but_public = Probe {
            paused: true,
            ..up()
        };
        assert!(status_badge(Some(&paused_but_public)).contains("AINDA PÚBLICO"));
        let paused = Probe {
            paused: true,
            public: None,
            protection: None,
            ..up()
        };
        assert_eq!(status_badge(Some(&paused)), "PAUSADO");
    }
    #[test]
    fn exposure_line_counts_down() {
        let timed = Probe {
            expose_until: Some(1_000 + 90),
            ..up()
        };
        assert!(exposure_line(&timed, 1_000).contains("mais 2 min"));
        let expired = Probe {
            expose_until: Some(500),
            ..up()
        };
        assert!(exposure_line(&expired, 1_000).contains("vencido"));
        assert!(exposure_line(&up(), 1_000).contains("sem prazo"));
        let paused = Probe {
            paused: true,
            ..up()
        };
        assert!(exposure_line(&paused, 1_000).contains("pausada"));
    }
    #[test]
    fn home_shows_the_checklist_and_shortcuts() {
        let mut app = App::new();
        app.status = Some(up());
        let text = home_lines(&app).join("\n");
        assert!(text.contains("Proteção OAuth: ativa"));
        assert!(text.contains("✔ Servidor local"));
        assert!(text.contains("[P] Pausar"));
        assert!(text.contains("[K] Checkpoint"));
    }
}
