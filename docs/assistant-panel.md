# Assistant panel design

**Status:** proposal for founder review, 9 November 2026. Nothing here is built. Estimates are agent hours. Related: [LLM integration](llm-integration.md), [datasheet assistant](datasheet-assistant.md), [live MCP](development.md#live-mcp), [board geometry for an LLM](llm-geometry.md).

## Decisions this design follows

- **MCP is the only AI surface.** That means its typed commands, its permission gate, the reviewable turn and undo. The in-app panel is a client of that surface and has no path of its own.
- **The panel has two kinds of connection:**
  - An **installed agent over ACP** (Claude Code, Codex, Gemini CLI and others). The agent brings its own model and login.
  - A **model endpoint** through the provider abstraction in `crates/assist`. This includes local OpenAI-compatible servers such as Ollama, LM Studio, vLLM and `llama-server`.
- **Run limits:** runs have token, request and time limits.
- **Workflows are MCP prompts,** such as datasheet → component. They run the same way in the panel and in any other MCP client.
- **LLM-first, small models included.** The panel works with small local models that have no vision.
- **Earlier decisions still apply:**
  - Edits are auto-allowed inside a reviewable turn. Files and publishing ask.
  - The app doesn't detect models. You enter the agent's settings, as in Solidcraft.
  - Prompts and harnesses per model can be edited.
  - We build our own panel. Solidcraft's `agent_chat` is a reference only.

## 1. One request path

Every request the panel sends is an MCP `tools/call`. The server code that answers it, `mcp::Server` in live mode, is the same code that answers Claude Code connected through `pcbcraft-cli mcp --live`. Each call reaches the window's pump (`serve_live`) and then:

1. `live::admit` applies the window-only list, the busy refusal, the revision check and the gate.
2. `live::run` runs it with `Session::lend` and then `Session::respond` on the window's engine.
3. The window labels the change and adds it to the undo history.

The panel never calls the engine directly.

```
           ACP agent (own process)                      Panel, endpoint backend (worker thread)
                   │ MCP over stdio                                  │ MCP JSON-RPC values, in process
       pcbcraft-cli mcp --live --port P --token-file F               │
                   │ mcp::Server (live)                     mcp::Server (live)
                   │ live::Client ── TCP, loopback, token          live::Client ── in-process channel
                   ▼                                                 ▼
             panel channel ───────────────► window pump (UI thread, between frames) ◄─── public channel
                                              live::admit: window-only · busy · revision · gate        (Help › Connect AI
                                              live::run: Session::lend → Session::respond             assistants…, external
                                              turn bookkeeping · labels · toasts                       MCP clients)
```

### Connection kinds

**ACP agent.** The agent starts our MCP server itself, because we list it in `session/new` (§2). That server is the existing `mcp --live` bridge. It connects to a **panel channel**: a second `live::Endpoint` on its own loopback port, with a fresh token per conversation. The token is in a file only the user can read, inside a private temporary folder, and never in the agent's arguments or environment. Solidcraft's agent chat uses this pattern (`listen_agent`). A second channel is needed because the window must know which requests belong to the panel's running turn. "Let AI assistants edit this project" in Help › Connect doesn't have to be on: starting a conversation is the consent, and the channel closes when the conversation ends.

**Model endpoint.** The panel's worker thread runs the tool loop (§3) and is an MCP client of an in-process `mcp::Server::live`. The only new part is the transport. `live::Client` today is a TCP connection, and it gains an in-process variant that sends the same `ControlRequest` (method, params, reply sender) into the panel channel's queue. Tool names, descriptions, server instructions, prompts, compact replies and refusals are therefore byte for byte what an ACP agent or Claude Code gets. The worker thread never touches the engine.

### What applies the same way to every request

| Rule | Where it lives | Panel (either kind) | External live client |
|---|---|---|---|
| Typed command and its schema | `Request` catalogue, `mcp::live_tools` | Same | Same |
| Window-only (`project.new`, `.open`, `.save`) | `live::admit` | Refused, `live.window_only` | Same |
| Busy (typing, dragging, menu, dialog, review) | `live::admit` | Refused, `live.busy`; reads run | Same |
| Revision | `admit`, and again in `Session::execute` | `stale_revision`, `live.revision_required` | Same |
| Permission gate | `Session::check_request` with the window's `Policy` | Run, Ask or Refuse | Same |
| Ask | `Verdict::Ask` | An Allow/Deny card in the conversation | The window's "AI assistant asks" prompt |
| Failed mutation | Engine | Document unchanged | Same |
| Reviewable turn | Window pump and engine (below) | One undo step per message | One step per edit, as today (Q4) |
| Undo label | Engine step label (below) | "AI assistant: *first line of message*" | "AI assistant edit: *subject*" |

### The reviewable turn

A **turn** is one message, from Send until the agent's `session/prompt` reply or the endpoint loop's last reply. The panel opens it on the panel channel. While the turn runs:

- **Edits run** under Auto-allow edits. Each one is its own engine step, so the agent's own `project_undo` works. An undo past the turn's first step is refused (`assistant.undo_outside_turn`).
- **Late requests are refused.** A panel-channel request that arrives outside a running turn is refused (`assistant.no_turn`), and so is one that arrives after Stop (`assistant.turn_ended`). A shared validity flag is checked before a request is admitted and again before it runs, as in Solidcraft.
- **Per-turn limits** (§3) are counted in the pump for both connection kinds, so an ACP agent has the same edit and request caps as an endpoint model.
- **At the end of the turn,** the engine folds the turn's steps into one with `Engine::join_since(revision_at_start, label)`. This is a generalisation of today's `join_last_step`. The per-turn edit cap stays below `MAX_UNDO` (100), so trimming can't cut into a turn before it is folded.
- **If you edit by hand during a turn,** for example by clicking between two assistant calls, your step is never folded into the assistant's. The turn becomes segments, each its own undo step ("AI assistant: … (1 of 2)"). The next request the assistant sends gets a note that the project changed by hand (§5 context).
- **Labels belong to steps in the core.** Undo entries gain an optional label. Undo, the toast, Messages and the MCP `project_undo` reply then all name the same thing. This replaces the app-side `live.edits` revision map.

**"Reviewable"** means the turn shows a **summary card**:

- what changed, by collection and by name, taken from each reply's `changes`;
- new and resolved issues, taken from each reply's `findings`;
- **Show**, which selects every changed object in the shared selection, so it works in 2D, 3D and every workspace;
- **Undo turn**, while the turn is still the latest step.

A run that works on a copy and offers the result as an accept-or-reject diff (`review-diff`) is a later slice (§7, S10).

## 2. ACP agents

Field names follow the [ACP specification](https://agentclientprotocol.com): protocol version `1`, JSON-RPC 2.0 over the agent's standard input and output.

### Launching and configuring

- **Adding an agent.** You add an agent in Settings › AI › Agents with a name, a command, its arguments and its environment variables. It runs without a shell. Presets fill these fields with known command lines (`claude-agent-acp`, `codex-acp`, `gemini --acp`, `opencode acp`). They are templates you edit: Pcbcraft doesn't probe PATH or detect anything (Q5).
- **Test.** **Test** starts the agent, sends `initialize` and shows `agentInfo`, `authMethods` and `agentCapabilities`.
- **The process.** The agent runs in its own process group. Stop, closing the panel, closing the window and quitting all end it the same way: close its stdin, then after a grace period send TERM, then KILL. This is Solidcraft's `process.rs` design, written again for us.
- **`initialize`.** We send `{protocolVersion: 1, clientCapabilities: {fs: {readTextFile: false, writeTextFile: false}, terminal: false}, clientInfo: {name: "pcbcraft", title: "Pcbcraft", version}}`. We offer no file system and no terminal, because the agent works through our tools. We read `agentCapabilities.promptCapabilities` (`image`, `audio`, `embeddedContext`), `agentCapabilities.mcpCapabilities`, `agentCapabilities.loadSession` and `authMethods`.
- **Sign-in.** Login is the agent's own, and Pcbcraft never holds an ACP agent's key. When the agent answers `-32000` "Authentication required" (on `session/new` or on the first `session/prompt`), the panel shows the agent's `authMethods`:
  - a terminal method appears as its command with Copy;
  - an agent method gets a button that calls `authenticate {methodId}`;
  - an environment method names the variable to set.
- **`session/new`.** We send `{cwd, mcpServers}`:
  - `cwd` is an empty working folder of Pcbcraft's own (`<data>/assistant/work/<conversation>`), not the project's folder.
  - `mcpServers` has one stdio entry: `{name: "pcbcraft-<random>", command: "<absolute path of pcbcraft-cli beside the app>", args: ["mcp", "--live", "--port", "P", "--token-file", "F"], env: []}`. Stdio is the transport every ACP agent must support, so we don't depend on `mcpCapabilities.http`. The server name is random per conversation, so a permission answer can't apply to some other server.
- **Model and options.** We read the agent's `configOptions` (model, reasoning effort, mode) from the session reply and change them with `session/set_config_option {sessionId, configId, value}`. The chosen value is remembered per agent.
- **System text, the ACP harness.** It goes in `systemPrompt: {mode: "append", content: [text]}` when the agent advertises `sessionCapabilities.systemPrompt`. Otherwise it is the first text block of the first `session/prompt`.
- **The agent's own tools.** Where an adapter offers a way to turn off its own shell, file and web tools, the agent's profile sets it. Claude's adapter takes `_meta.claudeCode.options.disallowedTools`. That is an adapter extension, not part of the spec, so it is per profile and checked per adapter version.

### Streaming

`session/update` notifications are drawn as they arrive:

| `sessionUpdate` | Shown as |
|---|---|
| `agent_message_chunk` | Reply text (Markdown) |
| `agent_thought_chunk` | Dimmed and folded reasoning |
| `tool_call`, `tool_call_update` | A tool row with `title`, `kind`, `status` (`pending`, `in_progress`, `completed`, `failed`) and `rawInput` |
| `plan` | A checklist, replaced as a whole |
| `usage_update` | A context meter from `used` and `size`; other fields are ignored |
| `config_option_update` | Replaces the option snapshot |

**Two views of one tool call.** For our own tools the panel channel is authoritative: it sees the exact command, its gate decision, its outcome and its changes. The agent's `tool_call` for the same call is folded into that row when the tool name and `rawInput` match. Agent tool calls that don't match get rows of their own.

### Cancellation

**Stop**, or Esc in the panel:

1. sends `session/cancel {sessionId}`;
2. answers every open `session/request_permission` with `{outcome: {outcome: "cancelled"}}`, as the spec requires;
3. declines every held Ask;
4. invalidates the turn flag, so later panel-channel requests are refused;
5. folds the turn's completed steps into one undo step;
6. waits for the `session/prompt` reply, which should have `stopReason: "cancelled"`.

If the agent hasn't answered after 10 s, the panel offers **End agent**. Other stop reasons (`max_tokens`, `max_turn_requests`, `refusal`) are reported plainly and never treated as an answer.

### Approvals

- **Questions about our tools.** When the agent sends `session/request_permission` for a tool of its `pcbcraft-<random>` server, the panel answers `allow_once` automatically and leaves an *Auto-allowed* note. Our gate decides the call when it arrives, so the agent's question adds nothing. A setting turns this off.
- **Questions about the agent's own tools** appear as a card with the agent's `options` (`allow_once`, `allow_always`, `reject_once`, `reject_always`).
- **Our gate's Ask** (an export, a file, publishing) appears as an Allow/Deny card in the conversation. It shows the command, the paths it names and why. A held Ask is declined just before the channel's `ASK_WAIT` runs out, and the model gets `gate.denied`.

## 3. Endpoints

### Provider abstraction

`crates/assist/src/model` already has what the panel needs:

- a neutral conversation (`Message`, `Part`, `ToolSpec`, `ModelRequest`, `ModelReply`) and the `ModelBackend` trait;
- Anthropic Messages and OpenAI-compatible backends;
- keys from the keychain for each base URL, refused over plain HTTP to another computer;
- recording and replay.

The panel adds three things:

- **Streaming,** so text, reasoning and tool calls arrive as they are made (`stream: true` on both wire formats). Cancel is checked between chunks, which fixes today's "Cancel after the current request" limit.
- **A general agent loop,** taken out of `run.rs`. Its input is the conversation, the tools from MCP `tools/list` (converted from `inputSchema` to `ToolSpec`), the profile and the limits. Each tool call becomes an MCP `tools/call` on the in-process client. The loop's events (text, reasoning, tool started and done, usage, stop) drive the panel. The datasheet runner's tool-specific code moves to MCP (§4).
- **Endpoint settings** in Settings › AI › Endpoints: provider, base URL, model id, key (masked, stored in the keychain) and request timeout. The capabilities are the ones you declare: tool calling, vision, native PDF, context size and output limit. Nothing is detected (`ModelProfile`).

### Profiles and harnesses per model

A run profile (`profile.rs`, `<config>/assist-profiles/NAME.json`) says what a model is told and how the loop reacts. The panel uses the same files and extends the format:

| Field | Today | Panel |
|---|---|---|
| `system` (`base`, `add` or `replace`) | Datasheet prompt | Panel base prompt; per-model additions |
| `omit_tools` | Assist tool names | MCP tool names |
| `harness.repeat_limit` | 3 | Same, for the panel loop |
| `prompts.<name>.add` | — | Paragraphs appended to one MCP prompt for this model (§4) |
| `instructions` | — | `full` or `short` server instructions (below) |
| `context_budget` | — | Tokens kept for the context preamble (§5) |

**For ACP agents,** a profile carries only the system text and adapter options, because the agent runs its own loop.

**Editing.** Settings › AI › Profiles lists the built-in and user profiles. **Copy** writes the built-in to the config folder, and then the file is edited in a text field. The file is checked when it is saved, with the same refusals `llm-profile` gives today. A profile that fails the check is never used. The built-in `claude`, `default` and `qwen3.8-flash-next` profiles carry over, and the tuned Flash Next additions move with the datasheet prompt.

### Local models, small context and no vision

- **Size of what is sent.** Today the server instructions are about 16 KB (`mcp.rs`, LIVE plus base), and the tool list has 40 commands with generated schemas. On a model with 32 k of context that is a large share of the window. Two measures:
  - `instructions: "short"` sends a one-paragraph orientation and points to `project_schema`;
  - with `tools: "compact"`, each tool is offered with its one-line description and its schema is fetched on demand through `project_schema`, which exists already.

  Both are measured on Flash Next and `qwen3.8:27b` before they become defaults (S4).
- **Compact replies** (`session/reply.rs`) already keep mutations small. A via on the demo board answers in under 4 KB.
- **When the context fills,** nothing is cut silently. The history only grows, as it does in the datasheet runner, because a rewrite breaks signed thinking and the cache. When the next request can't fit with 4 000 tokens of room, the turn stops and says why, and the panel offers **New conversation from a summary** (Q7). The truncation warning (the server counted fewer prompt tokens than the last request) carries over.
- **No vision.** Attaching an image is refused with the reason. PDFs reach the model as text through the document tools (§4). Board geometry goes as text views ([llm-geometry](llm-geometry.md)). Features that need vision say so instead of guessing.
- **No tool calling.** The endpoint is refused for the panel, with the reason: a model that can't call tools can't use the one request path.

### Limits

The limits, for every model local or hosted, are set in Settings › AI with these defaults (Q10):

| Limit | Per turn | Enforced |
|---|---|---|
| Model requests | 40 | Endpoint loop |
| Output tokens | 200 000 | Endpoint loop, worst case checked before each request |
| Prompt plus output tokens | 4 000 000 | Endpoint loop |
| Time | 30 min | Endpoint loop and panel (ACP) |
| Edits that run | 60 (below `MAX_UNDO`) | Panel channel, both kinds |
| Tool requests of any kind | 400 | Panel channel, both kinds |

**Output limits** still come from the model's declared limit, not from a small fixed number. **When a limit is reached,** the model gets a tool result saying so and has one more request to say what is left. An ACP agent's tokens are its own business, so we show `usage_update` but can't cap its tokens. The edit, request and time caps hold for it because they are counted in our channel.

## 4. Workflows as MCP prompts

`mcp.rs` gains the `prompts` capability: `prompts/list` and `prompts/get {name, arguments}`, as the MCP specification defines them. Arguments are strings. A prompt returns `messages`, a user message whose text names our tools. Built-in prompts live with the automation crate as plain data. They are the same for every client, so Claude Code (where they appear as slash commands), any other MCP client and the panel all start the same workflow from the same text.

**In the panel,** prompts appear as starting points in the empty conversation and behind the **/** key. Choosing one shows its arguments as a short form. The panel calls `prompts/get` and sends the returned text as the message: to the endpoint loop, or as `session/prompt` text blocks to an ACP agent. Model-specific tuning goes in the profile (`prompts.<name>.add` for endpoints, system text for agents), never into the prompt itself.

| Prompt | Arguments | What it does |
|---|---|---|
| `datasheet_to_component` | `datasheets` (paths, one per line), `part_name`, `notes` | The datasheet assistant's goal and method. It drafts a component, symbol, package, footprint and binding in the open project and publishes nothing (Q2) |
| `fix_issues` | `scope` (selection, sheet, board), `codes` | Read `project.issues`, fix what a typed command can, explain the rest |
| `review_schematic` | `sheet` | ERC and pin-report review, and explanation only (reads) |
| `route_nets` | `nets`, `layers` | Route through `layout.route_step` with shove |
| `explain` | `object` | Explain the selected object, rule or issue |

### Making the datasheet workflow an MCP workflow

The datasheet runner relies on things a plain MCP client doesn't have yet, and they move into the server, so every client benefits:

- **Document tools,** class Read, each with a path, so the folder limit decides as for any path:
  - `document.text {path, pages}` returns page-marked text and lists probable drawings, using `documents.rs` (`pdf-extract`);
  - `document.view_page {path, page}` returns an MCP image content block, using `hayro`.

  The 24-view and size limits stay.
- **The working pinout document** becomes server-side state for each `Session`, with `library.pinout_edit` as a command. Today it is held by the runner (`library_pinout_edit`, preview, then apply the previewed document).
- **Argument leniency** moves from the runner into `mcp.rs` `tools/call`: arrays or objects sent as JSON text, large integers as text, and refusals that say where invalid JSON broke.
- **`submit_review`** becomes a `review.submit` query that checks the report: it refuses a report citing an unlisted document or no page. The panel draws the report as a card, and **Save review…** is an Ask (Q3).
- **Repeat stop.** The repeat limit stays in the endpoint loop, because an ACP agent runs its own loop.

**After this,** `pcbcraft-cli design-part` is the same prompt run by the same endpoint loop on a headless session. The 16 Flash Next tuning recordings, run 1 and the TLV replays must still grade the same (S9).

**Coordination:** the `assist-to-mcp` agent is working in this area now. This section is the target shape, to agree with that work, not a second implementation.

## 5. UI

### Where it lives

- **The column.** The panel is a docked column at the window's right edge, outside the right column, so the subject card and Issues stay visible while you review a turn (Q1). Drag its header to dock it at the left beside the Browser. It is resizable from 300 to 560 pt.
- **Folding.** **Minimise** folds it to a 30 pt strip with a state dot and a count of open questions. × closes it and keeps the conversation.
- **Solidcraft conventions.** Side, width and fold are user settings, not project data. The panel is built from `ui_kit::Container` (enforced by `cargo xtask ui-containers`), with shared tokens.
- **Opening it.** The `assistant.toggle` command (⇧⌘A or Ctrl+Shift+A, through the shortcut registry, which works from the panel's own text field), View › Assistant, the command palette, and an AI button in the application bar beside today's *AI access* indicator.
- **One conversation per open project,** in memory only. Replacing the project (New, Open, recovery) ends it, and its agent stops.

### Conversation items

- **Your message,** with attachment chips and the context line it carried (for example "Selection: R3, R6 · Board 2D").
- **Reply text** as bounded Markdown, and reasoning dimmed and folded.
- **A tool row for each call:** the command (`layout.route_step`), a one-line summary, and its state (running, done, refused with the refusal's text, or denied). Expanding a row shows the input and the compact reply. A refusal is never shown twice.
- **Ask card:** "Export Gerbers to ~/boards/out?" with the command, the paths and why, and **Allow** (Enter) and **Deny** (Esc).
- **Summary card at the end of each turn:**

  ```
  ┌ Turn · "route the USB pair" ─────────────────────┐
  │ Changed  4 tracks · 2 vias · net USB_D+ USB_D−   │
  │ Issues   2 resolved · 1 new (clearance U1 ↔ J1)  │
  │ Requests 9 · 31 k tokens · 2 min                 │
  │                    [Show]  [Undo turn]           │
  └──────────────────────────────────────────────────┘
  ```

  The new issue links into Issues, which locates it. Tokens are shown for endpoints, and context from `usage_update` for agents.
- **No file diffs.** ACP `diff` content (`path`, `oldText`, `newText`) is for text files and doesn't fit a design. Review is the change list, the canvas selection and the issues. Large changes use the docked impact review (D9 in [sidebar IA](sidebar-ia.md)) or, later, review on a copy.

### Attachments

Attach by dropping a file on the panel, with the paperclip, or by pasting an image. A file is copied into the conversation's folder, which is one of the policy's allowed folders, so our document tools can read it.

| Attachment | ACP agent | Endpoint model |
|---|---|---|
| PDF | `resource` with `blob` (base64) and `mimeType: application/pdf` when `embeddedContext` is advertised; otherwise `resource_link {uri: file://…, name, mimeType, size}`. Either way, the message names the path for `document.text` and `document.view_page`, which work for every agent | Native PDF when declared; otherwise the message names the path and the model reads it through the document tools |
| Image (screenshot) | `image {data, mimeType}` only when `promptCapabilities.image` is set | Only when vision is declared |
| Anything else | Refused with the reason | Refused with the reason |

Datasheets and other documents are untrusted. They are titled as data and never as instructions, and the gate holds whatever they say.

### Context sent with each message

The live channel gets one new read, `window.context`, and the panel puts its answer at the head of each message, sized by the profile's `context_budget`. Any live client can call it too, so this isn't a panel-only path. It returns:

- the workspace and view (sheet, board, 2D or 3D, visible layers);
- the selection with the ids its tools take;
- open issue counts for what is in view;
- the steps made by hand since a given revision.

### Main states

```
 Not set up         Connecting           Sign-in needed               Ready
┌──────────────┐  ┌──────────────┐   ┌───────────────────────┐   ┌─────────────────────────────┐
│ Assistant    │  │ Claude Code ▾│   │ Claude Code ▾         │   │ qwen3.8 (local) ▾  ⋯   –  × │
│              │  │              │   │ Not signed in.        │   │ Start from:                 │
│ No agent or  │  │ Starting…    │   │ Run in a terminal:    │   │  Create part from datasheet │
│ endpoint yet.│  │              │   │ claude-agent-acp --cli│   │  Fix issues on this board   │
│ [Settings…]  │  │ [Cancel]     │   │  auth login  [Copy]   │   │  Review this schematic      │
└──────────────┘  └──────────────┘   │ [Connect again]       │   │ ┌─────────────────────────┐ │
                                     └───────────────────────┘   │ │ Message…        📎  [↑] │ │
                                                                 │ └─────────────────────────┘ │
                                                                 └─────────────────────────────┘
 Running                                Waiting for you              Stopped or limit reached
┌───────────────────────────────┐   ┌──────────────────────────┐   ┌──────────────────────────┐
│ you: route the USB pair       │   │ … tool rows …            │   │ Turn stopped: 40 model   │
│ ▸ reasoning (folded)          │   │ ┌ Allow export? ───────┐ │   │ requests (the per-turn   │
│ ✓ project_inspect             │   │ │ fabrication.export   │ │   │ limit). 6 edits kept as  │
│ ✓ layout.route_step  +2 trk   │   │ │ ~/boards/out/        │ │   │ one undo step.           │
│ ◌ layout.route_step …         │   │ │ [Deny]      [Allow]  │ │   │ [Show] [Undo turn]       │
│ Routing D− next to D+…        │   │ └──────────────────────┘ │   │ [Continue]               │
│ ● 9 req · 31 k tok    [Stop]  │   │ ● waiting      [Stop]    │   │                          │
└───────────────────────────────┘   └──────────────────────────┘   └──────────────────────────┘
```

Other states:

- **Agent ended.** The process exited, with its last standard error lines and **Restart**.
- **Endpoint unreachable.** The HTTP error, with the key redacted.
- **Busy refusals** don't need a state of their own. They are tool rows, and the model retries.

## 6. What exists and what is new

| Reused as is | Where |
|---|---|
| Typed engine, catalogue, classes, compact replies | `pcbcraft-core`, `crates/automation/src/session*` |
| MCP server, live mode, `live_tools` | `crates/automation/src/mcp.rs` |
| Live channel: `Endpoint`, `admit`, `run`, `Client`, `--live` bridge | `crates/automation/src/live.rs`, Solidcraft's control channel |
| Gate: `Policy`, `Mode`, `decide`, `check_request`, refusals | `crates/automation/src/gate.rs` |
| Window pump, busy rule, Ask prompt, AI indicator, toasts | `apps/pcbcraft/src/app/live_mcp.rs` |
| Model layer, backends, keys, endpoints, recording | `crates/assist/src/model*` |
| Run profiles, harness fixes | `crates/assist/src/profile.rs`, `profiles/` |
| PDF text and page rendering | `crates/assist/src/documents.rs` |
| Undo fold (`join_last_step`) | `pcbcraft-core` `Engine` |
| Shortcut registry, palette, `ui_kit` containers, toasts, Messages | app |

| New | Size |
|---|---|
| `Engine::join_since` and labels on undo steps | Small, in the core |
| Panel channel: a second endpoint, a turn flag, per-turn counters, turn-scoped refusals, `--token-file` | Automation and app |
| In-process `live::Client` transport | Automation |
| MCP `prompts`, document tools, server-side pinout document and argument leniency, `review.submit`, `window.context` | Automation |
| General endpoint loop with streaming and cancel; tools from `tools/list`; profile extensions | Assist |
| ACP client: process group, `initialize`, auth, `session/new`, updates, permissions, cancel, config options | New module in the app, or a small crate beside assist |
| Panel UI, Settings › AI | App |

**Reference only, no code copied:** Solidcraft's `agent_chat` (design doc `docs/design/in-app-agent-chat.md`) and tangency-labs/pcbcraft's `craft-agents` and `app/assistant`, both MIT/Apache. Both run the API loop by starting their CLI's MCP bridge and acting as its client. That confirms the single-path design, and our in-process transport avoids starting a subprocess for endpoints. Both show that the panel channel needs a turn validity flag and random server names.

## 7. Slices

At most five agents at once. Each slice has its own tests and ships on its own.

| # | Slice | Delivers | Estimate | Depends on |
|---|---|---|---|---|
| S1 | Core: `join_since`, step labels, segments around hand edits | One undo step per turn; labels in undo, Messages and MCP | 2–3 h | — |
| S2 | Automation: panel channel, in-process client, turn flag and counters, `--token-file`, `window.context` | The one path, tested headless with a scripted client | 4–6 h | — |
| S3 | MCP prompts and the datasheet workflow's server parts | `prompts/list`/`get`, document tools, pinout working document, leniency, `review.submit` | 5–8 h | Agreed with `assist-to-mcp` |
| S4 | Assist: general loop, streaming, cancel, tools from MCP, profile extensions, compact instructions and tools measured | Endpoint backend without UI; replay tests | 6–9 h | S2 interface |
| S5 | Panel UI on the endpoint backend: column, fold, transcript, tool rows, Ask card, summary card, states, ⇧⌘A | Chat in the app on a local model | 8–12 h | S1, S2, S4 |
| S6 | ACP backend | Claude Code, Codex, Gemini CLI or OpenCode in the panel | 6–9 h | S2, S5 shell |
| S7 | Settings › AI: agents, endpoints, keys, permission mode, limits, profile editor | Setup in the app | 4–6 h | S4 |
| S8 | Attachments | PDFs and images per capability | 2–4 h | S3, S5 |
| S9 | Datasheet prompt end to end; `design-part` on the same loop | Same graded results as today on the recordings | 4–6 h | S3, S4 |
| S10 | Review on a copy (accept or reject, `review-diff`) for large runs | Proposal flow | 4–6 h | S5 |
| S11 | VM evidence: local model through an endpoint, and an ACP agent on a local model | Recorded runs in the macOS VM | 2–3 h | S5–S8 |

**Order:** S1, S2, S3 and S4 in parallel, then S5 and S7, then S6, S8 and S9, then S10 and S11. That is about **47–72 agent hours** in total. It replaces slices 3–6 of the [LLM integration build order](llm-integration.md#build-order).

### Risks

- **ACP adapters change fast.** Package names have moved before (`@zed-industries/*` to `@agentclientprotocol/*`), and some options are adapter extensions (`_meta.claudeCode…`). Mitigation: options live in editable profiles, and a fake ACP agent in the tests pins the protocol we speak.
- **Agents' MCP tool timeouts** may be shorter than our 600 s `ASK_WAIT`, so a slow Allow could reach the agent as a failed call. Measure per agent, and decline before its timeout.
- **Folding tool calls into channel rows** by name and `rawInput` is a heuristic. An unmatched call shows twice (once as narration) and is never lost.
- **`pcbcraft-cli` must ship beside the app** in every package (the macOS bundle included). Otherwise an ACP agent has no MCP server, and the panel says so.
- **Streaming cancel with ureq.** A blocking read only ends at a chunk boundary or a timeout. Use a short read timeout and check the flag. A local model at about 17 tokens a second produces chunks often enough.
- **Small models and tool-list size.** If even compact tools don't fit 32 k, a model gets a profile-chosen subset (`omit_tools`). Measure before promising support for small contexts.
- **UI-thread execution.** Tool requests run between frames, so a long command, such as a fill, stalls frames for an assistant just as it does for a person. This is existing behaviour, not new risk, but turns make it more frequent.
- **Datasheet parity.** Moving the runner's conveniences into MCP must not lose the Flash Next gains. The replays grade it (S9).
- **Parallel work.** `assist-to-mcp` touches `crates/assist` now, so S3 and S4 start after they merge.

## 8. Open questions for the founder

1. **Panel placement.** A column of its own at the right edge, or replacing the right column while open? **Recommendation:** its own column, so the subject card and Issues stay visible during a turn. It can dock left.
2. **Where the datasheet workflow writes.** Into the open project as drafts (one turn step, nothing published), or into a new project file as `design-part` does? **Recommendation:** the open project in the panel and in live MCP, and a new file on the CLI, unchanged.
3. **Keeping the review report.** **Recommendation:** shown as a card, with **Save review…** as an Ask. The CLI keeps writing `NAME.review.md` and `.json` beside the project. Storing it in the project would change the file format.
4. **Turns for external live clients** (Claude Code through Help › Connect). They have no message boundaries we can see. **Recommendation:** keep one step per edit for them. The request path is the same, and only the grouping differs, because only the panel knows when a message starts and ends.
5. **Agent presets.** Fill-in command lines with no PATH probing, or fully manual? **Recommendation:** presets as templates. This is not detection and keeps setup short.
6. **Permission mode.** One for the window, or an override per agent? **Recommendation:** one window mode with an optional override per agent, which is what Solidcraft does. The default stays Auto-allow edits.
7. **Full context on a small model.** Stop and offer **New conversation from a summary**, or compact automatically? **Recommendation:** stop and offer. Automatic compaction rewrites history silently, which breaks signed thinking and the cache.
8. **Keeping conversations across restarts.** **Recommendation:** not in the first release. Conversations stay in memory, as in Solidcraft, and `session/load` comes later.
9. **Evidence without paid calls.** Running Claude Code over ACP uses your own subscription. **Recommendation:** slice evidence uses a local model, both as an endpoint and behind an ACP agent such as OpenCode with Ollama, plus the fake ACP agent. A Claude Code run happens only with your go.
10. **Default limits.** **Recommendation:** the table in §3: 40 model requests, 60 edits, 4 M tokens and 30 minutes per turn. They are editable, and the same for every model.
11. **The panel and "Let AI assistants edit this project".** **Recommendation:** keep them separate. Starting a panel conversation is consent for its own channel, and the Help › Connect switch stays for external clients, off at every start.
