# Datasheet assistant (as built)

2 November 2026; model layer 5 November 2026. AI-assisted component design: a model (Claude, or any OpenAI-compatible model, hosted or local) reads a part's datasheets and authors the component through the Pcbcraft tools; a person reviews and publishes. The first part is the Espressif ESP32-S31-WROOM-3 module ([below](#first-part-esp32-s31-wroom-3)).

## Design

- **Crate.** `pcbcraft-assist` (`crates/assist`) sits between `pcbcraft-automation` and the apps (`cargo xtask layers`: core 0, io/solidcraft/firmware 1, automation 2, assist 3, apps 4). It is the only crate with an HTTP client (ureq with rustls). The core, IO, automation and wasm builds are unchanged.
- **Model layer** (`crates/assist/src/model*`, 5 November 2026; LLM integration slice 1). The run sees one neutral conversation: messages of text, reasoning, tool calls, tool results, images and documents (`model::Part`), tool specs, one request and one reply shape, and a `ModelBackend` trait. Two backends convert it to their wire format:
  - **Anthropic Messages** (`model/anthropic.rs`): today's request, byte for byte (a test pins the body): adaptive thinking, `output_config.effort`, `fallbacks: "default"` with the `server-side-fallback-2026-07-01` beta, API version `2026-10-21`, cache breakpoints on the system prompt and the last document plus automatic caching of the tail, a token count for the first request. A response block the neutral fields can't rebuild exactly (every thinking block, a text block with extra fields, an unknown block) keeps its original JSON in the part's `replay` field, so assistant turns go back unchanged and the history stays append-only.
  - **OpenAI-compatible chat completions** (`model/openai.rs`): Ollama, LM Studio, `llama-server`, OpenAI, OpenRouter or any URL. It reads reasoning from `reasoning`, `reasoning_content`, `thinking` or leading `<think>` tags, and never sends it back. Tool arguments may arrive as a JSON string, an object, empty or malformed. A malformed call isn't run: the model gets an error result naming what was wrong (quoted, at most 400 characters) and the call goes back in the history with `{}`, because some servers parse arguments when they render the history. Missing or repeated call ids are replaced. `length` is a cut-off even beside a partial call; `stop` beside a call is a tool turn. A missing `usage` is estimated from the characters and flagged in the review. An error inside a 200 response is an error. When a server asks for `max_completion_tokens` instead of `max_tokens`, the request is sent again once under that name. Tool results go back as `tool` messages, and a page image follows them in one user message, because a tool message carries text only.
  - **Capabilities are declared per configured model** (`model::ModelProfile`), never inferred from a vendor name: tool calling, vision, native PDF, caching, adaptive thinking, server fallback, context size, output limits and an optional price. The Anthropic profile states what that API declares for its current models (1M context, PDFs, vision, caching) with prices from the price table by exact id. An OpenAI-compatible profile starts with tool calling only; the user declares the rest, and must state the context size the server gives the model.
  - **One recording format** (`model/record.rs`): JSON lines of neutral replies. `--record` wraps any backend; `--replay` answers from a file with no key and no network. Requests (which carry the datasheets) are never written. Run 1's recording and the reference replay were converted once by parsing each raw Messages response with the Anthropic backend's own parser, after checking that every response's content rebuilds byte for byte. Their test assertions are unchanged apart from the stop label, which is now neutral (`cut_off` for `max_tokens`).
- **Loop** (`run.rs`). `pcbcraft_assist::run` sends the datasheets and a goal with a fixed tool set and runs a manual tool loop. Every tool call goes through one function, `dispatch_tool_call`, and every request a tool sends goes through the automation gate (`tools::send`: `check_request` with the run's policy, `respond` on Run, and on Ask `respond_gated`, which refuses with a reason the model reads, because a run has no one to ask yet). The policy is auto-allow edits with the output project's folder as its only folder, so publishing and files elsewhere come back as refusals in tool results, not errors that end the run. Calls run in-process against an automation `Session`: the same typed requests and the one command engine the MCP server, CLI and desktop use. Each tool names the MCP command it runs (`library_pinout_preview` is `project_query library.pinout_preview`, and so on).
- **Tool set** (`crates/assist/src/tools.rs`): pinout schema (with `rule_api`, the pin-rule contract), pinout edit, preview and apply, the module form schema, templates, package plan and package-from-template, `library.set_marking`, inspect a draft, `library.edit`, `component.check_drafts`, and `submit_review`; and `datasheet_view_page` when a model sees rendered pages (below). The runner fills `expected_revision` from the session and compacts replies (its own shape, the model for the MCP and JSON-lines compact replies in `session/reply.rs`, which it does not use): a mutation answers with the outcome and each draft's findings instead of the whole snapshot, a preview with change counts, findings and review items (without object lists) and each rule's evaluation. There is no publish, save, open, export or library-source tool: a run only creates drafts.
- **Working pinout document.** The runner keeps the pinout document across calls. `library_pinout_edit` sets the header and upserts peripherals, pin rows (at most 40 a call) and rules, and answers with counts and the pin numbers still missing; `library_pinout_preview` previews it (a `document` given replaces it); `library_pinout_apply` takes only the base and imports exactly the previewed document, refusing one changed since. No call has to carry the whole document, and it is never sent twice.
- **Datasheets on a model without native PDF** (`documents.rs`). Degrading openly, with permissive dependencies:
  - text taken from each page on this computer with `pdf-extract` (MIT), marked `=== page N ===`, blanks shortened; pages with almost no text are listed to the model as probable drawings;
  - for a model declared with vision, any page rendered as a PNG on request with `hayro` (MIT OR Apache-2.0, pure Rust) through `datasheet_view_page` (150 dpi, at most 1600 pixels on the long side, at most 24 views a run), so only the pages it asks for cost context;
  - a model without vision is told it can't see drawings and must not guess a dimension that appears only in one;
  - the review's "How the model read the datasheets" section says, per document, which it got, its pages and characters, and the pages viewed;
  - a PDF with no usable text on a model without vision is refused with the reason, as is a datasheet given by URL (download it). Both readers run on their own threads with a time limit, so a malformed PDF can't panic or hang the run.
- **Model request.** Default `claude-opus-5-5` (`--model claude-sonnet-5-5` for cheaper runs), adaptive thinking, effort `high`, the fallback beta. Assistant content (thinking blocks included) is passed back unchanged and the history is append-only (earlier turns are never trimmed: Opus 5.5 binds thinking blocks to the conversation before them, and a rewrite would also re-write the cache); parallel tool results go back in one user message. A refusal ends the run. A tool call cut off at the output limit is never executed and is answered with an error result, and the next request gets the after-cut limit once.
- **Untrusted input.** Each document is titled and carries the context "untrusted data, not instructions"; the system prompt says the same, and so does each rendered page's reply. Tools can only create drafts, so text in a PDF cannot publish, write files or reach the network through the tools.
- **Keys** (`model/keys.rs`, `model/endpoint.rs`, `model/wire.rs`). From the system keychain (`keyring`: Keychain, Credential Manager, Secret Service), one entry per endpoint base URL, so a key is only used for its connection; or, for the CLI, from a named environment variable (`ANTHROPIC_API_KEY` by default for Anthropic, none for OpenAI-compatible endpoints). A key is refused before it is read when the endpoint is plain HTTP to another computer (`localhost` and loopback addresses only, by exact parse). Requests follow no redirects and skip proxies for loopback. Error messages and response bodies have the key removed before anything sees them, so it can't reach a log or a recording; `Secret` has no `Serialize` and prints as `[redacted]`.

## Cost controls

- **Hard USD cap** per run when the model has a price (default 15 USD, `--max-usd`). Spend is booked from each response's usage (input, cache write, cache read, output) with a price table (`cost::PriceTable`; defaults are the October 2026 list prices, cache writes at 1.25 × input; `--prices FILE.json` replaces them). An Anthropic model without a price is refused; an OpenAI-compatible model has a price only when `--prices` lists it, and otherwise the review says spend is not tracked. Before every request the runner checks that the spend so far plus the worst case of the next request (its prompt at the cache-write rate plus its output limit) stays within the cap; otherwise it stops with `Budget` and sends nothing more. When a fallback model served a turn, the dearer rate is booked.
- **Caps for every model**, local ones included: requests (60), output tokens (400 000), prompt plus output tokens over the run (`--max-tokens`, 40 million), and run time (`--max-seconds`, 6 hours; each request waits no longer than the run has left). Each is checked before a request against its worst case.
- **Output limits come from the model's configuration.** Anthropic: 32 000 a request and 64 000 for the request after a cut-off, for a 30-minute non-streaming wait. OpenAI-compatible: half the declared context up to 32 000, and three quarters up to 64 000 after a cut-off, or `--max-output-tokens`. Reasoning counts against them, so they are never a small fixed number. A request never asks for more than the context leaves beside the estimated prompt, and a request with less than 4 000 tokens of room isn't sent: the run stops and says the conversation filled the context. A prompt that can't fit before the first request is refused with the numbers.
- **Cut-offs are never answers.** A cut-off reply's calls aren't run; the model is told to continue in smaller steps; after three cut-off replies in a row the run stops with `cut off` and says so.
- **Truncation warning.** The history only grows, so a server that counts fewer prompt tokens than the last request (by more than a tenth), or a first request far below the datasheets' size, has probably dropped part of the conversation to fit its context. The review carries a warning saying so.
- **First-request estimate.** The Anthropic backend asks `messages/count_tokens` for the exact prompt; otherwise the runner estimates 3 000 tokens per PDF page for a native PDF (a page per 30 KB when the page objects are compressed), or a quarter of the characters for extracted text, plus the tools and system prompt.
- **Prompt caching** on Anthropic. Breakpoints on the system prompt (tools render before it) and on the last datasheet block, plus top-level automatic caching of the conversation tail. The tool list and system prompt are static, so the prefix caches across turns.
- **Measured spend.** The first live run ([run 1](datasheet-assistant-run1.md)) cost $7.23 over 13 requests: $2.27 to write the datasheets to the cache (453 k tokens), $3.01 of output, $1.24 of cache reads and $0.71 of other cache writes. About $2.70 of it went on probing an undocumented rule API, two replies cut off at the output limit, sending the pinout twice and a full preview plan; with the changes above the same part is estimated at about $5.10 ($4.60–5.60). Spend is reported per request in the review (Requests table, with each response's seconds).

## Review flow

1. The run ends with `submit_review`: every value used with document, page, table or figure, a confidence (high: table cell or dimensioned drawing; medium: derived or ambiguous; low: inferred or measured from a drawing) and the open questions. The runner refuses a report that cites an unlisted document or no page.
2. The CLI or desktop saves the drafts as a new project and writes `NAME.review.md` (open questions first, values least-confident first, spend, each pin rule's verdicts and failures over its samples, drafts with finding codes, the applied steps, every package pin with its default, functions, mux table and code, peripheral and role, source page and review flags, and each request's tokens, cost and tools) and `NAME.review.json` beside it. Existing files are never replaced. `pcbcraft-cli review-diff A.pcbcraft B.pcbcraft [NAME_A NAME_B]` compares two runs (or a run and a reference) pin by pin, rule by rule, pad by pad and by package form, keep-outs and marking.
3. The drafts carry the import review state (F8): every imported pin row is unreviewed (`pinout.unreviewed`), and a row flagged uncertain blocks publishing (`pinout.uncertain`). A person opens the project, checks the rows in the Pins tab against the report, and publishes. Nothing is published by the assistant.

## Using it

```sh
export ANTHROPIC_API_KEY=…            # never written anywhere by Pcbcraft
pcbcraft-cli design-part \
  --datasheet esp32-s31-wroom-3_wroom-3u_datasheet_en.pdf \
  --datasheet esp32-s31_datasheet_en.pdf \
  --name ESP32-S31-WROOM-3 --out esp32-s31-wroom-3.pcbcraft \
  --max-usd 15 --record wroom3.recording.jsonl
```

On a local model through Ollama's OpenAI-compatible endpoint (no key, no cost; the context is what the server gives the model, which `ollama ps` shows):

```sh
pcbcraft-cli design-part --provider openai --base-url http://localhost:11434/v1 \
  --model qwen3.8:27b --context-tokens 262144 --vision yes \
  --datasheet tlv757p.pdf --name TLV75733PDBVR --out tlv75733.pcbcraft \
  --notes "SOT-23-5 (DBV) package only" --record tlv757.recording.jsonl
```

Options: `--provider anthropic|openai`, `--base-url`, `--model`, `--key none|keychain|env:NAME` (default `env:ANTHROPIC_API_KEY` for Anthropic, `none` otherwise), the model's declarations `--context-tokens`, `--max-output-tokens`, `--vision yes|no`, `--native-pdf yes|no`, `--tools yes|no`, `--effort` (Anthropic `output_config.effort`; OpenAI-compatible `reasoning_effort`, sent only when given), `--prices FILE.json`, `--request-timeout SECONDS`; the caps `--max-usd`, `--max-requests`, `--max-tokens`, `--max-seconds`; `--notes` (extra instructions, e.g. which variant); `--profile NAME` (the [run profile](#run-profiles); chosen by the model id when not given); `--record FILE.jsonl` (append each reply, in the neutral format, to replay later) and `--replay FILE.jsonl` (no network; give the same model flags as the recorded run, since they decide how the datasheets are sent). Progress, tokens, seconds and spend go to stderr. `pcbcraft-cli llm-key set --base-url URL` reads a key from standard input into the system keychain for that endpoint (`delete`, `status` likewise); `--key keychain` uses it. `pcbcraft-cli llm-profile list` lists the run profiles, and `llm-profile copy NAME` copies a built-in one into the config folder to edit.

In the desktop app, the Component workspace start page has **Create from datasheet…** under New: datasheets (one path or URL per line), part name, output file, then provider (Anthropic, or OpenAI-compatible), endpoint, model, API key (masked; stored in the system keychain for that endpoint when the run starts, then cleared from the form), and for an OpenAI-compatible model its context size and whether it takes images or reads PDFs; the cost cap for Anthropic. The run goes on a background thread with progress, tokens, spend and Cancel (taking effect after the current request; requests are not streamed). It writes a new project and its review; the open project is not touched. A full settings page belongs to the assistant panel (LLM integration slice 3).

## Run profiles

5 November 2026. How a run talks to one model: plain data, separate from the model's declared capabilities (`ModelProfile`), which say what it can do; a run profile says what it is told and how the loop reacts (`crates/assist/src/profile.rs`).

```json
{
  "format": 1,
  "name": "qwen3.8-flash-next",
  "models": ["RadixArk/Qwen3.8-Flash-Next-NVFP4"],
  "system": {"add": ["paragraph", "paragraph"]},
  "omit_tools": [],
  "harness": {"repeat_limit": 3}
}
```

- `system`: `"base"` (the base prompt, `prompt.rs`), `{"add": [...]}` (the base prompt, then these paragraphs) or `{"replace": [...]}` (these paragraphs instead). Paragraphs are joined by blank lines, at most 64 KiB.
- `omit_tools`: tools the model isn't offered (never `submit_review`).
- `harness.repeat_limit` (2–20, default 3): the same refused call (same tool, same input) this many times ends the run with `stopped a repeating call` and the call named. From the second time, the refusal tells the model it is repeating itself.
- `models`: exact model ids the profile is chosen for when none is named.

**Built in** (`crates/assist/profiles/`): `claude` (the Claude ids; base prompt, every tool: today's request, unchanged), `qwen3.8-flash-next` ([tuned below](#tuning-qwen38-flash-next)) and `default` (base prompt), which any model without a profile gets. **Selection:** `--profile NAME`, else the first profile listing the model id, user files first, else `default`. The desktop form selects by its model field and logs the profile; the review report names it. **Editing:** `pcbcraft-cli llm-profile copy NAME` writes the built-in to `<config>/assist-profiles/NAME.json` (`PCBCRAFT_CONFIG_DIR`, else the platform config folder under `pcbcraft`), never over an existing file; a user file of a built-in's name takes its place. Every file is checked when read: unknown fields, another format, a name that isn't the file's, an unknown or required tool left out, an empty replacement or a limit out of range is refused with the file and reason, and the run doesn't start. There is no editor in the app yet.

**Harness fixes for every model** (found in the `qwen3.8:27b` run, measured below): repeated refused calls (above); every integer beyond 2^53 in a tool result goes to the model as text, so a 19-digit preview `base` can't be rounded (`library_pinout_apply` takes it as text or a number; in every recording in the repository and the tuning runs the base is the only such number a model copied back, and the conversion covers any other that appears); an array or object argument that arrives as JSON text is read as meant, and text that isn't valid JSON is refused with where it broke instead of serde's "expected a sequence". The report's shape fixes of the earlier run stay.

## MCP gaps closed

What an MCP client could not author for a module like the WROOM-3, and what now exists:

| Gap | Now |
|---|---|
| No package family for modules (shield can, edge or LGA pad rows on any side, ground array as one terminal, corner pads, antenna end) | `PackageForm::Module` (`library/package_form/module.rs`): rows numbered counter-clockwise from a stated first pad, a turned ground-pad array landing one terminal, single pads as their own terminals, castellation diameter, host land width and extension, can, antenna depth. Used through `library.package_plan` / `library.package_from_template` with row `{source: custom, family: module}`. |
| Footprints could not carry copper keep-outs | `FootprintDefinition::keepouts` (closed outline, every copper layer or the component side, optional pad ban). The module form adds the antenna keep-out; placed parts' keep-outs join board keepouts in fill computation (fill copper stays out on every layer) and in the keepout checks (tracks, vias, other parts' pads), reported against the part. |
| No module 3D model | Generated model (`pcbcraft-solidcraft` `models/module.rs`): soldermask-green board notched at castellations, gold pads with plated half-holes, nickel-silver can with rounded top edges, the printed antenna meander under the mask. New cited-estimate materials `NickelSilver`, `SolderMaskGreen`, `SolderMaskOverCopper`. The can is the marking face. |
| Marking artwork needed a SHA-256 an agent cannot compute | `library.set_marking` (MCP `library_set_marking`): text, SVG given as text (packed by hash in the session) and polarity items, one atomic step. |
| A peripheral could have at most 16 roles | 64 (a 24-bit RGB LCD bus with its controls has 28). |
| MCP instructions had no module workflow | The server instructions describe the module values and frame. |
| The pin-rule API was not in the schema an agent reads, and rules were only compiled before publishing (run 1 guessed helpers that compiled and could never run) | `library.pinout_schema` returns `rule_api`. Preview, every component check and publish evaluate each rule without selections and with each function, peripheral or role it names made active (alone, then together): failures are blocking `rule.runtime`/`rule.reference`/`rule.budget` findings, and the preview lists every rule's verdicts. |
| Mux codes from several tables that each start at F0 (IO MUX, LP IO MUX, analog) collided | `mux_table` beside `mux` on pinout and component functions, shown as `LP IO MUX F0` in the Pins tab and pin-mux panel and emitted by the firmware exports. |

## First part: ESP32-S31-WROOM-3

Sources (public Espressif documentation, untrusted input, no EDA library data): ESP32-S31-WROOM-3 & -3U datasheet v0.7 (pre-release, 2026-09-03), ESP32-S31 series datasheet v0.5 (2026-07-13), ESP32-S31 hardware design guidelines v0.3 (2026-10-09). The technical reference manual's GPIO matrix chapter is not yet published.

The reference was made before any live run: the tool calls were authored from the datasheets by the developing agent and replayed through `design-part --replay` against the real PDFs, so the drafts come out of the same tools and engine a live run uses: `examples/esp32-s31-wroom-3/` holds the replay, the resulting project and its review. Checked against the datasheet (Figure 10-1, Figure 11-1, Table 3-1, the IO MUX tables) by a script in the developer's scratch area:

- 99 terminals: 94 signal pads, EPAD pin 95 (nine 1.8 mm pads in a 3 × 3 array turned 45°, 6.5 mm along each turned axis, centred on the 22 × 24 mm pad body), corner pads 96–99 (0.8 mm).
- Numbering 1–25 down the left, 26–47 along the bottom, 48–72 up the right, 73–94 along the antenna-side row towards −x; pin 1 at (−10.3, 7.2) below corner pad 99; pitch 0.85 everywhere; first pads 1.1 mm (columns) and 1.375 mm (rows) from the corners; every pad at least 0.3 mm inside the 22 mm outline; side pads 0.8 × 0.4, top and bottom pads 0.4 × 0.8; land pattern equal to the pads (Figure 11-1).
- Every pad lands the component terminal of its datasheet pin through the binding (107 pads).
- Outline 22 × 30 × 3.5 mm, antenna area 22 × 6 mm as an all-layer keep-out, can 21 × 23 mm, pin-1 mark, courtyard, silkscreen and assembly drawings; the drafts are publishable apart from the review warning.
- 54 GPIOs with IO MUX F0–F4, LP IO MUX and analog functions (for example IO54: MTDO, GPIO54, SPI2_D, CAM_PCLK, ADC2_CH2_N), defaults from Table 2-3 and 2-7, eight rules (strapping IO36, IO37, IO60, IO61; JTAG, USB Serial/JTAG and UART0 pins; LP UART flow control excludes LP I2C).
- Published in a scratch copy and placed on a board, the part exports to a native model in 0.4 s.

The first live run (`examples/esp32-s31-wroom-3/run1/`) built the same geometry and a pinout that adds the DAC channels and corrects the LP SPI directions, but with broken pin rules and ambiguous mux codes; [the comparison](datasheet-assistant-run1.md) gives every difference with its datasheet page.

Espressif's own published CAD data for the module (footprint and STEP model in `espressif/kicad-libraries`, commit cfb91e9 of 2026-07-24, CC-BY-SA 4.0) was used afterwards like a datasheet, as a dimension source only (not redistributed; [example README](../examples/esp32-s31-wroom-3/README.md#cad-cross-check)). It confirms every footprint value above, the 1.0 mm board (1.004 in the STEP), the 21 × 23 mm can and the EPAD centre at y = −3; it settles the land pattern's "1.3" as the edges of a 0.5 mm chamfer on the antenna-side EPAD pad, and shows the can's only mark is a Ø0.5 hole at its −x/−y corner by pins 25/26, not a pin-1 mark. The remaining open questions: the chamfered pad (drawn square), the 15 mm clearance the guidelines ask of the host board, directions inferred where the table's type column was not read, and the -3U variant not built.

## Local model run: TLV75733PDBVR

5 November 2026: `qwen3.8:27b` (27.3 B, Q4_K_M) through Ollama's OpenAI-compatible endpoint on an M5 Max, 262 144 tokens of context as Ollama reports it, vision declared, no native PDF, no cost. The datasheet was TI's TLV757P (SBVS322C, 41 pages): 73 854 characters of text, with two pages listed as probable drawings. Other builds ran on the machine at the same time, so the times are rough. The recording and review are in [`examples/tlv75733-local/`](../examples/tlv75733-local/); a test replays them.

| | Opus 5.5 ([e2e run](e2e-esp32-devboard.md), effort medium) | `qwen3.8:27b`, first attempt | `qwen3.8:27b`, second attempt |
|---|---|---|---|
| Result | Review submitted | **Failed:** request cap (60) reached, no report | Review submitted |
| Requests | 13 | 60 | 29 |
| Output tokens | not recorded here | 31 010 | 38 861 |
| Time | not recorded here | 40 min of requests | 44 min |
| Cost | $1.06 | none | none |
| Pinout | Right | Right (never applied) | Right: 1 IN, 2 GND, 3 EN, 4 NC, 5 OUT (Table 4-1), with two pin rules (EN drive, output capacitor) |
| Footprint | TI's DBV0005A land: 0.6 × 1.1 mm pads | none | The SOT-23-5 starter row's 0.8 × 1.0 mm pads and 1.6 mm body width, **not** TI's 1.1 × 0.6 mm land and 1.75 mm body. It read both values from the drawing (pages 34–35, viewed as images) and listed each as an open question instead of using it |
| Wasted requests | 3 refused `library_package_plan` calls | 54 refused `library_pinout_apply` calls | 1 refused apply, 5 refused package plans, 11 refused reports |

What went wrong, and what changed:

- **The plan's base.** `library.pinout_preview` answers with a 19-digit `base`. The model copied it as a float (`2049172786040315600`, then `2.049…e+18`), was refused, and repeated the same mistake 54 times until the request cap. The runner already applies exactly the last preview, so the base is now optional, text digits are read, and a rounded number is named as the cause. In the second attempt the model sent the rounded base once, read the refusal and applied without it.
- **The report's shape.** The model sent `submit_review`'s fields without the `report` wrapper, its lists as JSON text, and its own field names (`name` for `item`, `table` for `location`). Serde names one error at a time, so it took eleven attempts. Now a missing wrapper and lists given as text are read as meant, and a refusal shows a complete example report. Replaying the recording still takes the same 29 requests, because its earlier reports also used the wrong field names.
- **Footprint values.** It understood the drawing and said so, but kept the starter row's values. The run is an honest draft with the right pins and a footprint a reviewer must fix. That is weaker than Opus 5.5, which read the land pattern into the footprint.
- **The loop held.** Parallel tool calls, reasoning under `reasoning`, page views, Ollama's prompt cache (1.35 M of 1.42 M prompt tokens read from it) and the caps all behaved as designed, and no reply was cut off at 32 000 output tokens. The first attempt's request time was 2 427 s against 4 343 s of wall-clock time; the gap was not reproduced in the second attempt (2 648 s against 2 650 s), so it is probably the machine sleeping.

## Known limits

- **GPIO matrix.** Peripherals that reach pins only through the ESP32 GPIO matrix are not listed per pin; a terminal has at most 32 functions. A "routable peripheral" (roles assignable to any pin of a set) would model it without enumeration; not built.
- **Module footprints are form lands**, not a parametric sketch template: they are edited through the form (3D tab rows) or numerically. Pad rows, the ground array and single pads are listed read-only in the desktop form.
- **Keep-out display.** Footprint keep-outs act on fills and checks but are not yet drawn in the footprint editor or the PCB 2D view.
- **Chamfered pads** (the WROOM-3 EPAD's antenna-side pad, 0.5 mm chamfer) are drawn square: the module form has no per-pad chamfer, although footprints can hold custom pad outlines.
- **Terminal matching.** `library.package_from_template` carries the package's terminals, and so the binding's mappings, over by number (`terminals: by_number`, the default). The second live run, made before this, got its terminals reused by list order after importing the pinout rows out of pin order, and had to re-key all 99 mappings with `library.edit`; the replay test `wroom3_binding` reproduces that case.
- **No streaming.** Requests are non-streaming (30 minutes for Anthropic, 90 for an OpenAI-compatible endpoint, never past the run's time cap), so Cancel takes effect after the current request. A local model at about 17 tokens a second can keep Cancel waiting for minutes.
- **One live run.** The HTTP transport has made one live run ([run 1](datasheet-assistant-run1.md)); its recording replays in the tests. The chunked pinout flow, the rule evaluation in the loop and the 64 000-token retry after a cut-off are tested only by replay so far.
- **Live runs per backend.** Anthropic: one live run ([run 1](datasheet-assistant-run1.md)) before the model layer, now replayed through it; no paid call was made since, so the Anthropic HTTP path is verified by a byte-for-byte request test, recordings and a loopback HTTP test, not live. OpenAI-compatible: the two local runs above, through Ollama only; LM Studio, `llama-server`, OpenAI and OpenRouter are untested live.
- **Keychain.** Storing and reading a key goes through `keyring`; the tests cover refusals and redaction but don't touch the real keychain, and the desktop key field needs a check in the app.
- **Rendered pages** cost about 2 000 tokens each and stay in the conversation; a run may view at most 24. Text extraction loses table layout, and scanned PDFs need vision.
- **Repeated refusals** are caught only when the call is identical; a model that varies a hopeless call slightly is still bounded only by the request cap.
- **Marking anchors.** A marking item sits at the centre, the pin-1 corner or an end; the WROOM-3 can's Ø0.5 mark at its −x/−y corner cannot be placed yet.
