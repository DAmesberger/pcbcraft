//! The interface's and assembly link's findings and refusals, in the
//! user's words (docs/architecture/board-interface.md, "Findings";
//! docs/board-sketch-references.md §7 for the `assembly.*` codes it
//! already named). Every code is listed once in [`CODES`] with whether it
//! blocks fabrication output, whether a waiver may lift that, and whether
//! it blocks publishing to the linked design. A broken assembly reference
//! is a warning in Issues that blocks only publishing (slice P1); a loop
//! through positions blocks fabrication output.
use crate::diagnostics::{Error, Finding, ObjectRef};

/// The interface changed since the last publish (advisory).
pub const UNPUBLISHED: &str = "interface.unpublished";
/// An exported datum's owner or feature is gone; it keeps its slot.
pub const DATUM_LOST: &str = "interface.datum_lost";
/// An exported datum was found again by geometry, not by name.
pub const DATUM_INEXACT: &str = "interface.datum_inexact";
/// The linked design changed since the board last read it (advisory).
pub const ASSEMBLY_STALE: &str = "assembly.stale";
/// The linked design still links an older published board.
pub const ASSEMBLY_OLDER_BOARD: &str = "assembly.older_board";
/// The board and the linked design both changed since they last agreed.
pub const ASSEMBLY_BOTH_CHANGED: &str = "assembly.both_changed";
/// A linked design feature uses a datum that no longer resolves.
pub const ASSEMBLY_REFERENCE_LOST: &str = "assembly.reference_lost";
/// A linked design feature's datum was found again only by geometry.
pub const ASSEMBLY_REFERENCE_INEXACT: &str = "assembly.reference_inexact";
/// A linked design feature references a face that is not exported.
pub const ASSEMBLY_UNEXPORTED_FACE: &str = "assembly.unexported_face";
/// The board and the linked design depend on each other's positions in a
/// loop.
pub const ASSEMBLY_CYCLE: &str = "assembly.cycle";
/// The board follows linked design geometry whose extent, but not its
/// position, the linked design takes from the board (advisory).
pub const ASSEMBLY_EXTENT_CYCLE: &str = "assembly.extent_cycle";
/// The linked design file was not found; the board uses its snapshot.
pub const ASSEMBLY_MISSING: &str = "assembly.missing";
/// The linked design was saved by a newer SolidCraft than this build reads.
pub const ASSEMBLY_NEWER: &str = "assembly.newer";
/// Refusal: a board edge driven by the linked design was edited locally.
pub const ASSEMBLY_DRIVEN_EDGE: &str = "assembly.driven_edge";
/// Refusal: publishing while a reference between the board and the linked
/// design is broken.
pub const ASSEMBLY_PUBLISH_BLOCKED: &str = "assembly.publish_blocked";

/// One finding code's consequences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeSpec {
    pub code: &'static str,
    /// Fabrication output is refused while the finding exists.
    pub blocks_fabrication: bool,
    /// A pre-flight waiver may lift the block. A position loop is never
    /// waived: the board is re-picked or the linked design rebuilt.
    pub waivable: bool,
    /// Publishing to the linked design is refused while the finding
    /// exists (`interface.publish`, [`ASSEMBLY_PUBLISH_BLOCKED`]); nothing
    /// else is.
    pub blocks_publish: bool,
}

const fn spec(
    code: &'static str,
    blocks_fabrication: bool,
    waivable: bool,
    blocks_publish: bool,
) -> CodeSpec {
    CodeSpec {
        code,
        blocks_fabrication,
        waivable,
        blocks_publish,
    }
}

/// Every finding code of the interface and the assembly link.
pub const CODES: [CodeSpec; 13] = [
    spec(UNPUBLISHED, false, true, false),
    spec(DATUM_LOST, false, true, false),
    spec(DATUM_INEXACT, false, true, false),
    spec(ASSEMBLY_STALE, false, true, false),
    spec(ASSEMBLY_OLDER_BOARD, false, true, false),
    spec(ASSEMBLY_BOTH_CHANGED, false, true, false),
    spec(ASSEMBLY_REFERENCE_LOST, false, true, true),
    spec(ASSEMBLY_REFERENCE_INEXACT, false, true, false),
    spec(ASSEMBLY_UNEXPORTED_FACE, false, true, false),
    spec(ASSEMBLY_CYCLE, true, false, false),
    spec(ASSEMBLY_EXTENT_CYCLE, false, true, false),
    spec(ASSEMBLY_MISSING, false, true, false),
    spec(ASSEMBLY_NEWER, false, true, false),
];

/// Whether a finding with `code` blocks publishing to the linked design.
pub fn blocks_publish(code: &str) -> bool {
    CODES.iter().any(|s| s.code == code && s.blocks_publish)
}

/// `interface.unpublished`: "3 datums changed since revision 14", or that
/// nothing was published yet.
pub fn unpublished(changed: usize, revision: Option<u32>) -> Finding {
    let datums = if changed == 1 { "datum" } else { "datums" };
    let message = match revision {
        Some(r) => format!("{changed} {datums} changed since revision {r}"),
        None => "The board interface has not been published".into(),
    };
    Finding::new(UNPUBLISHED, message, vec![ObjectRef::Board])
}

/// `interface.datum_lost`: `datum` is its display name ("J1 opening"),
/// `owner` the object it belonged to, when that still exists.
pub fn datum_lost(datum: &str, owner: Option<ObjectRef>) -> Finding {
    Finding::new(
        DATUM_LOST,
        format!("{datum} no longer exists on the board; unexport it or restore what it was on"),
        owner.into_iter().collect(),
    )
}

/// `interface.datum_inexact`.
pub fn datum_inexact(datum: &str, owner: Option<ObjectRef>) -> Finding {
    Finding::new(
        DATUM_INEXACT,
        format!("{datum} was found again by its shape, not by its name; check it"),
        owner.into_iter().collect(),
    )
}

/// `assembly.stale`.
pub fn assembly_stale() -> Finding {
    Finding::new(
        ASSEMBLY_STALE,
        "The linked design changed. Preview the update to see what moves".into(),
        vec![ObjectRef::Board],
    )
}

/// `assembly.older_board`: the assembly links board revision `revision`,
/// or (`None`) a board design file this board did not publish last and
/// does not remember.
pub fn assembly_older_board(revision: Option<u32>) -> Finding {
    let held = match revision {
        Some(revision) => format!("board revision {revision}"),
        None => "an older board".into(),
    };
    Finding::new(
        ASSEMBLY_OLDER_BOARD,
        format!(
            "The linked design still uses {held}. Run Update Links in SolidCraft, then check again"
        ),
        vec![ObjectRef::Board],
    )
}

/// `assembly.both_changed`.
pub fn assembly_both_changed() -> Finding {
    Finding::new(
        ASSEMBLY_BOTH_CHANGED,
        "The board and the linked design both changed since they last agreed. Preview the update \
         and choose which side wins where they conflict"
            .into(),
        vec![ObjectRef::Board],
    )
}

/// The refusal of a publish while references are broken: `broken` are the
/// findings' messages.
pub fn publish_blocked(broken: &[String]) -> Error {
    let first = broken.first().map(String::as_str).unwrap_or_default();
    let more = match broken.len() {
        0 | 1 => String::new(),
        n => format!(" ({} more)", n - 1),
    };
    Error::Conflict(format!(
        "{ASSEMBLY_PUBLISH_BLOCKED}: a reference between the board and the linked design is \
         broken, so publishing to it waits until it is resolved: {first}{more}"
    ))
}

/// `assembly.reference_lost`: "The linked design's sketch 'USB opening' uses
/// J1's opening. J1 was deleted, so the cutout would fail. Restore J1, or
/// pick another opening in SolidCraft." `feature` is the linked design's own
/// name for it, `datum` the datum's display name, `cause` what happened on
/// the board and `restore` what to restore.
pub fn assembly_reference_lost(
    feature: &str,
    datum: &str,
    cause: &str,
    restore: &str,
    objects: Vec<ObjectRef>,
) -> Finding {
    Finding::new(
        ASSEMBLY_REFERENCE_LOST,
        format!(
            "The linked design's {feature} uses {datum}. {cause}, so it would fail. Restore \
             {restore}, or pick another reference in SolidCraft"
        ),
        objects,
    )
}

/// `assembly.reference_inexact`.
pub fn assembly_reference_inexact(feature: &str, datum: &str, objects: Vec<ObjectRef>) -> Finding {
    Finding::new(
        ASSEMBLY_REFERENCE_INEXACT,
        format!(
            "The linked design's {feature} uses {datum}, which was found again only by its shape. \
             Check it in SolidCraft"
        ),
        objects,
    )
}

/// `assembly.unexported_face`: "Sketch 'Lid rib' is attached to a face of
/// U1 that is not exported. …"
pub fn assembly_unexported_face(feature: &str, part: &str, objects: Vec<ObjectRef>) -> Finding {
    Finding::new(
        ASSEMBLY_UNEXPORTED_FACE,
        format!(
            "The linked design's {feature} is attached to a face of {part} that is not exported. It \
             may move to another part when the board changes. Export the face to keep it stable"
        ),
        objects,
    )
}

/// `assembly.cycle`: `path` names the loop in both files' words ("Hole H2
/// follows boss 'Boss 3', and the linked design places the board by hole H2").
pub fn assembly_cycle(path: &str, objects: Vec<ObjectRef>) -> Finding {
    Finding::new(ASSEMBLY_CYCLE, path.to_owned(), objects)
}

/// `assembly.extent_cycle`: `path` names the loop in both files' words, as
/// for [`assembly_cycle`].
pub fn assembly_extent_cycle(path: &str, objects: Vec<ObjectRef>) -> Finding {
    Finding::new(ASSEMBLY_EXTENT_CYCLE, path.to_owned(), objects)
}

/// `assembly.missing`.
pub fn assembly_missing() -> Finding {
    Finding::new(
        ASSEMBLY_MISSING,
        "The linked design file was not found. The board uses the last known geometry".into(),
        vec![ObjectRef::Board],
    )
}

/// `assembly.newer`.
pub fn assembly_newer() -> Finding {
    Finding::new(
        ASSEMBLY_NEWER,
        "This linked design was saved by a newer SolidCraft than this Sparksmith can read".into(),
        vec![ObjectRef::Board],
    )
}

/// The refusal of a local edit to an edge the linked design drives.
pub fn driven_edge() -> Error {
    Error::Conflict(format!(
        "{ASSEMBLY_DRIVEN_EDGE}: this edge follows the linked design's wall. Break the link to edit \
         it here"
    ))
}
