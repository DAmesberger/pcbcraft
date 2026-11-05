# LLM integration design

**Status:** proposal for founder review, 5 November 2026. Nothing here is built. Estimates are rough agent hours. Related: [board geometry for an LLM](llm-geometry.md), [datasheet assistant](datasheet-assistant.md).

## Requirement

Founder: "this product needs to be an LLM first-class product. So we deeply need to think about seamless integration of LLMs into the app. furthermore, we need to abstract that of course, not only ANTHROPIC but also local LLMs, etc should be possible".

What that means in practice:

- **An LLM can do anything a user can,** through the same commands, in every workspace.
- **It works where you're working:** in the app, with your selection and your open issues, not in a separate tool you copy things into.
- **You stay in control:** you can see what it read and what it changed, and undo it in one step.
- **The model is your choice:** hosted or local, or an agent you already use. No feature is tied to one vendor.
- **Pcbcraft stays a complete editor without any LLM.** Nothing requires one, and the app works offline.

## What exists today

| Piece | State | Where |
|---|---|---|
| One typed engine for UI, CLI and MCP | Built | `crates/automation` |
| MCP server | Built; replies being compacted by `gap-mcp` | `crates/automation/src/mcp.rs` |
| Datasheet assistant | Built: a tool loop, a USD cap, recording, review report. **Anthropic only:** `api.rs` hard-codes the endpoint and headers, and `request_body` and the PDF blocks are Messages-shaped. Its `Transport` trait is a useful seam but sends that shape | `crates/assist` |
| Review of a proposed change | `pcbcraft-cli review-diff` compares two projects | CLI |
| In-app chat | **None** | |

**Solidcraft already has an agent chat** (`crates/ui-egui/src/agent_chat`, about 20,000 lines with tests, in both our pins). It's worth learning from closely:

- a `ChatBackend` seam the panel talks to, with two backends: ACP (a coding agent the user already has, on their own login, with no keys in the app) and API (OpenAI-compatible and Anthropic-compatible endpoints, including Ollama, LM Studio and OpenRouter presets);
- a gate: every tool request runs, waits for Allow or Deny, or never runs;
- each message's changes merge into one undo step;
- keys come from the system keychain and are refused over plain HTTP to another computer;
- the selection attached as context, and a note when the design changed by hand.

**But it can't be used as it stands.** `mod.rs` imports `SolveApp`, the gate lists Solidcraft's command names, and the API backend starts `solidcraft-cli mcp --connect`. So the panel belongs to Solidcraft's app; only some of the modules are protocol code.

## Architecture

```
Panel  Datasheet flow  Issue fixes  Routing  ...   (tasks)
          │
     Agent run: goal, tools, budget, recording, events
       │              │                │
   Model layer     Tool gate        Context
   (providers)         │          (selection, view,
       │          Typed engine     issues, changes)
  ┌────┼─────┐    (one command
 Anth. OpenAI ACP   surface)
       compat
```

### 1. Model layer

One neutral interface in `crates/assist`, with no vendor shapes above it:

- **Neutral conversation:** messages, tool calls, tool results, images and documents. Each backend converts them to its wire format.
- **Capabilities, declared per model and never assumed:** tool calling, vision, native PDF, thinking, caching, context size, and price if any. Features ask for capabilities instead of checking a vendor's name.
- **Backends:**
  - Anthropic Messages.
  - OpenAI-compatible: Ollama, LM Studio, `llama-server`, OpenAI, OpenRouter, any URL.
  - ACP: an agent the user already has. It runs its own loop and calls our MCP server, so it sits at the run level, not the model level.
- **Setup:** the user enters each agent's settings in Preferences (endpoint, model, key), as in Solidcraft. The app doesn't detect or suggest models.
- **Keys:** from the system keychain, for one connection only, never in a file or log. Refused over plain HTTP to another computer.

**Degrading openly:** a model without native PDF gets rendered pages or extracted text, and the review report says which. A model without vision gets text views only. A feature that can't work on a model says why, instead of giving a worse result silently.

### 2. Agent run

One loop for every task, generalised from `pcbcraft_assist::run`:

- input: a goal, a tool set, a budget and a context;
- output: events for the UI, and a recording that can be replayed offline;
- **budgets:** a USD cap for paid models, and token, time and request caps for all, including local;
- **output limits come from the model, not a small fixed number,** near what it supports, because reasoning counts against them. A cut-off is reported clearly and never treated as an answer;
- every task uses this, so recording, cost and approval work the same way everywhere.

### 3. Tools and gate

Tools are generated from the command catalogue, so nothing is hand-listed and everything goes through the one engine. Each command has a class:

| Class | Examples | Behaviour |
|---|---|---|
| Read | Queries, views, checks | Runs |
| Edit | Route, place, set a value | Runs inside the turn's reviewable change |
| Ask | Open, save or export files, library publish, anything with a path | Waits for Allow or Deny |
| Never | Keys, settings, crash reports | Refused |

The class lives with the command definition, so a new command can't be added without choosing one. Permission modes (ask every time, auto-allow edits, allow everything except Never) are set in Preferences, per model.

### 4. Context: the LLM sees what you see

Sent as short text the model can ask to expand, not as a snapshot:

- the workspace and view: which sheet, which board window, which layers;
- the selection, with the ids its tools take;
- open issues (ERC, DRC, production) for what's in view;
- **what changed by hand since its last reply,** so it doesn't undo your work;
- on request: full views through [the geometry queries](llm-geometry.md), sized by the model's context.

Part of this is budgeting: replies and views shrink to fit a small model's context, and the same conversation stays usable on a local model.

### 5. Proposals, not surprises

- Every message's changes merge into **one undo step,** labelled with the message.
- **Changes show on the canvas as they arrive,** with a summary card in the conversation: what changed, new and resolved issues, and cost.
- **Review:** a run on a large task, such as routing a board, can work on a copy and offer the result as a diff to accept or reject, using `review-diff`. Small edits apply directly, because stopping for every track would be tedious.
- **Reasons stay attached:** a routed net or chosen part can show why it was done that way.

Library publishing stays a person's decision, as the datasheet assistant does today.

### 6. Where it appears

- **A docked panel,** in shared UI containers, available in every workspace, one conversation per open design.
- **Right-click or Issues:** "Ask about this", "Fix this", with the object already attached.
- **Task entry points:** Create from datasheet, Review this schematic, Route these nets, Explain this rule. They're the same run with a preset goal and tool set, not separate features.
- **Command palette:** the same request, one keystroke away, through the shortcut registry.
- **A status line while running,** with Stop, spend and the current step.

### 7. Safety

- **Datasheets and other documents are untrusted.** Text in a PDF is data, never instructions, and the gate holds regardless of what a model was told. A run can't publish, write files or read outside its folder without Allow.
- **Clearly show where data goes:** the panel names the configured agent and whether its endpoint is local.
- **Failed commands leave the document unchanged,** which holds for LLMs like everyone else.
- **Automation and evidence:** recordings exclude keys. Any new endpoint is loopback-only with a fresh token.

### 8. Quality across models

A feature isn't "LLM-first" if it only works on the top model:

- **An eval per task** on recorded fixtures, graded by the engine: datasheet to part, ERC fix, net routing.
- **A measured model table:** each task on each model, with success, cost and time. It decides what the app recommends and what it warns about.
- Fixtures run offline in CI through replay. Live runs are on demand.

## Local models: first measurements

On this Mac (M5 Max, 137 GB), with other builds running, so timings are rough:

| Test | Result |
|---|---|
| `qwen3.8:27b` through Ollama's OpenAI-compatible API, with an output limit of 800 tokens | Stopped at the limit before reaching a tool call, 41 s. That limit was my test's mistake, not a property of the model |
| Same, with a limit of 6,000 | **Correct tool call,** with `net`, `from`, `to`, `layer` and `via` right, and it read the grid correctly. It finished in 806 tokens, 47 s, about 17 tokens per second |
| `Qwen3.8-Flash-Next` | **Not run yet.** It's the founder's suggestion: 88 GB as GGUF in the Hugging Face cache, not in Ollama, so it would run through `llama-server`. A vision projector file is present. I'll test it once fewer builds are running |

That's one sample, and the prompt was simple. It shows the path works, not that routing works. It also invented a mapping for C3, which wasn't in its legend, so refusals and clear legends matter.

## Reusing Solidcraft's chat

**Decision (founder, 5 November 2026): we don't use it; we build our own.** "if it is not exposed, we should not use it and do it ourselves. we might have different requirements anyways". Solidcraft's chat isn't exposed for other apps, so there's no fork change and no dependency on it. Its documented design is useful as a reference for what such a system needs, such as the gate, one undo step per message and keychain handling.

The Pcbcraft `Transport` and `Settings` types get replaced, with no compatibility layer needed before the first release.

## Build order

With at most 5 agents at once. Each slice is usable on its own.

| # | Slice | Delivers | Estimate |
|---|---|---|---|
| 1 | Model layer; port the datasheet assistant onto it | Create from datasheet on a local model, with a recorded comparison against Opus 5.5 | 6–9 h |
| 2 | Command classes and gate | A class on every command; enforced for MCP and runs | 3–5 h |
| 3 | Panel, context, undo per message, on the API backends | Chat in the app, in every workspace | 8–12 h |
| 4 | ACP backend | Use an agent you already have, with no keys | 3–5 h |
| 5 | Review: summary cards, accept or reject a copy | The proposal flow | 4–6 h |
| 6 | Entry points: Ask about this, Fix this, palette | Seamless access | 2–4 h |
| 7 | Evals and model table | Evidence per model | 3–4 h |

Slices 1 and 2 come first and can run in parallel, then 3. The [geometry work](llm-geometry.md) uses slices 1 and 2, so its benchmark waits for them. That's about 29–45 hours in total, and it sits alongside the current gap fixes and the kernel move.

**Overlaps to watch:** `gap-mcp` shapes replies now and must merge before slice 2. Slice 3's canvas changes may conflict with the kernel move only where they touch the 3D viewport, so its 3D part follows the move.

## Founder decisions (5 November 2026)

- **Default permission:** auto-allow edits inside a reviewable turn; files and publishing always ask.
- **Model setup:** no detection or suggestions, "too complex". The user enters the agent's settings, as in Solidcraft's Preferences.
- **Unattended routing versus accept-or-reject:** decided by testing first, through the [benchmark](llm-geometry.md).
