# Datasheet assistant: first live run

2 November 2026. The first Claude API run of the [datasheet assistant](datasheet-assistant.md) (`claude-opus-5-5`, effort high, 13 requests) built ESP32-S31-WROOM-3 drafts. This note compares them with the hand-made reference in [`examples/esp32-s31-wroom-3/`](../examples/esp32-s31-wroom-3/) and records where the run's tokens went and why. The run's recording and project are in [`examples/esp32-s31-wroom-3/run1/`](../examples/esp32-s31-wroom-3/run1/).

## Method

Both projects were compared with `pcbcraft-cli review-diff run1/esp32-s31-wroom-3.pcbcraft esp32-s31-wroom-3.pcbcraft` (pins, rules, pads, package form, keep-outs, marking from the drafts themselves). Then a script compared both pinout documents, with labels normalised for case and a `_PAD` suffix, against the IO MUX, LP IO MUX and analog tables. Every disagreement was checked in the PDFs. The replay tests in `crates/assist/src/tests/run1.rs` replay the recording and the reference through the current tools.

Sources: WROOM-3 = ESP32-S31-WROOM-3 & -3U datasheet v0.7; S31 = ESP32-S31 series datasheet v0.5. Page numbers are the documents' printed page numbers.

## Differences and verdicts

| Area | Run 1 | Reference | Verdict (source) |
|---|---|---|---|
| Terminals, numbers, names, classes | 99 rows; 94 edge pads, EPAD 95, corner pads 96–99; NC 33–39 and 91 as `device_nc` | Same | **Identical.** Both match (WROOM-3 Fig. 3-1 p.12, Table 3-1 pp.14–15). |
| Defaults | GPIO on IO pins; SPI2 F0 on IO9–IO19; MTDO/MTCK/MTDI/MTMS on IO54–57; UART0 on TX0/RX0; USB1P1 on IO33/34. Single-function pins have no explicit default | Same choices; explicit default on every pin | **Equivalent.** A single-function pin resolves to its only function. Defaults match the bold entries (S31 Table 2-3 p.23, note 1). |
| DAC_CH0/DAC_CH1 on IO4/IO5 | Listed | Missing | **Run 1 right.** S31 Table 2-8 p.31; §4.2.3.4 p.80 says that DAC uses dedicated pads on GPIO4 and GPIO5. |
| LP SPI direction (CK, CS, D, Q on IO2–IO5) | bidirectional | output (CK, CS, D), input (Q) | **Run 1 right.** S31 Table 2-5 p.26 types the LP SPI pads I1/O/T. |
| Function labels | As the tables write them (`LP_PROBE_TOP_OUT2`, `REF_GMAC_CLK_PAD`, `PAD_COMP0`, `USB1P1_N0`, `MTDO`) | Renamed (`lp_probe_out2`, `gmac_ref_clk`, `pad_comp_comp0`, `usb_serial_jtag_dm`, `jtag_mtdo`) | **Run 1 better.** It uses the table labels (S31 Tables 2-3, 2-5, 2-7), which a reviewer can check directly. |
| GPIO functions | One `GPIOn` (bidirectional) per pin, taking role `IO` of a `GPIO` peripheral (capacity 54) | `gpio_in` and `gpio_out`, no peripheral | **Reference better.** The prompt asks for input and output. Split functions give ERC its directions, and the run's capacity-54 GPIO peripheral appears as a peripheral in the pin-mux panel. |
| IO MUX F0 and F1 both GPIO (GPIO0–8, 20–53), LP F0 and F1 both LP_GPIO (GPIO0/1) | GPIO listed once, under F1 | Same | **Same omission.** It is harmless, because either code selects GPIO (S31 Tables 2-3, 2-5). |
| Mux codes | IO MUX, LP IO MUX and analog functions all use `F0`–`F4`. For example, `XTAL_32K_N [F0]` is the analog table's F0, and `LP_UART_DTRN_PAD [F0]` is the LP IO MUX F0 | Invented codes `LP_F0`…`LP_F3` and `ANALOG` | **Neither is correct.** Run 1 is ambiguous, and the reference uses codes that no table contains. Fixed by `mux_table` (below). |
| Matrix-only peripherals | 15 declared with roles that no pin takes (UART1–3, I2C0/1, I2S0/1, SPI3, CAN_FD, LEDC, MCPWM, RMT, PCNT, PARLIO, EMAC MDC/MDIO) | Not declared | **Reference right.** A peripheral that no function takes cannot be placed, and it misleads the pin-mux panel (S31 Table 2-8 note 2, p.32: these signals reach pins through the GPIO matrix). The prompt told the model to declare them; that instruction is now changed. |
| Row source pages | 0 of 99 rows | 99 of 99 | **Reference right.** The run cited pages only in the report. The prompt now asks for a source page on each row, and `submit_review` now runs `library.review_check`, which refuses run 1's report for it: 91 imported pins have a function without a source page. The replay ends there, refused. |
| Pull options | None | `pull: none/up/down` on GPIO pins | **Reference better.** The GPIOs have configurable pulls. The prompt now asks for them. |
| Pin rules | 8 rules with the same intents and pages (WROOM-3 Tables 4-1, 4-3, 4-4, 4-7 pp.18–22; S31 Table 2-5 note 2 p.26). All 8 call `pin_used`/`function_used`, which do not exist, and use `if` without `else` | 8 rules on the documented helpers. They cite S31 Tables 3-1, 3-4, 3-7 pp.36–40, which are equivalent tables | **Reference right; run 1 broken.** Replayed now, all 8 of run 1's rules fail evaluation (`rule.runtime`). All 8 of the reference's rules hold over their samples. The model reported the API as undocumented, which was true of the schema it received. |
| Pads, ground array, corner pads | Module form: rows at x = ±10.3, y = 8.3 / −14.3, pitch 0.85, 0.4 × 0.8 pads, 3 × 3 × 1.8 mm array at 45°, pitch 2.35, centre (0, −3), 0.8 mm corner pads | Same values | **Identical** (107 pads). Both agree with WROOM-3 Fig. 10-1 p.64 / Fig. 11-1 p.67: 20.6 and 22.6 between pad centres, 0.7 from the edges, 20.4 = 24 × 0.85, 17.85 = 21 × 0.85, 6.5 = 3 × 1.8 + 2 × 0.55, array centred in the 22 × 24 mm pad body. **Both miss** the chamfered antenna-side array pad (Figs. 3-1, 10-1). |
| Outline, can, keep-out | 22 × 30 × 3.5 mm, PCB 1.0 mm, can 21 × 23 mm 0.5 mm from the sides and bottom, keep-out = 22 × 6 mm antenna area | Same | **Identical** and as drawn (Fig. 10-1 p.64). The host-board clearance beyond the module is not stated in the datasheet. Both leave it open. |
| 3D model parameters | Generated from the same form | Same | **Identical.** |
| Marking | Part-name text and a polarity dot at the can centre | Part-name text | **Neither is correct.** The top view (Fig. 10-1 p.64) shows a Ø0.5 mark at the can's −x/−y corner, not at the centre and not at pin 1. Run 1's dot overlaps the text in the wrong place. Marking anchors offer only centre, pin-1 corner and the ends, so that corner cannot yet be expressed. |

The pinout content of run 1 is at least as good as the reference: it adds the DAC channels, corrects the LP SPI directions and keeps the datasheet labels. The tooling caused its defects: a rule API missing from the schema the model received, a mux format with no place for the table, and a prompt that asked for matrix-only peripherals. Its geometry is identical to the reference.

## Tooling changes from this run

| Problem in run 1 | Change | Test |
|---|---|---|
| (a) Rule API missing from the schema; rules never evaluated | `library.pinout_schema` returns `rule_api`, the full script contract with examples, and the `script` field documents the helpers. `library.pinout_preview` evaluates each rule without selections, with each function, peripheral or role it names literally made active, and with all of those together. It lists the results per rule (`rules`: default verdict, samples, verdicts, failures). Every component check does the same, so a failure is a blocking `rule.runtime`/`rule.reference`/`rule.budget` finding. An unknown helper is named as such. | `publishing_evaluates_each_rule_with_the_functions_it_names_active`, `the_preview_evaluates_each_rule_over_the_samples_it_names` (core); `the_first_live_runs_guessed_rule_helpers_fail_at_preview` (replay of run 1: all 8 rules reported in the preview the model received at request 7) |
| (b) Mux code collisions | `mux_table` beside `mux` on pinout functions and component functions, validated (`function.mux_table`). It appears in the pins report, the firmware IR, C header (`_MUX_TABLE`), Rust consts and Zephyr comments, the Pins tab (editable) and the pin-mux panel (`LP IO MUX F0`). | `mux_tables_travel_with_their_codes` (core), `mux_tables_are_named_beside_their_codes` (firmware) |
| (c) Two replies cut off at max_tokens | The runner keeps a working pinout document. `library_pinout_edit` upserts header, peripherals, at most 40 rows and the rules per call. Preview takes no document, and apply takes only the base and imports exactly what was previewed (refused when the document changed since). The request after a cut-off gets 64 000 output tokens (once). The request timeout is now 30 minutes. | `a_recorded_run_authors_the_module_and_submits_its_review` (chunked flow, apply without document), `the_first_live_run_replays_with_its_recorded_usage` (limits 32 000 → 64 000 after each cut-off) |
| (d) Context growth 453 k → 594 k | Previews reply with change counts, findings and review items without object lists, plus the rule checks. In run 1 the full-document preview drops from 73.7 k to 9.0 k characters, about 32 k to 4 k tokens. The document is no longer sent twice. Earlier turns are **not** trimmed or rewritten (see below). | `the_first_live_runs_guessed_rule_helpers_fail_at_preview` (compact preview < 20 000 characters), `every_request_extends_the_last_one_so_the_datasheet_cache_holds` |
| (e) Reviewer ergonomics | `review.md` adds a Pin rules table (verdicts and failures over the samples), a Pins table (every package pin with default, functions `[table code] PERIPHERAL.role direction`, source pages and flags) and a Requests table (stop reason, tokens, tools per request). `review.json` carries the same data. `pcbcraft-cli review-diff A.pcbcraft B.pcbcraft` diffs two runs pin by pin, rule by rule and pad by pad. | `the_review_diff_compares_the_run_with_the_reference_by_pin` |
| Prompt | Build the document in chunks, give `mux_table` where tables share codes, give source pages, directions from the type column, pulls, DAC; declare no matrix-only peripherals; write rules against `rule_api` with `else` on every `if`; don't probe the tools; add a polarity mark only where drawn. | Replays above; needs a live run to confirm behaviour. |

**Why superseded previews are not trimmed.** `claude-opus-5-5` binds each thinking block to the exact conversation before it. Editing an earlier tool result invalidates every later thinking block, and accounts created on or after 31 August 2026 get a 400 for it. Any edit also rewrites the cached conversation from that point. Server-side context editing is allowed, but it re-writes the cache on each clear and writes more tokens than it saves at this context size. So the fix is to send less. The breakpoints were checked: tools and system, the last datasheet block, then automatic caching of the tail (3 of 4). Every request after the first read the full datasheet prefix from the cache: 453 k cache reads at request 2, rising to 581 k. The test above asserts that every request extends the previous one unchanged.

## Tokens

Run 1, from the recording:

| Item | Tokens |
|---|---|
| Datasheets and tools written to the cache (request 1) | 453 125 |
| Output (thinking and tool inputs) | 150 455 |
| Cache reads (13 requests × 453–581 k) | 6 211 969 |
| Other cache writes (each turn's new content) | 141 878 |

By request: 1 wrote the datasheets to the cache. 2 produced 18.5 k output, mostly reading. 3, 4 and 7 were previews that tested guessed rule helpers, each also writing the previous turns to the cache, including the 32 k-token full preview plan. 5 and 8 were replies cut off at 32 k output. 6 wrote the full document in one call (31.2 k output). 9 sent the document again to apply it (27.9 k). 10–13 were package, marking, checks and report.

The changes above remove the probe previews, the cut-off replies and the second copy of the document, and shrink the previews; writing the document in about 3 edit calls adds about 470 k cache reads each. The datasheet cache write is then the largest single item. Leaving out documents the run does not need (it cited only the module and chip datasheets) is the next lever.

## To confirm with a second live run

- The model reads `rule_api` and writes working rules without probing, and fixes any rule the preview reports.
- It builds the pinout with `library_pinout_edit` in chunks of at most 40 rows, applies by base only, and nothing is cut off at max_tokens.
- It sets `mux_table` on LP IO MUX and analog functions and declares no matrix-only peripherals.
- A 64 000-token retry returns within the 30-minute non-streaming timeout. This path is untested live: run 1's cut-offs happened before this change.
- Measured tokens per request against run 1's, from the new Requests table.
- Compare the second run with run 1 and the reference using `pcbcraft-cli review-diff`.
