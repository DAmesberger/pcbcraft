//! The interface's and enclosure link's findings and refusals, in the
//! user's words (docs/architecture/board-interface.md, "Conflicts and
//! broken references"; docs/board-sketch-references.md §7 for the
//! `housing.*` codes it already named). Every code is listed once in
//! [`CODES`] with whether it blocks fabrication output and whether a
//! waiver may lift that. Slices B1 and B5 raise them and wire the blocking
//! ones into the production check.
use crate::diagnostics::{Error, Finding, ObjectRef};

/// The interface changed since the last publish (advisory).
pub const UNPUBLISHED: &str = "interface.unpublished";
/// An exported datum's owner or feature is gone; it keeps its slot.
pub const DATUM_LOST: &str = "interface.datum_lost";
/// An exported datum was found again by geometry, not by name.
pub const DATUM_INEXACT: &str = "interface.datum_inexact";
/// The enclosure changed since the board last read it (advisory).
pub const HOUSING_STALE: &str = "housing.stale";
/// The enclosure still links an older published board.
pub const HOUSING_OLDER_BOARD: &str = "housing.older_board";
/// The board and the enclosure both changed since they last agreed.
pub const HOUSING_BOTH_CHANGED: &str = "housing.both_changed";
/// An enclosure feature uses a datum that no longer resolves.
pub const HOUSING_REFERENCE_LOST: &str = "housing.reference_lost";
/// An enclosure feature's datum was found again only by geometry.
pub const HOUSING_REFERENCE_INEXACT: &str = "housing.reference_inexact";
/// An enclosure feature references a face that is not exported.
pub const HOUSING_UNEXPORTED_FACE: &str = "housing.unexported_face";
/// The board and the enclosure depend on each other in a loop.
pub const HOUSING_CYCLE: &str = "housing.cycle";
/// The enclosure file was not found; the board uses its snapshot.
pub const HOUSING_MISSING: &str = "housing.missing";
/// The enclosure was saved by a newer SolidCraft than this build reads.
pub const HOUSING_NEWER: &str = "housing.newer";
/// Refusal: a board edge driven by the enclosure was edited locally.
pub const HOUSING_DRIVEN_EDGE: &str = "housing.driven_edge";

/// One finding code's consequences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeSpec {
    pub code: &'static str,
    /// Fabrication output is refused while the finding exists.
    pub blocks_fabrication: bool,
    /// A pre-flight waiver may lift the block. Lost and inexact references
    /// are never waived, as for part dimensions (`board.reference_lost`):
    /// they are re-picked or acknowledged in an impact preview.
    pub waivable: bool,
}

const fn spec(code: &'static str, blocks_fabrication: bool, waivable: bool) -> CodeSpec {
    CodeSpec {
        code,
        blocks_fabrication,
        waivable,
    }
}

/// Every finding code of the interface and the enclosure link.
pub const CODES: [CodeSpec; 12] = [
    spec(UNPUBLISHED, false, true),
    spec(DATUM_LOST, false, true),
    spec(DATUM_INEXACT, false, true),
    spec(HOUSING_STALE, false, true),
    spec(HOUSING_OLDER_BOARD, false, true),
    spec(HOUSING_BOTH_CHANGED, false, true),
    spec(HOUSING_REFERENCE_LOST, true, false),
    spec(HOUSING_REFERENCE_INEXACT, true, false),
    spec(HOUSING_UNEXPORTED_FACE, false, true),
    spec(HOUSING_CYCLE, true, false),
    spec(HOUSING_MISSING, false, true),
    spec(HOUSING_NEWER, false, true),
];

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

/// `housing.stale`.
pub fn housing_stale() -> Finding {
    Finding::new(
        HOUSING_STALE,
        "The enclosure changed. Preview the update to see what moves".into(),
        vec![ObjectRef::Board],
    )
}

/// `housing.older_board`: the enclosure links board revision `revision`.
pub fn housing_older_board(revision: u32) -> Finding {
    Finding::new(
        HOUSING_OLDER_BOARD,
        format!(
            "The enclosure still uses board revision {revision}. Run Update Links in SolidCraft, \
             then check again"
        ),
        vec![ObjectRef::Board],
    )
}

/// `housing.both_changed`.
pub fn housing_both_changed() -> Finding {
    Finding::new(
        HOUSING_BOTH_CHANGED,
        "The board and the enclosure both changed since they last agreed. Update the enclosure \
         first; nothing was applied"
            .into(),
        vec![ObjectRef::Board],
    )
}

/// `housing.reference_lost`: "The enclosure's sketch 'USB opening' uses
/// J1's opening. J1 was deleted, so the cutout would fail. Restore J1, or
/// pick another opening in SolidCraft." `feature` is the enclosure's own
/// name for it, `datum` the datum's display name, `cause` what happened on
/// the board and `restore` what to restore.
pub fn housing_reference_lost(
    feature: &str,
    datum: &str,
    cause: &str,
    restore: &str,
    objects: Vec<ObjectRef>,
) -> Finding {
    Finding::new(
        HOUSING_REFERENCE_LOST,
        format!(
            "The enclosure's {feature} uses {datum}. {cause}, so it would fail. Restore \
             {restore}, or pick another reference in SolidCraft"
        ),
        objects,
    )
}

/// `housing.reference_inexact`.
pub fn housing_reference_inexact(feature: &str, datum: &str, objects: Vec<ObjectRef>) -> Finding {
    Finding::new(
        HOUSING_REFERENCE_INEXACT,
        format!(
            "The enclosure's {feature} uses {datum}, which was found again only by its shape. \
             Check it in SolidCraft"
        ),
        objects,
    )
}

/// `housing.unexported_face`: "Sketch 'Lid rib' is attached to a face of
/// U1 that is not exported. …"
pub fn housing_unexported_face(feature: &str, part: &str, objects: Vec<ObjectRef>) -> Finding {
    Finding::new(
        HOUSING_UNEXPORTED_FACE,
        format!(
            "The enclosure's {feature} is attached to a face of {part} that is not exported. It \
             may move to another part when the board changes. Export the face to keep it stable"
        ),
        objects,
    )
}

/// `housing.cycle`: `path` names the loop in both files' words ("Hole H2
/// follows boss 'Boss 3', and the enclosure places the board by hole H2").
pub fn housing_cycle(path: &str, objects: Vec<ObjectRef>) -> Finding {
    Finding::new(HOUSING_CYCLE, path.to_owned(), objects)
}

/// `housing.missing`.
pub fn housing_missing() -> Finding {
    Finding::new(
        HOUSING_MISSING,
        "The enclosure file was not found. The board uses the last known geometry".into(),
        vec![ObjectRef::Board],
    )
}

/// `housing.newer`.
pub fn housing_newer() -> Finding {
    Finding::new(
        HOUSING_NEWER,
        "This enclosure was saved by a newer SolidCraft than this Sparksmith can read".into(),
        vec![ObjectRef::Board],
    )
}

/// The refusal of a local edit to an edge the enclosure drives.
pub fn driven_edge() -> Error {
    Error::Conflict(format!(
        "{HOUSING_DRIVEN_EDGE}: this edge follows the enclosure's wall. Break the link to edit \
         it here"
    ))
}
