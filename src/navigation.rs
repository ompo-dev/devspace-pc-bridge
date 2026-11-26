//! Shared navigation model. The renderer and keyboard handler use the same actions.
use crate::{
    app::{App, Page},
    setup::Protection,
    storage,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ActionKind {
    Key(char),
    Page(Page),
    Setup(usize),
    Desktop(usize),
    Return,
}

pub(crate) struct Action {
    pub(crate) label: String,
    pub(crate) detail: &'static str,
    pub(crate) kind: ActionKind,
}

fn action(label: impl Into<String>, detail: &'static str, kind: ActionKind) -> Action {
    Action {
        label: label.into(),
        detail,
        kind,
    }
}
fn key(c: char, label: impl Into<String>, detail: &'static str) -> Action {
    action(label, detail, ActionKind::Key(c))
}
fn page(p: Page, label: &str, detail: &'static str) -> Action {
    action(label, detail, ActionKind::Page(p))
}
fn step(n: usize, label: &str, detail: &'static str) -> Action {
    action(label, detail, ActionKind::Setup(n))
}
fn desktop(n: usize, label: &str, detail: &'static str) -> Action {
    action(label, detail, ActionKind::Desktop(n))
}

pub(crate) const SETUP_STEPS: [&str; 5] = [
    "Dependências",
    "Pasta",
    "Servidor",
    "Conectar",
    "Manutenção",
];
pub(crate) const DESKTOP_STEPS: [&str; 4] = ["Conexão", "Aplicativos", "Sessão", "Permissões"];

pub(crate) fn core_ready(app: &App) -> bool {
    ["Git", "Node.js", "Tailscale"]
        .iter()
        .all(|name| app.prereqs.iter().any(|c| c.label == *name && c.ok))
}

pub(crate) fn workspace_ready(app: &App) -> bool {
    app.config.workspace.is_dir() && storage::root_risk(&app.config.workspace).is_none()
}

pub(crate) fn next_step(app: &App) -> usize {
    if !core_ready(app) {
        0
    } else if !workspace_ready(app) {
        1
    } else if !app.status.as_ref().is_some_and(|p| {
        p.configured
            && p.healthy()
            && !p.paused
            && matches!(p.protection, Some(Protection::Protected(_)))
    }) {
        2
    } else {
        3
    }
}

pub(crate) fn actions(app: &App) -> Vec<Action> {
    use Page::*;
    match app.page {
        Home => {
            let mut list = vec![
                step(
                    next_step(app),
                    "Continuar configuração",
                    "Assistente de configuração e verificação em etapas.",
                ),
                page(
                    Connect,
                    "Conectar meu chat",
                    "Escolher ChatGPT ou Claude e autorizar o conector.",
                ),
                page(
                    Desktop,
                    "Controlar janelas",
                    "Gerenciar acesso remoto a aplicativos específicos.",
                ),
                page(
                    Security,
                    "Segurança e diagnóstico",
                    "Controlar a exposição e verificar a proteção.",
                ),
                if let Some(tag) = &app.update {
                    key(
                        'u',
                        format!("Instalar atualização {tag}"),
                        "Baixar, verificar e instalar após sua confirmação.",
                    )
                } else {
                    key(
                        'n',
                        "Verificar atualizações",
                        "Consultar releases estáveis no GitHub.",
                    )
                },
            ];
            if app.tray_available {
                list.push(key(
                    'm',
                    "Minimizar para a bandeja",
                    "Manter o bridge em execução sem ocupar o terminal.",
                ));
            }
            if app
                .status
                .as_ref()
                .is_some_and(|p| p.open_without_auth() || (p.paused && p.public == Some(200)))
            {
                list.push(key(
                    'p',
                    "Pausar exposição agora",
                    "O endpoint ainda está acessível. Revise a configuração antes de prosseguir.",
                ));
            }
            list
        }
        Install => {
            match app.setup_step {
                0 => {
                    let mut list = vec![step(
                        1,
                        "Continuar para a pasta",
                        "Escolher somente a pasta de projetos que deseja autorizar.",
                    )];
                    for (k, name, label) in [
                        ('1', "Git", "Git"),
                        ('2', "Node.js", "Node.js"),
                        ('3', "Tailscale", "Tailscale"),
                        ('4', "Cloudflare", "Cloudflare (opcional)"),
                        ('5', "DevSpace", "DevSpace"),
                    ] {
                        let ok = app.prereqs.iter().any(|c| c.label == name && c.ok);
                        list.push(key(k, format!("{label} · {}", if ok { "instalado" } else { "instalar" }), "Verifica o componente e instala somente se necessário; exige confirmação."));
                    }
                    list.push(step(
                        4,
                        "Manutenção avançada",
                        "Diagnóstico, atualização e remoção do bridge.",
                    ));
                    list
                }
                1 => vec![
                    key(
                        'r',
                        "Escolher pasta de projetos",
                        "Uma pasta existente e restrita; não autorize todo o disco ou seu perfil.",
                    ),
                    step(
                        2,
                        "Continuar para o servidor",
                        "O instalador valida a pasta novamente.",
                    ),
                    step(
                        0,
                        "Voltar às dependências",
                        "Revisar os componentes instalados.",
                    ),
                ],
                2 => vec![
                    key(
                        'i',
                        "Configurar servidor e HTTPS",
                        "Instala DevSpace e Tailscale; exige confirmar a exposição pública.",
                    ),
                    key(
                        't',
                        "Testar conexão e OAuth",
                        "Testa o servidor local e público sem token.",
                    ),
                    step(
                        3,
                        "Continuar para a conexão",
                        "Autorizar seu chat após configurar o servidor.",
                    ),
                    step(1, "Voltar à pasta", "Revisar o diretório autorizado."),
                ],
                3 => vec![
                    page(
                        Connect,
                        "Abrir conexão do chat",
                        "Escolher seu chat e seguir as instruções de OAuth.",
                    ),
                    key(
                        't',
                        "Testar conexão",
                        "Verificar rede e proteção antes de usar.",
                    ),
                    step(2, "Voltar ao servidor", "Revisar configuração e HTTPS."),
                    step(
                        4,
                        "Manutenção avançada",
                        "Gerenciar a instalação e atualizações.",
                    ),
                ],
                _ => vec![
                    key(
                        'd',
                        "Diagnóstico completo",
                        "Somente leitura: verifica componentes e configuração.",
                    ),
                    key(
                        't',
                        "Testar HTTPS e OAuth",
                        "Verifica a conexão e o acesso sem token.",
                    ),
                    key('n', "Buscar atualização do app", "Consultar o GitHub."),
                    key(
                        'u',
                        "Remover tarefa do bridge",
                        "Não remove seus projetos nem credenciais; exige confirmação.",
                    ),
                    step(
                        0,
                        "Voltar ao assistente",
                        "Revisar dependências, pasta e servidor.",
                    ),
                ],
            }
        }
        Connect => vec![
            key('1', "Selecionar ChatGPT", "Usar o chat normal do ChatGPT."),
            key('2', "Selecionar Claude", "Usar o chat normal do Claude."),
            key(
                'c',
                "Copiar endereço MCP",
                "Copiar a URL HTTPS para adicionar no chat.",
            ),
            key(
                'o',
                "Abrir conectores no navegador",
                "Adicione o conector e confirme a autorização OAuth.",
            ),
            key(
                'p',
                "Abrir painel Owner",
                "Gerencie autorizações e clientes no DevSpace.",
            ),
            step(
                2,
                "Configurar servidor",
                "Voltar se ainda não houver um endereço válido.",
            ),
        ],
        Security => vec![
            key(
                'p',
                "Pausar publicação",
                "Interrompe a exposição do bridge quando o Funnel pode ser pausado com segurança.",
            ),
            key(
                'r',
                "Retomar com prazo opcional",
                "Exige confirmação PUBLICAR; você pode definir minutos.",
            ),
            key(
                't',
                "Testar proteção OAuth",
                "Uma resposta HTTP 200 sem token é um alerta de segurança.",
            ),
            key(
                'k',
                "Criar checkpoint Git",
                "Criar um ponto de restauração dos projetos.",
            ),
            key(
                'd',
                "Executar diagnóstico",
                "Verifica a configuração sem alterar arquivos.",
            ),
        ],
        Desktop => match app.desktop_step {
            0 => vec![
                key(
                    'p',
                    "Publicar endereço HTTPS",
                    "Cloudflare temporário; exige confirmar PUBLICAR DESKTOP.",
                ),
                key(
                    'c',
                    "Gerar PIN OAuth",
                    "Digite o PIN local apenas na página de autorização.",
                ),
                key(
                    's',
                    "Parar publicação",
                    "Fecha o túnel e desativa a sessão.",
                ),
                desktop(
                    1,
                    "Continuar: aplicativos",
                    "Definir os executáveis autorizados.",
                ),
                desktop(2, "Sessão de controle", "Ativar ou interromper a sessão."),
                desktop(3, "Permissões", "Definir captura e entrada."),
            ],
            1 => vec![
                key(
                    'a',
                    "Autorizar aplicativo",
                    "Informe o nome exato do executável .exe.",
                ),
                key(
                    'x',
                    "Retirar aplicativo",
                    "Remove o executável da lista permitida.",
                ),
                desktop(
                    2,
                    "Continuar: sessão",
                    "Ative o controle somente quando precisar.",
                ),
                desktop(0, "Voltar à conexão", "Revisar túnel e pareamento."),
            ],
            2 => vec![
                key(
                    'e',
                    "Ativar sessão temporária",
                    "Exige confirmação local, de 1 a 240 minutos.",
                ),
                key(
                    'b',
                    "Desligar controle agora",
                    "Encerra imediatamente a autorização de controle.",
                ),
                key(
                    'r',
                    "Revogar clientes OAuth",
                    "Desativa o controle e revoga todas as sessões.",
                ),
                desktop(3, "Gerenciar permissões", "Escolher captura e entrada."),
                desktop(
                    1,
                    "Voltar aos aplicativos",
                    "Revisar quais janelas podem ser usadas.",
                ),
            ],
            _ => vec![
                key(
                    'v',
                    "Alternar captura de tela",
                    "Permite ou impede capturas dos apps autorizados.",
                ),
                key(
                    'i',
                    "Alternar teclado e cliques",
                    "Permite ou impede eventos direcionados.",
                ),
                key(
                    'b',
                    "Desligar controle agora",
                    "Encerra a sessão de controle.",
                ),
                desktop(2, "Voltar à sessão", "Gerenciar o tempo de acesso."),
                desktop(0, "Voltar à conexão", "Gerenciar túnel e PIN."),
            ],
        },
        Logs => vec![
            action(
                if app.processing {
                    "Voltar à tela anterior"
                } else {
                    "Concluir e voltar"
                },
                "A tarefa continua em segundo plano se ainda estiver ativa.",
                ActionKind::Return,
            ),
            key(
                'y',
                "Copiar logs protegidos",
                "Remove informações sensíveis conhecidas antes de copiar.",
            ),
        ],
        Help => vec![
            step(
                next_step(app),
                "Abrir assistente",
                "Configurar o bridge passo a passo.",
            ),
            page(
                Connect,
                "Conectar chat",
                "Instruções para ChatGPT e Claude.",
            ),
            page(
                Security,
                "Segurança",
                "Verificação da exposição e proteção.",
            ),
            page(Logs, "Atividade", "Consultar a saída das operações."),
        ],
    }
}
