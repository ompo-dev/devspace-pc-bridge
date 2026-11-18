# DevSpace PC Bridge

Use projetos locais do Windows no ChatGPT via MCP e OAuth, com inicializacao automatica apos login.

Este projeto instala e configura as ferramentas oficiais [DevSpace](https://github.com/Waishnav/devspace), [Tailscale](https://tailscale.com/), Node.js e Git. Nao e um fork do DevSpace.

## Instalacao (Windows 10/11)

Requisitos: Windows, PowerShell 5.1+, permissao de administrador, WinGet (App Installer), internet e uma conta Tailscale que permita Funnel.

1. Baixe este repositorio como ZIP e extraia, ou clone usando Git.
2. Abra PowerShell **como Administrador** na pasta extraida.
3. Execute o arquivo **install.ps1** com o comando **.\install.ps1**.
4. Conclua login no Tailscale e habilitacao do Funnel no navegador, se solicitados.
5. Guarde a credencial Owner gerada pelo DevSpace em lugar seguro.
6. Copie a URL informada pelo instalador, terminada em **/mcp**. Cadastre-a no ChatGPT como conexao MCP Streamable HTTP com OAuth. Aprove a autorizacao do DevSpace.

O cadastro de conectores MCP depende dos recursos disponiveis em sua conta ChatGPT. Apos login no Windows, a tarefa DevSpacePCBridge verifica o servidor a cada 30 segundos.

Para escolher outra pasta de projetos, execute **.\install.ps1 -WorkspacesRoot 'D:\Projetos'**. Por padrao, a raiz de projetos e a pasta pai do repositorio.

## Testes e manutencao

| Comando PowerShell | Resultado |
|---|---|
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
| install.ps1 | Instalacao de dependencias, configuracao e tarefa |
| scripts/bridge.ps1 | Supervisao de saude e tentativa de recuperacao |
| scripts/doctor.ps1 | Diagnostico |
| scripts/uninstall.ps1 | Remocao controlada |
| tests/validate.ps1 | Validacao sem escrita |
| .github/workflows/validate.yml | CI no Windows |

## Licencas

Scripts de integracao sob MIT. DevSpace e Tailscale sao produtos independentes com suas proprias licencas e regras.
