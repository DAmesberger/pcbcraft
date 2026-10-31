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
        error.to_string().contains("Nothing was published"),
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
        p.body_size = [9.0, 9.0, 9.0];
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
            [0.0, -5.0],
            crate::library::PinDirection::Up,
            2.5,
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
        c.terminals = (1..=257)
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
        LibraryCommand::Publish { drafts: vec![] },
        LibraryCommand::Publish {
            drafts: vec![unknown],
        },
        LibraryCommand::Publish {
            drafts: vec![f.package, f.package],
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
