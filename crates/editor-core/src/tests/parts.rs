//! S2: exact manufacturer Parts through public commands and queries.
//! Manufacturers, MPNs and values are invented (`ExampleCo`, `EX-…`).
use super::support::*;
use crate::catalog::{self, CatalogContext, Index, SearchOptions};
use crate::completeness::{BomReport, PurchasingState, ResolveReport};
use crate::fields::{
    FieldSelection, FieldValue, PropertyEdit, PropertyInput, QuantityKind, RatedSpec, SpecMeaning,
    ToleranceBasis, ValueType,
};
use crate::library::{
    Basis, Definition, DistributorOffer, DraftEdit, PartDefinition, Provenance, Reference,
    ReferenceKind,
};
use crate::quantity::{UnitOfMeasure, UnitSet};
use crate::updates::{Change, PartAction, PartResolution, Resolutions};
use crate::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, Engine, Error, FieldRef, Id,
    InstanceCommand, Operation, PartRef, Query, QueryResult, Result, Revision, format,
};

const OHMS: crate::quantity::Dimension = [2, 1, -3, -2, 0, 0, 0];
const VOLTS: crate::quantity::Dimension = [2, 1, -3, -1, 0, 0, 0];
/// An invented SHA-256 digest of an attached datasheet copy.
const ASSET: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";

fn units(dimension: crate::quantity::Dimension, symbol: &str) -> QuantityKind {
    QuantityKind {
        units: UnitSet {
            dimension,
            units: vec![UnitOfMeasure {
                symbol: symbol.into(),
                aliases: vec![],
                scale: 1.0,
                si_prefixes: true,
            }],
        },
        default_unit: symbol.into(),
    }
}

fn quantity(si: f64) -> FieldValue {
    FieldValue::Quantity {
        si: dec(si),
        display_unit: None,
    }
}

struct Fields {
    resistance: FieldRef,
    voltage: FieldRef,
    tolerance: FieldRef,
}

fn publish_fields(engine: &mut Engine) -> Result<Fields> {
    let ohm = publish_kind(engine, "Resistance", units(OHMS, "Ω"))?;
    let volt = publish_kind(engine, "Voltage", units(VOLTS, "V"))?;
    let quantity_field = |kind| crate::fields::FieldDefinition {
        next_local: 1,
        value_type: ValueType::Quantity {
            kind,
            min: Some(0.0),
            max: None,
        },
        default: None,
        required_at: None,
    };
    Ok(Fields {
        resistance: publish_field(engine, "Resistance", quantity_field(ohm))?,
        voltage: publish_field(engine, "Rated voltage", quantity_field(volt))?,
        tolerance: publish_field(
            engine,
            "Tolerance",
            crate::fields::FieldDefinition {
                next_local: 1,
                value_type: ValueType::Tolerance {
                    basis: ToleranceBasis::Relative,
                },
                default: None,
                required_at: None,
            },
        )?,
    })
}

fn selections(f: &Fields) -> Vec<FieldSelection> {
    let mut all: Vec<FieldSelection> = [f.resistance, f.voltage, f.tolerance]
        .into_iter()
        .map(select)
        .collect();
    for s in &mut all[1..] {
        s.show_on_symbol = false;
    }
    all
}

struct Setup {
    engine: Engine,
    fields: Fields,
    fixture: Fixture,
}

/// A generic two-field resistor without values.
fn setup() -> Result<Setup> {
    let mut engine = Engine::default();
    let fields = publish_fields(&mut engine)?;
    let fixture = component_with_fields(
        &mut engine,
        &probe("Generic resistor", passive()),
        selections(&fields),
    )?;
    Ok(Setup {
        engine,
        fields,
        fixture,
    })
}

fn binding(f: &Fixture) -> BindingRef {
    BindingRef::new(f.binding, Revision::FIRST)
}

/// A Part of `component` and its binding rating `specs`.
fn part_body(
    mpn: &str,
    f: &Fixture,
    specs: Vec<(FieldRef, FieldValue, SpecMeaning)>,
) -> PartDefinition {
    PartDefinition {
        next_local: 3,
        manufacturer: "ExampleCo".into(),
        mpn: mpn.into(),
        component: f.r1(),
        binding: binding(f),
        specs: specs
            .into_iter()
            .map(|(field, value, meaning)| RatedSpec {
                field,
                value,
                meaning,
            })
            .collect(),
        references: vec![Reference {
            id: 1,
            kind: ReferenceKind::Datasheet,
            title: format!("{mpn} datasheet"),
            url: Some("https://example.com/exampleco/ex-r.pdf".into()),
            document_revision: Some("B".into()),
            asset: None,
            licence: None,
            provenance: Provenance::default(),
        }],
        offers: vec![DistributorOffer {
            id: 2,
            distributor: "Example Distribution".into(),
            sku: format!("EXD-{mpn}"),
            url: None,
            packaging: Some("reel".into()),
        }],
        provenance: Provenance {
            basis: Basis::FromReference(1),
            ..Provenance::default()
        },
        top_mark: None,
    }
}

fn draft_part(engine: &mut Engine, body: PartDefinition) -> Result<DefinitionId> {
    let id = new_draft(engine, DefinitionKind::Part)?;
    edit(
        engine,
        vec![DraftEdit {
            id,
            meta: meta(&body.mpn),
            definition: Definition::Part(Box::new(body)),
        }],
    )?;
    Ok(id)
}

fn publish_part(engine: &mut Engine, body: PartDefinition) -> Result<PartRef> {
    let id = draft_part(engine, body)?;
    let r = publish(engine, vec![id])?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("nothing published".into()))?;
    Ok(PartRef::new(r.id, r.revision))
}

fn resistor(mpn: &str, s: &Setup, ohms: f64) -> PartDefinition {
    part_body(
        mpn,
        &s.fixture,
        vec![
            (
                s.fields.resistance,
                quantity(ohms),
                SpecMeaning::ExactNominal,
            ),
            (
                s.fields.tolerance,
                FieldValue::Tolerance {
                    minus: dec(0.01),
                    plus: dec(0.01),
                },
                SpecMeaning::MaximumTolerance,
            ),
            (s.fields.voltage, quantity(25.0), SpecMeaning::MinimumRating),
        ],
    )
}

fn publish_resistor(s: &mut Setup, mpn: &str, ohms: f64) -> Result<PartRef> {
    let body = resistor(mpn, s, ohms);
    publish_part(&mut s.engine, body)
}

fn text(field: FieldRef, value: &str) -> PropertyEdit {
    PropertyEdit {
        field: field.id,
        input: PropertyInput::Text(value.into()),
    }
}

fn assign(engine: &mut Engine, instances: Vec<Id>, part: Option<PartRef>) -> Result<()> {
    apply(
        engine,
        Operation::Instance(InstanceCommand::AssignPart { instances, part }),
    )
    .map(|_| ())
}

fn bom(engine: &Engine) -> Result<BomReport> {
    match engine.query(Query::Bom {})? {
        QueryResult::Bom(report) => Ok(report),
        other => Err(Error::Invalid(format!("unexpected report {other:?}"))),
    }
}

fn groups(engine: &Engine) -> Result<ResolveReport> {
    match engine.query(Query::ResolveGroups {})? {
        QueryResult::ResolveGroups(report) => Ok(report),
        other => Err(Error::Invalid(format!("unexpected report {other:?}"))),
    }
}

fn placed(s: &mut Setup, designator: &str, ohms: &str) -> Result<Id> {
    let (id, _) = place(&mut s.engine, &s.fixture, designator, 1, [0.0, 0.0])?;
    set_properties(
        &mut s.engine,
        id,
        vec![
            text(s.fields.resistance, ohms),
            text(s.fields.tolerance, "±1%"),
            text(s.fields.voltage, "16 V"),
        ],
    )?;
    Ok(id)
}

fn purchasing_codes(engine: &Engine, id: Id) -> Result<(PurchasingState, Vec<String>)> {
    let report = inspect(engine, id)?.purchasing;
    Ok((
        report.state,
        report.findings.into_iter().map(|f| f.code).collect(),
    ))
}

#[test]
fn the_cascade_publishes_footprint_binding_component_and_part_in_one_set() -> Result<()> {
    let mut engine = Engine::default();
    let fields = publish_fields(&mut engine)?;
    let fixture = draft(&mut engine, &probe("Cascade", passive()))?;
    rewrite(&mut engine, fixture.component, |d| {
        if let Definition::Component(c) = d {
            c.fields = selections(&fields);
        }
    })?;
    let body = part_body(
        "EX-R0603-10K0-F",
        &fixture,
        vec![(fields.resistance, quantity(10e3), SpecMeaning::ExactNominal)],
    );
    // A Part whose binding the component does not list blocks the whole set.
    let stray = draft_part(
        &mut engine,
        PartDefinition {
            binding: BindingRef::new(fixture.binding, Revision(2)),
            ..body.clone()
        },
    )?;
    let mut set = fixture.all();
    set.push(stray);
    let before = engine.project().library.clone();
    let error = publish(&mut engine, set.clone()).err();
    assert_eq!(
        error.as_ref().map(Error::code),
        Some("conflict"),
        "{error:?}"
    );
    assert_eq!(engine.project().library, before, "nothing published");

    rewrite(&mut engine, stray, |d| {
        *d = Definition::Part(Box::new(body))
    })?;
    let published = publish(&mut engine, set)?;
    let kinds: Vec<DefinitionKind> = published.iter().map(|r| r.kind).collect();
    assert_eq!(kinds.last(), Some(&DefinitionKind::Part), "Part ranks last");
    assert_eq!(published.len(), 6);
    let part = engine
        .project()
        .library
        .part(PartRef::new(stray, Revision::FIRST))
        .ok_or_else(|| Error::NotFound("published part".into()))?;
    assert_eq!(part.definition.component, fixture.r1());
    Ok(())
}

#[test]
fn part_publication_checks_specs_records_and_pins() -> Result<()> {
    let mut s = setup()?;
    let good = resistor("EX-R0603-10K0-F", &s, 10e3);
    let other = publish_fields(&mut s.engine)?;
    let cases: Vec<(&str, PartDefinition)> = vec![
        (
            "part.identity",
            PartDefinition {
                mpn: " ".into(),
                ..good.clone()
            },
        ),
        (
            "part.spec_field",
            PartDefinition {
                specs: vec![RatedSpec {
                    field: other.resistance,
                    value: quantity(10e3),
                    meaning: SpecMeaning::ExactNominal,
                }],
                ..good.clone()
            },
        ),
        (
            "part.spec_duplicate",
            PartDefinition {
                specs: [good.specs.clone(), good.specs[..1].to_vec()].concat(),
                ..good.clone()
            },
        ),
        (
            "part.spec_value",
            PartDefinition {
                specs: vec![RatedSpec {
                    field: s.fields.resistance,
                    value: quantity(-1.0),
                    meaning: SpecMeaning::ExactNominal,
                }],
                ..good.clone()
            },
        ),
        (
            "part.spec_meaning",
            PartDefinition {
                specs: vec![RatedSpec {
                    field: s.fields.tolerance,
                    value: FieldValue::Tolerance {
                        minus: dec(0.01),
                        plus: dec(0.01),
                    },
                    meaning: SpecMeaning::MinimumRating,
                }],
                ..good.clone()
            },
        ),
        (
            "part.link",
            PartDefinition {
                references: vec![Reference {
                    url: Some("ftp://example.com/x".into()),
                    ..good.references[0].clone()
                }],
                ..good.clone()
            },
        ),
        (
            "part.hash",
            PartDefinition {
                references: vec![Reference {
                    asset: Some(ASSET.to_uppercase()),
                    ..good.references[0].clone()
                }],
                ..good.clone()
            },
        ),
        (
            "part.offer_text",
            PartDefinition {
                offers: vec![DistributorOffer {
                    sku: String::new(),
                    ..good.offers[0].clone()
                }],
                ..good.clone()
            },
        ),
        (
            "part.provenance_basis",
            PartDefinition {
                provenance: Provenance {
                    basis: Basis::FromReference(9),
                    ..Provenance::default()
                },
                ..good.clone()
            },
        ),
        (
            "reference.missing",
            PartDefinition {
                component: ComponentRef::new(s.fixture.component, Revision(4)),
                ..good.clone()
            },
        ),
    ];
    for (code, body) in cases {
        let id = draft_part(&mut s.engine, body)?;
        let report = check(&s.engine, crate::completeness::CheckTarget::Draft(id))?;
        let found: Vec<&str> = report.findings.iter().map(|f| f.code.as_str()).collect();
        assert!(found.contains(&code), "{code}: {found:?}");
        let before = s.engine.project().library.clone();
        assert!(publish(&mut s.engine, vec![id]).is_err(), "{code}");
        assert_eq!(s.engine.project().library, before, "{code}");
    }
    publish_part(&mut s.engine, good)?;
    Ok(())
}

#[test]
fn ce01_a_changed_value_after_an_mpn_is_chosen_is_a_mismatch_not_a_substitution() -> Result<()> {
    let mut s = setup()?;
    let ten = publish_resistor(&mut s, "EX-R0603-10K0-F", 10e3)?;
    let twenty_two = publish_resistor(&mut s, "EX-R0603-22K0-F", 22e3)?;
    let r1 = placed(&mut s, "R1", "10k")?;
    assert_eq!(
        purchasing_codes(&s.engine, r1)?,
        (PurchasingState::NoPart, vec![])
    );
    assign(&mut s.engine, vec![r1], Some(ten))?;
    let report = inspect(&s.engine, r1)?.purchasing;
    assert_eq!(
        report.state,
        PurchasingState::Resolved,
        "{:?}",
        report.findings
    );
    assert_eq!(report.mpn.as_deref(), Some("EX-R0603-10K0-F"));
    assert_eq!(report.manufacturer.as_deref(), Some("ExampleCo"));

    // 10 kΩ → 22 kΩ: the edit is accepted, the Part stays, and purchasing
    // names the mismatch.
    set_properties(&mut s.engine, r1, vec![text(s.fields.resistance, "22k")])?;
    let instance = s
        .engine
        .project()
        .instance(r1)
        .ok_or_else(|| Error::NotFound("R1".into()))?;
    assert_eq!(instance.part, Some(ten), "never substituted silently");
    let report = inspect(&s.engine, r1)?.purchasing;
    assert_eq!(report.state, PurchasingState::Unresolved);
    let codes: Vec<&str> = report.findings.iter().map(|f| f.code.as_str()).collect();
    assert_eq!(codes, ["part.spec_mismatch"]);
    assert!(
        report.findings[0].message.contains("Resistance 22 kΩ")
            && report.findings[0].message.contains("10 kΩ"),
        "{}",
        report.findings[0].message
    );
    assert!(
        inspect(&s.engine, r1)?
            .findings
            .iter()
            .all(|f| !f.code.starts_with("part."))
    );
    // The BOM marks the line unresolved.
    let report = bom(&s.engine)?;
    assert_eq!(report.unresolved, 1);
    assert!(report.lines[0].unresolved);
    assert_eq!(report.lines[0].mpn.as_deref(), Some("EX-R0603-10K0-F"));

    // Assigning the 10 kΩ Part again is refused, naming the field; the
    // document is unchanged. The 22 kΩ Part resolves it.
    let kept = Snapshot::of(&s.engine);
    let error = assign(&mut s.engine, vec![r1], Some(ten)).err();
    assert_eq!(error.as_ref().map(Error::code), Some("conflict"));
    assert!(
        error.is_some_and(|e| e.to_string().contains("Resistance")),
        "names the field"
    );
    kept.assert_kept(&s.engine, "refused assignment");
    assign(&mut s.engine, vec![r1], Some(twenty_two))?;
    assert_eq!(
        purchasing_codes(&s.engine, r1)?,
        (PurchasingState::Resolved, vec![])
    );
    assert_eq!(bom(&s.engine)?.unresolved, 0);
    Ok(())
}

#[test]
fn a_requirement_the_part_does_not_rate_is_never_an_implicit_pass() -> Result<()> {
    let mut s = setup()?;
    // The Part rates only the voltage; R1 also requires 10 kΩ and ±1 %.
    let voltage_only = publish_part(
        &mut s.engine,
        part_body(
            "EX-R0603-ANY",
            &s.fixture,
            vec![(s.fields.voltage, quantity(25.0), SpecMeaning::MinimumRating)],
        ),
    )?;
    let r1 = placed(&mut s, "R1", "10k")?;
    let kept = Snapshot::of(&s.engine);
    let error = assign(&mut s.engine, vec![r1], Some(voltage_only)).err();
    assert!(
        error.as_ref().is_some_and(
            |e| e.code() == "conflict" && e.to_string().contains("no rated Resistance")
        ),
        "{error:?}"
    );
    kept.assert_kept(&s.engine, "unrated requirement");
    // An instance that requires only the voltage, which the Part rates,
    // resolves.
    let (c1, _) = place(&mut s.engine, &s.fixture, "C1", 1, [10.0, 0.0])?;
    set_properties(&mut s.engine, c1, vec![text(s.fields.voltage, "16V")])?;
    assign(&mut s.engine, vec![c1], Some(voltage_only))?;
    assert_eq!(
        purchasing_codes(&s.engine, c1)?,
        (PurchasingState::Resolved, vec![])
    );
    // A later resistance requirement the Part does not rate is unresolved,
    // not silently resolved (CE-01).
    set_properties(&mut s.engine, c1, vec![text(s.fields.resistance, "22k")])?;
    assert_eq!(
        purchasing_codes(&s.engine, c1)?,
        (
            PurchasingState::Unresolved,
            vec!["part.spec_unrated".into()]
        )
    );
    assert_eq!(
        s.engine.project().instance(c1).and_then(|i| i.part),
        Some(voltage_only),
        "never substituted"
    );
    Ok(())
}

#[test]
fn ce02_a_25_v_part_meets_a_16_v_requirement_and_the_reverse_is_refused() -> Result<()> {
    let mut s = setup()?;
    let rated = |volts: f64| {
        part_body(
            &format!("EX-C-{volts}V"),
            &s.fixture,
            vec![(
                s.fields.voltage,
                quantity(volts),
                SpecMeaning::MinimumRating,
            )],
        )
    };
    let v25 = publish_part(&mut s.engine, rated(25.0))?;
    let v16 = publish_part(&mut s.engine, rated(16.0))?;
    let (c1, _) = place(&mut s.engine, &s.fixture, "C1", 1, [0.0, 0.0])?;
    let (c2, _) = place(&mut s.engine, &s.fixture, "C2", 1, [10.0, 0.0])?;
    set_properties(&mut s.engine, c1, vec![text(s.fields.voltage, "16V")])?;
    set_properties(&mut s.engine, c2, vec![text(s.fields.voltage, "25V")])?;
    assign(&mut s.engine, vec![c1], Some(v25))?;
    assert_eq!(
        purchasing_codes(&s.engine, c1)?,
        (PurchasingState::Resolved, vec![])
    );
    let kept = Snapshot::of(&s.engine);
    let error = assign(&mut s.engine, vec![c2], Some(v16)).err();
    assert!(
        error
            .as_ref()
            .is_some_and(|e| e.code() == "conflict" && e.to_string().contains("Rated voltage 25 V")),
        "{error:?}"
    );
    kept.assert_kept(&s.engine, "under-rated part");
    // A higher requirement on an assigned instance is a finding.
    set_properties(&mut s.engine, c1, vec![text(s.fields.voltage, "50V")])?;
    assert_eq!(
        purchasing_codes(&s.engine, c1)?,
        (
            PurchasingState::Unresolved,
            vec!["part.spec_mismatch".into()]
        )
    );
    Ok(())
}

#[test]
fn ce07_a_reference_without_a_url_keeps_its_identity_and_attached_copy() -> Result<()> {
    let mut s = setup()?;
    let mut body = resistor("EX-R0603-10K0-F", &s, 10e3);
    body.references[0].asset = Some(ASSET.into());
    let r1 = publish_part(&mut s.engine, body)?;
    // The link goes away in a new revision; the record and its copy stay.
    // A published draft stays open on its new revision.
    rewrite(&mut s.engine, r1.id, |d| {
        if let Definition::Part(p) = d {
            p.references[0].url = None;
        }
    })?;
    let r2 = publish(&mut s.engine, vec![r1.id])?;
    assert_eq!(r2[0].revision, Revision(2));
    let reopened = Engine::from_file(format::decode(&format::encode(&s.engine.to_file())?)?)?;
    assert_eq!(reopened.document(), s.engine.document());
    let library = &reopened.project().library;
    let reference = |revision| {
        library
            .part(PartRef::new(r1.id, revision))
            .map(|p| p.definition.references[0].clone())
            .ok_or_else(|| Error::NotFound("reference".into()))
    };
    let (before, after) = (reference(Revision::FIRST)?, reference(Revision(2))?);
    assert_eq!(after.url, None);
    assert_eq!((after.id, after.kind), (before.id, before.kind));
    assert_eq!(after.title, before.title);
    assert_eq!(after.asset.as_deref(), Some(ASSET));
    Ok(())
}

#[test]
fn assign_part_is_atomic_over_a_group_and_one_undo_step() -> Result<()> {
    let mut s = setup()?;
    let ten = publish_resistor(&mut s, "EX-R0603-10K0-F", 10e3)?;
    let r1 = placed(&mut s, "R1", "10k")?;
    let r2 = placed(&mut s, "R2", "10 kΩ")?;
    let r3 = placed(&mut s, "R3", "22k")?;
    let kept = Snapshot::of(&s.engine);
    let error = assign(&mut s.engine, vec![r1, r2, r3], Some(ten)).err();
    assert!(
        error.as_ref().is_some_and(|e| e.to_string().contains("R3")),
        "{error:?}"
    );
    kept.assert_kept(&s.engine, "one disagreeing instance");
    for bad in [vec![], vec![r1, r1]] {
        assert!(assign(&mut s.engine, bad, Some(ten)).is_err());
        kept.assert_kept(&s.engine, "malformed group");
    }
    let missing = PartRef::new(ten.id, Revision(9));
    assert_eq!(
        assign(&mut s.engine, vec![r1], Some(missing))
            .err()
            .map(|e| e.code()),
        Some("not_found")
    );

    let revision = s.engine.revision();
    assign(&mut s.engine, vec![r1, r2], Some(ten))?;
    assert_eq!(s.engine.revision(), revision + 1, "one command");
    let pinned = |e: &Engine, id| e.project().instance(id).and_then(|i| i.part);
    assert_eq!(
        (pinned(&s.engine, r1), pinned(&s.engine, r2)),
        (Some(ten), Some(ten))
    );
    s.engine.undo()?;
    assert_eq!((pinned(&s.engine, r1), pinned(&s.engine, r2)), (None, None));
    s.engine.redo()?;
    assign(&mut s.engine, vec![r1, r2], None)?;
    assert_eq!((pinned(&s.engine, r1), pinned(&s.engine, r2)), (None, None));
    Ok(())
}

#[test]
fn bom_and_resolve_groups_follow_requirements_footprint_and_parts() -> Result<()> {
    let mut s = setup()?;
    let ten = publish_resistor(&mut s, "EX-R0603-10K0-F", 10e3)?;
    publish_resistor(&mut s, "EX-R0603-22K0-F", 22e3)?;
    let r1 = placed(&mut s, "R1", "10k")?;
    let r10 = placed(&mut s, "R10", "10000")?;
    let r3 = placed(&mut s, "R3", "22k")?;
    let r2 = placed(&mut s, "R2", "10 kΩ")?;
    let r4 = placed(&mut s, "R4", "10k")?;
    assign(&mut s.engine, vec![r4], Some(ten))?;

    let report = groups(&s.engine)?;
    let shape: Vec<Vec<&str>> = report
        .groups
        .iter()
        .map(|g| g.designators.iter().map(String::as_str).collect())
        .collect();
    assert_eq!(
        shape,
        [vec!["R1", "R2", "R10"], vec!["R3"]],
        "natural order"
    );
    assert_eq!(report.groups[0].instances, [r1, r2, r10]);
    let group = &report.groups[0];
    assert_eq!(group.requirements.len(), 3);
    assert_eq!(
        group.query,
        format!(
            "kind:part implements:{}/1 Resistance:10kΩ Rated:16V Tolerance:±1%",
            s.fixture.component.0
        ),
        "keys resolve to one field each; values are single tokens"
    );
    // The query finds exactly the Part that satisfies the group.
    let index = Index::of_project(s.engine.project(), s.engine.drafts())?;
    let hits = index.search(
        &index.parse(&group.query).map_err(Error::from)?,
        &CatalogContext::default(),
        SearchOptions::default(),
    );
    let found: Vec<_> = hits.results.iter().filter_map(|h| h.entry.part()).collect();
    assert_eq!(found, [ten]);
    // One command resolves the whole group.
    assign(&mut s.engine, group.instances.clone(), Some(ten))?;
    let report = groups(&s.engine)?;
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].instances, [r3]);

    let report = bom(&s.engine)?;
    let lines: Vec<(Vec<&str>, u32, bool)> = report
        .lines
        .iter()
        .map(|l| {
            (
                l.designators.iter().map(String::as_str).collect(),
                l.quantity,
                l.unresolved,
            )
        })
        .collect();
    assert_eq!(
        lines,
        [
            (vec!["R1", "R2", "R4", "R10"], 4, false),
            (vec!["R3"], 1, true)
        ]
    );
    assert_eq!(report.unresolved, 1);
    assert_eq!(report.lines[0].part, Some(ten));
    assert_eq!(report.lines[1].state, PurchasingState::NoPart);
    let values: Vec<&str> = report.lines[1]
        .values
        .iter()
        .map(|v| v.text.as_str())
        .collect();
    assert_eq!(values, ["22 kΩ", "16 V", "±1%"]);
    Ok(())
}

/// E2E UX run finding 16: the BOM's Value is the value the schematic shows
/// (every typed field with a value, ratings included), else the Part's MPN,
/// else empty, never a component name (run 3, finding 25: an IC without a
/// Part shows nothing rather than its description).
#[test]
fn the_bom_value_is_the_shown_value_never_a_passive_component_name() -> Result<()> {
    let mut s = setup()?;
    let ten = publish_resistor(&mut s, "EX-R0603-10K0-F", 10e3)?;
    placed(&mut s, "R1", "10k")?;
    let r2 = place(&mut s.engine, &s.fixture, "R2", 1, [0.0, 0.0])?.0;
    assign(&mut s.engine, vec![r2], Some(ten))?;
    place(&mut s.engine, &s.fixture, "R3", 1, [0.0, 0.0])?;
    let ic = component_with_fields(
        &mut s.engine,
        &Spec {
            name: "Regulator EX-LDO",
            terminals: vec![
                (1, "IN", passive()),
                (2, "GND", passive()),
                (3, "OUT", passive()),
            ],
            units: vec![(
                1,
                "A",
                vec![(1, [0.0, 0.0]), (2, [0.0, -2.54]), (3, [5.08, 0.0])],
            )],
            package: vec![(1, "1"), (2, "2"), (3, "3")],
            mapping: vec![(1, 1), (2, 2), (3, 3)],
            physical: true,
        },
        vec![],
    )?;
    place(&mut s.engine, &ic, "U1", 1, [10.0, 0.0])?;

    let csv = crate::fabrication::assembly::bom_parts_csv(s.engine.project())?;
    let values: Vec<(&str, &str)> = csv
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut f = l.split(',');
            Some((f.next()?, f.next()?))
        })
        .collect();
    assert_eq!(
        values,
        [
            ("R1", "10 kΩ 16 V ±1%"),
            ("R2", "EX-R0603-10K0-F"),
            ("R3", ""),
            ("U1", "")
        ]
    );
    let grouped = crate::fabrication::assembly::bom_csv(s.engine.project())?;
    assert!(
        grouped
            .lines()
            .any(|l| l.starts_with("1,1,R1,10 kΩ 16 V ±1%,")),
        "{grouped}"
    );
    assert!(
        grouped.lines().any(|l| l.starts_with("3,1,R3,,")),
        "{grouped}"
    );
    Ok(())
}

#[test]
fn a_component_update_keeps_or_clears_the_part_and_never_repoints_it() -> Result<()> {
    let mut s = setup()?;
    let ten = publish_resistor(&mut s, "EX-R0603-10K0-F", 10e3)?;
    let r1 = placed(&mut s, "R1", "10k")?;
    let r2 = placed(&mut s, "R2", "10k")?;
    assign(&mut s.engine, vec![r1, r2], Some(ten))?;
    rewrite(&mut s.engine, s.fixture.component, |d| {
        if let Definition::Component(c) = d {
            c.designator_prefix = "R".into();
        }
    })?;
    publish(&mut s.engine, vec![s.fixture.component])?;
    let target = ComponentRef::new(s.fixture.component, Revision(2));
    let update = |resolutions| Change::ComponentRevision {
        instances: vec![r1, r2],
        target,
        resolutions,
    };
    let plan = s.engine.preview_update(update(Resolutions {
        parts: vec![PartResolution {
            instance: r2,
            action: PartAction::Clear,
        }],
        ..Resolutions::default()
    }));
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let stale: Vec<&String> = plan
        .warnings
        .iter()
        .filter(|w| w.contains("part.stale_component"))
        .collect();
    assert_eq!(stale.len(), 1, "{:?}", plan.warnings);
    assert!(stale[0].contains("R1"));
    assert!(
        plan.changes
            .iter()
            .any(|c| c == "R2: clear Part EX-R0603-10K0-F.")
    );
    s.engine.apply_update(
        s.engine.revision(),
        update(Resolutions {
            parts: vec![PartResolution {
                instance: r2,
                action: PartAction::Clear,
            }],
            ..Resolutions::default()
        }),
        false,
    )?;
    let project = s.engine.project();
    let r1_after = project
        .instance(r1)
        .ok_or_else(|| Error::NotFound("R1".into()))?;
    assert_eq!((r1_after.component, r1_after.part), (target, Some(ten)));
    assert_eq!(project.instance(r2).and_then(|i| i.part), None);
    // r2 lists the same binding revision, so only the component is stale.
    assert_eq!(
        purchasing_codes(&s.engine, r1)?,
        (
            PurchasingState::Unresolved,
            vec!["part.stale_component".into()]
        )
    );
    // Assigning the stale Part to the updated instance is refused.
    assert!(assign(&mut s.engine, vec![r2], Some(ten)).is_err());
    // A resolution must name an instance of the update.
    let plan = s.engine.preview_update(Change::ComponentRevision {
        instances: vec![r1],
        target,
        resolutions: Resolutions {
            parts: vec![PartResolution {
                instance: r2,
                action: PartAction::Keep,
            }],
            ..Resolutions::default()
        },
    });
    assert!(!plan.blockers.is_empty());
    Ok(())
}

#[test]
fn a_part_for_a_newer_revision_is_assigned_with_the_update_in_one_step() -> Result<()> {
    let mut s = setup()?;
    let r1 = placed(&mut s, "R1", "10k")?;
    rewrite(&mut s.engine, s.fixture.component, |d| {
        if let Definition::Component(c) = d {
            c.designator_prefix = "R".into();
        }
    })?;
    publish(&mut s.engine, vec![s.fixture.component])?;
    let target = ComponentRef::new(s.fixture.component, Revision(2));
    let mut body = resistor("EX-R0603-10K0-F", &s, 10e3);
    body.component = target;
    let part = publish_part(&mut s.engine, body)?;
    // R1 is still on r1: assigning names the newer revision, by name.
    let kept = Snapshot::of(&s.engine);
    let error = assign(&mut s.engine, vec![r1], Some(part)).err();
    let message = error.as_ref().map(ToString::to_string).unwrap_or_default();
    assert!(
        message.contains("Generic resistor r2") && message.contains("update R1"),
        "{message}"
    );
    assert!(
        !message.contains(&format!("{}", s.fixture.component)),
        "{message}"
    );
    kept.assert_kept(&s.engine, "newer Part");
    // Update and assign in one previewed transaction.
    let change = Change::ComponentRevision {
        instances: vec![r1],
        target,
        resolutions: Resolutions {
            parts: vec![PartResolution {
                instance: r1,
                action: PartAction::Assign { part },
            }],
            ..Resolutions::default()
        },
    };
    let plan = s.engine.preview_update(change.clone());
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        plan.changes
            .iter()
            .any(|c| c == "R1: assign Part EX-R0603-10K0-F."),
        "{:?}",
        plan.changes
    );
    let before = s.engine.revision();
    s.engine.apply_update(s.engine.revision(), change, false)?;
    let after = s
        .engine
        .project()
        .instance(r1)
        .ok_or_else(|| Error::NotFound("R1".into()))?;
    assert_eq!((after.component, after.part), (target, Some(part)));
    assert_eq!(
        purchasing_codes(&s.engine, r1)?,
        (PurchasingState::Resolved, vec![])
    );
    assert_eq!(s.engine.revision(), before + 1, "one step");
    // A Part that does not agree at the target refuses the whole update.
    let mut other = resistor("EX-R0603-22K0-F", &s, 22e3);
    other.component = target;
    let other = publish_part(&mut s.engine, other)?;
    let r2 = placed(&mut s, "R2", "10k")?;
    let kept = Snapshot::of(&s.engine);
    let plan = s.engine.preview_update(Change::ComponentRevision {
        instances: vec![r2],
        target,
        resolutions: Resolutions {
            parts: vec![PartResolution {
                instance: r2,
                action: PartAction::Assign { part: other },
            }],
            ..Resolutions::default()
        },
    });
    assert!(
        plan.blockers.iter().any(|b| b.contains("Resistance")),
        "{:?}",
        plan.blockers
    );
    kept.assert_kept(&s.engine, "refused update");
    Ok(())
}

#[test]
fn the_catalog_indexes_parts_with_mpn_badges_and_filters() -> Result<()> {
    let project = catalog::synthetic::project_with_parts(6)?;
    crate::validate::document(&project, &Default::default())?;
    let index = Index::of_project(&project, &Default::default())?;
    let search = |text: &str| -> Result<Vec<String>> {
        let report = index.search(
            &index.parse(text).map_err(Error::from)?,
            &CatalogContext::default(),
            SearchOptions::default(),
        );
        Ok(report.results.into_iter().map(|h| h.entry.name).collect())
    };
    assert_eq!(
        search("kind:part")?,
        [
            "EX-C0402-100N-16V",
            "EX-C0402-100N-25V",
            "EX-R0603-10K0-F",
            "EX-R0603-22K0-F"
        ]
    );
    assert_eq!(search("mpn:10k0")?, ["EX-R0603-10K0-F"]);
    // `mpn:` matches the MPN only, never a component named like one.
    assert!(search("kind:component EX-R0603-10K0-F")?.contains(&"EX-R0603-10K0-F".to_owned()));
    assert_eq!(search("kind:component mpn:10k0")?, Vec::<String>::new());
    // A rated minimum meets a lower value; an exact nominal must equal it.
    assert_eq!(
        search("kind:part max:16V")?,
        ["EX-C0402-100N-16V", "EX-C0402-100N-25V"]
    );
    assert_eq!(search("kind:part max:20V")?, ["EX-C0402-100N-25V"]);
    assert_eq!(search("kind:part max>=30V")?, Vec::<String>::new());
    assert_eq!(search("kind:part tol:±5%")?.len(), 2, "±1 % meets ±5 %");
    assert_eq!(search("kind:part 22k")?, ["EX-R0603-22K0-F"]);
    // An exact MPN ranks first.
    assert_eq!(
        search("EX-R0603-22K0-F")?.first().map(String::as_str),
        Some("EX-R0603-22K0-F")
    );
    let entries = index.entries();
    // A synthetic component is also named EX-R0603-10K0-F; kinds keep them
    // apart.
    let entry = |name: &str, kind: DefinitionKind| {
        entries
            .iter()
            .find(|e| e.name == name && e.kind == kind)
            .ok_or_else(|| Error::NotFound(name.into()))
    };
    let generic = entry("EX-R0603 generic resistor", DefinitionKind::Component)?;
    assert!(generic.badges.mpn && generic.component().is_some() && generic.part().is_none());
    assert!(
        !entry("EX-HDR-1X2-001", DefinitionKind::Component)?
            .badges
            .mpn
    );
    let part = entry("EX-R0603-10K0-F", DefinitionKind::Part)?;
    assert_eq!(part.kind, DefinitionKind::Part);
    assert!(part.component().is_none() && part.part().is_some());
    assert_eq!(part.implements, generic.component());
    assert_eq!(part.manufacturer.as_deref(), Some("ExampleCo"));
    assert!(part.badges.mpn && part.badges.symbol && part.badges.footprint);
    assert_eq!(part.packages, ["0603"]);
    let texts: Vec<&str> = part.fields.iter().map(|f| f.text.as_str()).collect();
    assert_eq!(texts, ["10 kΩ", "±1%"]);
    assert!(index.parse("kind:fixture").is_err());
    Ok(())
}

#[test]
fn a_dangling_part_pin_is_rejected_when_a_document_is_read() -> Result<()> {
    let mut s = setup()?;
    let ten = publish_resistor(&mut s, "EX-R0603-10K0-F", 10e3)?;
    let r1 = placed(&mut s, "R1", "10k")?;
    assign(&mut s.engine, vec![r1], Some(ten))?;
    let mut file = s.engine.to_file();
    let bytes = format::encode(&file)?;
    let text = String::from_utf8(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
    assert!(text.contains("\"part\""), "the pin persists");
    file.project.library.parts.clear();
    assert!(Engine::from_file(file).is_err());
    Ok(())
}
