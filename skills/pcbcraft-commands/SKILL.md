---
name: pcbcraft-commands
description: Design or change Pcbcraft commands, transactions, crate boundaries, CLI or MCP operations.
---

# Commands and boundaries

Use the proposed boundaries in [the product plan](../../docs/pcbcraft-plan.md). Add crates only when a concrete responsibility needs a boundary; do not scaffold every future editor first.

1. Define an original dotted command ID, typed input and output, validation, enabled conditions and structured error behavior. Distinguish queries, document mutations and external effects.
2. Implement the behavior in the engine. UI forms collect parameters and invoke it; CLI/MCP expose the same contract. Generate discovery/schema documentation from the registry.
3. Validate units, finite/ranged values, references and expected project revision before applying changes. Use stable IDs for operations; names are display labels.
4. Commit document changes atomically with one sensible undo step. Failed or cancelled operations preserve the prior state. Drag previews and route/shove previews use speculative edits; commit only the accepted result.
5. Test the result and failure rollback through the public command interface. Include malformed inputs and an outdated-revision case where relevant.

File writes and desktop launch are external effects, not reversible document mutations. Report their status separately; do not promise undo can retract a file or launch.

The electronics document owns circuit/layout truth. Solidcraft integration maps it to native 3D. Background results carry their input revision; apply only to a matching document. Cross-document transactions must not leave the PCB and its mechanical model referring to different committed states.

Keep lower layers free of window/GPU/transport APIs. The Solidcraft adapter can depend on the upstream engine without leaking its CAD types throughout the electrical document. Direct kernel access is reserved for shared Solidcraft implementation work under its own rules.

Routing uses a strategy interface over topology, obstacle queries and a transactional edit set. Manual routing must not encode assumptions that prevent later connected-track displacement, chain shoves or locked objects.
