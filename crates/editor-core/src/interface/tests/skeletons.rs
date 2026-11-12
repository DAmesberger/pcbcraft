//! Acceptance tests of slices B2 and B3, as placeholders that fail (B1's
//! are written, in `b1.rs`). Each slice removes the `ignore` from its tests
//! and writes the body; the name and the doc comment are the requirement,
//! from
//! docs/architecture/board-interface.md ("Slices and their tests") and the
//! design note's acceptance criteria. Never weaken one to make it pass.
use super::Fallible;

fn not_built(slice: &str) -> Fallible {
    Err(format!("{slice}: not built yet").into())
}

// B2: package datums.

/// A USB-C part file's `opening` plane becomes the package datum
/// `opening`, and reaches a placed instance in the board frame on both
/// sides and at every rotation.
#[test]
#[ignore = "B2: package datums"]
fn b2_a_part_file_opening_reaches_the_board_frame() -> Fallible {
    not_built("B2")
}

/// A package datum marked `external` is exported for every placement by
/// the automatic rule; one not marked needs `interface.export`.
#[test]
#[ignore = "B2: external package datums"]
fn b2_external_datums_are_exported_per_placement() -> Fallible {
    not_built("B2")
}

/// No opening is inferred from a part's name or category: a part named
/// "USB-C" without a declared datum exports none.
#[test]
#[ignore = "B2: no inference from names"]
fn b2_no_opening_is_inferred_from_a_name() -> Fallible {
    not_built("B2")
}

// B3: the exported design, the sidecar, publish and its preview
// (acceptance 1 to 5 and 8).

/// Acceptance 1 and 2: J1 moved 2 mm by a typed command; the preview lists
/// exactly one moved datum, "J1 opening, 2 mm along X".
#[test]
#[ignore = "B3: publish_preview"]
fn b3_the_preview_lists_the_one_moved_datum() -> Fallible {
    not_built("B3")
}

/// Acceptance 3: a headless SolidCraft test enclosure re-evaluates after
/// `component.update_links` with the cutout moved 2.000 mm ± 1 µm and no
/// other face of the wall moved.
#[test]
#[ignore = "B3: exported design"]
fn b3_the_enclosure_cutout_follows_by_two_millimetres() -> Fallible {
    not_built("B3")
}

/// Acceptance 4: closing and reopening the project, or building on another
/// machine, gives a byte-identical sidecar with the same face names.
#[test]
#[ignore = "B3: deterministic sidecar"]
fn b3_the_sidecar_is_byte_identical_after_reopen() -> Fallible {
    not_built("B3")
}

/// Acceptance 5: deleting an unrelated part, adding a fill and renaming J1
/// change nothing the enclosure references.
#[test]
#[ignore = "B3: stable references"]
fn b3_unrelated_edits_change_nothing_in_the_enclosure() -> Fallible {
    not_built("B3")
}

/// The sidecar's revision counts only publishes whose hash differs, and
/// `interface.publish` at a stale `expected_revision` writes nothing.
#[test]
#[ignore = "B3: interface.publish"]
fn b3_publish_records_a_revision_only_when_the_hash_changes() -> Fallible {
    not_built("B3")
}

/// Acceptance 8 (target, until measured): publishing the interface tier of
/// the ESP32 board takes under 100 ms in release.
#[test]
#[ignore = "B3: timing target; run in release with --ignored --nocapture"]
fn b3_publishing_the_interface_tier_is_under_100_ms() -> Fallible {
    not_built("B3")
}
