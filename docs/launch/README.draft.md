<!--
  Launch draft of the repository README. Replace README.md with this file at launch.
  Media slots: files under docs/launch/media/ are produced by tools/launch/ and are not committed
  until the final recording (see docs/launch/video-storyboard.md). Every claim below is checked
  against docs/editor-evidence.md and the as-built notes it links; re-check before publishing.
-->

<h1 align="center">Sparksmith</h1>

<p align="center">
  <b>PCB design where the board in 3D <i>is</i> the board.</b><br>
  Components, schematics, PCB layout and a photoreal, thickness-aware 3D view in one Rust app —<br>
  with one command engine that the UI, the CLI and your AI agent all drive.
</p>

<p align="center">
  <img src="docs/launch/media/hero.webp" alt="The 4-layer showcase board orbiting in the integrated 3D view: green lacquered soldermask, gold header pins, chip parts with solder fillets" width="900">
</p>

<p align="center">
  <a href="docs/launch/media/sparksmith-tour.mp4"><b>▶ Watch the two-minute tour</b></a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#drive-it-from-an-agent-mcp">MCP</a> ·
  <a href="#status">Status</a>
</p>

---

> **Early and honest.** Sparksmith is a working editor foundation, not yet a manufacturing-ready
> release. The features below exist and are tested; the [status](#status) section says what is
> still missing. Numbers are measurements from the linked notes, never targets.

## What makes it different

**One engine, three front doors.** Every design change — a wire, a via, a published component —
is a typed operation on one transactional engine. The desktop app, the `sparksmith` JSON-lines
protocol and the MCP server send the *same* operations, share one undo history, and a refused
operation leaves the document untouched. An agent cannot do anything a person could not undo.

**Components from datasheets, with a Claude agent.** *Create from datasheet…* hands the PDFs to a
Claude tool-use loop that drafts the component through the engine: pins and their multiplexed
functions, Rhai pin rules, the package from a cited template, footprint, binding and markings. It
only ever creates drafts and never publishes. Every value comes back in a review report with its
datasheet page and a confidence level. The first live run drafted the ESP32-S31-WROOM-3 module
in 13 requests ([run 1](docs/datasheet-assistant-run1.md)).

**Copper is real geometry.** Tracks, pads, plated barrels, vias and fills are thickness-aware
solids built by [Solidcraft](https://github.com/tangency-labs/solidcraft)'s parametric engine,
not decals on a plane. The PCB's 2D and 3D views are two views of one design, with shared
selection, net highlighting, properties and undo. Click a pad in 3D and the whole net lights up.

**It looks like the board you will get back.** A physically based studio look: lacquered
soldermask with orange peel and the FR-4 glass weave showing through, ENIG or HASL lands, tented
via domes, sectioned board edges, solder fillets. Eight soldermask colours, three mask finishes and
five pad finishes are one setting each.

**Push-and-shove routing.** The Route tool shoves other nets' tracks and vias aside, walks round
what cannot move or ignores it (**M** cycles the mode). Each click is one atomic transaction and
one undo step. Planning a pointer move on a 10,000-via, 1,500-track board takes at most 11.6 ms
in the worst measured case, inside one 60 Hz frame ([push-and-shove](docs/push-and-shove.md)).
Differential pairs route coupled at the gap derived from the stackup's impedance, and the Tune
tool adds length meanders ([high-speed routing](docs/high-speed-routing.md)).

**Production files in one command.** Gerber X2, Excellon, a job file, pick-and-place, BOMs and
PDF fabrication and assembly drawings, behind a production check with saved, reasoned waivers.
Every written layer is read back by an original Gerber reader and compared with the design:
0 mm² difference on every layer of the 4-layer showcase ([fabrication output](docs/fabrication-output.md)).

**Parts with a face.** Per-instance markings — top marks, value codes that follow the value,
polarity marks, colour bands, laser-etched text on metal cans, QR codes — wrap planar faces and
cylinders in the 3D view. They are display-only: they never enter the saved model or the
fabrication output ([markings](docs/component-model.md#markings-on-any-surface-d7-as-built)).

**Parts are parametric Solidcraft designs.** Package bodies are generated per package revision,
imported from STEP, GLB or `.solidcraft`, or — new — authored as ordinary parametric `.solidcraft`
part files: the ESP32-S31-WROOM-3 body is one, 110 bodies and 24 parameters, matching the earlier
generated model body for body ([lean parts](docs/part-templates.md#l5-as-built-3-november-2026)).
The whole board exports as a native, parametric `.solidcraft` model that plain Solidcraft reopens.

## Tour

| | |
| --- | --- |
| <img src="docs/launch/media/component-3d.webp" alt="Component 3D tab: a generated package model with markings"> | **Component authoring.** Symbol, Footprint, Pins, Mapping and 3D tabs over five revisioned drafts. 196 cited or estimated package rows over eight families build the package, its lands and a generated 3D model in one step. Footprints are sketches with dimensions and constraints. Publishing creates immutable revisions; placed instances keep the exact revision they use. |
| <img src="docs/launch/media/schematic.webp" alt="Schematic with sheets, net labels and the Place palette"> | **Schematic capture.** Several sheets with a hierarchy, net labels, power ports, title blocks, annotation, ERC and vector PDF printing. Press **P** and type `res>=10k 0603`: the Place palette searches with units (50,000 entries at ≤ 3.85 ms p95). The pin-mux panel assigns MCU peripherals with a solver that respects the part's pin rules. |
| <img src="docs/launch/media/route-shove.webp" alt="Route tool shoving a neighbouring track on the showcase board"> | **PCB layout.** Explicit schematic-to-board updates with a preview, placement, 45°/90°/free routing on any of 2 to 64 copper layers, vias, fab profiles and net classes, copper fills with thermals and hatching computed off the UI thread, and parametric board outlines with named edges. |
| <img src="docs/launch/media/board-3d.webp" alt="The showcase board in the 3D view"> | **Integrated 3D.** One keystroke from 2D. Fit, a navigation cube and trackpad gestures; a Layers popover hides mask, silk, copper layers or parts without rebuilding geometry. A finished 10,000-via board draws its first 3D frame in 8–37 ms in the running app. |
| <img src="docs/launch/media/production.webp" alt="The Production files dialog with its pre-flight report"> | **Production output.** Profiles, a pre-flight report computed on a worker, waivers with reasons, Folder or ZIP. The showcase's whole export takes 0.13 s; a 200 × 200 mm plane over 10,000 vias is generated in 0.12 s. |

<!-- Media slots: component-3d, schematic, route-shove, board-3d, production (.webp stills or short
     loops, 1200 px wide, from tools/launch). The proof take covers route-shove and component-3d. -->

## Quick start

Sparksmith builds with the Rust toolchain pinned in `rust-toolchain.toml` (1.96.0 with Clippy,
rustfmt and the wasm target). Use rustup, or the Nix dev shell, which provides exactly that toolchain:

```sh
git clone <repository-url> sparksmith && cd sparksmith
nix develop                     # optional: the pinned toolchain without rustup

# a routed 4-layer board with vias, fills and an inner plane
cargo run --locked --profile ci -p sparksmith-cli -- showcase showcase.sparksmith
cargo run --locked --profile ci -p sparksmith -- showcase.sparksmith
```

Things to try on the showcase:

- **R** arms Route. Click a pad, move towards another net's track and watch it shove; **M** cycles
  Shove → Walkaround → Ignore; **V** changes layer through a via; **Backspace** unwinds a step.
- **D** switches the PCB between Layout and 3D. Pinch or scroll to zoom, Shift+two fingers or
  right-drag to orbit, **1–7** for standard views.
- **⌘P** finds any command; **⌘/** lists every shortcut; hold **⌘** to see key badges.
- Ribbon **Output › Production** writes the Gerbers, drills, BOMs and drawings.

Other starting points:

```sh
cargo run --locked --profile ci -p sparksmith-cli -- parts parts.sparksmith   # 24 invented EX- parts for the Place palette
cargo run --locked --profile ci -p sparksmith -- examples/esp32-s31-wroom-3/esp32-s31-wroom-3.sparksmith
cargo run --locked --profile ci -p sparksmith-cli -- fab showcase.sparksmith out/   # production files from the CLI
cargo xtask ci                  # formatting, layers, licences, clean room, strict Clippy, tests, wasm
```

Developed and tested on macOS (Apple silicon). The Nix shell carries the Linux runtime libraries,
but Linux and Windows have not been checked as products yet.

## Drive it from an agent (MCP)

`sparksmith mcp` serves the engine as a Model Context Protocol server on stdio: the same typed
operations as the UI (`project_apply` with an expected revision), queries (`project_query`: library
search, component checks, BOM, pin reports, the production pre-flight), undo and redo, save and the
exports. Replies are compact (the outcome, the ids that changed, new findings) and `verbose: true`
returns the whole document; `project_schema` serves each operation's exact schema on demand, and
`sparksmith commands` prints every schema, generated from the Rust types.

Build the CLI once, then register it with your MCP client. For Claude Code:

```sh
cargo build --locked --profile ci -p sparksmith-cli
claude mcp add sparksmith -- "$PWD/target/ci/sparksmith" mcp "$PWD/showcase.sparksmith"
```

(Inside `nix develop` the binary is at `target/nix/ci/sparksmith`.) Any MCP client works the
same way: the command is `sparksmith mcp [FILE.sparksmith]`. Then ask, for example: *"Find the
unrouted connections on the showcase and route GND with a via to the inner plane."* The agent
works on a headless session of that file and saves it through `project_save`; open the file in the
desktop app to see the result. The desktop window does not yet follow an agent's edits live.

The same requests run over JSON lines without MCP:

```sh
cargo run --locked --profile ci -p sparksmith-cli -- stdio showcase.sparksmith
{"command":"project.query","params":{"query":"library.search","params":{"query":"res>=10k pkg:0603","limit":10}}}
```

To draft a component from datasheets (needs an Anthropic API key, held in memory only):

```sh
ANTHROPIC_API_KEY=… cargo run --locked --profile ci -p sparksmith-cli -- design-part \
  --datasheet PART.pdf --name PART --out part.sparksmith   # drafts + PART.review.md; publishes nothing
```

The desktop has the same as **Create from datasheet…** on the Component start page.

## Architecture

| Crate | Responsibility |
| --- | --- |
| `sparksmith-core` | Pure, wasm-compatible document, typed operations and queries, the one `Engine` with shared undo, checks, fills, routing, fabrication generation and the file format. |
| `sparksmith-io` | Bounded reads, atomic writes, crash recovery, library folders, fabrication and PDF output. |
| `sparksmith-solidcraft` | Builds the native Solidcraft model through a typed facade over Solidcraft's command API. |
| `sparksmith-firmware` | Pin-configuration exports: JSON IR, C header, Zephyr overlay skeleton, Rust constants. |
| `sparksmith-automation` | Headless `Session` shared by the JSON-lines transport and the MCP server. |
| `sparksmith-assist` | The in-app datasheet runner: a model tool-use loop over the automation session, with request, token and time limits. |
| `sparksmith`, `sparksmith-cli` | The egui/wgpu desktop editor and the command line (its binary is `sparksmith`). |

`cargo xtask ci` enforces the layer direction, the core module map, Solidcraft's UI containers,
dependency licences, the clean-room rule and strict Clippy, runs the test suite (over 1,300 tests at
the last recorded gate) and compiles the core for `wasm32-unknown-unknown`.

Files are atomic `.sparksmith` ZIP containers holding the exact revisioned library snapshot, so a
project reopens offline and a library edit never changes an existing board without a reviewed
update. Unsaved work is snapshotted and offered back after a crash.

## Status

Working today: component authoring with templates, pin functions and 3D models; multi-sheet
schematics with ERC and PDF; schematic-to-board updates; placement and routing with push-and-shove,
walkaround, differential pairs and length tuning; design rules, net classes and fills; the
integrated 3D view; production output with a pre-flight check; MCP, CLI and the datasheet assistant.

Not there yet, so you know before you try:

- **Not a manufacturing-ready release.** No first board has been fabricated from Sparksmith's
  output yet; the built-in fab profiles are estimates, and zero findings is not a DRC sign-off.
- **Routing:** dragging existing tracks with shove is planned; differential pairs and tuning do not
  shove; no autorouter; through vias only (no blind or buried vias).
- **Schematic:** no buses, and a child sheet cannot yet be reused under several sheet symbols.
- **High speed:** impedance is quasi-static closed-form (no field solver, no mask, etch or roughness).
- **3D:** no cast shadows from the key light; a realistic render mode with material graphs is in
  progress; very large boards still build slowly (10,000 vias with 2,000 tracks: 8.5 s).
- **Outputs:** no IPC-2581 or ODB++; no assembly variants or DNP flags.
- **Desktop:** the window does not follow MCP edits live; on macOS ⌘Q quits without the
  unsaved-work prompt (the work survives in recovery).

What comes next, in order: the first real board (an ESP32-S31 dev board, designed in Sparksmith now),
then track drag with shove, the realistic view mode, and the remaining first-release editors.
The [roadmap](ROADMAP.md) tracks execution; [editor evidence](docs/editor-evidence.md) records
what was verified and how.

## Licence and attribution

Sparksmith is licensed under the [MIT License](LICENSE-MIT) or the [Apache License, Version
2.0](LICENSE-APACHE), at your option, like Solidcraft.

Sparksmith builds on [Solidcraft](https://github.com/tangency-labs/solidcraft) (MIT OR Apache-2.0)
for its UI kit, renderer and parametric CAD engine, pinned to a fork with a product look and via
scaling. The UI fonts are IBM Plex Sans and Mono (SIL Open Font License). Third-party notices are
in [NOTICE](NOTICE) and [ATTRIBUTION.md](ATTRIBUTION.md).

Sparksmith is an independent clean-room implementation. It uses public specifications (Ucamco's
Gerber and job file formats, the Excellon description) and permissively licensed dependencies; it
contains no code or library data from GPL-family or commercial EDA tools, and `cargo xtask
clean-room` rejects EDA library files. Manufacturer CAD data is used only like a datasheet, with
its source and licence recorded. All example parts are invented (`EX-…`) or drafted from public
datasheets.
