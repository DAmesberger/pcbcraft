---
name: sparksmith-commands
description: Design or change Sparksmith commands, transactions, crate boundaries, CLI or MCP operations.
---

# Commands and boundaries

Use the proposed boundaries in [the product plan](../../docs/sparksmith-plan.md). Add crates only when a concrete responsibility needs a boundary; do not scaffold every future editor first.

1. Define an original dotted command ID, typed input and output, validation, enabled conditions and structured error behavior. Distinguish queries, document mutations and external effects.
2. Implement the behavior in the engine. UI forms collect parameters and invoke it; CLI/MCP expose the same contract. Generate discovery/schema documentation from the registry.
   Transports answer through `Session::respond`: a request that returns document state builds on `document_reply`, so its compact reply skips the snapshot. Operations and queries added to their enums are listed in `tools/list` and served by `project.schema` without further work; keep a tool's schema generated, never hand-listed.
   Declare the entry once, beside the enum, in its `declare_entries!` listing (core command enums, `Query`) or `Request`'s in `session.rs`; an unlisted variant does not compile. The line names its class, module (`in module` when not the family's), and any of `short`/`long`, `link`, `review`, `reach(…)`, `effect(…)`, `needs(…)` and `since(…)` ([extension surface](../../docs/extension-surface.md#declaring-an-entry)); never keep a fact about an entry in a list elsewhere. **Read:** changes nothing. **Edit:** a document change undo can take back. **Ask:** files, publishing and anything else undo cannot take back, or a person's sign-off. **Never:** keys, settings, crash reports. (**Show**, the window only, has no entries yet.) A parameter that names a file or folder has the type `FilePath`; the gate finds it from the schema, so naming a path makes the request at least Ask and limits it to the run's folders with no list to update. A new or changed entry raises its module's version in `permission::MODULES` (a new entry declares `since` it) and is recorded with `cargo xtask surface --update`; `cargo xtask surface --check` fails a change without the version it needs. `crates/automation/src/gate.rs` holds the rules, and `Session::respond` enforces them for the `Caller` its transport names: every model-facing transport passes `Caller::Model` (or `Caller::Allowed` after a person's Allow), never `Caller::Operator`, so the never class cannot run for a model. CLI `stdio` is model-facing by default (gated as `mcp`); only `stdio --operator`, the person running their own script, passes `Caller::Operator`.
3. Validate units, finite/ranged values, references and expected project revision before applying changes. Use stable IDs for operations; names are display labels. Every request that can change the document, undo, redo, new and open included, requires `expected_revision` and returns it from `Request::expected_revision`; `Session::execute` checks it once, before any file is read or written, and the engine checks again through `Engine::check_revision`. Never add a per-handler check.
4. Commit document changes atomically with one sensible undo step. Failed or cancelled operations preserve the prior state. Drag previews and route/shove previews use speculative edits; commit only the accepted result.
5. Test the result and failure rollback through the public command interface. Include malformed inputs and an outdated-revision case where relevant.

File writes and desktop launch are external effects, not reversible document mutations. Report their status separately; do not promise undo can retract a file or launch.

The electronics document owns circuit/layout truth. Solidcraft integration maps it to native 3D. Background results carry their input revision; apply only to a matching document. Cross-document transactions must not leave the PCB and its mechanical model referring to different committed states.

Keep lower layers free of window/GPU/transport APIs. The Solidcraft adapter can depend on the upstream engine without leaking its CAD types throughout the electrical document. Direct kernel access is reserved for shared Solidcraft implementation work under its own rules.

Nothing about standards or materials is installed for the process: the host makes one `standards::Context` (`sparksmith_io::standards::shipped_context`) and hands it to every engine (`Engine::set_context`, or the `new_project` constructors), which makes it current while an operation or query runs. Code outside an engine call reads the context current on its thread; a thread you start runs its work through `standards::carried`; a test that calls below the engine enters `Context::fixture()`. Never add a process-wide `OnceLock` of host data ([extension surface](../../docs/extension-surface.md#explicit-context)).

Routing uses a strategy interface over topology, obstacle queries and a transactional edit set. Manual routing must not encode assumptions that prevent later connected-track displacement, chain shoves or locked objects.
