# DevSpace PC Bridge

**Uma CLI de agentes para Windows: Rust + Ratatui, um executável, uma interface.**

Converse com Codex, Claude Code, OpenAI, Anthropic, OpenRouter e Ollama, use seus projetos locais e configure a ponte MCP DevSpace. O aplicativo possui chat, seleção de provedores, configuração, execução de comandos em segundo plano, logs, histórico local e streaming. Todos os menus ficam **dentro da mesma TUI**.

> Inspirado na experiência do [OpenClaude](https://github.com/Twigpine/openclaude), mas implementado em Rust do zero. **Não copiamos seu código.** O repositório OpenClaude avisa, em LICENSE, que contém material derivado do Claude Code de propriedade da Anthropic; não o redistribuímos.

## Instalar e começar

1. Baixe [**DevSpacePCBridge.exe**](https://github.com/ompo-dev/devspace-pc-bridge/releases) para Windows 10/11 e execute-o em um terminal Windows Terminal/PowerShell. **Não é necessário instalar Rust.**
2. Na aba **Provedores** (Ctrl+P), selecione sua opção. Para **Codex (conta ChatGPT)** ou **Claude Code (conta Claude)**, instale e faça login com as opções da aba **Setup** (Ctrl+S). Para **OpenAI/Anthropic/OpenRouter API**, cadastre sua própria chave com K; ela é armazenada no **Gerenciador de Credenciais do Windows**. Para **Ollama**, instale e execute o servidor e o modelo desejado separadamente.
3. Opcional: na aba **Setup**, configure DevSpace + Tailscale, registre o MCP no Codex ou Claude Code e conclua a autorização OAuth no navegador. Depois volte ao Chat e envie mensagens com Enter.

**O mesmo .exe cuida do restante da experiência.** O DevSpace e os provedores que exigem CLIs próprios são dependências separadas, instaláveis pelo assistente. Autenticação ChatGPT/Claude Code não equivale a uma chave de API; as opções de API exigem faturamento/acesso próprios.

### O que cada provedor oferece

| Provedor | Autenticação | Ferramentas locais |
|---|---|---|
| Codex CLI | Login próprio do Codex/ChatGPT | Sim, conforme permissões e sandbox do Codex |
| Claude Code CLI | Login próprio do Claude Code | Sim, conforme permissões do Claude Code |
| OpenAI API | Sua chave API | Chat e streaming; sem ferramentas automáticas |
| Anthropic API | Sua chave API | Chat e streaming; sem ferramentas automáticas |
| OpenRouter API | Sua chave API | Chat e streaming; sem ferramentas automáticas |
| Ollama | Servidor local | Chat e streaming; sem ferramentas automáticas |

O cliente Rust não contorna permissões, não compartilha cookies nem transforma assinaturas web em chaves de API. Para Claude/Codex, os agentes externos controlam as ferramentas, e o DevSpace é registrado como MCP nas configurações deles.

## Terminal: navegação

| Tecla / comando | Função |
|---|---|
| Tab | Chat → Provedores → Setup → Logs → Ajuda |
| Enter | Enviar chat / ativar provedor / executar ação |
| Ctrl+P / Ctrl+S / Ctrl+L | Abrir Provedores / Setup / Logs |
| Ctrl+N | Novo chat |
| Ctrl+Q | Sair quando não há tarefas executando |
| Ctrl+X | Solicitar cancelamento de CLI ou instalação em execução |
| Esc | Voltar ao Chat ou cancelar edição |
| F1 | Ajuda |
| Provedores: P / M / E / K | Pasta / Modelo / Endpoint / Chave API |
| Provedores: W | Solicitar permissão explícita para editar projetos |
| /provider codex | Selecionar provedor sem sair do Chat |
| /model gpt-4.1-mini | Selecionar modelo |
| /cd C:\Projetos | Selecionar raiz de projetos |
| /new, /help, /mcp, /status, /read, /write | Ações rápidas |

**As instalações e os comandos são executados sem abrir outra janela CMD.** O resultado aparece na aba Logs. O Windows ainda pode mostrar confirmação UAC, e logins podem abrir o navegador — isso é necessário para proteger o computador e as contas.

## DevSpace e MCP

Na aba Setup, existem ações para instalar dependências, executar diagnóstico, autenticar Codex/Claude e registrar o MCP.

Fluxo:

    Codex/Claude Code --- MCP OAuth --- HTTPS do Tailscale Funnel
                                             |
                                        DevSpace local
                                             |
                                          Projetos

O servidor DevSpace **não é substituído**; usamos [@waishnav/devspace](https://github.com/Waishnav/devspace). A instalação configura Node.js, Git, Tailscale, Funnel, DevSpace e tarefa do Windows quando necessário. Scripts do projeto são embutidos no executável durante a compilação.

A conexão MCP usa a URL https://HOSTNAME.ts.net/mcp. Ela é diferente da origem pública do DevSpace, que não inclui /mcp. Os clientes precisam concluir OAuth. A saúde HTTP **não** comprova que ferramentas MCP foram autenticadas.

## Privacidade e segurança

- As chaves API ficam no **Windows Credential Manager**; não ficam no JSON de configuração nem são publicadas no Git.
- Conversas são salvas em %APPDATA%\DevSpacePCBridge\config.json (texto local, **não criptografado**). Proteja sua conta Windows e faça a limpeza quando quiser.
- O modo inicial dos agentes é **somente leitura**. Para habilitar edições, escolha W e confirme digitando AUTORIZAR. Não há bypass automático de sandbox.
- A integração MCP dá ao agente acesso às permissões concedidas pelo DevSpace. Restrinja as raízes de projetos e revise operações sensíveis.
- **Tailscale Funnel publica um HTTPS acessível pela internet.** A segurança depende de OAuth/Owner do DevSpace e da configuração da máquina.
- O .exe distribuído pelo GitHub Actions ainda **não é assinado digitalmente**. Verifique sempre a origem da release.
- O computador precisa estar ligado e online; se a conta Tailscale expirar, a sessão pode precisar de nova autenticação.

## Limitações atuais

Esta versão **não é um port integral** do OpenClaude: não inclui seu sistema de plugins, subagentes, memória, ferramentas nativas, extensões de IDE ou todos os comandos. O objetivo é manter um núcleo **funcional e leve** com provedores reais e experiência unificada. Também não oferece aprovação interativa de todas as chamadas de ferramenta dos CLIs de terceiros: suas próprias políticas continuam valendo.

## Desenvolver

    cargo test
    cargo build --release

O binário fica em target\release\DevSpacePCBridge.exe. O GitHub Actions executa testes e publica releases para tags v*.

Para diagnosticar manualmente sem abrir a TUI:

    DevSpacePCBridge.exe --doctor
    .\tests\validate.ps1
    .\scripts\doctor.ps1

Arquitetura:

    src/
      main.rs       # TUI Ratatui, chat, menus, atalhos
      backend.rs    # Codex/Claude CLI + API streaming
      storage.rs    # Config, historico, Windows Credential Manager
      setup.rs      # Dependencias, DevSpace/MCP, logs em background
    scripts/        # Powershell supervisor/diagnostico/desinstalacao
    install.ps1     # Provisionamento Windows
    .github/workflows/

**Licença:** MIT para o código deste projeto. DevSpace, Codex, Claude Code, Tailscale e OpenClaude mantêm suas licenças e políticas próprias.
