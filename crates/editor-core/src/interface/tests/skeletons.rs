//! Acceptance tests of slices B2 and B3 (B1's are written, in `b1.rs`;
//! B2's bodies are in `package_datums.rs`, B3's in `b3.rs`). The name and
//! the doc comment are the requirement, from
//! docs/architecture/board-interface.md ("Slices and their tests") and the
//! design note's acceptance criteria. Never weaken one to make it pass.
use super::Fallible;

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
/// the automatic rule; one not marked needs `interface.export`.
#[test]
fn b2_external_datums_are_exported_per_placement() -> Fallible {
    super::package_datums::external_datums_are_exported_per_placement()
}

/// No opening is inferred from a part's name or category: a part named
/// "USB-C" without a declared datum exports none.
#[test]
fn b2_no_opening_is_inferred_from_a_name() -> Fallible {
    super::package_datums::no_opening_is_inferred_from_a_name()
}

// B3: the exported design, the sidecar, publish and its preview
// (acceptance 1 to 5 and 8). The core's halves are here, with their bodies
// in `super::b3`; the halves that need Solidcraft or files run where those
// are: the exported design and a headless SolidCraft enclosure in
// `pcbcraft-solidcraft` (`interface::tests`), and `interface.publish` and
// `interface.publish_preview` over the ESP32 example in the automation
// session (`session::interface_publish_tests`), under the same names.

/// Acceptance 1 and 2: J1 moved 2 mm by a typed command; the preview lists
/// exactly one moved datum, "J1 opening, 2 mm along X".
#[test]
fn b3_the_preview_lists_the_one_moved_datum() -> Fallible {
    super::b3::the_preview_lists_the_one_moved_datum()
}

/// The sidecar's revision counts only publishes whose hash differs, and
/// `interface.publish` at a stale `expected_revision` writes nothing (that
/// half in the automation session).
#[test]
fn b3_publish_records_a_revision_only_when_the_hash_changes() -> Fallible {
    super::b3::publish_records_a_revision_only_when_the_hash_changes()
}
