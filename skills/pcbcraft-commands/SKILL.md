---
name: pcbcraft-commands
description: Design or change Pcbcraft commands, transactions, crate boundaries, CLI or MCP operations.
---

# Commands and boundaries

Use the proposed boundaries in [the product plan](../../docs/pcbcraft-plan.md). Add crates only when a concrete responsibility needs a boundary; do not scaffold every future editor first.

1. Define an original dotted command ID, typed input and output, validation, enabled conditions and structured error behavior. Distinguish queries, document mutations and external effects.
2. Implement the behavior in the engine. UI forms collect parameters and invoke it; CLI/MCP expose the same contract. Generate discovery/schema documentation from the registry.
   Transports answer through `Session::respond`: a request that returns document state builds on `document_reply`, so its compact reply skips the snapshot. Operations and queries added to their enums are listed in `tools/list` and served by `project.schema` without further work; keep a tool's schema generated, never hand-listed.
   Choose a permission class beside the enum, in its `declare_classes!` listing (core command enums, `Query`) or `Request`'s in `session.rs`; an unlisted variant does not compile. **Read:** changes nothing. **Edit:** a document change undo can take back. **Ask:** files, publishing and anything else undo cannot take back, or a person's sign-off. **Never:** keys, settings, crash reports. A request that can name a path also adds it to `Request::paths`, `Request::NAMING_PATHS` and `path_samples` in `gate/tests.rs`; naming a path makes it at least Ask and is limited to the run's folders. `crates/automation/src/gate.rs` holds the rules, and `Session::respond` enforces them for the `Caller` its transport names: every model-facing transport passes `Caller::Model` (or `Caller::Allowed` after a person's Allow), never `Caller::Operator`, so the never class cannot run for a model. CLI `stdio` is model-facing by default (gated as `mcp`); only `stdio --operator`, the person running their own script, passes `Caller::Operator`.
3. Validate units, finite/ranged values, references and expected project revision before applying changes. Use stable IDs for operations; names are display labels. Every request that can change the document, undo, redo, new and open included, requires `expected_revision` and returns it from `Request::expected_revision`; `Session::execute` checks it once, before any file is read or written, and the engine checks again through `Engine::check_revision`. Never add a per-handler check.
4. Commit document changes atomically with one sensible undo step. Failed or cancelled operations preserve the prior state. Drag previews and route/shove previews use speculative edits; commit only the accepted result.
5. Test the result and failure rollback through the public command interface. Include malformed inputs and an outdated-revision case where relevant.

File writes and desktop launch are external effects, not reversible document mutations. Report their status separately; do not promise undo can retract a file or launch.

The electronics document owns circuit/layout truth. Solidcraft integration maps it to native 3D. Background results carry their input revision; apply only to a matching document. Cross-document transactions must not leave the PCB and its mechanical model referring to different committed states.

Keep lower layers free of window/GPU/transport APIs. The Solidcraft adapter can depend on the upstream engine without leaking its CAD types throughout the electrical document. Direct kernel access is reserved for shared Solidcraft implementation work under its own rules.

Routing uses a strategy interface over topology, obstacle queries and a transactional edit set. Manual routing must not encode assumptions that prevent later connected-track displacement, chain shoves or locked objects.
