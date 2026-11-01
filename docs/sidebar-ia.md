# Right sidebar and feedback: information architecture

Status: **contract (slice S0)**. This page fixes the information architecture (IA) of the right column, the Issues surface and transient feedback, plus the code contract the implementation slices build on. Behaviour does not change until those slices land. It condenses the reviewed design from the 2026 UX audit; the founder decisions it records are in [Decisions](#decisions).

Vocabulary: a **card** is one bordered panel in the right column. The **subject slot** is the upper card. **Issues** is the list of document findings. A **refusal** is an operation the engine rejected.

## Problems this solves

The audit found these problems, most harmful first:

- One unbounded scroll mixes object properties, tool settings, command forms, findings, help and component data. Some cards run 3–6 screens.
- Errors repeat across five channels: findings, the editor `notice`, inline errors, the canvas chip and Checks.
- Findings are verbose, all red and ungrouped, and most are not clickable. PCB checks are capped at 40, so some can never be seen.
- Review panels dump engine state ("Candidate PCB check:", Debug output, IDs).
- Tool settings look like object properties.
- Content moves under the pointer.
- Command errors appear far from where they were made, in engine wording.
- Internal IDs and codes leak into text.

## Decisions

| # | Decision |
| --- | --- |
| D1 | Issues is a card stacked under the subject, collapsed to an always-visible 28 pt bar. |
| D2 | Checks run live, cached per document revision. The ribbon's Check becomes the Issues toggle. Off-thread computation is a **target**, done only if measurement needs it. |
| D3 | The card commits live, per field. Paired values commit together. Apply exists only in dialogs. |
| D4 | Component-level data moves to a new **Details** tab, first among the Component tabs. |
| D5 | Tool settings show in a tool card in the subject slot. |
| D6 | Unrouted connections, unplaced instances and unresolved purchasing have severity **To do**. Publish blockers are always Error. |
| D7 | Solidcraft kit internals are ported locally with attribution. The pinned Solidcraft fork is not changed. |
| D8 | Core owns issue titles and details through `project.issues` (`Query::Issues`), so CLI and MCP show the same words. |
| D9 | Impact reviews are docked in the subject slot, with the canvas preview visible. |
| D10 | Resolve parts is a table `ui_kit::Window`, like Library sources. |
| D11 | Layer and view toggles move to a navigation-bar popover. |
| D12 | The surface is called **Issues**. |
| D13 | The pcbcraft-ui skill's bootstrap line now allows this fixed column layout (still not a docking framework). |

## Principles

These are rules for every slice. [pcbcraft-ui](../skills/pcbcraft-ui/SKILL.md) carries them as skill rules.

1. **One card, one subject.** The card shows the selection or, with nothing selected, the workspace's document object (component, sheet or board). The header (icon, name, kind chip, "…" menu) names it; the body never repeats the name. A multi-selection shows the shared properties and a count.
2. **Tools configure in a tool card, only while armed.** A tool card never shares a title with an object section. Tool help is one hint line.
3. **Commands with inputs are dialog panels.** Each has a 40 pt header and a body capped by `body_rows`, which is the only scroll. The 48 pt footer is pinned, with Cancel and then the primary. The refusal shows once, as the last body row. A disabled primary explains itself on hover. A destructive choice is the primary's variant, never a middle button.
4. **Problems have exactly three homes:**
   - Issues, for document findings;
   - inline field errors, for invalid input in one field;
   - the refusal row of the dialog or review that was refused.

   The card carries only a reserved one-line issue slot that links into Issues. No problem shows in two homes at once. A review's blockers are its own content.
5. **Nothing that blocks a stage is ever hidden or uncounted.** The Issues bar is always visible, including while a dialog, tool card or review is open. It counts every blocking finding. Folding and merging change presentation only: the count, every `ObjectRef` and the raw text survive in Copy report and Messages. No finding is suppressed because the user is mid-command.
6. **One line per message, details on demand.** A title has at most 80 characters, clipped, with the full text on hover. It names objects, never IDs, codes or Debug output.
7. **Every issue locates.** A click selects and frames every referenced object, for every `ObjectRef` kind, across workspaces and tabs. A Fix button appears only when a typed command, or a dialog that issues one, resolves the issue.
8. **Core decides severity.** Error (×) is the only red; then Warning (!), To do (○) and Info (i).
9. **Feedback is transient and single-channel.** Toasts carry command results and gesture refusals. The Messages window holds the history. Pills show persistent modes. The editor `notice` string is retired.
10. **Help on demand.** Use one clipped hint line with a tooltip, control tooltips, the canvas hint or an empty-state box. Never repeat the canvas hint.
11. **Bounded everywhere.** The card fills the column. A list longer than 8 rows goes into a bounded frame with "Show all". Nothing is silently capped. No scroller nests inside another, except a bounded list frame. **Target** (until measured): every default state fits one screen at 860×1081 pt.
12. **Stable layout.** Nothing is inserted above the control being edited. Footers are pinned and the issue slot is reserved. The scroll offset resets when the subject or mode changes and is kept while editing.
13. **One commit model in the card: live, per field.** A field commits on Enter or focus loss, as one typed command and one undo step. Paired values (position x/y, size w×h) commit together. Invalid input never commits and is marked; Esc reverts. Staged multi-field commits exist only in dialogs.
14. **Same structure in 2D, 3D and every workspace.** 3D hides 2D-only tools and layers. Selection, Issues and undo are shared.

## Layout

```
┌ Ribbon ─────────────────────────────────────────────────────────────────────┐
│ [Select][Route][…]                  │ Publish │ … │ [× 4 ! 2 ○ 36  Issues] │
├─────────┬───────────────────────────────────────────────┬───────────────────┤
│ Browser │        ( Routing N22 · 0.3 mm  × )   ← pills   │ Properties /      │
│ + issue │                canvas                          │ tool card /       │
│ badges  │                                      ┌───────┐ │ dialog / review   │
│         │                                      │× 4 ! 2│ │  header 40 pt     │
│         │                                      └───────┘ │  body (scroll)    │
│         │ hint line (bottom-left)       toasts (centre)  │  footer 48 pt     │
│         │                                                │───────────────────│
│         │                                                │ Issues bar 28 pt  │ ← always visible
└─────────┴───────────────────────────────────────────────┴───────────────────┘
```

- `app/chrome.rs` `workspace_area` owns the column. The subject slot holds exactly one of: Properties card, tool card, dialog panel or review panel.
- Collapsed, Issues is a 28 pt bar. Expanded, the subject keeps its header and 10 rows, or half the column if that is more, after upstream `stack_first_bounds`.
- The column is a fixed layout, not a docking framework, as [the product plan](pcbcraft-plan.md) asks ("Initial panel layouts should be fixed and coherent").
- Toasts sit bottom-centre of the canvas, clear of the bottom-left hint line. The canvas count badge sits top-right.

## Properties card by subject

Sections are listed in order. Every card ends with its issue slot.

| Workspace / subject | Card | Moves elsewhere |
| --- | --- | --- |
| Component, **Details** tab (new, first) | DEFINITION (name, designator prefix, revision/state); FIELDS table; PARTS list (opening one opens the Part dialog; New Part…); LIBRARY (origin; Promote to library…, Duplicate, also in "…"); PROVENANCE (collapsed). Footer: Update placed instances…, only when nothing is unpublished and older-revision instances exist. | Removed from every geometry tab. |
| Component, Symbol, nothing selected | SYMBOL: unit picker + Add unit; Terminals list (capped, add row). | Grid and snap → nav-bar popover; selection help → canvas hint. |
| Component, pin | PIN · Electrical (name, class, functions summary + Edit…); PIN · Graphic (pin end, direction, length, marks, show). Footer: Remove pin. | "Selected terminal" merges into PIN. |
| Component, terminal without a pin on this unit | TERMINAL (name, class, functions). Footer: Remove terminal. | — |
| Component, shape or text | GRAPHIC (geometry, stroke, fill) or TEXT (content kind, text, size). Footer: Remove. | — |
| Component, Footprint, nothing selected | FOOTPRINT (datum, rotation); MASK AND PASTE (collapsed); PARAMETERS table with add row, package values collapsed and read-only; TERMINALS list. | Template picker and Assign by number → dialogs; pad defaults → Pad tool card. |
| Component, pad | PAD (number, terminal, kind, side, position, rotation, shape, size); a derived pad is one `summary_row` with "Edit sketch ›"; HOLE only for through-hole; MASK AND PASTE collapsed. | — |
| Component, Mapping | PACKAGE TERMINALS table (designation · terminal). | — |
| Component, 3D | BODY (style, L/W/H); MODEL OFFSET; "Approximate envelope" as an Info chip. | — |
| Schematic, nothing selected | SHEET: instances, wires, multi-terminal nets, unresolved parts with Resolve parts…. | Help → hint; Checks → Issues. |
| Schematic, instance | INSTANCE (designator, component · revision + Open ›, position pair, rotation, mirror, units when several); PROPERTIES (one field each, inline error or rating warning); PART ("None · unresolved" + Choose… / Clear); PIN FUNCTIONS (collapsed, live). Footer: Remove…. | Apply/Revert removed. A commit that leaves purchasing unresolved toasts "Purchasing unresolved for R1 · Undo". |
| Schematic, net or wire | NET (name, members list); WIRE only when a wire was picked. Footer: Disconnect wire…. | — |
| Schematic, Place active | COMPONENT preview (name · revision, key values, package/footprint, has/missing line, draft lineage, Library update row). | "Part" → "Component"; the closed-palette prompt → canvas hint + "Find component…". |
| PCB, Board | BOARD (size · copper layers; finish summary + Board setup…). | Routing → tool card; layers and view → popover; fabrication form → Board setup dialog; Checks → Issues. |
| PCB, placement | PLACEMENT (position pair with inline "Must stay inside the board", rotation, side, locked); CONNECTIONS (pad · net · routed, capped). Footer: Rotate 90°, Flip, Schematic ›. | — |
| PCB, track | Header "Track · N22". TRACK (net, layer, width through typed layout commands, length, ends, locked). Footer: Delete track. | Route-tool fields. |
| PCB, unplaced instance | Header "R21 · Not placed"; one row + Update PCB…. | — |

Elsewhere: SVG import and Arrange results become an error toast and a disabled reason. Library sources stays a `ui_kit::Window`. Recovery messages become Info toasts.

## Tool card

The tool card replaces Properties while a tool other than Select is armed. Esc or Select restores Properties with the previous selection. The Issues bar stays.

- **Route:** Width, Layer, Net (from the first pad), one hint line. The routing state is the pill "Routing N22 ×".
- **Polygon:** Stroke, Corner radius, Fill, Points (read-only); footer Cancel and Finish.
- **Pad:** Kind, Side, Size. **Rectangle, Pin and Wire** get their own cards.
- "Highlight complete net" and "Show airwires" move to the view popover. 3D shows no 2D tool card.

## Dialogs and reviews

Dialogs are docked in the subject slot.

| Dialog | Opened from | Primary |
| --- | --- | --- |
| New footprint from template | Footprint ribbon | Build; on a non-empty footprint "Replace footprint" with a Warning row. W1b builds it on C3's atomic `replace` (clear and build in one undo step); until then Clear stays a secondary footer action behind `kit::confirm`. Findings are not suppressed while it is open. |
| Assign pads by number | Issue Fix / "…" | "Assign 2 pads" (`LibraryCommand::AssignTerminals`) |
| Part (new or edit) | Details › Parts; schematic "New Part for these values…" | Publish Part, or "Publish with component" (`PartsAction::PublishCascade`). Disabled hover: "3 issues block publishing · Show". After publish: "Back to R1 ›". |
| Choose part | Instance PART | Choose |
| Resolve parts | Sheet / Issues Fix | — (a table `ui_kit::Window`, D10) |
| Board setup | Board card | OK |
| Function edit | Pin with more than one function | OK |
| Update review | Update PCB, update instances, disconnect, remove, library update | Apply update, gated; a `ReviewPanel` on C2 data with the canvas preview and canvas input disabled |

A review shows a summary line first, then blockers with their acknowledgements, then collapsed change groups ("CHANGES", "AFTER THIS UPDATE · × 4 new · ✓ 0 resolved"). A stale preview turns the summary into "× The project changed · Recompute". After Apply, a toast says what changed, with Undo and Show.

## Issues

**One engine.** Today the UI calls `layout::checks` and `erc::checks` directly, Component findings come from `component.check_drafts`, and the headless session builds its own report. `project.issues` replaces all of them for display: UI, CLI and MCP use the one query. The app only lays it out and adds Fix launchers and locate.

**Severity, decided in core.** A finding is:

- **Error** when it blocks a stage (`Publish`, `BoardPlaceable`, `ElectricallyResolved`, or the draft publish check), and for manufacturing rules (clearance, courtyard, outline) and ERC conflicts. `Finding.blocks` alone is not a reliable source, because footprint, layout and ERC findings are built with `Finding::new`.
- **Warning** for lint and purchasing or rating mismatches.
- **To do** for unrouted connections, unplaced instances and unresolved purchasing.
- **Info** otherwise.

Anything that blocks publishing is always Error and always counted.

**Merging.** Core emits one finding with both `ObjectRef`s for a same-cause pair: pad ↔ package terminal, binding ↔ package terminal, and per-instance copies of one shared Part. A side with no counterpart keeps its own finding. The app never merges by matching text.

**Scope.** This workspace (default), All or Selection, and a severity filter, as two dropdowns that share the card's width. A dot marks blockers in another area, count first so a clipped bar keeps it ("● 2 blocking in Component").

**Counting.** The bar, the ribbon toggle and the canvas badge show "× n ! n ○ n". The badge hides To do while errors exist; the bar never does. Results are cached per document revision, reusing the editor caches (`BoardCopper`, part_editor `findings`).

**Rows.** Each row has a glyph, a one-line title, a ×n pill, locate (⌖), an optional Fix and a disclosure (cause, fix, object chips). The row is a button. Groups are uncapped and virtualised. The footer has a one-line coverage hint with a tooltip and Copy report, which lists every raw finding.

**Canvas and Browser.** Existing markers stay. The located issue shows the pill "Showing: Clearance R3 ↔ R6 ×" with ‹ ›. Browser rows get severity dots. The Browser never hosts forms or Issues.

## Feedback

- **Toasts:** a severity glyph, at most 3 rows and 6 s each, with Undo / Show / Details. Up to 3 are stacked, and a repeat counts ×n. A click opens Messages.
- **Messages window:** newest first, ×n folding, Copy all, Clear, File › Messages… (the app has no Help menu yet). It keeps 200 entries of up to 4000 characters, with the full engine text.
- **Explicit severity:** `App::say` takes a severity, which replaces guessing it from the text "fail".
- **Pills:** routing, highlighted net, shown issue and layer focus, each with ×.
- **`notice` retired:** each editor's `notice` call sites become a success toast, an error toast, or a disabled reason or canvas hint.

**Refusal routing.** `App::apply` hands a `Refused { origin, error }` back to the editor that produced the operation, on the next frame.

- A field origin shows `kit::field_error` on that field, with no toast.
- A dialog origin shows on the dialog's refusal row.
- A canvas gesture shows a toast with Show; a route stays in progress.
- Only refusals no editor claims fall back to a toast.

Wording is chosen by `Error` variant and code, never by matching strings. For example, "Not published: 3 issues block it · Show" opens Issues filtered to Publish.

## Message style

- A title is one sentence fragment of at most 80 characters, with no final full stop, designed for about 56 characters at 860 pt and clipped, with the full text on hover.
- It names objects by user names. It never shows IDs, codes, Debug output, coordinates or API names.
- It omits the designator when the card or row already shows the object.
- Details are a cause (one sentence), a fix (an imperative, plus a Fix button) and object chips. The raw text goes to Messages and Copy report.

| Severity | Glyph | Token |
| --- | --- | --- |
| Error | × | `error` |
| Warning | ! | `warning` |
| To do | ○ | `text_dim` |
| Info | i | `accent` (glyph only; text uses `text`) |
| OK | ✓ | `form_success` |

Glyphs are drawn by `kit::severity_glyph`, never typed.

**Enforcement.** `pcbcraft_core::issues::style::check` is the one implementation. It flags:

- empty text, more than 80 characters, several lines or a final full stop;
- `ID 8`, `[68]` and `[3, 4]`, `net 32` and `D20 r2`;
- a `rule.code:` prefix;
- Debug shapes such as `Some(…)`, `Id(3)` or `Name { field: … }`.

Its tests pin the rewrite table of the design (`issues/tests.rs`). C1 runs it over every finding code's fixtures and every `Error` display. K3 runs it over the refusal wording (`refusal.rs`). K4 extends `cargo xtask ui-forms` so feature modules show `Error`/`Warning` text only through `kit::field_error` or `kit::refusal`.

## Contract (slice S0)

These signatures are fixed. Bodies are minimal and not yet wired, so behaviour is unchanged. A slice may extend a type; it changes a signature only together with every slice that depends on it.

**Core** (`crates/editor-core/src/issues.rs`, module 16a of the [module map](component-model.md#d-module-map)):

- `Severity { Error, Warning, ToDo, Info }`, ordered most severe first;
- `IssueArea { Component, Schematic, Board }`;
- `IssueScope { All, Area { area }, Selection { objects } }` (at most `MAX_SCOPE_OBJECTS`);
- `IssueGroup { code, key, title }`;
- `IssueDetail { cause, fix }`;
- `IssueRow { severity, area, group, title, detail, finding }`, where `finding` is the unchanged `Finding` (code, raw message, every `ObjectRef`, `blocks`);
- `IssueCounts`, `AreaCount`, and `IssueReport { rows, counts, elsewhere, coverage }`;
- `MAX_TITLE = 80`;
- `report(&Project, &Drafts, &IssueScope) -> Result<IssueReport>`, served as `Query::Issues { scope }` (`project.issues`) → `QueryResult::Issues`. Since C1 it returns the real report on desktop, CLI and MCP; a failure or an oversized scope (`Error::Limit`) refuses rather than return an empty list, so no transport mistakes it for "no issues".
- `style::{check, Problem}`.
- `object_names(&Project, &Drafts, &[ObjectRef]) -> Vec<String>` (A1): the user names of objects, as titles name them, for object chips.

**Desktop** (`apps/pcbcraft/src`):

| Item | File | Signature |
| --- | --- | --- |
| Inspector return type | `kit/card.rs` | `Inspector { header: shell::Header, subject: egui::Id, footer: bool, issues: Option<IssueSlot> }`; `card(ui, &Inspector, FnMut(&mut Ui, Region) -> Option<R>) -> CardResponse<R>`; `Region { Body, Footer }`; `column(rect, issues_expanded) -> Column { subject, issues }`; `Subject { Properties, Tool, Dialog, Review }`; `FOOTER_HEIGHT = 48` |
| Severity | `kit/severity.rs` | `Level { Error, Warning, ToDo, Info, Ok }` (`From<Severity>`), `ink(level)`, `severity_glyph(painter, center, size, level)` |
| Issues widgets | `kit/issues.rs` | `issue_row(ui, id, &IssueRowView) -> IssueRowResponse`, `issues_list(ui, id, &[IssueGroupView], row) -> Option<IssueListEvent>`, `issue_line(ui, &IssueCounts, label)`, `issues_bar(ui, &IssueCounts, expanded, elsewhere)`, `count_badge(ui, &IssueCounts, hide_to_do_with_errors)`, `detail(ui, &IssueDetail)`, `ISSUES_BAR_HEIGHT = 28` |
| Toasts and pills | `kit/toast.rs` | `Toasts::{push(level, text, actions, now), expire(now), show(ctx, canvas, now) -> Option<ToastEvent>}`, `ToastAction { Undo, Show, Details }`, `pill(ui, level, text) -> PillResponse { clicked, closed }`; K3 added `Toasts::{get, is_empty, withdraw(action), dismiss_errors}`, `toast_text` and, for editors without app access, `post(ctx, level, text, actions)` (the app drains it each frame into toasts and Messages) |
| Messages | `kit/messages.rs` | `Messages::{push(level, text, now), clear, entries, copy_all, window(ctx) -> MessagesResponse}`, `KEEP = 200`, `MAX_CHARS = 4000`; K3 added `len`, `is_empty` |
| Dialog | `kit/dialog.rs` | `DialogPanel { id, icon, title, kind, body_rows, primary, primary_kind, primary_disabled, refusal, reset_scroll }::show(ui, body) -> Option<DialogAction>`; `confirm(ctx, id, title, text, proceed) -> Option<bool>` |
| Review | `kit/review.rs` | `ReviewPanel { id, icon, title, summary, stale, blockers: &[ReviewLine], groups: &[ReviewGroup], apply }::show(ui, acked) -> Option<ReviewAction>`; R1 added `show_with(ui, acked, refusal, inputs)` (the refusal row, and a review's own inputs between blockers and groups) |
| Update review | `update_preview.rs` | R1: `KIND` (the header kind the column hosts as a docked review, without a Properties card), `content(&UpdatePlan, &Project, note, choices) -> Content`, `marks(&UpdatePlan) -> Marks { changed, removed, tracks, issues, focus }`, `UpdatePreview::marks()` and `paint_board(painter, project, &marks, screen, outline)` for the canvas preview |
| Rows | `kit/rows.rs` | `tool_card(ui, &ToolCard, rows) -> ToolCardResponse { done }`, `hint(ui, text, tip)`, `list_frame(ui, len, max_rows, row) -> ListFrameResponse { show_all }`, `table(ui, &[TableColumn], rows, cell, add_row) -> TableResponse { add }`, `summary_row(ui, summary, action) -> bool`, `field_error(ui, &field, message, not_stored)`, `refusal(ui, message)`, `LIST_MAX_ROWS = 8` |
| Locate | `issues.rs` | `trait Locate { fn locate(&mut self, engine: &Engine, refs: &[ObjectRef]) -> bool }`. It takes the engine so an editor can resolve tabs and frames; it changes selection and camera only. |
| Issues surface | `issues.rs` | A1 added `Surface` (the card's state and cached answers: `card`, `pill`, `badge`, `ribbon_toggle`, `badges`, `toggle`, `open`, `show_selection`), `Request { Locate { refs, area }, Fix }`, `Editors` and `route(editors, engine, refs, preferred) -> Option<Workspace>` (the issue's area first, then the others), `Badges`, `report_text` and `interim::{Part, Schematic, Pcb}` (locate through each editor's public selection until its W slice implements `Locate`; then `route` takes the editor and `interim` goes) |
| Ribbon toggle | `shell.rs` | A1 added `IssuesToggle { counts, expanded }`, a fourth `ribbon` argument and `RibbonEvent::Issues` |
| Browser badge | `kit/tree.rs` | A1 added `Row::issue: Option<Level>` |
| Pill width | `kit/toast.rs` | A1: `pill` truncates its text into the room left after ×, so a narrow pill keeps its ×; the Showing pill keeps clear of the badge corner |
| Fix launchers | `issues.rs` | `Fix { Apply { label, operation }, Open { label, dialog: FixDialog } }`, `fix(&IssueRow) -> Option<Fix>` |
| Refusal hand-back | `refusal.rs` | `Refused { origin: Origin, error: Error }`, `Origin { Field(egui::Id), Dialog(egui::Id), Canvas, Command }`, `Refused::claim(origin) -> Option<String>`, `shown_error(&Error) -> String`; K3 added `mark(ctx, origin)` (an editor declares the origin of the operation it returns this frame; otherwise the call site's default applies: inspector and draft commits `Command`, canvas `Canvas`), `claim(ctx, origin) -> Option<String>` (on the next frame), `level(&Error) -> Level`, `presentable(text)` and `clip(text, max)` |
| Component card | `part_editor.rs` | W1a added, for the column to adopt: `PartEditor::{inspector_card(&Engine) -> Inspector, inspector_region(ui, &Engine, Region) -> Option<Operation>, subject(ui, &Engine) -> CardResponse<Operation>, subject_kind() -> Subject}` and `impl issues::Locate for PartEditor`. Until `app/chrome.rs` calls `subject`, `inspector()` draws the body, the issue slot and the footer's actions in the one body it is handed, and the header names the tool while one is armed. |

The kit files carry `#![allow(dead_code)]` until their slice wires them; the slice that wires a file removes the allowance. After wave 3, `kit/card.rs` keeps one only on `Subject` (Properties is the only subject until R1 and the W slices), and the kit re-exports other than `issues_bar` and `rows` stay allowed until A1, R1 and the W slices use them. A1 removed the allowances on `kit/issues.rs`, `kit/severity.rs` and `pill`; only `issue_row` (no caller outside the list yet) and `Level::ALL` (tests only) keep one.

## Slices and file ownership

Every slice ends with `cargo xtask ci`. A visible slice also adds real-app screenshots of the audit states it touches, at 860×1081 and at full width. Hours are wall-clock **targets**. A slice edits only the files it owns. Each kit port adds or updates its ATTRIBUTION entry.

| Slice | Content | Files owned | Depends |
| --- | --- | --- | --- |
| **B0** Bugs (done) | B-a…B-j | the three editors | — |
| **S0** Contract (this page) | IA, types, signatures, style checker, skill rules, planned attribution | `docs/sidebar-ia.md`, `crates/editor-core/src/issues*`, the new kit and app files above | — |
| **S1** Split (done) | Mechanical, no behaviour change: `part_editor::inspector()` per-view bodies → `part_editor/{details,symbol,footprint,mapping,model}_card.rs`; the Part section → `part_props.rs`; the same for the schematic and PCB `inspector()` dispatch (see [the card files](#card-files-after-s1)) | the three editors | B0 |
| **K1** Card + column (done) | `kit::card`; the `chrome.rs` column stack with the always-visible Issues bar slot | `kit/card.rs`, `shell.rs`, `app/chrome.rs` (column) | S0 |
| **K2** Issues kit (done) | severity, rows, list, line, bar, badge, detail | `kit/severity.rs`, `kit/issues.rs` | S0 |
| **K3** Feedback kit + routing (done) | toast, Messages, pill; `App::say(severity)`; `Refused` from `App::apply`; `status_chip` removed | `kit/toast.rs`, `kit/messages.rs`, `refusal.rs`, `app.rs`, `app/chrome.rs` (status only; rebased after K1) | S0 |
| **K4** Dialog kit (done) | DialogPanel, ReviewPanel, tool_card, hint, list_frame, table, summary_row, field_error, refusal, confirm; the `ui-forms` message rule | `kit/dialog.rs`, `kit/review.rs`, `kit/rows.rs`, `xtask/src/ui_forms.rs` | S0 |
| **C1** Core issues and messages (done) | `project.issues` (also on CLI/MCP) with severity, group and title; same-cause merging in the rules; names-not-IDs pass over `Finding.message` and `Error` display (not `updates.rs`); CLI/MCP golden tests; core message-style tests over fixtures | `crates/editor-core` except `updates.rs`; `crates/automation` | S0 |
| **C2** Core review data (done) | Structured update plan (changes by kind, area and refs; line + detail); findings diff (new/resolved) replacing the "Candidate … check" lines; conflicting tracks with net, layer and length; wording of the `updates.rs` blockers | `crates/editor-core/src/updates.rs`, `updates/` | S0 |
| **C3** Core commands (done) | `FootprintFromTemplate { replace }` (atomic clear + build, one undo, on CLI/MCP); track width/layer edit commands if absent | `crates/editor-core/src/commands/`, `library/templates/` | S0 |
| **A1** Issues surface (done) | `issues.rs`; Issues card content; ribbon Issues toggle replaces Check; canvas badge; cross-workspace locate through `Locate`; Showing pill; Browser badges | `issues.rs`, `app/browser.rs`, ribbon wiring | K1, K2, C1 |
| **R1** Review panel (done) | `update_preview.rs` on `ReviewPanel` with C2 data and canvas preview; the column hosts a `KIND` subject as a docked review (`app/chrome.rs`), and the PCB canvas outlines the plan (`pcb_editor.rs`, one call) | `update_preview.rs` | K4, C2 |
| **W1a** Component Details + Symbol (done) | Details tab (with LIBRARY), Symbol, pin/terminal/graphic/text cards, grid popover, draw tool cards, `Locate`, this workspace's `notice` sites | `part_editor.rs` (the `inspector()` dispatch, tabs and the workspace's `Locate`), `part_editor/{details,symbol}_card.rs`, `part_editor/symbol_props.rs` | S1, K1, K3, K4 |
| **W1b** Component Footprint + Mapping + 3D | Footprint and pad cards with `summary_row`; template dialog (C3 when merged, else Clear + confirm); Assign dialog; Pad tool card; Mapping table; 3D card; `Locate` targets for its tabs (methods in its own files that W1a's `Locate` calls); `notice` sites | `part_editor/footprint_*.rs` (with `footprint_card.rs`), `part_editor/{mapping,model}_card.rs`, `part_editor/model_{body,props}.rs` | S1, K1, K3, K4 |
| **W1c** Part dialog (done) | Part as a `DialogPanel` with cascade publish, gated primary and return link. Additive hooks: `PartEditor::part_dialog` is called first in `inspector()`; the app sets `part_return_to` on a schematic New Part request and follows `take_part_return` back to the instance | `part_editor/part_props.rs` | S1, K4 |
| **W2a** Schematic cards | Sheet, Instance (live per field), Net/Wire, Place preview; Checks section removed; `Locate`; `notice` sites | `schematic_editor.rs`, `schematic_editor/{sheet,instance,net}_card.rs`, `place_palette.rs` (preview) | S1, K1, K3 |
| **W2b** Schematic dialogs | Choose part dialog; Resolve parts table Window | `resolve_parts.rs`, `place_palette.rs` (assign part; merged after W2a) | S1, K4 |
| **W3** PCB | Board card + Board setup; Placement and Track cards; Route tool card; view popover; 3D rules; unplaced state; `Locate`; `notice` sites | `pcb_editor.rs`, `pcb_editor/{board,placement,track,route}_card.rs` | S1, K1, K3, K4, C3 (track edits) |
| **V1** Evidence | After-screenshots of every audit state; measure the one-screen target; update [editor evidence](editor-evidence.md); scenarios for locate, toast, refusal on a field, review gating and a stable layout while typing | `docs/`, tests | all |

**Waves.**

1. B0 ∥ S0.
2. S1.
3. K1, K2, K3, K4, C1, C2 and C3 in parallel. K3 rebases its `chrome.rs` edits after K1.
4. A1, R1, W1a, W1b, W1c, W2a, W2b and W3 in parallel. Files are disjoint after S1. W slices only call `kit::issue_line`, implement `Locate` and migrate their own `notice` sites.
5. V1.

### Card files after S1

Paths are relative to `apps/pcbcraft/src`. Each editor's `inspector()` stays in the editor file and only dispatches to these files.

| File | Holds |
| --- | --- |
| `part_editor.rs` | the card dispatch by subject (`Inspected`): header, subject identity, footer and issue slot; the Details and Symbol tabs; `Locate` |
| `part_editor/details_card.rs` | the Details tab: DEFINITION, FIELDS, PARTS, LIBRARY, PROVENANCE; Update placed instances… |
| `part_editor/symbol_card.rs` | the Symbol tab: SYMBOL (unit, terminal list), the PIN · Electrical and TERMINAL cards |
| `part_editor/footprint_card.rs` | the Footprint tab: tool options, template section, footprint and pad sections (bodies in `footprint_*.rs`) |
| `part_editor/mapping_card.rs` | the Mapping tab: package terminals and their mapping |
| `part_editor/model_card.rs` | the 3D tab: Body and placement (bodies in `model_body.rs`, `model_props.rs`) |
| `part_editor/part_props.rs` | the Parts section and its glue (`part_section`) |
| `schematic_editor/sheet_card.rs` | Sheet, Place and Checks |
| `schematic_editor/instance_card.rs` | the selected instance: properties, Part, pin functions |
| `schematic_editor/net_card.rs` | the selected wire's net |
| `pcb_editor/board_card.rs` | Board and fabrication, Layers, Board checks |
| `pcb_editor/placement_card.rs` | Placement, and the not-placed state |
| `pcb_editor/track_card.rs` | the selected track |
| `pcb_editor/route_card.rs` | Routing |
