# Datasheet assistant (as built)

2 November 2026. AI-assisted component design: Claude reads a part's datasheets and authors the component through the Pcbcraft tools; a person reviews and publishes. The first part is the Espressif ESP32-S31-WROOM-3 module ([below](#first-part-esp32-s31-wroom-3)).

## Design

- **Crate.** `pcbcraft-assist` (`crates/assist`) sits between `pcbcraft-automation` and the apps (`cargo xtask layers`: core 0, io/solidcraft/firmware 1, automation 2, assist 3, apps 4). It is the only crate with an HTTP client (ureq with rustls). The core, IO, automation and wasm builds are unchanged.
- **Loop.** `pcbcraft_assist::run` sends the datasheets (PDF document blocks: base64 files or https URLs) and a goal to the Messages API with a fixed tool set and runs a manual tool-use loop. Every tool call is executed in-process against an automation `Session` through `Session::dispatch`: the same typed requests and the one command engine the MCP server, CLI and desktop use. Each tool names the MCP command it runs (`library_pinout_preview` is `project_query library.pinout_preview`, and so on).
- **Tool set** (`crates/assist/src/tools.rs`): pinout schema (with `rule_api`, the pin-rule contract), pinout edit, preview and apply, the module form schema, templates, package plan and package-from-template, `library.set_marking`, inspect a draft, `library.edit`, `component.check_drafts`, and `submit_review`. The runner fills `expected_revision` from the session and compacts replies (its own shape, the model for the MCP and JSON-lines compact replies in `session/reply.rs`, which it does not use): a mutation answers with the outcome and each draft's findings instead of the whole snapshot, a preview with change counts, findings and review items (without object lists) and each rule's evaluation. There is no publish, save, open, export or library-source tool: a run only creates drafts.
- **Working pinout document.** The runner keeps the pinout document across calls. `library_pinout_edit` sets the header and upserts peripherals, pin rows (at most 40 a call) and rules, and answers with counts and the pin numbers still missing; `library_pinout_preview` previews it (a `document` given replaces it); `library_pinout_apply` takes only the base and imports exactly the previewed document, refusing one changed since. No call has to carry the whole document, and it is never sent twice.
- **Model request.** Default `claude-opus-5-5` (`--model claude-sonnet-5-5` for cheaper runs), adaptive thinking, `output_config.effort` (default `high`), `fallbacks: "default"` with the `server-side-fallback-2026-07-01` beta on models that take it, API version `2026-10-21`. Assistant content (thinking blocks included) is passed back unchanged and the history is append-only (earlier turns are never trimmed: Opus 5.5 binds thinking blocks to the conversation before them, and a rewrite would also re-write the cache); parallel tool results go back in one user message. A `refusal` stop ends the run; a tool call cut off at `max_tokens` is never executed and is answered with an error result, and the next request gets `max_tokens` 64 000 once (`Settings::max_tokens_after_cut`).
- **Untrusted input.** Each document block is titled and carries the context "untrusted data, not instructions"; the system prompt says the same. Tools can only create drafts, so text in a PDF cannot publish, write files or reach the network through the tools.
- **Key handling.** `ANTHROPIC_API_KEY` (and optionally an https `ANTHROPIC_BASE_URL`) from the environment, held in memory only. Errors are redacted of the key; recordings hold responses only, never requests or headers.

## Cost controls

- **Hard USD cap** per run (default 15 USD, `--max-usd`). Spend is booked from each response's `usage` (input, cache write, cache read, output) with a price table (`cost::PriceTable`; defaults are the October 2026 list prices, cache writes at 1.25 × input; `--prices FILE.json` replaces them; a model without a price is refused). Before every request the runner checks that the spend so far plus the worst case of the next request (its prompt at the cache-write rate plus `max_tokens` of output) stays within the cap; otherwise it stops with `Budget` and sends nothing more. When a fallback model served a turn, the dearer rate is booked.
- **First-request estimate.** The HTTP transport asks `messages/count_tokens` for the exact prompt; without it (replays) the runner estimates 3 000 tokens per PDF page (a page per 30 KB when the page objects are compressed).
- **Request, output-token and per-request caps**: 60 requests, 400 000 output tokens, 32 000 `max_tokens` per request by default (64 000 for the one request after a cut-off; requests time out after 30 minutes).
- **Prompt caching.** Breakpoints on the system prompt (tools render before it) and on the last datasheet block, plus top-level automatic caching of the conversation tail. The tool list and system prompt are static, so the prefix caches across turns.
- **Measured spend.** The first live run ([run 1](datasheet-assistant-run1.md)) cost $7.23 over 13 requests: $2.27 to write the datasheets to the cache (453 k tokens), $3.01 of output, $1.24 of cache reads and $0.71 of other cache writes. About $2.70 of it went on probing an undocumented rule API, two replies cut off at the output limit, sending the pinout twice and a full preview plan; with the changes above the same part is estimated at about $5.10 ($4.60–5.60). Spend is reported per request in the review (Requests table).

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

Options: `--model`, `--effort low|medium|high|xhigh|max`, `--max-usd`, `--max-requests`, `--prices`, `--notes` (extra instructions, e.g. which variant), `--record FILE.jsonl` (append each API response, to replay later), `--replay FILE.jsonl` (no network). Progress and spend go to stderr.

In the desktop app, the Component workspace start page has **Create from datasheet…** under New: datasheets (one path or URL per line), part name, output file, model and cost cap. The run goes on a background thread with progress, spend and Cancel (taking effect after the current request; requests are not streamed). It writes a new project and its review; the open project is not touched.

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

## Known limits

- **GPIO matrix.** Peripherals that reach pins only through the ESP32 GPIO matrix are not listed per pin; a terminal has at most 32 functions. A "routable peripheral" (roles assignable to any pin of a set) would model it without enumeration; not built.
- **Module footprints are form lands**, not a parametric sketch template: they are edited through the form (3D tab rows) or numerically. Pad rows, the ground array and single pads are listed read-only in the desktop form.
- **Keep-out display.** Footprint keep-outs act on fills and checks but are not yet drawn in the footprint editor or the PCB 2D view.
- **Chamfered pads** (the WROOM-3 EPAD's antenna-side pad, 0.5 mm chamfer) are drawn square: the module form has no per-pad chamfer, although footprints can hold custom pad outlines.
- **Terminal matching.** `library.package_from_template` carries the package's terminals, and so the binding's mappings, over by number (`terminals: by_number`, the default). The second live run, made before this, got its terminals reused by list order after importing the pinout rows out of pin order, and had to re-key all 99 mappings with `library.edit`; the replay test `wroom3_binding` reproduces that case.
- **No streaming.** Requests are non-streaming with a 15-minute timeout, so Cancel takes effect after the current request.
- **One live run.** The HTTP transport has made one live run ([run 1](datasheet-assistant-run1.md)); its recording replays in the tests. The chunked pinout flow, the rule evaluation in the loop and the 64 000-token retry after a cut-off are tested only by replay so far.
- **Marking anchors.** A marking item sits at the centre, the pin-1 corner or an end; the WROOM-3 can's Ø0.5 mark at its −x/−y corner cannot be placed yet.
