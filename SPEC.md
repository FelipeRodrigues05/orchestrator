# Orquestrador de agentes (Rust + GTK4): arquitetura

Oct 28, 2026 · @Felipe Rodrigues

## Visão geral

Um app desktop em Rust + GTK4 onde você abre um ou mais projetos, cria grupos de trabalho e coloca em cada grupo agentes com uma task e uma persona; os agentes de um projeto se conhecem e podem chamar uns aos outros. Você fala com o app por voz, e quando um agente precisa de permissão aparece um popup e o app diz em voz alta quem pede o quê.

Três princípios guiam o desenho:

- **Core sem GTK.** Toda a lógica (projetos, agentes, permissões) vive num crate `core` testável sem janela; o crate `app` só desenha o estado e envia comandos.
- **Tudo conversa por canais.** Agentes, voz e UI são atores independentes ligados por mensagens, como goroutines + channels em Go.
- **Nenhuma ação sensível sem passar pelo broker.** O agente nunca executa algo que exige permissão sem que o pedido passe por um único ponto central, que decide por regra ou pergunta a você.

## Stack e crates

O app é agnóstico de provedor: usa os agentes de linha de comando que você já tem instalados ou conecta um modelo por chave de API. Os detalhes estão na seção Provedores de agente logo abaixo.

| Camada | Crate / ferramenta | Por quê |
| --- | --- | --- |
| UI | `gtk4` (gtk4-rs) + `libadwaita` + `pangocairo` | A control room é uma grade de caracteres desenhada com Cairo/Pango, popup de permissão incluído (ADR 0014) |
| Async | `tokio` + `async-channel` | tokio roda agentes e I/O; async-channel funciona tanto no tokio quanto no loop do GLib |
| Agentes locais | `agent-client-protocol` (ACP) | Protocolo aberto (JSON-RPC por stdio) que Gemini CLI fala nativo e Claude Code e Codex falam por adaptador; inclui pedido de permissão |
| Agentes por API | `reqwest` + loop de tools próprio | Anthropic, OpenAI, Gemini, OpenRouter ou Ollama local; o app executa as tools, então a permissão é natural |
| Permissões (fallback) e equipe | Servidor MCP mínimo próprio (ADRs 0016 e 0018) | Tool de permissão para CLIs sem ACP (`--permission-prompt-tool` do Claude Code) e as tools `list_agents`/`ask_agent` para os CLIs |
| Chaves de API | `oo7` ou `keyring` (Secret Service) | Chave fica no chaveiro do GNOME/KDE, nunca no SQLite |
| Captura de áudio | `cpal` (PipeWire) | Microfone em 16 kHz mono para o STT |
| Voz → texto | `whisper-rs` (whisper.cpp) | Português bem melhor que Vosk; modelo `small` em CPU, `medium` com GPU, `tiny` para a wake word |
| Texto → voz | Piper (subprocesso) + `rodio` | Voz `pt_BR-faber-medium` local e natural; speech-dispatcher/espeak soa robótico |
| Integração Wayland | `ashpd` (xdg-desktop-portal) | Atalho global via portal `GlobalShortcuts` |
| Persistência | `rusqlite` ou `sqlx` (SQLite) + `serde` | Projetos, grupos, agentes, histórico e regras de permissão |
| Notificação | `gio::Notification` | Aviso do sistema com botões Permitir/Negar quando a janela não está em foco |
| Bandeja | `ksni` (StatusNotifierItem) | Ícone na barra: abre/esconde a sala, liga a wake word, sai; fica em alerta com pedido pendente |
| Logs | `tracing` | Um span por agente facilita depurar vários ao mesmo tempo |

## Provedores de agente

Todo agente, venha de onde vier, vira um `AgentRunner` que emite os mesmos eventos e manda os mesmos `PedidoPermissao` ao broker. O resto do app não sabe qual provedor está por trás.

| Família | Exemplos | Como o app descobre | Como chega a permissão |
| --- | --- | --- | --- |
| CLI local com ACP | Gemini CLI, Claude Code e Codex via adaptador ACP | Procura binários conhecidos no `PATH` e mostra os encontrados | Mensagem `session/request_permission` do protocolo |
| CLI local sem ACP | Claude Code direto, outros CLIs | Mesmo scan do `PATH`; configuração manual do comando | Tool MCP do app (`--permission-prompt-tool`) ou modo só leitura |
| Modelo por API | Anthropic, OpenAI, Gemini, OpenRouter | Você cola a chave em Configurações → Provedores | O próprio loop de tools do app pergunta antes de executar |
| Modelo local | Ollama, llama.cpp server | Detecta `localhost:11434` | Igual ao modelo por API |

Na tela de criação de agente você escolhe o provedor e o modelo; um grupo pode misturar provedores (ex.: Claude Code para o backend, Gemini para revisar). Recomendo começar por ACP, porque um único runner cobre vários CLIs e já traz o pedido de permissão padronizado.

## Modelo de dados

Vários projetos podem ficar abertos ao mesmo tempo, um deles em foco (ADR 0017). Um projeto tem vários grupos, um grupo tem vários agentes, e cada agente tem uma task, uma persona opcional e uma política de permissão. Cada agente trabalha num git worktree próprio dentro do projeto, para que dois agentes não editem o mesmo arquivo ao mesmo tempo.

```rust
struct Projeto { id: Uuid, nome: String, raiz: PathBuf }

struct Grupo {
    id: Uuid, projeto_id: Uuid,
    regras: Vec<RegraPermissao>,  // conferidas depois das do agente
    max_ativos: Option<usize>,    // limite de agentes trabalhando ao mesmo tempo
    nome: String,          // "API de pagamentos"
    objetivo: String,      // contexto comum passado a todos os agentes do grupo
}

struct Agente {
    id: Uuid, grupo_id: Uuid,
    nome: String,          // nome falado pelo TTS: "Testes"
    task: String,          // "escrever testes de integração do handler /pix"
    persona: String,       // "só backend; pede ao Integração para testar o front" (vazio: nenhuma)
    provedor: Provedor,    // CliAcp { comando } | CliMcp { comando } | Api { servico, modelo, chave_ref } | Local { url, modelo }
    worktree: PathBuf,
    estado: EstadoAgente,  // Parado | Rodando | AguardandoPermissao | Concluido | Erro
    regras: Vec<RegraPermissao>,
}

struct PedidoPermissao {
    id: Uuid, agente_id: Uuid,
    ferramenta: String,    // "Bash", "Edit", "WebFetch"
    entrada: serde_json::Value, // ex.: { "command": "cargo test" }
    resumo_falado: String, // "Testes quer rodar cargo test"
    criado_em: DateTime<Utc>,
}

enum Decisao { Permitir, PermitirSempre(RegraPermissao), Negar { motivo: String } }

struct RegraPermissao { ferramenta: String, padrao: Option<String>, efeito: Efeito } // Efeito: Permitir | Negar | Perguntar
```

O `PedidoPermissao` vive só em memória enquanto espera; a decisão tomada vai para o histórico no SQLite.

## Arquitetura de runtime

A GTK fica sozinha na thread principal só desenhando; um runtime tokio numa thread própria roda orquestrador, broker, servidor MCP e voz; cada agente de CLI é um processo filho (Claude Code, opencode, Codex, Gemini) e cada agente por API é uma task tokio. É o mesmo modelo de goroutine por agente + channel do Go, com `tokio::spawn` + `mpsc`.

&#91;embedded content: arquitetura · GTK, runtime tokio e processos dos agentes\]

O broker é o único que abre popup e manda o TTS falar; o ator de voz devolve a ele o "permitir" ou "negar" falado.

Cada tipo de agente implementa o mesmo trait, para trocar Claude Code por API sem mexer no resto:

```rust
#[async_trait]
trait AgentRunner: Send {
    async fn iniciar(&mut self, agente: &Agente, mcp_url: &str) -> Result<()>;
    async fn enviar(&mut self, texto: String) -> Result<()>;
    fn eventos(&mut self) -> mpsc::Receiver<EventoAgente>; // texto, tool_use, custo, fim
    async fn parar(&mut self) -> Result<()>;
}
```

Três implementações cobrem todas as famílias: `AcpRunner` (spawn do CLI e JSON-RPC por stdio), `McpCliRunner` (spawn do CLI com a tool MCP de permissão do app, como `claude -p --output-format stream-json --permission-prompt-tool mcp__orq__permissao`) e `ApiRunner` (chamadas HTTP com loop de tools próprio, servindo também o Ollama). Todas rodam no worktree do agente.

## Fluxo de permissão

O agente fica bloqueado esperando um `oneshot` até você decidir, por clique ou por voz. Cada agente recebe uma URL do servidor MCP interno com um token próprio, então o app sempre sabe qual agente está pedindo.

1. O Claude Code do agente quer rodar uma tool que exige permissão (ex.: `Bash` com `cargo test`) e chama a tool MCP `permissao` do app, passada em `--permission-prompt-tool`.
2. O handler MCP identifica o agente pelo token, monta o `PedidoPermissao` com um `oneshot::Sender<Decisao>` e envia ao **broker**; depois espera o `Receiver`.
3. O broker confere as regras do agente e do grupo. Se uma regra casa, responde na hora sem incomodar você.
4. Se nenhuma regra casa, o pedido entra numa fila e vira evento para a UI e para a voz:
   - UI: o modal desenhado na sala com agente, grupo, ferramenta e comando, e as opções Permitir, Sempre (agente), Grupo e Negar. Pedidos simultâneos aparecem um por vez, com um contador dos que faltam.
   - Voz: o TTS fala "Agente Testes, do grupo API, pede permissão para rodar cargo test". Pedidos de permissão furam a fila de fala.
   - Sistema: `gio::Notification` se a janela estiver fora de foco.
5. Você responde clicando ou falando "permitir", "sempre" ou "negar". A decisão volta pelo `oneshot`.
6. O handler devolve ao Claude Code `{"behavior":"allow","updatedInput":…}` ou `{"behavior":"deny","message":…}` e o agente continua.

Sem resposta em um tempo configurável (ex.: 10 min), o pedido vira negação com motivo "sem resposta", para o agente não ficar travado para sempre.

Os passos 1, 2 e 6 usam o Claude Code via MCP como exemplo. Num agente ACP o pedido chega como `session/request_permission` e a resposta é a opção escolhida; num agente por API o próprio `ApiRunner` cria o pedido antes de executar a tool. Nos três casos o broker, o popup e a voz são os mesmos.

## Voz

Push-to-talk e comandos fixos vieram primeiro; a wake word ("ei <pet>") e o roteamento pelo modelo do pet vieram depois.

**Entrada (STT)**

- Segurar Ctrl+Espaço (na janela ou pelo atalho global) ou chamar o pet pelo nome liga a captura com `cpal` em 16 kHz mono.
- Ao soltar, o áudio vai para um worker em `spawn_blocking` com `whisper-rs`, no idioma escolhido (`:lang`).
- O texto passa por um parser simples de comandos: "permitir", "sempre", "negar" respondem o pedido aberto; "grupo API, agente Testes: …" manda o resto como mensagem ao agente; qualquer outra frase falada vai para o pet, que decide (texto digitado vai para o agente selecionado).
- Enquanto você fala, o TTS fica mudo, para o microfone não captar a voz do app.

**Saída (TTS)**

- Um ator de fala com fila de prioridade: permissão > erro > agente concluíu > o resto.
- Cada frase vira áudio pelo Piper (`echo texto | piper --model <voz> --output_raw`, padrão `pt_BR-faber-medium`) e toca com `rodio`.
- Frases curtas e sempre no formato "agente, grupo, o que quer", para você entender sem olhar a tela.

## Interface

A interface é uma control room em ASCII: uma única grade de caracteres monoespaçados, fundo preto, vermelho só para o que é urgente (ADR 0014). Não há painéis encaixáveis; tudo cabe numa tela, e o teclado é o caminho principal (o mouse também funciona).

**A sala**

- Topo: relógio, latência real do runtime, tokens gastos e contagem de agentes por estado; abas F1–F5 (overview, agentes, permissões, log, setup) e a luz de CAUTION, que pisca enquanto houver pedido sem resposta.
- Sector map: cada grupo é um bloco com seus agentes (estado, uptime, atividade, provedor, ocupação `1/2` quando há limite), ligado ao pet no centro; a ligação fica dupla e vermelha quando o agente pede permissão.
- À direita, a fila de permissões e o log de eventos; embaixo, o console do agente selecionado, a faixa de voz e a linha de comando.
- À esquerda do sector map, a coluna PROJECTS lista os projetos abertos: agentes trabalhando e um selo vermelho piscando quando há pedido esperando naquele projeto. `Alt+1…9`, clique ou `[` `]` trocam o projeto em foco; o sector map e a tabela de agentes mostram só o projeto em foco, e a fila de permissões, o CAUTION e o log continuam globais (ADR 0017).
- O layout salvo por projeto é a aba aberta e o agente selecionado.

**Popup de permissão**

- Um modal desenhado na própria sala (borda vermelha dupla, o comando num quadro, contador "1 de N" e tempo restante) com `[A]llow`, `A[l]ways` (regra no agente), `[G]roup` (regra no grupo), `[D]eny` e `[Esc] later`, que só esconde o modal.
- Pedidos arriscados (um `Bash` que escreve fora do worktree) aparecem com aviso e nunca viram regra.
- Com a janela sem foco sai uma `gio::Notification` com Permitir/Negar.

**Paleta de comandos e assistente**

- `:` `Ctrl+P` ou `Espaço` abrem a paleta no estilo LazyVim/VS Code, com busca fuzzy: novo agente, novo grupo, abrir projeto, start/stop, abrir no editor, trocar provedor, modelo do pet, chave de API, atualizar CLIs, instalar voz, views e "ir para agente". Um texto que não casa com nada roda como comando (`open ~/repo`, `new …`, `limit API 2`).
- `n` abre o assistente de novo agente: grupo (escolher ou criar) › nome › provedor › task › persona (opcional).
- Texto livre (Enter) vai para o agente selecionado, ou para o pet com `@pet …`. Um `@` no começo de uma palavra abre a lista de arquivos do projeto (`git ls-files` do worktree do agente) e insere `@caminho`; o briefing diz aos agentes que `@caminho` é um arquivo do projeto.
- `o` (ou `:open` sem caminho) lista os projetos recentes, os repositórios git achados em `~/www`, `~/code`, `~/projects` e afins (`ORCH_PROJECT_DIRS` troca a lista) e um "Browse…" com o seletor de pastas do sistema; `:open shop` abre pelo nome da pasta.

**Abrir no editor**

- O app junta os editores achados no `PATH` (`code`, `zed`/`zeditor`, `cursor`, `subl`, `nvim`, `hx`) com os apps `.desktop` lidos por `gio::AppInfo` (VS Code, Zed, IDEs JetBrains).
- `e` (ou `:edit`) abre o worktree do agente; `:edit project` abre a raiz. O último editor usado vira o padrão. Editores de terminal abrem dentro do terminal padrão.

**O pet**

- O pet é o anfitrião (ADR 0012): escuta (STT), fala (TTS) e repassa ordens. Frases fixas ("permitir", "sempre", "negar", "resumo", "grupo X, agente Y: …") são resolvidas na hora por um parser; o resto vai para o modelo do pet, que age com tools do app (listar, mandar mensagem, criar grupo e agente, iniciar/parar, responder permissão pelo broker, trocar provedor) e responde em voz.
- O modelo do pet é configurável (`:pet`); sem escolha, um modelo rápido por API quando há chave, senão um modelo local.
- O desenho é ASCII, gerado por usuário (cada pessoa tem o seu pet), com estados ocioso, vigiando, ouvindo, pensando, falando, alerta e offline (ADR 0015).
- **Onde ele aparece:** no centro da sala. Flutuar sobre a área de trabalho fora do app exige layer-shell (`gtk4-layer-shell`: KDE, Hyprland, Sway; não GNOME) e é pós-MVP.

## Riscos

O maior risco é vários agentes mexendo no mesmo repositório; o segundo é você ser bombardeado de pedidos de permissão.

| Risco | Mitigação |
| --- | --- |
| Agentes do mesmo grupo editam os mesmos arquivos e geram conflito | Um git worktree e uma branch por agente; o merge é um passo explícito seu ou de um agente líder |
| Fadiga de permissão: dezenas de popups por hora | "Sempre permitir" com padrão (ex.: `Bash(cargo test:*)`), regras por grupo, leitura liberada por padrão |
| Wayland não deixa o app ficar sempre no topo nem roubar foco | Notificação do sistema com botões Permitir/Negar + aviso por voz; o popup completo abre quando você volta à janela |
| Atalho global de voz no Wayland depende do compositor | Portal `GlobalShortcuts` via `ashpd` (KDE e GNOME 48+); sem suporte, atalho só com a janela em foco |
| Cada CLI fala de um jeito e muda entre versões | ACP como caminho principal, MCP como fallback, tudo atrás de `AgentRunner`; versionar os adaptadores |
| Agente via API não tem as ferramentas prontas de um CLI | Conjunto mínimo de tools no app (ler, editar, buscar, rodar comando) reaproveitado por todos os provedores |
| Chave de API vazar | Guardada no Secret Service, nunca em log nem no SQLite |
| Whisper lento em CPU | Modelo `small`, frases curtas por push-to-talk, feature de GPU (CUDA ou Vulkan) quando houver |
| Misturar loop do GTK com tokio trava a UI | GTK só na thread principal, tokio numa thread própria, ponte por `async-channel` |
| Custo e limite de uso com N agentes em paralelo | Limite de agentes ativos por grupo e contador de uso por provedor na UI |
| Agentes chamando uns aos outros em loop ou gastando tokens à toa | Perguntas só dentro do projeto, ciclo recusado, timeout de 20 min, limite de ativos por grupo; o log mostra cada pergunta e resposta |
| Agente com acesso a shell faz algo destrutivo | Negação como padrão para escrita fora do worktree; sandbox (bubblewrap) numa fase posterior |

## Roteiro de MVP

Cada etapa termina em algo que dá para ver funcionando; a feature principal (popup + voz de permissão) chega na etapa 4, e o segundo tipo de provedor na etapa 6.

1. **Esqueleto.** Workspace cargo com `core` e `app`; janela libadwaita com a control room em ASCII (ADR 0014) usando dados falsos; runtime tokio numa thread e um evento chegando na UI.
2. **Um agente via ACP.** Descoberta de CLIs no `PATH`, spawn de um agente ACP (ex.: Gemini CLI ou Claude Code com adaptador), saída no painel e envio de mensagem pela caixa de texto. Botão "Abrir em" com os editores detectados.
3. **Popup de permissão.** `session/request_permission` → broker → `adw::AlertDialog` com Permitir e Negar, mais notificação com botões.
4. **Pet com voz de saída.** Pet no centro do app com os estados animados; ele anuncia os pedidos de permissão com o Piper.
5. **Grupos de verdade.** Vários agentes por grupo, worktree por agente, layout dos painéis e dados salvos no SQLite.
6. **Provedor por API.** Tela de provedores com chave no Secret Service, `ApiRunner` com o conjunto mínimo de tools, Ollama local.
7. **Pet que escuta.** Push-to-talk + `whisper-rs`, atalho global via portal; o pet vira agente anfitrião com tools do app e responde permissões, repassa ordens e resume o estado.
8. **Menos interrupções.** "Sempre permitir" com padrões, fila de pedidos, timeout.
9. **Depois do MVP.** Pet flutuando no desktop via layer-shell, fallback MCP para CLIs sem ACP, agente líder que divide a task do grupo, wake word, sandbox.

## Decisões

- **Provedores:** agnóstico. Usa os CLIs de agente instalados na máquina e também modelos conectados por chave de API (decidido em 28/10/2026).
- **Sistema:** só Linux com Wayland por enquanto.

* **Interface:** uma control room em ASCII desenhada numa grade de caracteres (sector map, fila de permissões, log de eventos, console do agente e faixa de voz), no lugar dos painéis encaixáveis do `libpanel`; os painéis não precisam flutuar (decidido em 28/10/2026, ADR 0014). O popup de permissão é desenhado na própria control room (com notificação do sistema quando a janela está sem foco) e há uma paleta de comandos estilo LazyVim/VS Code com assistente de novo agente.
* **Pet:** anda livre pela tela quando o desktop suporta layer-shell (KDE, Hyprland, Sway) e fica no centro do app quando não suporta (GNOME).
* **Pet anfitrião:** o parser de frases fixas responde na hora; o resto vai para um modelo com tools do app. O pet é ASCII e gerado por usuário, no lugar do sprite sheet (decidido em 28/10/2026, ADR 0015).
* **Permissões:** timeout configurável (`:timeout`); "sempre permitir" pode virar regra do agente ou do grupo; um `Bash` que parece escrever fora do worktree sempre pergunta e nunca vira regra, até o sandbox da etapa 9 (decidido em 28/10/2026).
* **Agentes:** a troca de provedor/modelo no meio da task reinicia o agente no mesmo worktree com um resumo da conversa; CLIs desatualizados são atualizados automaticamente uma vez e o agente tenta de novo; cada grupo pode ter um limite de agentes ativos, e o excedente espera na fila; tokens gastos aparecem por agente, provedor e pet (decidido em 28/10/2026).
* **Etapa 9 (pós-MVP):** sandbox com bubblewrap ligado por padrão (só o worktree e as pastas de estado dos CLIs são graváveis); Claude Code direto via `McpCliRunner` com um servidor MCP mínimo de permissão (ADR 0016); `:merge` commita o worktree do agente e faz merge do branch, abortando em conflito; `:lead` faz o pet dividir o objetivo do grupo em agentes; pet flutuante via layer-shell (`:pet float`); wake word opcional (`:wake on`, "ei <pet>") com VAD por energia e whisper `tiny` (decidido em 29/10/2026).
* **Voz e idioma:** idioma configurável (`:lang pt|en`): transcrição, respostas do pet e anúncios falados seguem a escolha; a voz do Piper é configurável (`:voice <id>`, baixada se faltar). A wake word fica ligada por padrão quando há STT ("ei <pet>", `:wake off` desliga) e compara o nome pela fonética. O pet abre o editor, um terminal, o e-mail (`:email <url>` define o webmail) e links http/https/mailto, nunca arquivos ou programas (decidido em 29/10/2026).
* **Vários projetos:** vários projetos ficam abertos ao mesmo tempo, com um em foco; abrir outro não para os agentes do primeiro, e fechar um (`:close`) para só os dele. A coluna PROJECTS fica à esquerda do sector map (decidido em 29/10/2026, ADR 0017).
* **Personas e equipe:** cada agente tem uma persona opcional (quem ele é, o foco, o que deixa para os outros). Os agentes de um projeto se conhecem: o briefing lista os colegas, e as tools `list_agents` e `ask_agent` (no loop de tools dos agentes por API e no servidor MCP do app para CLIs) entregam uma pergunta a um colega e devolvem a resposta do turno dele. Perguntas ficam no projeto, ciclos são recusados e há timeout de 20 min (decidido em 29/10/2026, ADR 0018).
* **Agentes sem projeto e personas prontas:** criar um agente ou grupo sem projeto aberto abre o workspace pessoal (`~/.local/share/orchestrator/personal`, um repositório git criado na hora) com o grupo Personal, então agentes de marketing, escrita ou planejamento funcionam como os outros (worktree, sandbox, broker). O passo de persona oferece modelos prontos com as boas práticas de cada área (marketing, copywriter, produto, backend, frontend, revisão, integração, documentação, dados, assistente pessoal) ou uma descrição própria (decidido em 03/11/2026).
* **Modelo e leitura:** além do provedor, cada agente escolhe o modelo dentro dele (`:model <id>` ou "Model of …"): nos CLIs ACP que oferecem a opção `model` da sessão a troca é na sessão viva, no Claude Code o agente reinicia com `--model` mantendo a conversa, nos agentes por API e locais muda o modelo do provedor. O log de eventos e o console de cada agente rolam (roda do mouse, PageUp/PageDown, Shift para o log, End volta ao fim) sem pular quando chegam linhas novas. O objetivo do grupo é opcional (decidido em 03/11/2026).
* **Model router:** quando um agente bate no limite de uso (tokens, cota, rate limit), o par provedor/modelo fica de lado por 1 h e o agente continua no modelo mais próximo disponível: mesmo nível em outro provedor (Opus → gpt-6-astra) antes de um nível abaixo (Sonnet), com o resumo da conversa no mesmo worktree. `:fallback` fixa a ordem, `auto` volta à proximidade, `reset` libera todos (decidido em 03/11/2026, ADR 0020).
* **Leitura das respostas:** o console e o modo leitura (F6 / `r`, a conversa do agente em tela cheia numa coluna de leitura) renderizam o Markdown dos agentes: títulos, listas, negrito, código inline, blocos de código em caixa com a linguagem, tabelas; linhas de ferramenta ficam apagadas. Cada agente tem uma cor (automática, ou `:color`/"Color of …" com dez cores sem vermelho) que pinta suas linhas no log e seu nome no mapa, na tabela e no console; avisos e erros continuam vermelhos (decidido em 04/11/2026).
* **Uso e economia:** cada agente mostra o contexto ocupado (%), os tokens e o custo quando o provedor informa (console, tabela F2, total no cabeçalho). Passando de 85% do contexto (`:compact`), o agente continua no fim do turno com o briefing e um resumo da conversa sem as linhas de ferramenta; o briefing lista os colegas em uma linha cada e pede respostas curtas. O log marca o que o operador digitou (`you›agente`, em branco) separado do que o agente disse (na cor dele), e o anúncio falado de permissão diz o nome do arquivo, não o caminho, e omite flags (decidido em 04/11/2026).
* **Temas:** cinco temas de cor (mono, âmbar, verde fósforo, nord, claro) com `:theme` ou pela paleta, salvos entre execuções; o layout não muda e o vermelho continua só para o que é urgente (decidido em 04/11/2026, ADR 0021).
* **Startup:** ao abrir, a sala liga como um CRT, mostra um checklist com o que o runtime realmente achou (provedores, sandbox, voz, modelo do pet, projetos) e o pet acorda; qualquer tecla pula e `:intro off` desliga (decidido em 04/11/2026, ADR 0022).
* **Agentes globais e memória:** um agente pode ir para a biblioteca (`:save`) e ser usado em qualquer projeto com a mesma persona, provider, modelo, cor e regras de permissão, que ficam sincronizados entre as cópias; cada agente tem uma memória em Markdown (`memory/<id>.md`) que ele escreve com a ferramenta `remember` (ou o operador com `:remember`), e as notas mais recentes entram no briefing (decidido em 04/11/2026, ADR 0023).
* **Grupos multi-projeto:** um grupo pode abranger outros projetos (`:attach API: ~/code/site`); cada agente do grupo ganha um worktree (ou a própria pasta, sem git) em cada projeto anexado, que chega à CLI como diretório adicional (ACP `additionalDirectories`, `--add-dir` no Claude Code, ferramentas da API, sandbox e broker), aparece no briefing, e o `:merge` faz o merge em cada projeto (decidido em 04/11/2026, ADR 0024).
* **Desktop:** o app se instala por usuário (`scripts/install.sh`: binário `orchestrator`, ícone e entrada no menu), roda no próprio escopo systemd para o portal reconhecer o atalho global, e tem ícone na bandeja; com a bandeja, fechar a sala só a esconde (decidido em 29/10/2026, ADR 0019).
* **Idioma do código:** tudo em inglês — identificadores, módulos, comentários, logs e textos da UI. Os nomes em português deste spec são só descritivos; no código use o glossário abaixo (decidido em 28/10/2026).

  | Spec | Código |
  | --- | --- |
  | `Projeto { nome, raiz }` | `Project { name, root }` |
  | `Grupo { projeto_id, nome, objetivo, regras, max_ativos }` | `Group { project_id, name, goal, rules, max_active }` |
  | `Agente { grupo_id, nome, task, persona, provedor, worktree, estado, regras }` | `Agent { group_id, name, task, persona, provider, worktree, state, rules }` |
  | `Provedor::{CliAcp { comando }, CliMcp, Api { servico, modelo, chave_ref }, Local}` | `Provider::{CliAcp { command }, CliMcp, Api { service, model, key_ref }, Local}` |
  | `EstadoAgente::{Parado, NaFila, Rodando, AguardandoPermissao, Concluido, Erro}` | `AgentState::{Stopped, Queued, Running, AwaitingPermission, Done, Error}` |
  | `PedidoPermissao { agente_id, ferramenta, entrada, resumo_falado, criado_em }` | `PermissionRequest { agent_id, tool, input, spoken_summary, created_at }` |
  | `Decisao::{Permitir, PermitirSempre, Negar { motivo }}` | `Decision::{Allow, AlwaysAllow, Deny { reason }}` |
  | `RegraPermissao { ferramenta, padrao, efeito }` / `Efeito::{Permitir, Negar, Perguntar}` | `PermissionRule { tool, pattern, effect }` / `Effect::{Allow, Deny, Ask}` |
  | `AgentRunner::{iniciar, enviar, eventos, parar}` / `EventoAgente` | `AgentRunner::{start, send, events, stop}` / `AgentEvent` |
  | módulos `modelo`, `orquestrador`, `voz/{stt,tts,comandos}`, `persistencia`, `descoberta`, `ui/sala` | `model`, `orchestrator`, `voice/{stt,tts,commands}`, `persistence`, `discovery`, `ui/room` |
  | lib do crate `core` | `orch_core` (o pacote continua `core`) |
