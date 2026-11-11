# Board geometry for an LLM: design and benchmark plan

**Status:** proposal, 5 November 2026. Nothing is built, and nothing here has been measured. The encodings below are candidates for the benchmark to decide between. Examples are illustrative. Estimates are rough agent hours. No hosted model run starts until the founder approves the [limits](#limits).

## Goal

An LLM should be able to reason about a board and route it through MCP. The founder's requirement: "the most compact, LLM-friendly way of encoding this spatial information, but so that we can have tooling that can convert it to human-understandable concepts."

So the design has to deliver four things:

1. **Few tokens**, so a whole board can be worked on within a context and a budget.
2. **Reliable reasoning**, measured as routes that pass the checks, not as how readable the text looks.
3. **Traceable to a person:** anything the LLM read or wrote can be shown on the board.
4. **One engine:** nothing bypasses the typed commands or the design rules.
5. **Any LLM:** Pcbcraft is an LLM-first product (founder, 5 November 2026), and that includes local models. So nothing here may depend on one provider, and the encoding has to work with models that have small contexts and no vision.

## Starting point

- Routing commands exist: `layout.route_step`, `route_preview`, `route_pair`, `length_report`, `tune_preview`, with walkaround and shove. `gap-mcp` is making replies compact, and `gap-board` is adding `detour_factor` and `max_detour`.
- There's no query that describes the space on a board. A client today would read the snapshot's coordinates.
- The ESP32 run routed its 187 steps through a scripted client. No LLM has yet routed a board in Pcbcraft, so there's no evidence about what helps one do it.

## Design principles

**Say intent; let the engine do the geometry.** Exact coordinates and clearance maths are what an LLM is worst at and what the router already does well. An LLM should decide topology: which layer, which side of a part, where to change layers, which order. The router turns that into legal copper, or refuses and says why.

**Name things; don't quote coordinates.** `U1.42` is shorter than a coordinate pair, can't be mistyped into a legal-looking location, and is already the id a person sees.

**Show a small window, on request.** A board is too much to read at once, and routing a net only needs its neighbourhood.

**Same data, two renderings.** Every view comes from one query result, rendered as text for the LLM or as a picture for a person. Each symbol traces back to an object id.

**The reply teaches.** A refusal says where and why in the same language, and suggests the next move, so a failed attempt costs little.

## The three layers

### 1. Overview: what's left to do

```
board 40x64mm 4L top/GND/PWR/bot rules jlcpcb-4-layer-1oz
unrouted 12: USB_DP U1.42~J1.A6 23.5mm | VBUS J1.A4~U2.1 18.2mm | ...
congestion(top) 8x13 cells, 0-9:
  0013100
  0257300
```

### 2. Window: what's around here

A text grid per layer, in cells sized from the clearance, with the named objects listed beside it:

```
top x10-20 y4-9 cell0.5 g=GND
..##..aa..
..##..aa..
gggg..##..
a=U1 #=copper(other net) g=fill(GND) .=free
pins: U1.42@7,0 U1.43@8,0
```

Candidate alternatives are in [Encodings](#encodings).

### 3. Route: state a move

An LLM can state a net's route at three levels, from the most to the least delegated:

| Level | Example | Who decides the path |
|---|---|---|
| Goal | `route USB_DP U1.42 J1.A6 layer=top` | The router |
| Guided | `route VBUS J1.A4 U2.1 via=right-of:C3,left-of:R1 max_detour=1.3` | The LLM picks the side; the router picks the copper |
| Path | `route SDA U1.12: E6.0 NE2.4 via W3.1` | The LLM, in directions and lengths; the rules still apply |

The path form uses eight compass directions and millimetres, relative to a start pin, which converts directly to route steps. All three go through `layout.route_step` or `route_pair`, so undo, revisions and the rules are unchanged.

A reply is short:

```
ok USB_DP top 24.1mm detour1.03 vias0 clearance-min0.21
refused VBUS at x18.2 y7.4: needs 0.2mm to C3.1, has 0.12. Try: right-of:C3 or layer=bot
```

## Human tooling

These are part of the design, not an extra:

- **`pcbcraft-cli view`** prints or renders any query result. The same call with `--format png` draws that window, with the same labels.
- **A trace of a session:** each request, the window it read and the copper it produced, as a filmstrip of pictures you can step through. It also makes a good demo clip for the launch video.
- **A desktop overlay** can come later: highlight what the LLM last read and the route it proposed, as a preview to accept or reject.

## Encodings

The benchmark compares these, all with the same route commands:

| | Encoding | Why it might win | Why it might lose |
|---|---|---|---|
| A | Object list as compact JSON, in a window (baseline) | Exact, no invention | Coordinates are hard to reason over; more tokens |
| B | Text grid plus named objects | Space is explicit and compact | Long-distance relations; misreads along a row and column |
| C | Labelled picture of the window | Vision may be the natural way to see space | Image tokens; fine details at low resolution |
| D | Relations only: free corridors and neighbours as a graph (`U1.E -> gap(C3,R1) 0.8mm -> J1.N`) | Closest to how routing decisions are made | Harder to build; can hide what it leaves out |

The winner may be a mix, such as B or C for a window plus D for corridors. That gets a run of its own only if the first results suggest it.

## Benchmark

### Tasks

From `examples/esp32-s31-devboard`, with the tracks of the target nets removed and the placement, fills and rules kept:

| # | Task | What it tests |
|---|---|---|
| T1 | A short signal net, 2 pins | Basic use |
| T2 | VBUS: power width, around parts | Detours and class rules |
| T3 | The USB pair | Coupling and fan-in |
| T4 | A header bus on the bottom layer | Order and crossings |
| T5 | All signal nets on the board | Whether it holds up at scale |

A **held-out board** takes the winning encoding at the end, so the result can't simply reflect tuning on the ESP32. I'd use the shipped showcase.

### Grading

The engine does the grading, with no LLM judge and no judgement calls in the numbers:

- nets completed, and connectivity checked against the netlist;
- DRC errors introduced, and new warnings;
- **quality by kind of net** (founder, 5 November 2026), each computed from the final copper, so the same board always gets the same numbers, and compared with the example's existing routing:

  | Kind | Grade | Needs |
  |---|---|---|
  | Point to point | Detour and avoidable layer changes: a change needed because source and target are on different layers doesn't count | Nothing new |
  | Bus or multi-drop (SPI) | For each source and sink: length, layer changes, branches off the path, and skew across the bus | Source and sinks per net: from function directions where known, otherwise set by the user |
  | Power and ground | Resistance from the source to each load. Inductance comes later, because it needs the return path | Net marked as power; source and loads; a resistance solver for tracks, vias and pours |
  | Differential pair | Skew and coupling, already reported | Nothing new |

  These stay separate numbers per kind, not one score. **The power row's solver is a future idea (founder, 5 November 2026):** the end-to-end PCB design process comes first. Until then, power and ground are graded on connectivity and DRC. Likely approach when it comes: a conductance grid over the copper, solved with `faer` (MIT);
- pair skew and coupled length on T3;
- input, cached and output tokens, tool calls and time.

**Headline number:** tokens per correctly completed net, because a short attempt that fails takes more tokens in the end. The founder also gets pictures of each best and worst result, since a route can pass every check and still look wrong.

### Method

- **Models:** `claude-opus-5-5` at a fixed, explicit effort as the reference, because it's the hosted model we have measurements for, plus Qwen 3.8 Flash Next on the local Mac, through an OpenAI-compatible endpoint, and possibly smaller local models. A local model needs no hosted account but takes longer. An encoding that only works on the largest model doesn't win outright; the report gives results per model. Encoding C needs vision and is skipped on models without it.
- **Runner:** talks to models through a provider interface, not the Messages API directly, and records in one neutral format, so each run can be replayed and audited offline. Results are appended only, because editing earlier tool results invalidates Opus 5.5's thinking blocks. Provider features such as prompt caching are optional extras, and no result may depend on them.
- **Comparison:** each encoding gets the same tasks, repeats and prompt except for its description of the encoding. The prompt isn't tuned per encoding before the first results, since that would favour whichever gets the most attention.
- **Key:** injected per command from the Keychain, as in `AGENTS.md`; recordings must not contain it.

### Stages

| Stage | Work | Model use |
|---|---|---|
| 0. Tooling | Queries, renderers, route forms, task setup, grader, runner. Needs `gap-mcp` merged. About 8–12 h | None |
| 1. Offline | Token counts of every encoding on the same windows. Round-trip tests: every symbol maps to the right object. Anything clearly too big is dropped | Token counting only |
| 2. Pilot | T1 and T2, each encoding, 2 repeats | Live runs |
| 3. Main | T1–T5, surviving encodings, 3 repeats. T5 is one run per encoding at first | Live runs |
| 4. Held-out | Winner on the second board | Live runs |

Stop and report after stage 2 if nothing completes T1 reliably, because then the problem is the route language or the tools, not the encoding.

### Limits

The runner caps every run by requests, tokens and time, stops a run at its cap and records it as failed. The pilot measures tokens per task; those numbers go to the founder before the main run.

### Deciding

The encoding that wins is the one with the fewest tokens per correctly completed net, as long as it introduces no DRC errors, on T1–T4. If two are within about 20%, the one simpler to maintain wins. If none completes T3 and T4 reliably, the honest outcome is that LLM routing stays at the goal level, with the router doing more, and the report says so.

## After the benchmark

- Ship the winning view and route commands as MCP tools.
- Add a route-board skill or prompt with what the benchmark taught.
- Keep the benchmark as a regression test, run on demand because it calls hosted models, with the offline token counts in normal CI.

## Risks

| Risk | Handling |
|---|---|
| The router does too little for goal-level commands to work | Shows up in T2 and T4. Router fixes then come first, and they benefit people too |
| Results reflect this one board | The held-out board |
| Text and picture drift apart | Both render from one query result, and the round-trip test checks symbols against objects |
| Few repeats give noisy results | Report ranges. A narrow win is treated as a tie |

## Open questions for the founder

1. **Scope of autonomy:** the founder chose to test first (5 November 2026). The benchmark's results decide between unattended routing and proposing routes to accept.


**Local model:** the founder suggested Qwen 3.8 Flash Next and left experiments open. First results are in [LLM integration](llm-integration.md#local-models-first-measurements).
