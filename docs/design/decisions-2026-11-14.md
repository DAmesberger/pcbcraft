# Founder decisions, 14 November 2026

These decisions apply to the three plans in this folder: [file formats](file-format-plan.md), [part generation](part-generation-plan.md) and [extension ABI](extension-abi-design.md). Where a plan's text differs, this page wins.

## File formats
- **Container:** a project is a folder named `<name>.sparksmith`, registered as a macOS package. "Share as single file" writes a ZIP of the same tree with the same extension.
- **Library folders** are macOS packages as well.
- **Versioning:** the file format is versioned from now on, starting at 2 and raised on every incompatible change. Optional capabilities are named features. Old files go through a chain of converters, golden trees are kept for every version, and a format-lock gate runs in CI. "No migrations" applied only to the design phase.
- **Encoding:** keyed JSON Lines on disk, one record per line, with the id first and lengths in integer nanometres. Single documents are laid-out JSON. Agent query replies are tables (columns named once, then rows). Agents read the design through scoped queries, not whole files.
- **Agent evidence:** plan tests with real agents on a more complex board.
- **Tracks:** one `board/tracks.jsonl`, sorted by net, then by id. Vias are stored the same way.
- **Ids** stay small integers.
- **Binaries:** plain git; Git LFS is an opt-in. Caches are never put in a shared ZIP; missing caches are rebuilt in a background task.
- **Revisions** nothing references are kept until an explicit, previewed prune.
- **Drafts** live in the versioned tree.
- **Integrity:** the tree digest is defined now; signing comes later.
- The shipped parts shelf becomes an ordinary library folder with GP5.
- A generated part that fails its check makes the project refuse to open.

## Part generation
- The plan is approved: one mechanism per job.
  - WASM generators make whole parts.
  - A 3D body is always a `.solidcraft` part file with values. A generator bundle (manifest, module, part files) is identified by one hash.
  - Footprints drawn by hand use sketch footprints.
  - Rhai makes no geometry.
  - The USB-C receptacle and WROOM-3 ship as library data.
- After implementation, the AI part-generation tests are run again.

## Extension ABI
- Decisions 1–14 are taken as recommended. In particular:
  - one plugin ABI, where each capability declares its effect (`none`, `read` or `edit`);
  - a `show` verb for the window;
  - the built-in agent runs in process;
  - plugins edit a fork, and the host applies the result as one undo step;
  - a custom JSON ABI on wasmi;
  - module versions start at 1.0, with `surface.lock`.
- Only the newest MCP protocol is supported. `apply` keeps a plain `op` string.

## Limits and settings
- No caps on design size. Limits on the size of untrusted input stay.
- Impedance tolerance is a setting; nothing is assumed in code.
- Fab profiles carry manufacturing options with sourced limits:
  - copper weight is an option tied to the stackup;
  - option defaults live in the profile file;
  - switching profiles carries over choices that match;
  - "Recommended" uses JLCPCB's 0.2 mm preferred via drill;
  - plugged vias and 1-layer boards come later.
