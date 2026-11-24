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

### Instalador Windows, WinGet e assinatura

Além do executável portátil, as próximas releases são configuradas para publicar
`DevSpacePCBridge-Setup.exe` (instalador NSIS de escopo do usuário), o respectivo
SHA-256 e `winget-manifests.zip`. **O executável e o instalador funcionam sem
assinatura digital**, mas o Windows/SmartScreen pode alertar: verifique o hash e
a origem. Não há assinatura confiável sem um certificado real; não usamos
certificados falsos nem desativamos a proteção do Windows. O instalador só copia
o bridge e cria o atalho/menu Desinstalar: nunca apaga DevSpace, OAuth ou projetos.

Os manifestos WinGet são gerados com o hash efetivo de cada instalador e URL
da release. **Isso ainda não é publicação na fonte pública do WinGet**: depende
de a release estar disponível e de um PR aceito no repositório `winget-pkgs`.
O pipeline só gera o pacote, sem enviar PR automaticamente.

Para compilar o instalador localmente: `cargo build --release`, depois execute
`makensis packaging/DevSpacePCBridge.nsi` com NSIS instalado.

### Dependências individualmente

Na tela **Configurar**, use `1` (Git), `2` (Node), `3` (Tailscale),
`4` (Cloudflared) ou `5` (DevSpace). Confirme digitando `INSTALAR GIT`,
`INSTALAR NODE`, etc. Cada operação verifica a instalação, evita reinstalar
um programa existente e mostra o progresso e possíveis erros em **Logs**.
WinGet instala pacotes oficiais e npm instala apenas a versão fixada do
DevSpace. Para conferir sem instalar: `scripts/install-component.ps1 -Check`.
Login, permissões administrativas e exposição pública permanecem decisões
separadas; uma dependência instalada não implica acesso remoto ativo.

### Bandeja do Windows

Enquanto o aplicativo estiver aberto, um ícone fica na bandeja do Windows.
Na tela inicial, pressione `M` para ocultar o terminal; clique duas vezes no
ícone ou selecione **Mostrar terminal** para voltar. O menu também permite
**Desligar desktop remoto** (encerra o túnel, desliga o controle e revoga os
clientes) e **Sair**. Alguns terminais modernos não fornecem uma janela Win32
ocultável: nesses casos o programa deixa o terminal visível em vez de escondê-lo
sem forma de restaurá-lo.

### Desktop remoto e captura de janelas

O aplicativo inicia um servidor MCP adicional **somente em
`http://127.0.0.1:7677/mcp`**, independente do DevSpace existente na porta
7676. Esse servidor implementa as funções novas: `desktop_status`,
`desktop_list_windows`, `desktop_screenshot`, `desktop_inspect`,
`desktop_click`, `desktop_type`, `desktop_key` e `desktop_scroll`.

Na aba **Desktop remoto**, você pode liberar/remover executáveis por nome
(`A`/`X`), alternar captura (`V`) e entrada (`I`), ativar por prazo (`E`,
confirme `ATIVAR 30`), desativar (`B`) e revogar todos os tokens (`R`).
O acesso começa **desligado** em toda execução, e nenhuma janela fica
disponível por padrão. Senhas, gerenciadores de senhas, terminais e títulos
suspeitos são bloqueados, com limite de ações e logs de auditoria.

Para permitir acesso de um chat ao desktop, pressione `P` e confirme
`PUBLICAR DESKTOP`. O aplicativo inicia um **Cloudflare Quick Tunnel**
temporário, sem necessidade de conta, mostra a URL HTTPS terminada em `/mcp`
e preserva OAuth/PKCE. Abra `C` para obter o PIN de pareamento local e conclua
o fluxo OAuth no navegador, **sem colar o PIN no chat**. `S` para encerrar o
túnel imediatamente. O Quick Tunnel muda de endereço ao reiniciar e não é
indicado para operação permanente. O túnel e o acesso ao desktop são encerrados
ao sair do aplicativo. Não é uma substituição do Funnel/Owner do DevSpace.

Windows tem um único foco e um cursor global. O backend de desktop envia
ações via UI Automation/mensagens direcionadas ao aplicativo permitido, sem
`SendInput` e sem mover o cursor físico. É um método best-effort, **não um
segundo mouse independente**: jogos, apps com entrada bruta, janelas minimizadas
ou GPU protegida podem não responder ou permitir screenshot. Nunca captura
deliberadamente a área de trabalho inteira. Capturas podem conter dados
sensíveis visíveis na janela autorizada, portanto libere somente apps seguros.

Teste seguro do servidor local, sem liberar qualquer app:
`DevSpacePCBridge.exe --desktop-doctor` (HTTP 200 para health e metadados,
401 para MCP sem token e 421 para Host externo).

**Gateway unificado:** o DevSpace e o MCP de desktop continuam servidores
distintos, cada um com sua própria autenticação; ainda não há um gateway que
mescle as ferramentas em um único endpoint OAuth. Não conecte um proxy
desprotegido na tentativa de uni-los.

1. Baixe **DevSpacePCBridge.exe** na página de [Releases](https://github.com/ompo-dev/devspace-pc-bridge/releases). Abra-o no Windows 10 ou 11; **não é preciso instalar Rust**.
2. Vá a **Configurar** (Tab ou Ctrl+S), escolha a pasta que os agentes poderão utilizar e inicie a instalação com **I**. Confirme digitando **PUBLICAR** — o Funnel disponibiliza um endereço HTTPS na internet.
3. Conclua as autorizações do Windows, Tailscale e o registro Owner do DevSpace, quando solicitados.
4. Vá à tela **Conectar chats** (Ctrl+C), selecione ChatGPT ou Claude, copie o endpoint MCP com **C** e abra a página de conectores com **O**.
5. Cadastre a URL terminada em **/mcp**, autorize por OAuth e teste solicitando a leitura de um arquivo da pasta permitida.

A instalação configura os pré-requisitos necessários via WinGet/npm, como Node.js, Git, Tailscale e DevSpace. Ela não precisa instalar Codex, Claude Code, SDKs de modelos nem APIs pagas.

**Para instalações existentes:** o aplicativo procura o endereço MCP já configurado no computador, sem recriar credenciais. Para diagnosticar, pressione **D**; para conferir a rede, **T**. Não execute outra instalação sobre um servidor já configurado antes de revisar o estado atual.

**Versão do DevSpace:** a instalação usa a versão testada com o bridge (1.1.1), e não a mais recente do npm. Se já existir outra versão, o instalador apenas avisa e não altera nada.

**Pastas recusadas:** a instalação e a tela Configurar recusam a raiz de um disco, a pasta pessoal inteira, pastas que a contêm e pastas de credenciais (.ssh, .aws, .gnupg, .kube, .claude, .codex, .devspace, AppData). Use uma pasta de projetos. Quem precisar pode usar `install.ps1 -AllowBroadRoot`, sob sua responsabilidade.

**Desinstalar:** em Configurar, pressione **U** e digite **REMOVER** para apagar só a tarefa agendada. Digite **REMOVER FUNNEL** para também desativar o Funnel. Como o `tailscale funnel reset` apaga todas as regras de Funnel e Serve da máquina, o script se recusa se houver outras portas publicadas (repita com `-Force` para confirmar). Configurações, credenciais, DevSpace, Node, Git e Tailscale são preservados.

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
| Configurar: U | Remover a tarefa do bridge (confirmar com REMOVER ou REMOVER FUNNEL) |
| Início: P | Pausar a publicação (desliga o Funnel) |
| Início: R | Retomar a publicação (digite PUBLICAR ou PUBLICAR 60) |
| Início: K | Checkpoint git dos projetos |
| Início: T / D | Testar HTTPS e OAuth / diagnóstico |
| Logs: Y | Copiar logs sem hostname, usuário e tokens |

As letras de atalho funcionam com Caps Lock ou Shift. A barra inferior mostra o estado, atualizado a cada 30 segundos: ONLINE, OFFLINE, NÃO CONFIGURADO, PAUSADO, ALERTA: SEM OAUTH ou PAUSA NÃO EFETIVA. A tela inicial também traz a disponibilidade da última hora, um checklist (Git, Node.js, DevSpace, Tailscale, servidor local, HTTPS e OAuth) e avisa quando há uma versão nova.

## Pausa, prazo e proteção

- **Teste de proteção (T):** o bridge chama o `/mcp` público **sem token**. `401`/`403` significa que o OAuth está ativo. Um `2xx` significa que o endpoint está aberto: a barra mostra ALERTA e o `--doctor` sai com código 2.
- **Pausar (P):** desliga o Funnel e grava a marca `paused`; o supervisor deixa de religar o Funnel. O servidor local continua rodando, só a internet deixa de alcançar o PC. O Tailscale instalado só documenta `funnel reset` para remover regras, e ele apaga **todas**; por isso a pausa só é aplicada quando a porta do bridge é a única publicada. Se houver outras, o bridge avisa que o Funnel continua ligado.
- **Retomar (R):** digite `PUBLICAR` para ficar ativo sem prazo, ou `PUBLICAR 60` para pausar sozinho em 60 minutos (1 a 1440). O prazo é respeitado pelo aplicativo e pelo supervisor.
- Uma nova instalação (`I`) limpa pausa e prazo, pois é uma decisão explícita de publicar.

## Checkpoints git

Antes de liberar escrita a um chat, pressione **K** (ou rode `DevSpacePCBridge.exe --checkpoint [pasta]`). Para cada repositório git na pasta de projetos o bridge guarda um ponto de restauração com os arquivos atuais, inclusive os não versionados (respeitando o `.gitignore`).

O snapshot usa um índice temporário: não altera seu índice, seus arquivos, seu branch nem o `git stash`. Fica em `refs/devspace-checkpoints/`, que nenhuma listagem de branches mostra, e os 20 mais recentes de cada repositório são mantidos. Arquivos não versionados entram no objeto do repositório, que cresce um pouco.

    DevSpacePCBridge.exe --checkpoints <repositório>
    DevSpacePCBridge.exe --restore <repositório> <nome> --yes

A restauração devolve os arquivos do checkpoint, mas **não apaga** arquivos criados depois dele. O estado atual é salvo como um novo checkpoint antes de restaurar, então o desfazer também é desfazível.

**Todas as telas do aplicativo são Ratatui**, e os comandos ficam em segundo plano com logs na própria TUI. O sistema ainda pode abrir o navegador para OAuth ou solicitar uma aprovação administrativa. Isso é independente do terminal do aplicativo.

## Segurança

- **Funnel expõe uma URL pública:** só publique quando entender os riscos; a autenticação deve permanecer ativa.
- Não remova a proteção Owner/OAuth do DevSpace e evite permitir acesso a todo o disco. O bridge recusa raízes amplas, e o diagnóstico avisa se o `allowedRoots` do DevSpace está amplo demais.
- A TUI **não simula** políticas de permissões. Controle de leitura/escrita pertence ao servidor MCP, ao usuário do Windows e às permissões do cliente ChatGPT/Claude.
- Uma resposta HTTP 200 de /healthz confirma rede, **não** uma sessão MCP autenticada nem a permissão de editar arquivos. Para saber se o OAuth está ativo, use o teste de proteção (`T`).
- Qualquer ferramenta que modifica arquivos deve ser revisada antes de usá-la. Confirme pedidos de escrita no cliente.
- O instalador preserva configurações existentes e nunca publica auth.json, tokens ou senhas no Git. Saídas de instalação são exibidas na TUI apenas na memória; não compartilhe logs que possam conter dados sensíveis.
- A presença de um endpoint público não significa que ChatGPT ou Claude já tenham sido conectados: a configuração OAuth precisa ser concluída em cada plataforma.
- **Auto-update com confirmação:** ao iniciar e periodicamente (a cada 4 horas), o bridge consulta as releases estáveis do GitHub. Só avisa se a release for mais recente e publicar os dois arquivos oficiais `DevSpacePCBridge.exe` e `DevSpacePCBridge.exe.sha256`. Em **Início**, pressione `U`, confirme digitando `ATUALIZAR`, e acompanhe o progresso em **Logs**. O aplicativo baixa o novo executável na mesma pasta, confere tamanho e SHA-256 e inicia um helper separado feito a partir da versão anterior. O helper espera o bridge sair, preserva um backup da versão anterior, troca o executável e reinicia a nova versão. Se a troca falhar, tenta restaurar a versão anterior. `N` consulta manualmente o GitHub. A verificação automática pode ser desativada com `DEVSPACE_BRIDGE_NO_UPDATE_CHECK=1` (a consulta manual continua possível). **Não é uma atualização silenciosa:** a troca depende da aprovação local.
- **Primeira migração:** versões antigas que só exibiam um aviso, mas não possuíam o atualizador, exigem instalar manualmente uma versão com auto-update uma vez. As próximas releases com o recurso podem atualizar a partir do app.
- **Segurança da distribuição:** o `.sha256` evita baixar um arquivo diferente do publicado na mesma release, mas não substitui uma assinatura Authenticode. O download deve vir de assets do repositório oficial, por HTTPS; o updater não executa um EXE recebido pela internet antes de verificar sua integridade. Confira as permissões de escrita na pasta do executável (instalação per-user ou EXE portátil).
- **Copía de logs (`Y`):** a redação é de melhor esforço (hostname `.ts.net`, nome de usuário do Windows e textos parecidos com token). Releia antes de colar em algum lugar público.
- A distribuição atual do Windows não é assinada digitalmente. As releases a partir da próxima trazem um arquivo `.sha256` (a v0.4.0 não tem; o GitHub mostra o hash dela na página da release); confira com `(Get-FileHash .\DevSpacePCBridge.exe -Algorithm SHA256).Hash` antes de executar.

## Limitações verificadas

O bridge não modifica as capacidades oferecidas pela conta do ChatGPT/Claude. Não existe técnica legítima para transformar uma assinatura pessoal em acesso MCP de escrita quando a plataforma não permite isso.

Este programa não incorpora o código do OpenClaude, Codex ou Claude Code. **Nenhum motor de IA é instalado.** O objetivo é que o usuário continue usando os chats existentes e consiga conectar o PC através do servidor MCP.

## Desenvolvimento

    cargo test
    cargo build --release

O executável é gerado em target/release/DevSpacePCBridge.exe. Testes e releases são feitos por GitHub Actions em Windows.

    DevSpacePCBridge.exe --version
    DevSpacePCBridge.exe --doctor    # saída 0 = saudável, 1 = indisponível ou pausado, 2 = endpoint aberto sem OAuth
    .\tests\validate.ps1

O código Rust fica em `src/` (`app` estado e teclas, `ui` desenho, `setup` tarefas e sondas, `tunnel` pausa/retomada, `checkpoint`, `storage`, `redact`, `update`, `checks`, `clock`). Os testes dos scripts ficam em `tests/scripts.tests.ps1`, executam as funções reais dos `.ps1` e não dependem do Pester.

Os scripts PowerShell em install.ps1 e scripts/ são incorporados ao executável Rust e devem conter apenas ASCII (o PowerShell 5.1 lê arquivos sem BOM como ANSI); o `validate.ps1` confere isso. As preferências da TUI ficam em %APPDATA%\DevSpacePCBridge\bridge-ui.json. Dados anteriores em config.json são preservados.

**Licença:** MIT para o código deste projeto. DevSpace e Tailscale são dependências de terceiros com suas próprias licenças.
