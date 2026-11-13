//! The interface contract's own rules (slice B0): keys, kinds, tokens,
//! slots that are never reused, the table's invariants, the sidecar's
//! versioned reading and the recorded schemas. `skeletons` holds the
//! acceptance tests slices B1 to B3 fill in.
use super::*;
use crate::ids::{ContentHash, InterfaceSourceId};
use crate::library::footprint_source::{SketchCurveId, SketchRef};
use crate::library::{Outline, Segment};
use crate::model::board::BoardFeatureId;
use crate::model::dimension::{BoardRef, FollowedFaceId};
use crate::model::interface::{AutoRule, DatumId, DatumSource, Tier};
use std::collections::BTreeMap;

mod b1;
mod b3;
mod b4;
mod b5;
mod p1;
mod package_datums;
mod skeletons;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const SCHEMA_FILE: &str = "../../docs/architecture/board-interface.schema.json";

fn key(owner: DatumOwner, role: DatumRole, name: Option<&str>) -> DatumKey {
    DatumKey {
        owner,
        role,
        name: name.map(str::to_owned),
    }
}

fn datum(id: u64, slot: u32, key: DatumKey) -> InterfaceDatum {
    InterfaceDatum {
        id: DatumId(id),
        slot: Slot(slot),
        key,
        source: DatumSource::User,
        feature: None,
        name: None,
    }
}

fn opening(id: u64, slot: u32, instance: u64) -> InterfaceDatum {
    datum(
        id,
        slot,
        key(
            DatumOwner::Instance { id: instance },
            DatumRole::PartDatum,
            Some("opening"),
        ),
    )
}

#[test]
fn tokens_are_built_from_ids_only() -> Fallible {
    let board = |role, name| datum(1, 0, key(DatumOwner::Board, role, name));
    let cases = [
        (board(DatumRole::BoardFrame, None), "board.frame"),
        (board(DatumRole::BoardTop, None), "board.top"),
        (board(DatumRole::BoardBottom, None), "board.bottom"),
        (board(DatumRole::BoardOutline, None), "board.outline"),
        (board(DatumRole::BoardThickness, None), "board.thickness"),
        (
            board(DatumRole::BoardParameter, Some("corner_R")),
            "board.parameter.corner_R",
        ),
        (board(DatumRole::SideHeight, Some("top")), "side.top"),
        (
            datum(
                2,
                1,
                key(
                    DatumOwner::Hole {
                        id: BoardFeatureId(12),
                    },
                    DatumRole::Hole,
                    None,
                ),
            ),
            "h12",
        ),
        (
            datum(
                3,
                2,
                key(
                    DatumOwner::Cutout {
                        id: BoardFeatureId(7),
                    },
                    DatumRole::BoardCutout,
                    None,
                ),
            ),
            "cut7",
        ),
        (
            datum(
                4,
                3,
                key(DatumOwner::Instance { id: 80 }, DatumRole::PartFrame, None),
            ),
            "i80.frame",
        ),
        (
            datum(
                5,
                4,
                key(
                    DatumOwner::Instance { id: 80 },
                    DatumRole::PartEnvelope,
                    None,
                ),
            ),
            "i80.envelope",
        ),
        (opening(6, 5, 80), "i80.opening"),
    ];
    for (datum, expected) in cases {
        assert_eq!(token(&datum)?, expected);
    }

    // A display name never reaches the token.
    let mut renamed = opening(6, 5, 80);
    renamed.name = Some("USB-C at the front".into());
    assert_eq!(token(&renamed)?, "i80.opening");

    // A feature datum's token is its datum id, whatever it is on.
    let mut feature = datum(
        412,
        6,
        key(DatumOwner::Instance { id: 80 }, DatumRole::Feature, None),
    );
    feature.feature = Some(BoardRef::Board(SketchRef::Curve(SketchCurveId(
        "l2".into(),
    ))));
    assert_eq!(token(&feature)?, "d412");
    Ok(())
}

#[test]
fn keys_hold_their_owner_and_name_syntax() {
    let refused = [
        // A part datum on the board, without a name, or with a reserved one.
        key(DatumOwner::Board, DatumRole::PartDatum, Some("opening")),
        key(DatumOwner::Instance { id: 1 }, DatumRole::PartDatum, None),
        key(
            DatumOwner::Instance { id: 1 },
            DatumRole::PartDatum,
            Some("frame"),
        ),
        key(
            DatumOwner::Instance { id: 1 },
            DatumRole::PartDatum,
            Some("Opening"),
        ),
        // A hole that is not a board hole; a named board face.
        key(DatumOwner::Board, DatumRole::Hole, None),
        key(DatumOwner::Board, DatumRole::BoardTop, Some("top")),
        // A side that does not exist.
        key(DatumOwner::Board, DatumRole::SideHeight, Some("left")),
        // A parameter name that is not an identifier.
        key(DatumOwner::Board, DatumRole::BoardParameter, Some("2w")),
        // A feature cannot belong to a cutout loop.
        key(
            DatumOwner::Cutout {
                id: BoardFeatureId(1),
            },
            DatumRole::Feature,
            None,
        ),
    ];
    for k in refused {
        let refusal = check_key(&k)
            .err()
            .and_then(|e| e.rule().map(str::to_owned));
        assert_eq!(refusal.as_deref(), Some("interface.key"), "{k:?}");
    }
}

#[test]
fn roles_have_their_kinds() {
    use DatumKind as K;
    use DatumRole as R;
    let expected = [
        (R::BoardFrame, Some(K::Frame)),
        (R::BoardTop, Some(K::Plane)),
        (R::BoardBottom, Some(K::Plane)),
        (R::BoardOutline, Some(K::Profile)),
        (R::BoardCutout, Some(K::Profile)),
        (R::BoardThickness, Some(K::Parameter)),
        (R::BoardParameter, Some(K::Parameter)),
        (R::Hole, Some(K::Axis)),
        (R::PartFrame, Some(K::Frame)),
        (R::PartEnvelope, Some(K::Volume)),
        (R::SideHeight, Some(K::Volume)),
        (R::PartDatum, None),
        (R::Feature, None),
    ];
    for (role, kind) in expected {
        assert_eq!(role_kind(role), kind, "{role:?}");
    }
}

#[test]
fn slots_are_never_reused() -> Fallible {
    let mut table = InterfaceTable::default();
    for id in 0..3 {
        let slot = allocate_slot(&mut table)?;
        table.datums.push(opening(id, slot.0, 100 + id));
    }
    check_table(&table)?;
    let gone = retire(&mut table, Slot(1))?;
    assert_eq!(gone.id, DatumId(1));
    assert_eq!(table.retired, vec![Slot(1)]);
    // The next slot follows the last one ever allocated, not the gap.
    assert_eq!(allocate_slot(&mut table)?, Slot(3));
    check_table(&table)?;
    // Retiring twice finds nothing; the slot stays retired once.
    assert!(retire(&mut table, Slot(1)).is_err());
    assert_eq!(table.retired, vec![Slot(1)]);
    Ok(())
}

#[test]
fn the_slot_bound_refuses_without_changing_the_table() {
    let mut table = InterfaceTable {
        next_slot: crate::model::interface::MAX_SLOTS,
        ..InterfaceTable::default()
    };
    let rule = allocate_slot(&mut table)
        .err()
        .and_then(|e| e.rule().map(str::to_owned));
    assert_eq!(rule.as_deref(), Some("interface.slots"));
    assert_eq!(table.next_slot, crate::model::interface::MAX_SLOTS);
}

#[test]
fn the_table_refuses_a_reused_or_unallocated_slot() {
    let table = |datums: Vec<InterfaceDatum>, retired: Vec<u32>, next_slot| InterfaceTable {
        next_slot,
        datums,
        retired: retired.into_iter().map(Slot).collect(),
        ..InterfaceTable::default()
    };
    let refused = [
        // A retired slot in use again.
        table(vec![opening(1, 1, 10)], vec![1], 2),
        // A slot never allocated.
        table(vec![opening(1, 2, 10)], vec![], 2),
        // A retired slot never allocated.
        table(vec![], vec![4], 2),
        // Two datums in one slot, or out of slot order.
        table(vec![opening(1, 0, 10), opening(2, 0, 11)], vec![], 1),
        table(vec![opening(1, 1, 10), opening(2, 0, 11)], vec![], 2),
        // Two datums with one id, or one key.
        table(vec![opening(1, 0, 10), opening(1, 1, 11)], vec![], 2),
        table(vec![opening(1, 0, 10), opening(2, 1, 10)], vec![], 2),
    ];
    for t in refused {
        let rule = check_table(&t)
            .err()
            .and_then(|e| e.rule().map(str::to_owned));
        assert_eq!(rule.as_deref(), Some("interface.table"), "{t:?}");
    }
}

#[test]
fn the_table_refuses_an_assembly_feature_and_a_doubled_feature() {
    let feature = |id, slot, f| {
        let mut d = datum(id, slot, key(DatumOwner::Board, DatumRole::Feature, None));
        d.feature = Some(f);
        d
    };
    let curve = || BoardRef::Board(SketchRef::Curve(SketchCurveId("l2".into())));
    let assembly_link = InterfaceTable {
        next_slot: 1,
        datums: vec![feature(1, 0, BoardRef::FollowedFace(FollowedFaceId(1)))],
        ..InterfaceTable::default()
    };
    let doubled = InterfaceTable {
        next_slot: 2,
        datums: vec![feature(1, 0, curve()), feature(2, 1, curve())],
        ..InterfaceTable::default()
    };
    let mut keyed = opening(3, 0, 10);
    keyed.feature = Some(curve());
    let stray = InterfaceTable {
        next_slot: 1,
        datums: vec![keyed],
        ..InterfaceTable::default()
    };
    for t in [assembly_link, doubled, stray] {
        assert!(check_table(&t).is_err(), "{t:?}");
    }
}

fn frame() -> Frame {
    Frame {
        origin: Vec3([20.0, 0.0, 3.29]),
        x: Vec3([1.0, 0.0, 0.0]),
        y: Vec3([0.0, 0.0, 1.0]),
    }
}

fn sidecar() -> Sidecar {
    let square = Outline {
        start: [-4.5, -1.75],
        segments: vec![
            Segment::Line { to: [4.5, -1.75] },
            Segment::Line { to: [4.5, 1.75] },
            Segment::Line { to: [-4.5, 1.75] },
        ],
    };
    Sidecar {
        format: FORMAT.into(),
        source_id: InterfaceSourceId(0x5eed),
        revision: 14,
        hash: ContentHash([7; 32]),
        units: "mm".into(),
        tier: Tier::Interface,
        datums: vec![SidecarDatum {
            id: DatumId(412),
            slot: Slot(2),
            token: "i80.opening".into(),
            name: "J1 opening".into(),
            role: "part.opening".into(),
            kind: DatumKind::Volume,
            owner: DatumOwner::Instance { id: 80 },
            geometry: DatumGeometry::Volume {
                frame: frame(),
                loops: vec![square],
                height: 8.0,
            },
            clearance_mm: Some(0.3),
            status: DatumStatus::Resolved,
            hash: ContentHash([9; 32]),
            faces: BTreeMap::from([("front".to_owned(), "F7:start".to_owned())]),
        }],
        retired_slots: vec![Slot(3)],
        unresolved_slots: vec![],
    }
}

#[test]
fn a_sidecar_reads_back_as_written() -> Fallible {
    let written = serde_json::to_string(&sidecar())?;
    assert_eq!(Sidecar::read(&written)?, sidecar());
    Ok(())
}

#[test]
fn a_sidecar_ignores_fields_added_within_its_major() -> Fallible {
    let mut value = serde_json::to_value(sidecar())?;
    value["published_by"] = "a later Sparksmith".into();
    value["datums"][0]["colour"] = "red".into();
    assert_eq!(Sidecar::read(&value.to_string())?, sidecar());
    Ok(())
}

#[test]
fn a_sidecar_of_another_major_or_format_is_refused() -> Fallible {
    let with = |format: &str| -> Fallible<Option<String>> {
        let mut value = serde_json::to_value(sidecar())?;
        value["format"] = format.into();
        Ok(Sidecar::read(&value.to_string())
            .err()
            .and_then(|e| e.rule().map(str::to_owned)))
    };
    assert_eq!(
        with("sparksmith-board-interface/2")?.as_deref(),
        Some("interface.format_newer")
    );
    for other in [
        "sparksmith-board-interface",
        "sparksmith-board-interface/",
        "sparksmith-board-interface/1.1",
        "other-format/1",
    ] {
        assert_eq!(with(other)?.as_deref(), Some("interface.format"), "{other}");
    }
    let refused = Sidecar::read("[1, 2]")
        .err()
        .and_then(|e| e.rule().map(str::to_owned));
    assert_eq!(refused.as_deref(), Some("interface.format"));
    Ok(())
}

#[test]
fn a_sidecar_out_of_bounds_is_refused() -> Fallible {
    let mut doubled = sidecar();
    let mut copy = doubled.datums[0].clone();
    copy.slot = Slot(4);
    copy.id = DatumId(413);
    doubled.datums.push(copy);

    let mut retired = sidecar();
    retired.retired_slots = vec![Slot(2)];

    let mut huge = sidecar();
    if let DatumGeometry::Volume { height, .. } = &mut huge.datums[0].geometry {
        *height = f64::INFINITY;
    }

    let mut wrong_kind = sidecar();
    wrong_kind.datums[0].kind = DatumKind::Axis;

    let mut bad_token = sidecar();
    bad_token.datums[0].token = "J1 opening".into();

    let mut inches = sidecar();
    inches.units = "in".into();

    for case in [doubled, retired, huge, wrong_kind, bad_token, inches] {
        // Infinity is not JSON: check directly as well as through reading.
        assert!(case.check().is_err(), "{case:?}");
        if let Ok(text) = serde_json::to_string(&case) {
            assert!(Sidecar::read(&text).is_err(), "{text}");
        }
    }

    let too_long = " ".repeat(sidecar::MAX_SIDECAR_BYTES + 1);
    let rule = Sidecar::read(&too_long)
        .err()
        .and_then(|e| e.rule().map(str::to_owned));
    assert_eq!(rule.as_deref(), Some("interface.sidecar_size"));
    Ok(())
}

#[test]
fn commands_have_their_wire_names_and_refuse_unknown_fields() -> Fallible {
    let names: Vec<&str> = InterfaceCommand::CLASSES.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "interface.export",
            "interface.unexport",
            "interface.rename",
            "interface.auto",
            "interface.record_publish",
        ]
    );
    let export: InterfaceCommand = serde_json::from_value(serde_json::json!({
        "command": "interface.export",
        "params": {"target": {"target": "package_datum", "instance": 80, "name": "opening"}}
    }))?;
    assert_eq!(
        export,
        InterfaceCommand::Export {
            target: ExportTarget::PackageDatum {
                instance: 80,
                name: DatumName("opening".into()),
            },
            name: None,
        }
    );
    let auto: InterfaceCommand = serde_json::from_value(serde_json::json!({
        "command": "interface.auto", "params": {"rule": "holes", "on": false}
    }))?;
    assert_eq!(
        auto,
        InterfaceCommand::Auto {
            rule: AutoRule::Holes,
            on: false
        }
    );
    let unknown = serde_json::from_value::<InterfaceCommand>(serde_json::json!({
        "command": "interface.unexport", "params": {"datum": 1, "force": true}
    }));
    assert!(unknown.is_err());
    let publish: PublishParams = serde_json::from_value(serde_json::json!({
        "path": "board.solidcraft", "expected_revision": 7
    }))?;
    assert_eq!(publish.tier, Tier::Mechanical, "the default tier");
    Ok(())
}

#[test]
fn every_finding_code_is_listed_once() {
    let codes: std::collections::BTreeSet<&str> = findings::CODES.iter().map(|c| c.code).collect();
    assert_eq!(codes.len(), findings::CODES.len());
    // A broken assembly reference blocks only publishing to the linked
    // design; an inexact one blocks nothing.
    let spec = |code: &str| findings::CODES.iter().find(|c| c.code == code).copied();
    assert!(
        spec(findings::ASSEMBLY_REFERENCE_LOST)
            .is_some_and(|s| !s.blocks_fabrication && s.blocks_publish)
    );
    assert!(
        spec(findings::ASSEMBLY_REFERENCE_INEXACT)
            .is_some_and(|s| !s.blocks_fabrication && !s.blocks_publish)
    );
    // A loop through positions blocks fabrication with no waiver; one
    // through extents only is advisory.
    assert!(spec(findings::ASSEMBLY_CYCLE).is_some_and(|s| s.blocks_fabrication && !s.waivable));
    assert!(
        spec(findings::ASSEMBLY_EXTENT_CYCLE)
            .is_some_and(|s| !s.blocks_fabrication && !s.blocks_publish)
    );
    // Only broken references block publishing.
    let publishing: Vec<&str> = findings::CODES
        .iter()
        .filter(|c| c.blocks_publish)
        .map(|c| c.code)
        .collect();
    assert_eq!(publishing, [findings::ASSEMBLY_REFERENCE_LOST]);
    assert_eq!(
        findings::unpublished(3, Some(14)).message,
        "3 datums changed since revision 14"
    );
    assert_eq!(
        findings::driven_edge().rule(),
        Some(findings::ASSEMBLY_DRIVEN_EDGE)
    );
}

/// The recorded schemas are the generated ones. Set `PCBCRAFT_BLESS=1` to
/// rewrite the file after a deliberate contract change.
#[test]
fn the_recorded_schemas_match() -> Fallible {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(SCHEMA_FILE);
    let generated = schemas();
    if std::env::var_os("PCBCRAFT_BLESS").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&generated)? + "\n")?;
    }
    let recorded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    assert!(
        recorded == generated,
        "docs/architecture/board-interface.schema.json is stale; rerun with PCBCRAFT_BLESS=1"
    );
    Ok(())
}
