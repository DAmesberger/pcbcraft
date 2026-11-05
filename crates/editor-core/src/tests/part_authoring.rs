//! Part authoring through the public commands: a footprint from a pad table,
//! the standard fields, redrawing a symbol, an envelope's default place and a
//! tabbed form's datasheet land pattern. Expected geometry is by hand from the
//! rows in each test.
use super::support::{self, Fixture, Snapshot, Spec, apply, body, passive};
use crate::fields;
use crate::geometry::Bounds;
use crate::library::bundle::content_hash;
use crate::library::graphics::{DrawingLayer, Item};
use crate::library::package_form::tabbed::{TabbedForm, TabbedLands};
use crate::library::pad_table::{FootprintFromPads, HoleRow, PadRow, PadTableReport, SlotRow};
use crate::library::symbol_style::SymbolKind;
use crate::library::templates::catalog::{FamilyKind, NewFromTemplate, RowSource, SkeletonHint};
use crate::library::{
    Definition, FootprintDefinition, HoleShape, ModelPlacement, PackageDefinition, PackageEdit,
    PackageForm, PadKind, QuickAction, SymbolDefinition, standard_fields,
};
use crate::{
    Engine, Error, FieldRef, LibraryCommand, Operation, Outcome, Query, QueryResult, Result,
    TerminalId, UnitId,
};

fn library(engine: &mut Engine, command: LibraryCommand) -> Result<Outcome> {
    apply(engine, Operation::Library(command))
}

fn footprint(engine: &Engine, id: crate::DefinitionId) -> Result<FootprintDefinition> {
    match body(engine, id)? {
        Definition::Footprint(f) => Ok(f),
        other => Err(Error::Invalid(format!("not a footprint: {other:?}"))),
    }
}

fn package(engine: &Engine, id: crate::DefinitionId) -> Result<PackageDefinition> {
    match body(engine, id)? {
        Definition::Package(p) => Ok(p),
        other => Err(Error::Invalid(format!("not a package: {other:?}"))),
    }
}

fn symbol(engine: &Engine, id: crate::DefinitionId) -> Result<SymbolDefinition> {
    match body(engine, id)? {
        Definition::Symbol(s) => Ok(s),
        other => Err(Error::Invalid(format!("not a symbol: {other:?}"))),
    }
}

/// A draft of three package terminals A1, B1 and S on centred pads, so the
/// envelope has not moved, with a 9 × 7 × 3.2 envelope.
fn connector(engine: &mut Engine) -> Result<Fixture> {
    let spec = Spec {
        name: "Receptacle",
        terminals: vec![
            (1, "A", passive()),
            (2, "B", passive()),
            (3, "S", passive()),
        ],
        units: vec![(
            10,
            "A",
            vec![(1, [-7.5, 0.0]), (2, [-7.5, -2.5]), (3, [7.5, 0.0])],
        )],
        package: vec![(1, "A1"), (2, "B1"), (3, "S")],
        mapping: vec![(1, 1), (2, 2), (3, 3)],
        physical: true,
    };
    let fixture = support::draft(engine, &spec)?;
    let mut sized = package(engine, fixture.package)?;
    sized.envelope = [9.0, 7.0, 3.2];
    let meta = support::meta("Receptacle package");
    support::edit(
        engine,
        vec![crate::library::DraftEdit {
            id: fixture.package,
            meta,
            definition: Definition::Package(sized),
        }],
    )?;
    Ok(fixture)
}

fn pad_row(number: &str, at: [f64; 2], terminal: &str) -> PadRow {
    PadRow {
        number: number.into(),
        at,
        size: [0.6, 1.2],
        shape: Default::default(),
        rotation_deg: 0.0,
        side: Default::default(),
        drill: None,
        terminal: Some(terminal.into()),
        mechanical: false,
    }
}

fn slot_row(number: &str, at: [f64; 2], terminal: &str) -> SlotRow {
    SlotRow {
        number: number.into(),
        at,
        slot: [0.6, 1.6],
        land: Some([1.0, 2.0]),
        plated: true,
        terminal: Some(terminal.into()),
    }
}

/// The table: two lands on y = 0, two shell slots at y = 2 and two pegs. The
/// origin is the board edge, not the middle.
fn table(draft: crate::DefinitionId) -> FootprintFromPads {
    FootprintFromPads {
        draft,
        pads: vec![
            pad_row("A1", [-3.0, 0.0], "A1"),
            pad_row("B1", [3.0, 0.0], "B1"),
        ],
        slots: vec![
            slot_row("S1", [-4.32, 2.0], "S"),
            slot_row("S2", [4.32, 2.0], "S"),
        ],
        holes: vec![
            HoleRow {
                number: String::new(),
                at: [-2.89, 1.5],
                diameter: 0.65,
            },
            HoleRow {
                number: String::new(),
                at: [2.89, 1.5],
                diameter: 0.65,
            },
        ],
        assign_by_number: false,
        replace: true,
    }
}

fn from_pads(engine: &mut Engine, request: FootprintFromPads) -> Result<PadTableReport> {
    match library(engine, LibraryCommand::FootprintFromPads(Box::new(request)))? {
        Outcome::FootprintFromPads { report } => Ok(report),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

fn courtyard(footprint: &FootprintDefinition) -> Result<Bounds> {
    footprint
        .graphics
        .iter()
        .find(|g| g.layer == DrawingLayer::Courtyard)
        .and_then(|g| match &g.item {
            Item::Shape { shape, .. } => Some(shape.bounds(0.0)),
            Item::Text(_) => None,
        })
        .ok_or_else(|| Error::NotFound("no courtyard".into()))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn a_pad_table_makes_the_footprint_and_one_undo_step() -> Result<()> {
    let mut engine = Engine::default();
    let f = connector(&mut engine)?;
    let before_footprint = footprint(&engine, f.footprint)?;
    let before_package = package(&engine, f.package)?;
    let revision = engine.revision();
    let report = from_pads(&mut engine, table(f.footprint))?;
    assert_eq!(engine.revision(), revision + 1, "one undo step");
    assert_eq!(report.pads.len(), 6);
    assert!(report.unassigned.is_empty(), "{:?}", report.unassigned);
    assert!(report.unlanded.is_empty(), "{:?}", report.unlanded);

    let built = footprint(&engine, f.footprint)?;
    assert!(built.source.is_none(), "an explicit footprint");
    assert_eq!(built.pads.len(), 6);
    assert_eq!(built.pads[0].kind, PadKind::Smd);
    assert_eq!(built.pads[0].number, "A1");
    let package_now = package(&engine, f.package)?;
    let terminal = |d: &str| package_now.terminals.iter().find(|t| t.designation == d);
    assert_eq!(built.pads[0].terminal, terminal("A1").map(|t| t.id));
    // Both slots land the one shell terminal.
    assert_eq!(built.pads[2].terminal, terminal("S").map(|t| t.id));
    assert_eq!(built.pads[3].terminal, terminal("S").map(|t| t.id));
    assert_eq!(built.pads[2].kind, PadKind::PlatedThrough);
    let hole = built.pads[2].hole.ok_or(Error::NotFound("slot".into()))?;
    // 0.6 × 1.6 along y: width 0.6, length 1.6, turned a quarter.
    assert_eq!(
        hole.shape,
        HoleShape::Slot {
            width: 0.6,
            length: 1.6,
            angle_deg: 90.0
        }
    );
    assert!(hole.plated);
    assert_eq!(built.pads[4].kind, PadKind::NonPlatedHole);
    assert_eq!(built.pads[4].terminal, None);

    // Everything spans x ±4.82 (slot land 4.32 + 0.5) and y −0.6 (land
    // 1.2 / 2) to 3.0 (slot 2 + land 1), so the middle is (0, 1.2).
    assert_eq!(report.envelope_moved_to, Some([0.0, 1.2, 0.0]));
    assert_eq!(package_now.placement.translation, [0.0, 1.2, 0.0]);
    // The 9 × 7 body there is x ±4.5, y −2.3 to 4.7. The courtyard adds
    // 0.25 and rounds out to 0.05: x ±5.10 from the land, y −2.55 to 4.95.
    let yard = courtyard(&built)?;
    assert!(
        close(yard.min[0], -5.1) && close(yard.max[0], 5.1),
        "{yard:?}"
    );
    assert!(
        close(yard.min[1], -2.55) && close(yard.max[1], 4.95),
        "{yard:?}"
    );
    // The silkscreen and the assembly outline are there.
    assert!(
        built
            .graphics
            .iter()
            .any(|g| g.layer == DrawingLayer::Silkscreen)
    );
    assert!(
        built
            .graphics
            .iter()
            .any(|g| g.layer == DrawingLayer::Assembly)
    );

    // The drawn set publishes, and undo takes back pads and body together.
    engine.undo()?;
    assert_eq!(footprint(&engine, f.footprint)?, before_footprint);
    assert_eq!(package(&engine, f.package)?, before_package);
    engine.redo()?;
    assert_eq!(footprint(&engine, f.footprint)?, built);
    support::publish(&mut engine, f.all())?;
    Ok(())
}

#[test]
fn a_bad_table_changes_nothing() -> Result<()> {
    let mut engine = Engine::default();
    let f = connector(&mut engine)?;
    let before = Snapshot::of(&engine);
    let refused = |engine: &mut Engine, request: FootprintFromPads, code: &str| {
        let what = format!("{code}: {request:?}");
        let result = library(engine, LibraryCommand::FootprintFromPads(Box::new(request)));
        let error = format!("{result:?}");
        assert!(result.is_err() && error.contains(code), "{what}: {error}");
        before.assert_kept(engine, &what);
    };

    let mut keeps = table(f.footprint);
    keeps.replace = false;
    refused(&mut engine, keeps, "footprint.template_not_empty");

    let mut empty = table(f.footprint);
    (empty.pads, empty.slots, empty.holes) = (vec![], vec![], vec![]);
    refused(&mut engine, empty, "no rows");

    let mut unknown = table(f.footprint);
    unknown.pads[0].terminal = Some("Z9".into());
    refused(&mut engine, unknown, "does not have");

    let mut unnumbered = table(f.footprint);
    unnumbered.pads[0].number = " ".into();
    refused(&mut engine, unnumbered, "needs a number");

    let mut nan = table(f.footprint);
    nan.pads[0].at = [f64::NAN, 0.0];
    refused(&mut engine, nan, "finite");

    let mut zero = table(f.footprint);
    zero.pads[0].size = [0.0, 1.0];
    refused(&mut engine, zero, "footprint.pad_size");

    let mut drilled = table(f.footprint);
    drilled.pads[0].side = crate::library::PadSide::Bottom;
    drilled.pads[0].drill = Some(0.5);
    refused(&mut engine, drilled, "footprint.pad_side");

    let mut big = table(f.footprint);
    big.slots[0].slot = [3.0, 3.0];
    refused(&mut engine, big, "footprint.hole_outside_land");

    let mut unplated = table(f.footprint);
    unplated.slots[0].plated = false;
    refused(&mut engine, unplated, "no land");

    let mut not_a_footprint = table(f.package);
    not_a_footprint.draft = f.package;
    refused(&mut engine, not_a_footprint, "not a footprint");

    let mut too_many = table(f.footprint);
    too_many.pads = vec![pad_row("1", [0.0, 0.0], "A1"); crate::library::MAX_PADS + 1];
    refused(&mut engine, too_many, "at most");

    // Malformed input never reaches the engine.
    let text = r#"{"command": "library.footprint_from_pads",
                   "params": {"draft": 1, "pads": [], "stray": true}}"#;
    assert!(serde_json::from_str::<LibraryCommand>(text).is_err());
    Ok(())
}

#[test]
fn pads_land_terminals_only_as_the_table_says() -> Result<()> {
    let mut engine = Engine::default();
    let f = connector(&mut engine)?;
    let mut request = table(f.footprint);
    for pad in &mut request.pads {
        pad.terminal = None;
    }
    for slot in &mut request.slots {
        slot.terminal = None;
    }
    let report = from_pads(&mut engine, request.clone())?;
    assert_eq!(report.unassigned, ["A1", "B1", "S1", "S2"]);
    assert_eq!(report.unlanded, ["A1", "B1", "S"]);
    assert!(
        footprint(&engine, f.footprint)?
            .pads
            .iter()
            .all(|p| p.terminal.is_none())
    );
    // By number: A1 and B1 match; the slots' numbers name no terminal.
    request.assign_by_number = true;
    let report = from_pads(&mut engine, request)?;
    assert_eq!(report.unassigned, ["S1", "S2"]);
    assert_eq!(report.unlanded, ["S"]);
    Ok(())
}

#[test]
fn replacing_keeps_identities_unused_and_rebuilds_the_drawings() -> Result<()> {
    let mut engine = Engine::default();
    let f = connector(&mut engine)?;
    from_pads(&mut engine, table(f.footprint))?;
    let first = footprint(&engine, f.footprint)?;
    let highest = first.pads.iter().map(|p| p.id.0).max().unwrap_or(0);
    from_pads(&mut engine, table(f.footprint))?;
    let second = footprint(&engine, f.footprint)?;
    assert!(
        second.pads.iter().all(|p| p.id.0 > highest),
        "ids are not reused"
    );
    let yards = second
        .graphics
        .iter()
        .filter(|g| g.layer == DrawingLayer::Courtyard)
        .count();
    assert_eq!(yards, 1, "the old courtyard is gone");
    Ok(())
}

#[test]
fn a_set_placement_stays_and_a_plain_edit_is_stored_as_given() -> Result<()> {
    let mut engine = Engine::default();
    let f = connector(&mut engine)?;
    // A plain draft edit moves the pads 11 mm along x and nothing else: pads
    // at 9.5, 11 and 12.5, no courtyard.
    let mut moved = footprint(&engine, f.footprint)?;
    for pad in &mut moved.pads {
        pad.position[0] += 11.0;
    }
    support::edit(
        &mut engine,
        vec![crate::library::DraftEdit {
            id: f.footprint,
            meta: support::meta("Receptacle footprint"),
            definition: Definition::Footprint(moved),
        }],
    )?;
    assert_eq!(
        package(&engine, f.package)?.placement,
        ModelPlacement::default()
    );

    // The explicit action centres the body on the box of those pads.
    library(
        &mut engine,
        LibraryCommand::EditPackage {
            draft: f.package,
            edits: vec![PackageEdit::ApplyQuick(QuickAction::CentreOnCourtyard)],
        },
    )?;
    assert_eq!(
        package(&engine, f.package)?.placement.translation,
        [11.0, 0.0, 0.0]
    );

    // A placement someone set stays when a table rebuilds the pads.
    let set = ModelPlacement {
        translation: [1.0, 0.0, 0.0],
        rotation_deg: [0.0; 3],
    };
    library(
        &mut engine,
        LibraryCommand::EditPackage {
            draft: f.package,
            edits: vec![PackageEdit::SetPlacement(set)],
        },
    )?;
    let report = from_pads(&mut engine, table(f.footprint))?;
    assert_eq!(report.envelope_moved_to, None);
    assert_eq!(package(&engine, f.package)?.placement, set);
    Ok(())
}

#[test]
fn a_typed_form_or_another_model_keeps_its_placement() -> Result<()> {
    use crate::library::package_fit::envelope_default;
    use crate::library::package_form::{ChipForm, ChipKind};
    let footprint = FootprintDefinition {
        pads: vec![crate::library::Pad::smd_rect(
            crate::PadId(1),
            crate::ids::PackageTerminalId(1),
            "1",
            [5.0, 0.0],
            [1.0, 1.0],
        )],
        ..FootprintDefinition::default()
    };
    let generic = PackageDefinition::default();
    let moved = envelope_default(&generic, &footprint)?.ok_or(Error::NotFound("moved".into()))?;
    assert_eq!(moved.translation, [5.0, 0.0, 0.0]);
    let typed = PackageDefinition {
        form: PackageForm::Chip(Box::new(ChipForm::new(ChipKind::Resistor, [1.0, 0.5, 0.5]))),
        ..PackageDefinition::default()
    };
    assert_eq!(envelope_default(&typed, &footprint)?, None);
    let generated = PackageDefinition {
        model: crate::library::PackageModel::Generated {
            family_rev: 1,
            materials: Default::default(),
        },
        ..PackageDefinition::default()
    };
    assert_eq!(envelope_default(&generated, &footprint)?, None);
    // Nothing to centre over.
    assert_eq!(
        envelope_default(&generic, &FootprintDefinition::default())?,
        None
    );
    Ok(())
}

#[test]
fn a_datum_offset_moves_the_body_to_the_same_place() -> Result<()> {
    use crate::library::package_fit::envelope_default;
    let mut footprint = FootprintDefinition {
        pads: vec![crate::library::Pad::smd_rect(
            crate::PadId(1),
            crate::ids::PackageTerminalId(1),
            "1",
            [5.0, 2.0],
            [1.0, 1.0],
        )],
        ..FootprintDefinition::default()
    };
    footprint.datum.offset = [1.0, 1.0];
    let package = PackageDefinition {
        envelope: [2.0, 2.0, 1.0],
        ..PackageDefinition::default()
    };
    let placement =
        envelope_default(&package, &footprint)?.ok_or(Error::NotFound("moved".into()))?;
    // In the datum frame the pad centre is (5 − 1, 2 − 1).
    assert_eq!(placement.translation, [4.0, 1.0, 0.0]);
    // Through the one frame the outline is centred on the pad again.
    let mut placed = package.clone();
    placed.placement = placement;
    let outline = footprint.envelope_outline(&placed);
    assert_eq!(outline.center(), [5.0, 2.0]);
    Ok(())
}

#[test]
fn the_standard_fields_import_in_one_step_and_read_their_units() -> Result<()> {
    let mut engine = Engine::default();
    let revision = engine.revision();
    let Outcome::Imported { report } =
        library(&mut engine, LibraryCommand::ImportStandardFields {})?
    else {
        return Err(Error::Invalid("not an import".into()));
    };
    assert_eq!(engine.revision(), revision + 1, "one undo step");
    assert_eq!(report.revisions.len(), 15, "seven kinds, eight fields");
    let library_now = &engine.project().library;
    assert_eq!(library_now.quantity_kinds.len(), 7);
    assert_eq!(library_now.fields.len(), 8);

    let field = |name: &str| -> Result<FieldRef> {
        library_now
            .fields
            .iter()
            .find(|f| f.meta.name == name)
            .map(|f| FieldRef::new(f.id, f.revision))
            .ok_or_else(|| Error::NotFound(format!("no field {name}")))
    };
    // Hand-read: 4k7 is 4700 Ω; 100n is 100e-9 F; 5% is 0.05 of the nominal.
    let read = |name: &str, text: &str| -> Result<fields::FieldValue> {
        let view = library_now
            .field_view(field(name)?)
            .ok_or_else(|| Error::NotFound(name.into()))?;
        fields::parse_value(text, &view.field.definition.value_type, view.kind_body())
    };
    let si = |value: fields::FieldValue| match value {
        fields::FieldValue::Quantity { si, .. } => si,
        _ => f64::NAN,
    };
    assert!((si(read("Resistance", "4.7k")?) - 4700.0).abs() < 1e-9);
    assert!((si(read("Capacitance", "100nF")?) - 100e-9).abs() < 1e-18);
    assert!((si(read("Inductance", "10uH")?) - 10e-6).abs() < 1e-15);
    assert!((si(read("Rated voltage", "16V")?) - 16.0).abs() < 1e-9);
    assert!((si(read("Rated current", "500mA")?) - 0.5).abs() < 1e-9);
    assert!((si(read("Rated power", "250mW")?) - 0.25).abs() < 1e-9);
    assert!((si(read("Frequency", "40MHz")?) - 40e6).abs() < 1e-3);
    assert!(read("Rated voltage", "16A").is_err(), "another dimension");
    let fields::FieldValue::Tolerance { minus, plus } = read("Tolerance", "5%")? else {
        return Err(Error::Invalid("not a tolerance".into()));
    };
    assert!((minus - 0.05).abs() < 1e-12 && (plus - 0.05).abs() < 1e-12);

    // Again: reused, and the document (revision, undo) is left alone.
    let before = Snapshot::of(&engine);
    let Outcome::Imported { report } =
        library(&mut engine, LibraryCommand::ImportStandardFields {})?
    else {
        return Err(Error::Invalid("not an import".into()));
    };
    assert!(report.revisions.iter().all(|r| r.reused));
    before.assert_kept(&engine, "importing the fields again");

    engine.undo()?;
    assert!(engine.project().library.fields.is_empty());
    assert!(engine.project().library.quantity_kinds.is_empty());
    Ok(())
}

#[test]
fn the_standard_fields_query_gives_a_bundle_library_import_takes() -> Result<()> {
    let mut engine = Engine::default();
    let QueryResult::StandardFields(bundle) = engine.query(Query::StandardFields {})? else {
        return Err(Error::Invalid("not the bundle".into()));
    };
    assert_eq!(*bundle, standard_fields::bundle()?, "equal on every call");
    library(&mut engine, LibraryCommand::Import { bundle })?;
    assert_eq!(engine.project().library.fields.len(), 8);
    // The two ways are the same copies.
    let before = Snapshot::of(&engine);
    library(&mut engine, LibraryCommand::ImportStandardFields {})?;
    before.assert_kept(&engine, "the command reuses the query's import");
    Ok(())
}

/// A published record is immutable: changing the built-in definitions needs a
/// new revision, or a project holding the old copy refuses the import. This
/// pins the content of the first revisions.
#[test]
fn the_standard_records_do_not_change() -> Result<()> {
    let bundle = standard_fields::bundle()?;
    let root = bundle
        .records
        .iter()
        .find(|r| r.lineage == bundle.root.lineage)
        .ok_or_else(|| Error::NotFound("root".into()))?;
    assert_eq!(root.meta.name, "Resistance");
    assert_eq!(
        content_hash(root)?.to_string(),
        "940a4e0297cf0fecc2a9ad8d29c0a1910fcaddfd116c1a6d5975fb870a487b70"
    );
    Ok(())
}

/// A component of two terminals whose symbol is the IC box a pinout import
/// draws, with one shown field's text left out of the fixture.
fn two_pin_box(engine: &mut Engine) -> Result<Fixture> {
    let spec = Spec {
        name: "Indicator",
        terminals: vec![(1, "K", passive()), (2, "A", passive())],
        units: vec![(10, "A", vec![(1, [-7.5, 0.0]), (2, [7.5, 0.0])])],
        package: vec![(1, "1"), (2, "2")],
        mapping: vec![(1, 1), (2, 2)],
        physical: true,
    };
    support::draft(engine, &spec)
}

fn restyle(
    engine: &mut Engine,
    draft: crate::DefinitionId,
    kind: SymbolKind,
    terminals: Option<Vec<TerminalId>>,
) -> Result<Outcome> {
    library(
        engine,
        LibraryCommand::RestyleSymbol {
            draft,
            kind,
            terminals,
        },
    )
}

#[test]
fn a_symbol_is_redrawn_as_another_kind_keeping_its_unit() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin_box(&mut engine)?;
    let before = symbol(&engine, f.symbol)?;
    let revision = engine.revision();
    restyle(&mut engine, f.symbol, SymbolKind::Led, None)?;
    assert_eq!(engine.revision(), revision + 1, "one undo step");

    let drawn = symbol(&engine, f.symbol)?;
    let [unit] = drawn.units.as_slice() else {
        return Err(Error::Invalid("one unit".into()));
    };
    assert_eq!(unit.id, UnitId(10), "instances name the same unit");
    assert_eq!(unit.label, "A");
    assert!(!unit.graphics.is_empty());
    // A two-terminal symbol ends at x = ±5; terminal 1 (the cathode) first.
    let at = |t: u32| unit.pin(TerminalId(t)).map(|p| p.position);
    let (one, two) = (at(1).unwrap_or_default(), at(2).unwrap_or_default());
    assert_eq!([one[0].abs(), two[0].abs()], [5.0, 5.0]);
    assert!(one[0] != two[0]);
    // Fresh local IDs: none reuses a retired one, all below the counter.
    assert!(unit.graphics.iter().all(|g| g.id.0 >= before.next_local));
    assert!(unit.graphics.iter().all(|g| g.id.0 < drawn.next_local));
    assert!(drawn.next_local > unit.id.0);

    // The same drawing with the terminals stated the other way swaps them.
    restyle(
        &mut engine,
        f.symbol,
        SymbolKind::Led,
        Some(vec![TerminalId(2), TerminalId(1)]),
    )?;
    let swapped = symbol(&engine, f.symbol)?;
    let unit2 = swapped
        .units
        .first()
        .ok_or(Error::NotFound("unit".into()))?;
    assert_eq!(unit2.pin(TerminalId(2)).map(|p| p.position), Some(one));
    assert_eq!(unit2.pin(TerminalId(1)).map(|p| p.position), Some(two));

    // Back to a box, any count.
    restyle(&mut engine, f.symbol, SymbolKind::IcBox, None)?;
    restyle(&mut engine, f.symbol, SymbolKind::Connector, None)?;
    restyle(&mut engine, f.symbol, SymbolKind::Pushbutton, None)?;
    support::publish(&mut engine, f.all())?;
    Ok(())
}

#[test]
fn undo_returns_the_old_drawing() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin_box(&mut engine)?;
    let before = symbol(&engine, f.symbol)?;
    restyle(&mut engine, f.symbol, SymbolKind::Pushbutton, None)?;
    engine.undo()?;
    assert_eq!(symbol(&engine, f.symbol)?, before);
    Ok(())
}

#[test]
fn a_redraw_that_does_not_fit_changes_nothing() -> Result<()> {
    let mut engine = Engine::default();
    let f = two_pin_box(&mut engine)?;
    let before = Snapshot::of(&engine);
    let mut refused = |draft, kind, terminals, code: &str| {
        let result = restyle(&mut engine, draft, kind, terminals);
        let error = format!("{result:?}");
        assert!(result.is_err() && error.contains(code), "{code}: {error}");
        before.assert_kept(&engine, code);
    };
    // Three terminals' drawing on two terminals.
    refused(f.symbol, SymbolKind::NpnBjt, None, "template.symbol");
    // One terminal's.
    refused(f.symbol, SymbolKind::Ground, None, "template.symbol");
    // A list that is not exactly the component's terminals.
    let one = Some(vec![TerminalId(1)]);
    refused(f.symbol, SymbolKind::Led, one, "symbol.restyle_terminals");
    let twice = Some(vec![TerminalId(1), TerminalId(1)]);
    refused(f.symbol, SymbolKind::Led, twice, "symbol.restyle_terminals");
    let other = Some(vec![TerminalId(1), TerminalId(99)]);
    refused(f.symbol, SymbolKind::Led, other, "symbol.restyle_terminals");
    // Not a symbol.
    refused(f.package, SymbolKind::Led, None, "not a symbol");
    refused(
        crate::DefinitionId(9999),
        SymbolKind::Led,
        None,
        "no longer exists",
    );
    Ok(())
}

#[test]
fn a_symbol_of_several_units_is_not_redrawn() -> Result<()> {
    let mut engine = Engine::default();
    let spec = Spec {
        name: "Dual",
        terminals: vec![(1, "A", passive()), (2, "B", passive())],
        units: vec![
            (10, "A", vec![(1, [-7.5, 0.0])]),
            (11, "B", vec![(2, [-7.5, 0.0])]),
        ],
        package: vec![(1, "1"), (2, "2")],
        mapping: vec![(1, 1), (2, 2)],
        physical: true,
    };
    let f = support::draft(&mut engine, &spec)?;
    let before = Snapshot::of(&engine);
    let result = restyle(&mut engine, f.symbol, SymbolKind::Led, None);
    let error = format!("{result:?}");
    assert!(
        result.is_err() && error.contains("symbol.restyle_units"),
        "{error}"
    );
    before.assert_kept(&engine, "several units");
    Ok(())
}

/// TI's DBZ land pattern for a SOT-23-3 in the gap log: pads 0.6 × 1.3 at a
/// 2.1 mm row span.
fn dbz() -> TabbedForm {
    let mut form = TabbedForm::typical();
    form.lands = TabbedLands {
        size: Some([0.6, 1.3]),
        span: Some(2.1),
        tab_size: None,
    };
    form
}

#[test]
fn a_tabbed_form_s_land_pattern_builds_the_pads() -> Result<()> {
    let mut engine = Engine::default();
    let new = |form: TabbedForm, name: &str| NewFromTemplate {
        row: RowSource::Custom {
            family: FamilyKind::Tabbed,
        },
        values: Some(PackageForm::Tabbed(Box::new(form))),
        skeleton: SkeletonHint::IcBox,
        name: name.into(),
        designator_prefix: None,
        symbol: None,
    };
    let footprint_of = |engine: &mut Engine, request| -> Result<FootprintDefinition> {
        let Outcome::DraftsCreated { ids } =
            library(engine, LibraryCommand::NewFromTemplate(Box::new(request)))?
        else {
            return Err(Error::Invalid("no drafts".into()));
        };
        let id = ids
            .get(3)
            .copied()
            .ok_or(Error::NotFound("footprint".into()))?;
        footprint(engine, id)
    };
    let cited = footprint_of(&mut engine, new(dbz(), "Cited"))?;
    assert_eq!(cited.pads.len(), 3);
    for pad in &cited.pads {
        assert_eq!(pad.shape.extent(), [0.6, 1.3]);
        assert!(close(pad.position[1].abs(), 1.05), "{:?}", pad.position);
    }
    // Along x the grid stays the form's: 0.95 mm either side of the middle.
    assert!(close(cited.pads[0].position[0], -0.95));
    assert!(close(cited.pads[1].position[0], 0.95));
    assert!(close(cited.pads[2].position[0], 0.0));

    // Without it, the estimates: 0.4 + 0.2 wide, 0.4 + 0.6 long; a 2.4 span,
    // foot 0.4, so land centres at 1.2 − 0.2 + 0.1.
    let estimated = footprint_of(&mut engine, new(TabbedForm::typical(), "Estimated"))?;
    let extent = estimated.pads[0].shape.extent();
    assert!(close(extent[0], 0.6) && close(extent[1], 1.0), "{extent:?}");
    assert!(close(estimated.pads[0].position[1], -1.1));
    Ok(())
}

#[test]
fn a_tab_keeps_its_place_and_takes_its_own_land() -> Result<()> {
    use crate::library::package_form::tabbed::Tab;
    use crate::library::samples::form_lands;
    let mut form = TabbedForm {
        leads_per_side: [3, 0],
        pitch: 2.3,
        body_length: 6.5,
        body_width: 3.5,
        body_height: 1.6,
        standoff: 0.05,
        span: 7.0,
        lead_width: 0.7,
        lead_thickness: 0.25,
        foot_length: 0.9,
        tab: Some(Tab {
            width: 3.0,
            length: 0.9,
            flange: None,
        }),
        lead_step: [1, 1],
        lands: TabbedLands::default(),
    };
    let ids: Vec<_> = (1..=4).map(crate::ids::PackageTerminalId).collect();
    let estimate = form_lands(&PackageForm::Tabbed(Box::new(form)), &ids)
        .ok_or(Error::NotFound("lands".into()))?;
    form.lands = TabbedLands {
        size: Some([1.0, 2.0]),
        span: Some(6.0),
        tab_size: Some([3.5, 2.5]),
    };
    let cited = form_lands(&PackageForm::Tabbed(Box::new(form)), &ids)
        .ok_or(Error::NotFound("lands".into()))?;
    assert_eq!(cited[0].shape.extent(), [1.0, 2.0]);
    assert!(close(cited[0].position[1], -3.0));
    assert_eq!(cited[3].shape.extent(), [3.5, 2.5]);
    assert_eq!(
        cited[3].position, estimate[3].position,
        "the tab is not a row"
    );
    Ok(())
}

#[test]
fn bad_land_values_are_the_form_s_range_problems() {
    let mut form = dbz();
    assert!(
        PackageForm::Tabbed(Box::new(form))
            .range_problems()
            .is_empty()
    );
    form.lands.size = Some([0.0, 1.3]);
    assert!(
        !PackageForm::Tabbed(Box::new(form))
            .range_problems()
            .is_empty()
    );
    form = dbz();
    form.lands.span = Some(f64::NAN);
    assert!(
        !PackageForm::Tabbed(Box::new(form))
            .range_problems()
            .is_empty()
    );
}

#[test]
fn a_form_without_lands_serialises_as_before() -> Result<()> {
    let text =
        serde_json::to_string(&TabbedForm::typical()).map_err(|e| Error::Invalid(e.to_string()))?;
    assert!(!text.contains("lands"), "{text}");
    let back: TabbedForm =
        serde_json::from_str(&text).map_err(|e| Error::Invalid(e.to_string()))?;
    assert_eq!(back, TabbedForm::typical());
    let text = serde_json::to_string(&dbz()).map_err(|e| Error::Invalid(e.to_string()))?;
    let back: TabbedForm =
        serde_json::from_str(&text).map_err(|e| Error::Invalid(e.to_string()))?;
    assert_eq!(back, dbz());
    Ok(())
}
