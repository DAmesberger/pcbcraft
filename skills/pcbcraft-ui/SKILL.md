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

For a visible change, run the actual app when available, reproduce the interaction and inspect a real screenshot or recording. Add an interaction scenario for behavior that can regress. Include small-window layout and both views when affected. Do not claim visual verification from a mockup or from compilation alone.

During bootstrap, establish the small shell and controls needed by the acceptance fixture; avoid a general docking framework or unrelated UI redesign.
