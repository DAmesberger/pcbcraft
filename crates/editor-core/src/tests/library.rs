//! Publication, drafts and definition checks through public commands.
use super::support::*;
use crate::completeness::CheckTarget;
use crate::library::{Definition, DefinitionMeta, DraftEdit};
use crate::{
    BindingRef, DefinitionId, DefinitionKind, DefinitionRef, Engine, Error, FootprintRef,
    LibraryCommand, ObjectRef, Operation, PackageRef, Result, Revision, Stage, SymbolRef,
    TerminalId, format,
};
use serde_json::json;

fn two(name: &str) -> Spec<'_> {
    Spec {
        name,
        terminals: vec![(1, "A", passive()), (2, "B", passive())],
        units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
        package: vec![(1, "1"), (2, "2")],
        mapping: vec![(1, 1), (2, 2)],
        physical: true,
    }
}

/// Publishing `ids` must fail with `conflict` and change nothing.
fn refused(engine: &mut Engine, ids: Vec<DefinitionId>, expect: &str) -> Result<()> {
    let before = engine.document().clone();
    let revision = engine.revision();
    let can_undo = engine.can_undo();
    let error = match publish(engine, ids) {
        Ok(published) => {
            return Err(Error::Invalid(format!("{expect}: published {published:?}")));
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), "conflict", "{expect}: {error}");
    assert!(
        error.to_string().contains("Not published: "),
        "{expect}: {error}"
    );
    assert_eq!(engine.document(), &before, "{expect}");
    assert_eq!(engine.revision(), revision, "{expect}");
    assert_eq!(engine.can_undo(), can_undo, "{expect}");
    Ok(())
}

#[test]
fn five_definitions_publish_atomically_and_become_drafts_of_their_revisions() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    let revision = engine.revision();
    let published = publish(&mut engine, f.all())?;
    assert_eq!(engine.revision(), revision + 1, "one undo step");
    let kinds: Vec<_> = published.iter().map(|r| (r.kind, r.revision)).collect();
    assert_eq!(
        kinds,
        vec![
            (DefinitionKind::Package, Revision(1)),
            (DefinitionKind::Footprint, Revision(1)),
            (DefinitionKind::Binding, Revision(1)),
            (DefinitionKind::Symbol, Revision(1)),
            (DefinitionKind::Component, Revision(1)),
        ],
        "dependency order"
    );
    let library = &engine.project().library;
    assert_eq!(
        (
            library.components.len(),
            library.symbols.len(),
            library.packages.len(),
            library.footprints.len(),
            library.bindings.len()
        ),
        (1, 1, 1, 1, 1)
    );
    for id in f.all() {
        assert_eq!(
            engine.drafts().get(id).and_then(|d| d.base),
            Some(Revision(1))
        );
    }
    engine.undo()?;
    assert!(engine.project().library.components.is_empty());
    Ok(())
}

#[test]
fn any_single_blocking_finding_publishes_nothing() -> Result<()> {
    type Breaker = fn(&mut Engine, &Fixture, &Fixture) -> Result<()>;
    let cases: Vec<(&str, Breaker)> = vec![
        ("missing mapping", |e, f, _| {
            with(e, f.binding, binding_body, |b| {
                b.mappings.pop();
            })
        }),
        ("pin for an unknown terminal", |e, f, _| {
            with(e, f.symbol, symbol_body, |s| {
                s.units[0].pins[1].terminal = TerminalId(99)
            })
        }),
        ("binding of another lineage", |e, f, other| {
            with(e, f.component, component_body, |c| {
                c.implementations = vec![BindingRef::new(other.binding, Revision(1))]
            })
        }),
        ("footprint of another package revision", |e, f, other| {
            with(e, f.binding, binding_body, |b| {
                b.footprints = vec![FootprintRef::new(other.footprint, Revision(1))]
            })
        }),
        ("dangling reference", |e, f, _| {
            with(e, f.component, component_body, |c| {
                c.representations = vec![SymbolRef::new(DefinitionId(999), Revision(1))]
            })
        }),
        ("local ID at or above next_local", |e, f, _| {
            with(e, f.component, component_body, |c| c.next_local = 2)
        }),
        ("lowercase designator prefix", |e, f, _| {
            with(e, f.component, component_body, |c| {
                c.designator_prefix = "r".into()
            })
        }),
        ("empty designator prefix", |e, f, _| {
            with(e, f.component, component_body, |c| {
                c.designator_prefix = String::new()
            })
        }),
    ];
    for (name, breaker) in cases {
        let mut engine = Engine::default();
        let other = component(&mut engine, &two("Other"))?;
        let f = draft(&mut engine, &two("Pair"))?;
        breaker(&mut engine, &f, &other)?;
        refused(&mut engine, f.all(), name)?;
    }
    Ok(())
}

/// The blocking finding codes `component.check` reports for a draft.
fn blocking_codes(engine: &Engine, id: DefinitionId) -> Result<Vec<String>> {
    Ok(check(engine, CheckTarget::Draft(id))?
        .findings
        .into_iter()
        .filter(|f| f.blocks_stage(Stage::Publish))
        .map(|f| f.code)
        .collect())
}

#[test]
fn two_existing_revisions_of_one_lineage_are_refused() -> Result<()> {
    type Breaker = fn(&mut Engine, &Fixture) -> Result<()>;
    // Each case lists r1, which is published, beside the pending r2, so both
    // references resolve and only the lineage rule can refuse them.
    // (finding code, whether the binding draft carries it, breaker)
    let cases: Vec<(&str, bool, Breaker)> = vec![
        ("component.duplicate_symbol_lineage", false, |e, f| {
            with(e, f.component, component_body, |c| {
                c.representations
                    .push(SymbolRef::new(f.symbol, Revision(1)))
            })
        }),
        ("component.duplicate_binding_lineage", false, |e, f| {
            with(e, f.component, component_body, |c| {
                c.implementations
                    .push(BindingRef::new(f.binding, Revision(1)))
            })
        }),
        ("binding.duplicate_footprint_lineage", true, |e, f| {
            with(e, f.binding, binding_body, |b| {
                b.footprints
                    .push(FootprintRef::new(f.footprint, Revision(1)))
            })
        }),
    ];
    for (code, on_binding, breaker) in cases {
        let mut engine = Engine::default();
        let f = component(&mut engine, &two("Pair"))?;
        retarget(&mut engine, &f)?;
        breaker(&mut engine, &f)?;
        let checked = if on_binding { f.binding } else { f.component };
        let codes = blocking_codes(&engine, checked)?;
        assert!(codes.iter().any(|c| c == code), "{code}: {codes:?}");
        assert!(
            !codes.iter().any(|c| c == "reference.missing"),
            "{code}: both revisions exist: {codes:?}"
        );
        refused(&mut engine, f.all(), code)?;
    }
    Ok(())
}

#[test]
fn a_symbol_or_binding_needs_an_existing_owner_component() -> Result<()> {
    type Breaker = fn(&mut Engine, &Fixture) -> Result<()>;
    let cases: Vec<(&str, bool, Breaker)> = vec![
        ("unset symbol owner", true, |e, f| {
            with(e, f.symbol, symbol_body, |s| s.component = DefinitionId(0))
        }),
        ("unknown symbol owner", true, |e, f| {
            with(e, f.symbol, symbol_body, |s| {
                s.component = DefinitionId(999)
            })
        }),
        ("unset binding owner", false, |e, f| {
            with(e, f.binding, binding_body, |b| {
                b.component = DefinitionId(0)
            })
        }),
        ("unknown binding owner", false, |e, f| {
            with(e, f.binding, binding_body, |b| {
                b.component = DefinitionId(999)
            })
        }),
    ];
    for (name, symbol, breaker) in cases {
        let mut engine = Engine::default();
        let f = draft(&mut engine, &two("Pair"))?;
        breaker(&mut engine, &f)?;
        let checked = if symbol { f.symbol } else { f.binding };
        let codes = blocking_codes(&engine, checked)?;
        assert!(
            codes.iter().any(|c| c == "reference.owner_mismatch"),
            "{name}: {codes:?}"
        );
        // Without the component in the publish set, nothing owns them.
        let alone = if symbol {
            vec![f.symbol]
        } else {
            vec![f.package, f.footprint, f.binding]
        };
        refused(&mut engine, alone, name)?;
        refused(&mut engine, f.all(), name)?;
    }
    // The owner published in the same set resolves.
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    publish(&mut engine, f.all())?;
    Ok(())
}

#[test]
fn checking_a_stale_draft_reports_the_draft_not_the_stored_revision() -> Result<()> {
    let mut engine = Engine::default();
    let f = component(&mut engine, &two("Pair"))?;
    cascade(&mut engine, &f, |_| Ok(()))?;
    let stored = check(
        &engine,
        CheckTarget::Revision(DefinitionRef {
            kind: DefinitionKind::Package,
            id: f.package,
            revision: Revision(2),
        }),
    )?;
    assert!(stored.publishable);
    apply(
        &mut engine,
        Operation::Library(LibraryCommand::DiscardDraft { id: f.package }),
    )?;
    draft_from(
        &mut engine,
        DefinitionRef {
            kind: DefinitionKind::Package,
            id: f.package,
            revision: Revision(1),
        },
    )?;
    with(&mut engine, f.package, package_body, |p| {
        p.envelope_nm = [nm(9.0), nm(9.0), nm(9.0)];
    })?;
    let report = check(&engine, CheckTarget::Draft(f.package))?;
    assert_eq!(
        report.revision.revision,
        Revision(2),
        "its pending revision"
    );
    assert!(!report.publishable, "a stale draft cannot publish");
    assert_ne!(report.findings, stored.findings);
    assert_eq!(
        report
            .findings
            .iter()
            .map(|f| (f.code.as_str(), f.blocks))
            .collect::<Vec<_>>(),
        vec![("definition.stale_base", Some(Stage::Publish))]
    );
    let all = match engine.query(crate::Query::CheckDrafts {})? {
        crate::QueryResult::Drafts(reports) => reports,
        other => return Err(Error::Invalid(format!("{other:?}"))),
    };
    assert_eq!(all.len(), engine.drafts().items.len());
    assert!(
        all.contains(&report),
        "the batch agrees with the single check"
    );
    Ok(())
}

#[test]
fn a_retired_local_id_is_never_reused() -> Result<()> {
    let mut engine = Engine::default();
    let f = component(
        &mut engine,
        &Spec {
            terminals: vec![
                (1, "A", passive()),
                (2, "B", passive()),
                (3, "C", passive()),
            ],
            units: vec![(
                1,
                "A",
                vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0]), (3, [0.0, 5.0])],
            )],
            package: vec![(1, "1"), (2, "2"), (3, "3")],
            mapping: vec![(1, 1), (2, 2), (3, 3)],
            ..two("Triple")
        },
    )?;
    // r2 retires terminal 3; package terminal 3 then carries terminal 2.
    cascade(&mut engine, &f, |e| {
        with(e, f.component, component_body, |c| {
            c.terminals.retain(|t| t.id != TerminalId(3))
        })?;
        with(e, f.binding, binding_body, |b| {
            b.mappings[2].terminal = TerminalId(2)
        })?;
        with(e, f.symbol, symbol_body, |s| {
            s.units[0].pins.retain(|a| a.terminal != TerminalId(3))
        })
    })?;
    // r3 re-adds 3 with another meaning, with a freshly allocated function.
    with(&mut engine, f.component, component_body, |c| {
        c.next_local = 8;
        c.terminals.push(crate::library::Terminal {
            id: TerminalId(3),
            name: "Different".into(),
            class: crate::library::TerminalClass::Electrical(
                crate::functions::TerminalFunctions::fixed(
                    crate::FunctionId(7),
                    crate::functions::Direction::Input,
                ),
            ),
        })
    })?;
    with(&mut engine, f.symbol, symbol_body, |s| {
        s.units[0].pins.push(crate::library::Pin::new(
            TerminalId(3),
            pt([0.0, -5.0]),
            crate::library::PinDirection::Up,
            nm(2.5),
        ))
    })?;
    with(&mut engine, f.binding, binding_body, |b| {
        b.mappings[2].terminal = TerminalId(3)
    })?;
    retarget(&mut engine, &f)?;
    refused(&mut engine, f.all(), "reused retired ID")?;
    let report = check(&engine, CheckTarget::Draft(f.component))?;
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == "definition.local_id_reused" && f.blocks == Some(Stage::Publish))
    );
    Ok(())
}

#[test]
fn a_stale_base_conflicts() -> Result<()> {
    let mut engine = Engine::default();
    let f = component(&mut engine, &two("Pair"))?;
    cascade(&mut engine, &f, |_| Ok(()))?;
    apply(
        &mut engine,
        Operation::Library(LibraryCommand::DiscardDraft { id: f.package }),
    )?;
    draft_from(
        &mut engine,
        DefinitionRef {
            kind: DefinitionKind::Package,
            id: f.package,
            revision: Revision(1),
        },
    )?;
    let before = engine.document().clone();
    let revision = engine.revision();
    let error = publish(&mut engine, vec![f.package]).err();
    assert_eq!(error.as_ref().map(|e| e.code()), Some("conflict"));
    assert!(error.is_some_and(|e| e.to_string().contains("newer revision")));
    assert_eq!(engine.document(), &before);
    assert_eq!(engine.revision(), revision);
    Ok(())
}

#[test]
fn decode_rejects_publish_blocking_revisions_and_reopens_advisory_ones() -> Result<()> {
    let mut engine = Engine::default();
    let advisory = component(
        &mut engine,
        &Spec {
            physical: false,
            ..two("Schematic only")
        },
    )?;
    let report = check(&engine, CheckTarget::Revision(advisory.r1().erased()))?;
    assert!(report.publishable);
    assert_eq!(
        codes(&report.findings),
        vec![(
            "component.no_implementation".to_string(),
            vec![ObjectRef::Definition {
                id: advisory.component
            }]
        )]
    );
    assert_eq!(report.findings[0].blocks, None);
    component(&mut engine, &two("Pair"))?;
    let bytes = format::encode(&engine.to_file())?;
    let reopened = Engine::from_file(format::decode(&bytes)?)?;
    assert_eq!(reopened.project(), engine.project());
    let mut broken = serde_json::to_value(engine.to_file())?;
    broken["project"]["library"]["bindings"][0]["definition"]["mappings"] = json!([]);
    let error = format::decode(&serde_json::to_vec(&broken)?).err();
    assert!(error.is_some_and(
        |e| e.code() == "invalid_input" && e.to_string().contains("maps no package terminal")
    ),);
    Ok(())
}

#[test]
fn empty_and_contradictory_drafts_persist_but_nan_and_oversize_are_rejected() -> Result<()> {
    let mut engine = Engine::default();
    let empty = new_draft(&mut engine, DefinitionKind::Component)?;
    let f = draft(&mut engine, &two("Contradictory"))?;
    with(&mut engine, f.footprint, footprint_body, |fp| {
        fp.pads.push(fp.pads[0].clone()); // duplicate local ID
        fp.pads[0].shape = crate::library::PadShape::Rect { size: [-1.0, 0.0] };
    })?;
    let bytes = format::encode(&engine.to_file())?;
    let reopened = Engine::from_file(format::decode(&bytes)?)?;
    assert_eq!(reopened.drafts(), engine.drafts());
    assert_eq!(
        reopened.drafts().get(empty).map(|d| &d.definition),
        Some(&Definition::empty(DefinitionKind::Component))
    );
    let before = engine.document().clone();
    let revision = engine.revision();
    let nan = with(&mut engine, f.footprint, footprint_body, |fp| {
        fp.datum.offset = [f64::NAN, 0.0]
    });
    assert_eq!(nan.err().map(|e| e.code()), Some("invalid_input"));
    let oversize = with(&mut engine, f.component, component_body, |c| {
        let over = u32::try_from(crate::library::MAX_TERMINALS + 1).unwrap_or(u32::MAX);
        c.terminals = (1..=over)
            .map(|i| crate::library::Terminal {
                id: TerminalId(i),
                name: i.to_string(),
                class: passive(),
            })
            .collect()
    });
    assert_eq!(oversize.err().map(|e| e.code()), Some("limit_exceeded"));
    let kind_change = edit(
        &mut engine,
        vec![DraftEdit {
            id: f.package,
            meta: meta("x"),
            definition: Definition::empty(DefinitionKind::Symbol),
        }],
    );
    assert_eq!(kind_change.err().map(|e| e.code()), Some("invalid_input"));
    assert_eq!(engine.document(), &before);
    assert_eq!(engine.revision(), revision);
    Ok(())
}

#[test]
fn check_reports_exactly_the_named_draft_or_revision_and_stores_nothing() -> Result<()> {
    let mut engine = Engine::default();
    let f = component(&mut engine, &two("Pair"))?;
    with(&mut engine, f.binding, binding_body, |b| {
        b.mappings.pop();
    })?;
    let before = engine.to_file();
    let revision = engine.revision();
    let draft = check(&engine, CheckTarget::Draft(f.binding))?;
    assert!(!draft.publishable);
    assert_eq!(draft.revision.revision, Revision(2), "pending revision");
    assert_eq!(
        draft
            .findings
            .iter()
            .map(|f| (f.code.as_str(), f.blocks))
            .collect::<Vec<_>>(),
        vec![("binding.mapping_missing", Some(Stage::Publish))]
    );
    let published = check(
        &engine,
        CheckTarget::Revision(BindingRef::new(f.binding, Revision(1)).erased()),
    )?;
    assert!(published.publishable);
    assert!(published.findings.is_empty());
    // The component draft is unchanged; checked against the pending binding it
    // still lists r1, so it has no findings.
    let component = check(&engine, CheckTarget::Draft(f.component))?;
    assert!(component.findings.is_empty());
    assert_eq!(engine.to_file(), before, "queries store nothing");
    assert_eq!(engine.revision(), revision);
    let text = serde_json::to_string(&engine.to_file())?;
    assert!(!text.contains("verified") && !text.contains("complete"));
    let missing = engine.query(crate::Query::CheckDefinition {
        target: CheckTarget::Revision(PackageRef::new(f.package, Revision(9)).erased()),
    });
    assert_eq!(missing.err().map(|e| e.code()), Some("not_found"));
    Ok(())
}

#[test]
fn new_library_commands_fail_atomically() -> Result<()> {
    let mut engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    prime_history(&mut engine)?;
    let kept = Snapshot::of(&engine);
    let revision = kept.revision();
    let unknown = DefinitionId(999);
    let operations = vec![
        LibraryCommand::NewDraft {
            meta: DefinitionMeta {
                name: "x".repeat(crate::library::MAX_LABEL + 1),
                ..meta("x")
            },
            definition: Definition::empty(DefinitionKind::Package),
        },
        LibraryCommand::NewDraft {
            meta: meta("NaN"),
            definition: Definition::Footprint(crate::library::FootprintDefinition {
                datum: crate::library::Datum {
                    offset: [f64::NAN, 0.0],
                    rotation_deg: 0.0,
                },
                ..Default::default()
            }),
        },
        LibraryCommand::DraftFrom {
            revision: DefinitionRef {
                kind: DefinitionKind::Component,
                id: unknown,
                revision: Revision(1),
            },
        },
        LibraryCommand::DraftFrom {
            revision: f.r1().erased(),
        },
        LibraryCommand::Edit {
            edits: vec![DraftEdit {
                id: unknown,
                meta: meta("x"),
                definition: Definition::empty(DefinitionKind::Component),
            }],
        },
        LibraryCommand::Edit { edits: vec![] },
        LibraryCommand::DiscardDraft { id: unknown },
        LibraryCommand::Publish {
            drafts: vec![],
            close: false,
        },
        LibraryCommand::Publish {
            drafts: vec![unknown],
            close: false,
        },
        LibraryCommand::Publish {
            drafts: vec![f.package, f.package],
            close: false,
        },
        LibraryCommand::GeneratePads {
            draft: unknown,
            script: crate::scripting::Script {
                api_version: 1,
                source: String::new(),
                units: vec![],
                inputs: Default::default(),
            },
        },
        LibraryCommand::GeneratePads {
            draft: f.component,
            script: crate::scripting::Script {
                api_version: 1,
                source: String::new(),
                units: vec![],
                inputs: Default::default(),
            },
        },
    ];
    for command in operations {
        let operation = Operation::Library(command);
        assert!(engine.apply(revision + 1, operation.clone()).is_err());
        let what = format!("{operation:?}");
        assert!(engine.apply(revision, operation).is_err(), "{what}");
        kept.assert_kept(&engine, &what);
    }
    // The draft limit: the 65th draft allocates nothing.
    while engine.drafts().items.len() < crate::library::drafts::MAX_DRAFTS {
        new_draft(&mut engine, DefinitionKind::Package)?;
    }
    prime_history(&mut engine)?;
    let kept = Snapshot::of(&engine);
    let full = engine.apply(
        kept.revision(),
        Operation::Library(LibraryCommand::NewDraft {
            meta: meta("65th"),
            definition: Definition::empty(DefinitionKind::Package),
        }),
    );
    assert_eq!(full.err().map(|e| e.code()), Some("limit_exceeded"));
    kept.assert_kept(&engine, "draft limit");
    for malformed in [
        json!({"editor":"Library","operation":{"command":"library.publish","params":{"drafts":[1],"x":1}}}),
        json!({"editor":"Library","operation":{"command":"library.new_draft","params":{"meta":{"name":"x","description":"","provenance":""},"definition":{"kind":"resistor","body":{}}}}}),
    ] {
        assert!(serde_json::from_value::<Operation>(malformed).is_err());
    }
    Ok(())
}

// ---- F1 footprint rules -------------------------------------------------

use crate::geometry::Shape;
use crate::library::footprint_source::{
    ArrayId, EvaluatorRef, Expr, FootprintParam, FootprintSource, GenerationKey, KeyedOverride,
    PadOverride, ParamName, ParamUnit, SketchBlob, SolveStatus, SolveSummary,
};
use crate::library::graphics::{GraphicEdit, GraphicTarget};
use crate::library::{
    ApertureId, CornerMask, DrawingLayer, Fill, FootprintDefinition, FootprintGraphic, Hole,
    HoleShape, Item, MaskRule, Outline, Pad, PadKind, PadShape, PasteRule, PasteWindow, Polarity,
    Segment, Side, TextContent,
};
use crate::{GraphicId, Outcome, PackageTerminalId, PadId};

fn empty_source() -> FootprintSource {
    FootprintSource {
        evaluator: EvaluatorRef::default(),
        sketch: SketchBlob("{}".into()),
        parameters: vec![],
        pads: vec![],
        arrays: vec![],
        graphics: vec![],
        keys: vec![],
        overrides: vec![],
        solve: SolveSummary::default(),
        datum: None,
    }
}

fn shape_graphic(id: u32, layer: DrawingLayer, shape: Shape, fill: Fill) -> FootprintGraphic {
    FootprintGraphic {
        id: GraphicId(id),
        layer,
        side: Side::Component,
        item: Item::Shape {
            shape: shape_nm(shape),
            width_nm: nm(0.1),
            fill,
        },
    }
}

/// Codes of every finding `component.check` reports for the footprint draft.
fn footprint_codes(engine: &Engine, f: &Fixture, blocking: bool) -> Result<Vec<String>> {
    Ok(check(engine, CheckTarget::Draft(f.footprint))?
        .findings
        .into_iter()
        .filter(|x| x.blocks_stage(Stage::Publish) == blocking)
        .map(|x| x.code)
        .collect())
}

#[test]
fn each_footprint_rule_blocks_publication() -> Result<()> {
    type Breaker = fn(&mut FootprintDefinition);
    let cases: Vec<(&str, Breaker)> = vec![
        ("footprint.terminal_required", |fp| {
            fp.pads[0].terminal = None
        }),
        ("footprint.terminal_forbidden", |fp| {
            fp.pads[0].kind = PadKind::Fiducial
        }),
        ("footprint.hole_kind", |fp| {
            fp.pads[0].kind = PadKind::PlatedThrough
        }),
        ("footprint.hole_kind", |fp| {
            fp.pads[0].hole = Some(Hole {
                shape: HoleShape::Round { diameter: 0.2 },
                plated: true,
                offset: [0.0, 0.0],
            })
        }),
        ("footprint.hole_outside_land", |fp| {
            fp.pads[0].kind = PadKind::PlatedThrough;
            fp.pads[0].hole = Some(Hole {
                shape: HoleShape::Round { diameter: 0.4 },
                plated: true,
                offset: [0.3, 0.0],
            });
        }),
        ("footprint.hole_shape", |fp| {
            fp.pads[0].kind = PadKind::PlatedThrough;
            fp.pads[0].hole = Some(Hole {
                shape: HoleShape::Slot {
                    width: 0.4,
                    length: 0.2,
                    angle_deg: 0.0,
                },
                plated: true,
                offset: [0.0, 0.0],
            });
        }),
        ("footprint.hole_outside_land", |fp| {
            fp.pads[0].vias.push(crate::library::PadVia {
                id: ApertureId(1),
                at: [0.25, 0.0],
                drill: 0.3,
            })
        }),
        ("footprint.paste_window_outside_pad", |fp| {
            fp.pads[0].paste = PasteRule::Windows(vec![PasteWindow {
                id: ApertureId(1),
                shape: shape_nm(Shape::rect([-0.2, -0.2], [0.4, 0.2])),
            }])
        }),
        ("footprint.paste_window_shape", |fp| {
            fp.pads[0].paste = PasteRule::Windows(vec![PasteWindow {
                id: ApertureId(1),
                shape: shape_nm(Shape::Line {
                    a: [0.0, 0.0],
                    b: [0.1, 0.0],
                }),
            }])
        }),
        ("footprint.aperture_duplicate", |fp| {
            let window = PasteWindow {
                id: ApertureId(1),
                shape: shape_nm(Shape::rect([-0.1, -0.1], [0.1, 0.1])),
            };
            fp.pads[0].paste = PasteRule::Windows(vec![window.clone(), window]);
        }),
        ("footprint.pad_size", |fp| {
            fp.pads[0].shape = PadShape::Rect { size: [0.0, 0.8] }
        }),
        ("footprint.pad_shape", |fp| {
            fp.pads[0].shape = PadShape::RoundRect {
                size: [0.6, 0.8],
                radius_ratio: 0.7,
                max_radius: None,
                corners: CornerMask::ALL,
            }
        }),
        ("footprint.pad_shape", |fp| {
            fp.pads[0].shape = PadShape::Custom {
                outline: Outline {
                    start: [0.0, 0.0],
                    segments: vec![
                        Segment::Line { to: [1.0, 0.0] },
                        Segment::Line { to: [1.0, 1.0] },
                    ],
                },
            }
        }),
        ("footprint.pad_shape", |fp| {
            // A bow tie crosses itself.
            fp.pads[0].shape = PadShape::Custom {
                outline: Outline {
                    start: [0.0, 0.0],
                    segments: vec![
                        Segment::Line { to: [1.0, 1.0] },
                        Segment::Line { to: [1.0, 0.0] },
                        Segment::Line { to: [0.0, 1.0] },
                        Segment::Line { to: [0.0, 0.0] },
                    ],
                },
            }
        }),
        ("footprint.pad_side", |fp| {
            fp.pads[0].kind = PadKind::NonPlatedHole;
            fp.pads[0].terminal = None;
            fp.pads[0].side = crate::library::PadSide::Bottom;
        }),
        ("footprint.courtyard_open", |fp| {
            fp.next_local = 10;
            fp.graphics.push(shape_graphic(
                9,
                DrawingLayer::Courtyard,
                Shape::Line {
                    a: [0.0, 0.0],
                    b: [1.0, 0.0],
                },
                Fill::None,
            ));
        }),
        ("graphic.fill_body", |fp| {
            fp.next_local = 10;
            fp.graphics.push(shape_graphic(
                9,
                DrawingLayer::Assembly,
                Shape::rect([0.0, 0.0], [1.0, 1.0]),
                Fill::Body,
            ));
        }),
        ("graphic.degenerate", |fp| {
            fp.next_local = 10;
            fp.graphics.push(shape_graphic(
                9,
                DrawingLayer::Silkscreen,
                Shape::rect([0.0, 0.0], [0.0, 1.0]),
                Fill::None,
            ));
        }),
        ("text.content_kind", |fp| {
            fp.next_local = 10;
            let mut text = crate::library::designator_placeholder(
                GraphicId(9),
                pt([0.0, 0.0]),
                DrawingLayer::Silkscreen,
            );
            if let Item::Text(t) = &mut text.item {
                t.content = TextContent::ComponentName;
            }
            fp.graphics.push(text);
        }),
        ("footprint.polarity_pad_unknown", |fp| {
            fp.polarity = Some(Polarity {
                pad: PadId(99),
                markers: vec![],
            })
        }),
        ("footprint.polarity_marker_unknown", |fp| {
            fp.next_local = 10;
            fp.graphics.push(shape_graphic(
                9,
                DrawingLayer::Courtyard,
                Shape::rect([-3.0, -3.0], [3.0, 3.0]),
                Fill::None,
            ));
            fp.polarity = Some(Polarity {
                pad: PadId(1),
                markers: vec![GraphicId(9)],
            });
        }),
        ("footprint.param_reserved", |fp| {
            let mut source = empty_source();
            source.parameters.push(FootprintParam {
                name: ParamName("pkg_L".into()),
                expr: Expr("1 mm".into()),
                unit: ParamUnit::Mm,
                comment: String::new(),
            });
            fp.source = Some(Box::new(source));
        }),
        ("footprint.param_name", |fp| {
            let mut source = empty_source();
            source.parameters.push(FootprintParam {
                name: ParamName("2pitch".into()),
                expr: Expr("1 mm".into()),
                unit: ParamUnit::Mm,
                comment: String::new(),
            });
            fp.source = Some(Box::new(source));
        }),
        ("footprint.param_expr", |fp| {
            let mut source = empty_source();
            source.parameters.push(FootprintParam {
                name: ParamName("pitch".into()),
                expr: Expr("  ".into()),
                unit: ParamUnit::Mm,
                comment: String::new(),
            });
            fp.source = Some(Box::new(source));
        }),
        ("footprint.param_duplicate", |fp| {
            let mut source = empty_source();
            let param = FootprintParam {
                name: ParamName("pitch".into()),
                expr: Expr("1 mm".into()),
                unit: ParamUnit::Mm,
                comment: String::new(),
            };
            source.parameters = vec![param.clone(), param];
            fp.source = Some(Box::new(source));
        }),
        ("footprint.solve_failed", |fp| {
            let mut source = empty_source();
            source.solve.status = SolveStatus::Failed;
            fp.source = Some(Box::new(source));
        }),
        ("footprint.over_constrained", |fp| {
            let mut source = empty_source();
            source.solve.status = SolveStatus::OverConstrained;
            fp.source = Some(Box::new(source));
        }),
        ("footprint.override_orphaned", |fp| {
            let mut source = empty_source();
            source.overrides.push(KeyedOverride {
                key: GenerationKey::Array {
                    array: ArrayId(7),
                    i: 0,
                    j: 0,
                },
                value: PadOverride::default(),
            });
            fp.source = Some(Box::new(source));
        }),
    ];
    for (code, breaker) in cases {
        let mut engine = Engine::default();
        let f = draft(&mut engine, &two("Pair"))?;
        with(&mut engine, f.footprint, footprint_body, breaker)?;
        let codes = footprint_codes(&engine, &f, true)?;
        assert!(codes.iter().any(|c| c == code), "{code}: {codes:?}");
        refused(&mut engine, f.all(), code)?;
    }
    Ok(())
}

/// Footprint findings name a pad by its number, the user's label, not
/// by its internal id.
#[test]
fn footprint_findings_name_pads_by_number() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(&mut engine, f.footprint, footprint_body, |fp| {
        fp.pads[0].terminal = None;
        fp.pads[0].number = "A7".into();
    })?;
    let finding = check(&engine, CheckTarget::Draft(f.footprint))?
        .findings
        .into_iter()
        .find(|x| x.code == "footprint.terminal_required")
        .ok_or_else(|| Error::NotFound("no terminal finding".into()))?;
    assert!(
        finding.message.starts_with("Pad A7 "),
        "{}",
        finding.message
    );
    Ok(())
}

/// Every pad kind and shape, consistently built, publishes.
#[test]
fn consistent_pads_of_every_kind_and_shape_publish() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(&mut engine, f.footprint, footprint_body, |fp| {
        // Pad 1: plated-through oblong with a slot; pad 2: rounded SMD with
        // paste windows and a via.
        fp.pads[0].kind = PadKind::PlatedThrough;
        fp.pads[0].shape = PadShape::Oblong { size: [1.0, 2.0] };
        fp.pads[0].rotation_deg = 90.0;
        fp.pads[0].hole = Some(Hole {
            shape: HoleShape::Slot {
                width: 0.5,
                length: 1.2,
                angle_deg: 90.0,
            },
            plated: true,
            offset: [0.0, 0.0],
        });
        fp.pads[1].shape = PadShape::RoundRect {
            size: [1.2, 1.2],
            radius_ratio: 0.25,
            max_radius: Some(0.2),
            corners: CornerMask::ALL,
        };
        fp.pads[1].position = [3.0, 0.0];
        fp.pads[1].paste = PasteRule::Windows(vec![
            PasteWindow {
                id: ApertureId(1),
                shape: shape_nm(Shape::rect([-0.5, -0.5], [-0.05, 0.5])),
            },
            PasteWindow {
                id: ApertureId(2),
                shape: shape_nm(Shape::rect([0.05, -0.5], [0.5, 0.5])),
            },
        ]);
        fp.pads[1].vias.push(crate::library::PadVia {
            id: ApertureId(3),
            at: [0.0, 0.0],
            drill: 0.2,
        });
        // A terminal-less mounting hole, a fiducial and a mechanical tab
        // with an exact outline (a half-disc: a line and a bulge arc).
        let mut hole = Pad::smd_rect(PadId(3), PackageTerminalId(1), "MH", [0.0, 4.0], [1.0, 1.0]);
        hole.kind = PadKind::NonPlatedHole;
        hole.terminal = None;
        hole.shape = PadShape::Circle { diameter: 1.2 };
        hole.hole = Some(Hole {
            shape: HoleShape::Round { diameter: 1.0 },
            plated: false,
            offset: [0.0, 0.0],
        });
        let mut fiducial = Pad::smd_rect(
            PadId(4),
            PackageTerminalId(1),
            "FID",
            [4.0, 4.0],
            [1.0, 1.0],
        );
        fiducial.kind = PadKind::Fiducial;
        fiducial.terminal = None;
        fiducial.shape = PadShape::Circle { diameter: 1.0 };
        fiducial.mask = MaskRule::Expansion(0.5);
        let mut tab = Pad::smd_rect(PadId(5), PackageTerminalId(1), "", [-4.0, 4.0], [1.0, 1.0]);
        tab.kind = PadKind::Mechanical;
        tab.terminal = None;
        tab.shape = PadShape::Custom {
            outline: Outline {
                start: [-1.0, 0.0],
                segments: vec![
                    Segment::Line { to: [1.0, 0.0] },
                    Segment::Arc {
                        to: [-1.0, 0.0],
                        bulge: 1.0,
                    },
                ],
            },
        };
        tab.rotation_deg = 30.0;
        fp.pads.extend([hole, fiducial, tab]);
        fp.next_local = 6;
    })?;
    let codes = footprint_codes(&engine, &f, true)?;
    assert!(codes.is_empty(), "{codes:?}");
    publish(&mut engine, f.all())?;
    Ok(())
}

#[test]
fn footprint_lint_is_advisory_and_follows_drawings() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    // Three pads 0.6 mm wide at 0.7 mm pitch leave a 0.1 - 2 × 0.05 = 0 mm
    // mask web; no courtyard; no polarity.
    with(&mut engine, f.footprint, footprint_body, |fp| {
        fp.pads[0].position = [-0.7, 0.0];
        fp.pads[1].position = [0.0, 0.0];
        let mut extra = fp.pads[1].clone();
        extra.id = PadId(3);
        extra.position = [3.0, 0.0];
        fp.pads.push(extra);
        fp.next_local = 4;
    })?;
    let advisory = footprint_codes(&engine, &f, false)?;
    for code in [
        "footprint.courtyard_missing",
        "footprint.polarity_missing",
        "footprint.mask_web",
    ] {
        assert!(advisory.iter().any(|c| c == code), "{code}: {advisory:?}");
    }
    assert_eq!(
        advisory
            .iter()
            .filter(|c| *c == "footprint.mask_web")
            .count(),
        1,
        "only the close pair: {advisory:?}"
    );
    assert!(footprint_codes(&engine, &f, true)?.is_empty());
    // Drawing a courtyard and marking pin 1 clears those lints.
    let outcome = apply(
        &mut engine,
        Operation::Library(LibraryCommand::EditGraphics {
            draft: f.footprint,
            edits: vec![
                GraphicEdit::Add {
                    target: GraphicTarget::Footprint {
                        layer: DrawingLayer::Courtyard,
                        side: Side::Component,
                    },
                    item: Item::Shape {
                        shape: shape_nm(Shape::rect([-2.0, -1.0], [4.0, 1.0])),
                        width_nm: nm(0.05),
                        fill: Fill::None,
                    }
                    .into(),
                },
                GraphicEdit::SetPolarity {
                    polarity: Some(Polarity {
                        pad: PadId(1),
                        markers: vec![],
                    }),
                },
            ],
        }),
    )?;
    assert!(
        matches!(&outcome, Outcome::GraphicsEdited { added } if added == &vec![GraphicId(4)]),
        "{outcome:?}"
    );
    let advisory = footprint_codes(&engine, &f, false)?;
    assert!(
        !advisory
            .iter()
            .any(|c| c == "footprint.courtyard_missing" || c == "footprint.polarity_missing"),
        "{advisory:?}"
    );
    Ok(())
}

#[test]
fn footprint_graphic_edits_are_atomic_and_keep_polarity_consistent() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    let silk = GraphicTarget::Footprint {
        layer: DrawingLayer::Silkscreen,
        side: Side::Component,
    };
    let line = Item::Shape {
        shape: shape_nm(Shape::Line {
            a: [-1.0, 1.0],
            b: [1.0, 1.0],
        }),
        width_nm: nm(0.15),
        fill: Fill::None,
    };
    let edit = |edits| {
        Operation::Library(LibraryCommand::EditGraphics {
            draft: f.footprint,
            edits,
        })
    };
    apply(
        &mut engine,
        edit(vec![
            GraphicEdit::Add {
                target: silk,
                item: line.clone().into(),
            },
            GraphicEdit::SetPolarity {
                polarity: Some(Polarity {
                    pad: PadId(1),
                    markers: vec![GraphicId(3)],
                }),
            },
            GraphicEdit::SetLayer {
                id: GraphicId(3),
                layer: DrawingLayer::Assembly,
                side: Side::Opposite,
            },
        ]),
    )?;
    let Definition::Footprint(fp) = body(&engine, f.footprint)? else {
        return Err(Error::Invalid("not a footprint".into()));
    };
    assert_eq!(
        (fp.graphics[0].layer, fp.graphics[0].side),
        (DrawingLayer::Assembly, Side::Opposite)
    );
    assert_eq!(fp.polarity_pad().map(|p| p.id), Some(PadId(1)));
    // Each refused batch changes nothing.
    let kept = Snapshot::of(&engine);
    for (what, edits) in [
        (
            "unknown polarity pad",
            vec![GraphicEdit::SetPolarity {
                polarity: Some(Polarity {
                    pad: PadId(99),
                    markers: vec![],
                }),
            }],
        ),
        (
            "unit target",
            vec![GraphicEdit::Add {
                target: GraphicTarget::Unit(crate::UnitId(1)),
                item: line.clone().into(),
            }],
        ),
        (
            "symbol pin edit",
            vec![GraphicEdit::SetPinTextHeight { height: nm(1.0) }],
        ),
        (
            "courtyard marker after a good add",
            vec![
                GraphicEdit::Add {
                    target: GraphicTarget::Footprint {
                        layer: DrawingLayer::Courtyard,
                        side: Side::Component,
                    },
                    item: line.clone().into(),
                },
                GraphicEdit::SetPolarity {
                    polarity: Some(Polarity {
                        pad: PadId(1),
                        markers: vec![GraphicId(4)],
                    }),
                },
            ],
        ),
        (
            "unknown graphic",
            vec![GraphicEdit::Remove { id: GraphicId(42) }],
        ),
    ] {
        assert!(
            engine.apply(kept.revision(), edit(edits)).is_err(),
            "{what}"
        );
        kept.assert_kept(&engine, what);
    }
    // A symbol draft refuses footprint layers.
    assert!(
        engine
            .apply(
                kept.revision(),
                Operation::Library(LibraryCommand::EditGraphics {
                    draft: f.symbol,
                    edits: vec![GraphicEdit::Add {
                        target: silk,
                        item: line.clone().into(),
                    }],
                }),
            )
            .is_err()
    );
    // Removing a marker drops it from the polarity; the pad stays.
    apply(
        &mut engine,
        edit(vec![GraphicEdit::Remove { id: GraphicId(3) }]),
    )?;
    let Definition::Footprint(fp) = body(&engine, f.footprint)? else {
        return Err(Error::Invalid("not a footprint".into()));
    };
    assert_eq!(
        fp.polarity,
        Some(Polarity {
            pad: PadId(1),
            markers: vec![]
        })
    );
    Ok(())
}

#[test]
fn derived_footprint_graphics_are_edited_through_their_role() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(&mut engine, f.footprint, footprint_body, |fp| {
        let mut source = empty_source();
        source
            .graphics
            .push(crate::library::footprint_source::GraphicRole {
                id: GraphicId(3),
                layer: DrawingLayer::Assembly,
                side: Side::Component,
                source: crate::library::footprint_source::GraphicSource::Derived(
                    crate::library::footprint_source::DerivedGraphic::AssemblyFromEnvelope,
                ),
                width: Expr("0.1 mm".into()),
                fill: Fill::None,
            });
        fp.source = Some(Box::new(source));
        fp.graphics.push(shape_graphic(
            3,
            DrawingLayer::Assembly,
            Shape::rect([-2.0, -1.5], [2.0, 1.5]),
            Fill::None,
        ));
        fp.next_local = 4;
    })?;
    let kept = Snapshot::of(&engine);
    let error = engine.apply(
        kept.revision(),
        Operation::Library(LibraryCommand::EditGraphics {
            draft: f.footprint,
            edits: vec![GraphicEdit::Translate {
                ids: vec![GraphicId(3)],
                delta: pt([1.0, 0.0]),
            }],
        }),
    );
    assert_eq!(error.err().map(|e| e.code()), Some("conflict"));
    kept.assert_kept(&engine, "derived graphic");
    Ok(())
}

#[test]
fn footprint_source_and_drawing_bounds_refuse_the_draft() -> Result<()> {
    type Breaker = fn(&mut FootprintDefinition);
    let cases: Vec<(&str, Breaker)> = vec![
        ("oversized expression", |fp| {
            let mut source = empty_source();
            source.parameters.push(FootprintParam {
                name: ParamName("pitch".into()),
                expr: Expr("1".repeat(crate::library::footprint_source::MAX_EXPR + 1)),
                unit: ParamUnit::Mm,
                comment: String::new(),
            });
            fp.source = Some(Box::new(source));
        }),
        ("oversized sketch", |fp| {
            let mut source = empty_source();
            source.sketch =
                SketchBlob(" ".repeat(crate::library::footprint_source::MAX_SKETCH_BYTES + 1));
            fp.source = Some(Box::new(source));
        }),
        ("too many polarity markers", |fp| {
            fp.polarity = Some(Polarity {
                pad: PadId(1),
                markers: (1..=9).map(GraphicId).collect(),
            });
        }),
        ("too many drawing vertices", |fp| {
            fp.next_local = 20;
            for id in 10..19 {
                fp.graphics.push(shape_graphic(
                    id,
                    DrawingLayer::Silkscreen,
                    Shape::polyline((0..1000).map(|i| [f64::from(i) * 0.01, 0.0]).collect()),
                    Fill::None,
                ));
            }
        }),
        ("oversized pad number", |fp| {
            fp.pads[0].number = "9".repeat(crate::library::MAX_LABEL + 1)
        }),
        ("non-finite pad default", |fp| {
            fp.defaults.mask_expansion_mm = f64::INFINITY
        }),
    ];
    for (what, breaker) in cases {
        let mut engine = Engine::default();
        let f = draft(&mut engine, &two("Pair"))?;
        let kept = Snapshot::of(&engine);
        let result = with(&mut engine, f.footprint, footprint_body, breaker);
        assert!(result.is_err(), "{what}");
        kept.assert_kept(&engine, what);
    }
    Ok(())
}

#[test]
fn samples_carry_standard_drawings_that_publish_without_lint() -> Result<()> {
    let engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let footprint = engine
        .project()
        .library
        .footprints
        .iter()
        .find(|p| p.id == f.footprint)
        .map(|p| p.definition.clone())
        .ok_or_else(|| Error::NotFound("demo footprint".into()))?;
    for layer in [
        DrawingLayer::Silkscreen,
        DrawingLayer::Courtyard,
        DrawingLayer::Assembly,
    ] {
        assert!(
            footprint.graphics.iter().any(|g| g.layer == layer),
            "{layer:?}"
        );
    }
    assert_eq!(
        footprint
            .graphics
            .iter()
            .filter(|g| matches!(&g.item, Item::Text(t) if t.content == TextContent::Designator))
            .count(),
        2,
        "designators on silkscreen and assembly"
    );
    assert!(footprint.polarity.is_none(), "never inferred");
    assert!(footprint.lint(f.footprint).is_empty());
    Ok(())
}

#[test]
fn layout_validate_bounds_placed_footprint_graphic_vertices() -> Result<()> {
    let engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let mut project = engine.project().clone();
    crate::layout::validate(&project)?;
    // Every placed demo part draws a full-size footprint: 8 × 1024 vertices.
    for published in &mut project.library.footprints {
        if published.id == f.footprint {
            published.definition.graphics = (0..8)
                .map(|i| {
                    shape_graphic(
                        100 + i,
                        DrawingLayer::Silkscreen,
                        Shape::polyline((0..1024).map(|k| [f64::from(k) * 0.001, 0.0]).collect()),
                        Fill::None,
                    )
                })
                .collect();
        }
    }
    let template = project
        .placements
        .first()
        .cloned()
        .ok_or_else(|| Error::NotFound("demo placement".into()))?;
    let instance = project
        .instance(template.instance)
        .cloned()
        .ok_or_else(|| Error::NotFound("demo instance".into()))?;
    let limit = crate::layout::MAX_PLACED_GRAPHIC_VERTICES / 8192;
    let add = |project: &mut crate::model::Project| {
        let id = project.next_id;
        project.next_id += 1;
        project.instances.push(crate::model::ComponentInstance {
            text_placements: Default::default(),
            id,
            ..instance.clone()
        });
        project.placements.push(crate::model::Placement {
            instance: id,
            ..template.clone()
        });
    };
    while project.placements.len() < limit {
        add(&mut project);
    }
    crate::layout::validate(&project)?;
    add(&mut project);
    let error = crate::layout::validate(&project).err();
    assert!(
        error
            .as_ref()
            .is_some_and(|e| e.to_string().contains("graphic vertices")),
        "{error:?}"
    );
    Ok(())
}
