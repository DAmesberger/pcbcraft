---
name: pcbcraft-ui
description: Build or change Pcbcraft panels, controls, navigation, 2D/3D selection and visible workflows in the Solidcraft UI style.
---

# UI and interaction

Inspect the matching implementation in the pinned Solidcraft checkout before adapting a UI pattern. Its tokens, UI kit, container policy and navigation are the reference; a screenshot alone is not the implementation contract. Record adapted source and notices in [ATTRIBUTION.md](../../ATTRIBUTION.md).

- Centralize colors, spacing, type and control dimensions in tokens. Use a shared outer container and control kit. New feature panels provide content and commands, not their own chrome or navigation code.
- Component, Schematic and PCB share one shell. Component authoring exposes Symbol, Footprint, Mapping and 3D together. PCB 2D/3D is a view switch with common selection, properties and undo.
- Map picks to stable source object IDs. A copper item and its net remain distinguishable across rendering batches. Net membership highlighting includes disconnected same-net islands; physical connectivity inspection is a different action.
- Preserve selection/focus on view changes. Use one navigation input path with capabilities for flat/orbit modes. Respect visibility for normal picks; make hidden-layer inspection explicit.
- Keep command invocation separate from gestures and forms. UI actions and agent commands must cause the same document changes.
- Show failed evaluations, stale pours and approximate component models accurately. A busy background operation must not freeze navigation or silently replace newer work.

## Right column, Issues and feedback

[Sidebar IA](../../docs/sidebar-ia.md) is the contract for the right column. Follow these rules:

- The subject slot shows one card for one subject: Properties, a tool card while a tool is armed, a docked dialog or a docked review. The header names the subject; the body never repeats it.
- A command with inputs is a `kit::dialog::DialogPanel`. Its footer is pinned, primary last, and a destructive choice is the primary's variant. A refusal shows once, as the dialog's last body row. A disabled primary says why on hover.
- A problem has one home: Issues for document findings, `kit::field_error` for one field's invalid input, or the refusal row of the refused dialog or review. A card shows only its reserved `kit::issue_line`.
- Nothing that blocks a stage is hidden or uncounted. The Issues bar stays visible during dialogs, tools and reviews. Folding changes presentation only; never suppress a finding because a command is in progress.
- Core decides severity and wording through `project.issues`. Lay its rows out; never compute, merge or reword findings in the app. Every row locates its objects through `issues::Locate`. Offer Fix only when a typed command, or a dialog that issues one, resolves the issue.
- A message is one line of at most 80 characters that names objects, never IDs, codes or Debug output (`pcbcraft_core::issues::style::check`). Help is one `kit::hint` line, not a paragraph, and never repeats the canvas hint.
- The card commits live, per field: Enter or focus loss sends one typed command and one undo step, paired values commit together, and invalid input is marked and never committed. Staged Apply exists only in dialogs.
- Command results and gesture refusals are toasts. Messages holds the history, and pills show persistent modes. Route a refusal back to its origin (`refusal::Refused`). Do not add editor `notice` strings or `kit::Tone::Note` text.
- Bound every list (`kit::list_frame`, 8 rows, then "Show all"); never cap one silently. Nothing is inserted above the control being edited. The scroll offset resets when the subject changes.

`cargo xtask ui-forms` and `cargo xtask ui-containers` enforce building with the kit and the shared containers. `issues::style::check` defines the wording rules; its tests cover the refusal wording now, and slice C1 runs it over every core finding. The slices listed in the IA add the remaining checks. Until then, apply these rules in review.

For a visible change, run the actual app when available, reproduce the interaction and inspect a real screenshot or recording. Add an interaction scenario for behavior that can regress. Include small-window layout and both views when affected. Do not claim visual verification from a mockup or from compilation alone.

The right column is a fixed layout of one subject card above the Issues card. Do not add a general docking framework or unrelated UI redesign.
