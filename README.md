# DevSpace PC Bridge

**ChatGPT e Claude normais, acessando seu Windows via MCP. Sem Codex CLI, Claude Code ou chaves API de modelos.**

O **DevSpace PC Bridge** é um aplicativo Windows de terminal, implementado em **Rust + Ratatui**, que ajuda a instalar, configurar, monitorar e conectar um servidor MCP ao ChatGPT e Claude no navegador ou aplicativo.

O servidor de ferramentas é o [DevSpace](https://github.com/Waishnav/devspace), acessível através de HTTPS com Tailscale Funnel e protegido por autenticação Owner/OAuth. O bridge **não é um modelo de IA, cliente de chat ou editor próprio**.

## O que acontece

    ChatGPT normal / Claude normal
                 |
           MCP remoto HTTPS
                 |
        Tailscale Funnel + OAuth
                 |
         DevSpace (seu Windows)
                 |
       Projetos, arquivos e ferramentas

O modelo continua rodando no ChatGPT/Claude. É o servidor MCP que disponibiliza ferramentas para leitura, operações e modificações **conforme as permissões concedidas**.

## Instalação

1. Baixe **DevSpacePCBridge.exe** na página de [Releases](https://github.com/ompo-dev/devspace-pc-bridge/releases). Abra-o no Windows 10 ou 11; **não é preciso instalar Rust**.
2. Vá a **Configurar** (Tab ou Ctrl+S), escolha a pasta que os agentes poderão utilizar e inicie a instalação com **I**. Confirme digitando **PUBLICAR** — o Funnel disponibiliza um endereço HTTPS na internet.
3. Conclua as autorizações do Windows, Tailscale e o registro Owner do DevSpace, quando solicitados.
4. Vá à tela **Conectar chats** (Ctrl+C), selecione ChatGPT ou Claude, copie o endpoint MCP com **C** e abra a página de conectores com **O**.
5. Cadastre a URL terminada em **/mcp**, autorize por OAuth e teste solicitando a leitura de um arquivo da pasta permitida.

A instalação configura os pré-requisitos necessários via WinGet/npm, como Node.js, Git, Tailscale e DevSpace. Ela não precisa instalar Codex, Claude Code, SDKs de modelos nem APIs pagas.

**Para instalações existentes:** o aplicativo procura o endereço MCP já configurado no computador, sem recriar credenciais. Para diagnosticar, pressione **D**; para conferir a rede, **T**. Não execute outra instalação sobre um servidor já configurado antes de revisar o estado atual.

## Conectar ao chat que você já usa

### ChatGPT

Em [Plugins do ChatGPT](https://chatgpt.com/plugins), use **+ > Adicionar servidor MCP personalizado**, quando disponível na sua conta e workspace. Informe o endpoint HTTPS do bridge, configure OAuth, revise as ações e escolha o plugin na conversa usando **@**.

**Limite importante:** a disponibilidade de MCP personalizado e de ferramentas de escrita depende do plano e das permissões. Segundo a [documentação oficial da OpenAI](https://help.openai.com/en/articles/12584461-connecting-custom-mcp-servers-in-chatgpt), MCP completo com ferramentas de escrita é disponibilizado para Business e Enterprise/Edu; Pro oferece conectores personalizados com permissões de leitura/fetch. A disponibilidade no plano pessoal não deve ser presumida.

### Claude

Em [Personalizar > Conectores](https://claude.ai/customize/connectors), adicione um **conector personalizado** com a URL HTTPS /mcp. Autorize por OAuth e habilite o conector na conversa normal. Claude conecta aos servidores remotos a partir de sua infraestrutura em nuvem, não diretamente de localhost.

Veja o [guia oficial da Anthropic](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp) para as opções por plano e a autenticação.

## Teclas

| Tecla | Ação |
|---|---|
| Tab / Esc | Trocar tela / voltar ao início |
| Ctrl+C | Abrir a seção de conexão com chats |
| Ctrl+S | Instalação/configuração |
| Ctrl+L | Logs |
| Ctrl+X | Cancelar uma operação em andamento |
| Ctrl+Q | Sair |
| Conectar: 1 / 2 | ChatGPT / Claude |
| Conectar: C / O / P | Copiar URL / abrir chat / abrir painel Owner |
| Configurar: R / I | Editar raiz de projetos / iniciar instalação |
| Configurar: D / T | Diagnóstico / testar saúde HTTPS |

**Todas as telas do aplicativo são Ratatui**, e os comandos ficam em segundo plano com logs na própria TUI. O sistema ainda pode abrir o navegador para OAuth ou solicitar uma aprovação administrativa. Isso é independente do terminal do aplicativo.

## Segurança

- **Funnel expõe uma URL pública:** só publique quando entender os riscos; a autenticação deve permanecer ativa.
- Não remova a proteção Owner/OAuth do DevSpace e evite permitir acesso a todo o disco.
- A TUI **não simula** políticas de permissões. Controle de leitura/escrita pertence ao servidor MCP, ao usuário do Windows e às permissões do cliente ChatGPT/Claude.
- Uma resposta HTTP 200 de /healthz confirma rede, **não** uma sessão MCP autenticada nem a permissão de editar arquivos.
- Qualquer ferramenta que modifica arquivos deve ser revisada antes de usá-la. Confirme pedidos de escrita no cliente.
- O instalador preserva configurações existentes e nunca publica auth.json, tokens ou senhas no Git. Saídas de instalação são exibidas na TUI apenas na memória; não compartilhe logs que possam conter dados sensíveis.
- A presença de um endpoint público não significa que ChatGPT ou Claude já tenham sido conectados: a configuração OAuth precisa ser concluída em cada plataforma.
- A distribuição atual do Windows não é assinada digitalmente.

## Limitações verificadas

O bridge não modifica as capacidades oferecidas pela conta do ChatGPT/Claude. Não existe técnica legítima para transformar uma assinatura pessoal em acesso MCP de escrita quando a plataforma não permite isso.

Este programa não incorpora o código do OpenClaude, Codex ou Claude Code. **Nenhum motor de IA é instalado.** O objetivo é que o usuário continue usando os chats existentes e consiga conectar o PC através do servidor MCP.

## Desenvolvimento

    cargo test
    cargo build --release

O executável é gerado em target/release/DevSpacePCBridge.exe. Testes e releases são feitos por GitHub Actions em Windows.

    DevSpacePCBridge.exe --version
    DevSpacePCBridge.exe --doctor
    .\tests\validate.ps1

Os scripts PowerShell em install.ps1 e scripts/ são incorporados ao executável Rust. As preferências da TUI ficam em %APPDATA%\DevSpacePCBridge\bridge-ui.json. Dados anteriores em config.json são preservados.

**Licença:** MIT para o código deste projeto. DevSpace e Tailscale são dependências de terceiros com suas próprias licenças.
