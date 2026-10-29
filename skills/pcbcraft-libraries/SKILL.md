---
name: pcbcraft-libraries
description: Change Pcbcraft symbols, footprints, terminal mappings, library revisions, project snapshots or persistent design formats.
---

# Libraries and persistence

Use [GLOSSARY.md](../../GLOSSARY.md) and [part library design](../../docs/part-library-design.md) as the domain reference. Keep electrical definitions, symbol views, packages, footprints, bindings and manufacturer parts distinct even when one UI authors them together.

## Identity and mapping

- Preserve stable definition, terminal, pad and instance identities across renaming and reannotation. Treat a label correction and an electrical remapping as different operations.
- Make mappings explicit and complete. A package terminal belongs to at most one electrical terminal; multiple physical terminals can share electrical intent while retaining independent routing obligations.
- Same-net membership does not establish copper connectivity. Distinguish device-declared NC, project no-connect decisions and non-electrical holes.
- Validate exact manufacturer selections against project requirements. Changing a generic value must surface any incompatible retained MPN.

## Revision updates

Save immutable revisions and snapshot the complete dependency closure, including native 3D models and their parameter interfaces. Required bytes must be available offline; a hash and missing remote URL are not a portable snapshot.

Before applying an update, compute an impact plan tied to the current project revision: changed terminals/nets, pad geometry, affected tracks, mechanical transforms and required assets. Apply as one undoable transaction or leave the old state intact. Preserve valid copper; flag detached tracks and ambiguous remaps instead of silently reconnecting or retagging them.

Make editing scope explicit: project-local definition/selected instances versus source library. Library edits never mutate existing project snapshots automatically.

## File changes

Version persistence explicitly. Test old-file migration, unsupported-new-feature refusal, damaged input, and offline reopen. Write atomically through a shared persistence routine. Treat renderer caches as disposable, versioned data; retain authoritative parametric definitions.

Pcbcraft's format uses its own migration mechanism once implemented; do not refer to nonexistent upstream-style pending folders. A Solidcraft format change follows the actual instructions and migration machinery in the upstream checkout.

Record imported asset provenance/license separately from code licensing. Report unsupported input features without silently changing the electrical meaning. Cover the relevant edge cases from the library proposal through the public command API.
