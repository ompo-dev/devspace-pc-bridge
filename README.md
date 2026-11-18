# DevSpace PC Bridge

**Baixe um unico .exe, abra e configure o DevSpace no Windows usando um terminal interativo.** Interface escrita em Rust com [Ratatui](https://ratatui.rs/), inspirada na experiencia de terminal do [OpenClaude](https://github.com/Twigpine/openclaude).

O executavel contem os scripts do projeto. Ele prepara as ferramentas oficiais [DevSpace](https://github.com/Waishnav/devspace), [Tailscale](https://tailscale.com/), Node.js e Git. **Nao e um fork do DevSpace nem inclui o mecanismo de agentes do OpenClaude.**

## Comece em 3 passos (Windows 10/11)

1. Abra a pagina de [Releases](https://github.com/ompo-dev/devspace-pc-bridge/releases), baixe **DevSpacePCBridge.exe** e execute-o (duplo clique). Nao precisa instalar Rust ou compilar nada.
2. No terminal, escolha a pasta de projetos com **R** e inicie a configuracao com **I**. Autorize o Windows (UAC), conecte sua conta Tailscale e permita Funnel se for solicitado.
3. No final, guarde sua senha Owner e copie a URL do MCP com **C**. Cadastre essa URL no ChatGPT como MCP Streamable HTTP com OAuth; aprove o acesso no DevSpace.

Requisitos no computador: **Windows + acesso a internet + WinGet (App Installer) + permissao de administrador**. O instalador prepara Node.js, Git, DevSpace e Tailscale conforme necessidade. A conta Tailscale e a autorizacao OAuth ainda precisam ser feitas pelo usuario.

O cadastro de conectores MCP depende dos recursos disponiveis na sua conta ChatGPT. Apos login no Windows, a tarefa DevSpacePCBridge verifica o servidor a cada 30 segundos. Seu PC precisa continuar ligado e online para responder.

Teclas: **I** instalar, **R** pasta, **D** diagnostico, **O** painel, **C** copiar MCP, **H** ajuda, **Q** sair. Tambem pode navegar com setas e Enter.

## Alternativa manual (para usuarios avancados)

Baixe o codigo-fonte, abra PowerShell como Administrador na pasta e execute **.\install.ps1**. A raiz de projetos padrao e a pasta pai do repositorio. Para escolher outra, execute **.\install.ps1 -WorkspacesRoot 'D:\Projetos'**.

## Testes, compilacao e manutencao

| Comando PowerShell | Resultado |
|---|---|
| cargo test | Testes Rust (apenas desenvolvedores) |
| cargo build --release | Compila target\release\DevSpacePCBridge.exe |
| .\install.ps1 -Check | Preflight sem alterar o ambiente |
| .\tests\validate.ps1 | Verificacao offline de sintaxe e preflight |
| .\scripts\doctor.ps1 | Diagnostico local e publico |
| .\scripts\uninstall.ps1 | Remove somente a tarefa de inicio automatico |
| .\scripts\uninstall.ps1 -DisableFunnel | Remove tarefa e desativa Funnel no dispositivo |

Uma chamada GET /healthz com HTTP 200 nao prova acesso MCP. Teste **open_workspace + read** no ChatGPT e repita depois de reiniciar o Windows.

## Fluxo

ChatGPT (MCP + OAuth) -> HTTPS em hostname individual *.ts.net -> Tailscale Funnel -> DevSpace em 127.0.0.1:7676 -> arquivos / Git / terminal do Windows.

O Agendador de Tarefas inicia o script de supervisao apos login. O servico Windows do Tailscale mantem o Funnel em background. O script verifica a saude e tenta recuperar o servidor sem duplicar um processo na mesma porta.

### Diagnostico

| Sintoma | Acao |
|---|---|
| winget ausente | Instale App Installer da Microsoft |
| Node instalado, PATH antigo | Abra um novo PowerShell e repita |
| Tailscale nao online | Entre na conta e confira a conexao |
| Funnel falhou | Confirme permissoes de administrador e configuracao da tailnet |
| Health local falhou | Rode devspace doctor, verifique a porta e os logs |
| Health publico falhou | Verifique tailscale status e tailscale funnel status |
| Login MCP necessario | Autorize Owner/OAuth pelo hostname correto |
| Workspace recusado | Verifique workspaces.allowedRoots em devspace config get |
| Servico nao reiniciou apos login | Verifique tarefa DevSpacePCBridge no Agendador |

## Seguranca

- **Tailscale Funnel e publico na internet.** A protecao depende da autenticacao OAuth do DevSpace e da seguranca do sistema.
- O executavel ainda nao e assinado digitalmente. O Windows SmartScreen pode mostrar uma advertencia. Confirme que o arquivo veio deste repositorio.
- O aplicativo abre uma segunda janela elevada durante a instalacao, mostrando o progresso e erros. Feche-a apos ler o resultado.
- Nao inclua no Git auth.json, tokens, certificados, configuracoes privadas ou logs. O repositorio possui .gitignore para ajudar.
- A pasta de projetos autorizada deve ser a menor possivel. Comandos locais ainda podem ter permissoes do usuario do Windows fora dela.
- Instalacoes existentes sao preservadas: nao executa init --force, nao substitui credenciais, nem interrompe servidores em funcionamento.
- Caso haja outra configuracao DevSpace com porta ou publicBaseUrl diferente, o instalador para e pede revisao manual.
- O Supervisor nao reinicia sozinho processos travados quando ainda ocupam a porta; verifique manualmente antes de encerrar processos.
- Desinstalar a tarefa nao desabilita automaticamente o Funnel publico. A opcao -DisableFunnel pode afetar outros servicos publicados na mesma maquina.
- A operacao remota depende do computador ligado, rede disponivel, login no Windows, conta Tailscale valida e OAuth ativo.
- O instalador exige administrador para instalar pacotes e configurar o sistema. O supervisor roda com permissoes limitadas do usuario interativo.

## Estrutura

| Caminho | Funcao |
|---|---|
| Cargo.toml e src/main.rs | Aplicativo Rust/Ratatui e assistente interativo |
| install.ps1 | Instalacao de dependencias, configuracao e tarefa |
| scripts/bridge.ps1 | Supervisao de saude e tentativa de recuperacao |
| scripts/doctor.ps1 | Diagnostico |
| scripts/uninstall.ps1 | Remocao controlada |
| tests/validate.ps1 | Validacao sem escrita |
| .github/workflows/validate.yml | CI no Windows |

## Releases

O GitHub Actions compila e testa automaticamente para Windows. Tags v* geram uma release com o executavel .exe (sem Node.js nem Rust embutidos no binario). Os componentes que sao usados pelo servidor sao instalados na configuracao inicial.

## Licencas

App e scripts de integracao sob MIT. DevSpace, OpenClaude e Tailscale sao projetos independentes; nenhum codigo-fonte do OpenClaude e copiado para este projeto.
