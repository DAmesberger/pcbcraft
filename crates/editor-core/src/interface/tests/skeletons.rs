//! Acceptance tests of slices B1 to B3, as placeholders that fail. Each
//! slice removes the `ignore` from its tests and writes the body; the name
//! and the doc comment are the requirement, from
//! docs/architecture/board-interface.md ("Slices and their tests") and the
//! design note's acceptance criteria. Never weaken one to make it pass.
use super::Fallible;

fn not_built(slice: &str) -> Fallible {
    Err(format!("{slice}: not built yet").into())
}

// B1: the datum model, automatic rules, slot table, commands, persistence.

/// A new board exports its frame, top, bottom, thickness, outline, every
/// cutout and mounting hole, and the tallest part per side, each in its own
/// slot, through `interface.list`.
#[test]
#[ignore = "B1: automatic rules"]
fn b1_automatic_rules_export_the_board() -> Fallible {
    not_built("B1")
}

/// Switching a rule off retires the slots of its datums; on again
/// allocates new slots, never the retired ones.
#[test]
#[ignore = "B1: interface.auto"]
fn b1_switching_a_rule_retires_and_never_reuses() -> Fallible {
    not_built("B1")
}

/// `interface.export` of a part face, a pad, a board curve, a part frame
/// and envelope and a board parameter allocates one slot each and is one
/// undo step each; exporting the same thing twice is refused unchanged.
#[test]
#[ignore = "B1: interface.export"]
fn b1_export_allocates_one_slot_per_datum() -> Fallible {
    not_built("B1")
}

/// Exporting a housing reference, a missing instance or an unknown package
/// datum is refused and leaves document, revision and history unchanged.
#[test]
#[ignore = "B1: refusals"]
fn b1_refused_exports_change_nothing() -> Fallible {
    not_built("B1")
}

/// `interface.unexport` retires the slot; a later export takes a new one.
#[test]
#[ignore = "B1: interface.unexport"]
fn b1_unexport_retires_the_slot() -> Fallible {
    not_built("B1")
}

/// `interface.rename` and renaming the part (J1 → J7) change the display
/// name only: id, slot and token stay.
#[test]
#[ignore = "B1: interface.rename"]
fn b1_renames_change_the_display_name_only() -> Fallible {
    not_built("B1")
}

/// Slots, retired slots and ids are the same after undo, redo, save and
/// reopen; undoing an export does not give its slot to the next export.
#[test]
#[ignore = "B1: persistence"]
fn b1_slots_survive_undo_redo_save_and_reopen() -> Fallible {
    not_built("B1")
}

/// Deleting the part an exported datum belongs to reports the datum as
/// lost (`interface.datum_lost`) and keeps its slot until it is
/// unexported.
#[test]
#[ignore = "B1: lost datums"]
fn b1_a_deleted_owner_makes_the_datum_lost() -> Fallible {
    not_built("B1")
}

/// Selecting, hovering and moving the camera never touch the table.
#[test]
#[ignore = "B1: view state is not document state"]
fn b1_selection_never_changes_the_interface() -> Fallible {
    not_built("B1")
}

// B2: package datums. The bodies are in `super::package_datums`; the
// part-file reader's half of the first also runs against the real USB4105
// part file in `pcbcraft-solidcraft` (`board::package_datum_tests`).

/// A USB-C part file's `opening` plane becomes the package datum
/// `opening`, and reaches a placed instance in the board frame on both
/// sides and at every rotation.
#[test]
fn b2_a_part_file_opening_reaches_the_board_frame() -> Fallible {
    super::package_datums::a_part_file_opening_reaches_the_board_frame()
}

/// A package datum marked `external` is exported for every placement by
/// the automatic rule; one not marked needs `interface.export`. B2 gives
/// the rule its input (`interface::package::external_datums`, tested in
/// `package_datums`); the rule and `interface.export` are B1's.
#[test]
#[ignore = "B1: the external_datums rule and interface.export"]
fn b2_external_datums_are_exported_per_placement() -> Fallible {
    not_built("B1")
}

/// No opening is inferred from a part's name or category: a part named
/// "USB-C" without a declared datum exports none.
#[test]
fn b2_no_opening_is_inferred_from_a_name() -> Fallible {
    super::package_datums::no_opening_is_inferred_from_a_name()
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
