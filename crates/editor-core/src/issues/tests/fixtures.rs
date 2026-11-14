//! `project.issues` over fixtures: severities, merging, scopes and the
//! message style of every title, raw message and refusal.
use super::super::*;
use super::{Problem, check, naming};
use crate::completeness::{self, PurchasingState};
use crate::layout::LayoutCommand;
use crate::library::{BindingDefinition, Definition, DraftEdit, FootprintDefinition};
use crate::model::Layer;
use crate::tests::support::*;
use crate::{Engine, Operation, Query, QueryResult};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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

fn issues(engine: &Engine, scope: IssueScope) -> Fallible<IssueReport> {
    match engine.query(Query::Issues { scope })? {
        QueryResult::Issues(report) => Ok(report),
        other => Err(format!("unexpected report {other:?}").into()),
    }
}

fn all(engine: &Engine) -> Fallible<IssueReport> {
    issues(engine, IssueScope::All)
}

/// Every finding the rule owners report for the project, counted
/// independently of `project.issues`, plus one per instance without a Part.
fn raw_findings(engine: &Engine) -> Fallible<usize> {
    let project = engine.project();
    let mut count = 0usize;
    for report in completeness::drafts(project, engine.drafts())? {
        count += report.findings.len();
    }
    for instance in &project.instances {
        let report = completeness::instance(project, instance.id)?;
        count += report.findings.len() + report.purchasing.findings.len();
        count += usize::from(report.purchasing.state == PurchasingState::NoPart);
    }
    count += crate::erc::checks(project).len();
    count += crate::layout::checks(project, &Default::default()).len();
    Ok(count)
}

/// What holds for every report: styled titles, names in raw text, errors
/// for every blocker, counts that match, most severe first, and no finding
/// lost to merging.
fn well_formed(engine: &Engine) -> Fallible<IssueReport> {
    let report = all(engine)?;
    for row in &report.rows {
        assert_eq!(
            check(&row.title),
            Vec::<Problem>::new(),
            "title {:?}",
            row.title
        );
        assert_eq!(
            check(&row.group.title),
            Vec::<Problem>::new(),
            "group {:?}",
            row.group.title
        );
        for finding in std::iter::once(&row.finding).chain(&row.also) {
            assert_eq!(
                naming(&finding.message),
                Vec::<Problem>::new(),
                "{}",
                finding.message
            );
            assert!(
                KNOWN_CODES.contains(&finding.code.as_str()),
                "{}",
                finding.code
            );
        }
        for text in [&row.detail.cause, &row.detail.fix].into_iter().flatten() {
            assert_eq!(naming(text), Vec::<Problem>::new(), "{text}");
        }
        if row.blocks().is_some() {
            assert_eq!(row.severity, Severity::Error, "{}", row.title);
        }
    }
    let mut recount = IssueCounts::default();
    for row in &report.rows {
        match row.severity {
            Severity::Error => recount.error += 1,
            Severity::Warning => recount.warning += 1,
            Severity::ToDo => recount.to_do += 1,
            Severity::Info => recount.info += 1,
        }
    }
    assert_eq!(report.counts, recount);
    assert!(
        report
            .rows
            .windows(2)
            .all(|w| w[0].severity <= w[1].severity)
    );
    let merged: usize = report.rows.iter().map(IssueRow::findings).sum();
    assert_eq!(merged, raw_findings(engine)?, "every finding is in a row");
    assert!(!report.coverage.is_empty());
    Ok(report)
}

fn titles(report: &IssueReport) -> Vec<&str> {
    report.rows.iter().map(|r| r.title.as_str()).collect()
}

#[test]
fn the_demo_reports_unrouted_copper_and_missing_parts_as_to_do() -> Fallible<()> {
    let engine = Engine::demo()?;
    let report = well_formed(&engine)?;
    let to_do: Vec<&IssueRow> = report
        .rows
        .iter()
        .filter(|r| r.severity == Severity::ToDo)
        .collect();
    assert!(
        to_do.iter().any(|r| r.group.code == "board.unrouted"),
        "{:?}",
        titles(&report)
    );
    let no_part: Vec<&str> = to_do
        .iter()
        .filter(|r| r.group.code == rules::NO_PART)
        .map(|r| r.title.as_str())
        .collect();
    assert_eq!(no_part, ["R1: no Part", "R2: no Part"]);
    // Every unrouted pad of one net is one row naming the net.
    let unrouted: Vec<&IssueRow> = report
        .rows
        .iter()
        .filter(|r| r.group.code == "board.unrouted")
        .collect();
    for row in &unrouted {
        let nets: Vec<&ObjectRef> = row
            .objects()
            .filter(|o| matches!(o, ObjectRef::Net { .. }))
            .collect();
        assert!(nets.windows(2).all(|w| w[0] == w[1]), "one net per row");
        assert!(row.findings() >= 2, "{}", row.title);
        assert!(row.title.ends_with("pads unrouted"), "{}", row.title);
    }
    Ok(())
}

/// Every object of every demo issue has a user name for its chip, one per
/// object and in order, in the message style and never an ID.
#[test]
fn every_issue_object_has_a_user_name() -> Fallible<()> {
    let engine = Engine::demo()?;
    let report = all(&engine)?;
    let mut named = 0;
    for row in &report.rows {
        let objects: Vec<ObjectRef> = row.objects().copied().collect();
        let names = object_names(engine.project(), engine.drafts(), &objects);
        assert_eq!(names.len(), objects.len(), "{}", row.title);
        for name in &names {
            assert_eq!(naming(name), Vec::<Problem>::new(), "{name}");
            assert!(
                name.chars().next().is_some_and(|c| !c.is_lowercase()),
                "{name}"
            );
        }
        named += names.len();
    }
    assert!(named > 0);
    let r1 = engine
        .project()
        .instances
        .iter()
        .find(|i| i.designator == "R1")
        .map(|i| ObjectRef::Instance { id: i.id })
        .ok_or("R1")?;
    assert_eq!(
        object_names(engine.project(), engine.drafts(), &[r1]),
        ["R1"]
    );
    Ok(())
}

#[test]
fn a_publish_blocker_is_an_error_that_is_counted() -> Fallible<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(&mut engine, f.component, component_body, |c| {
        c.designator_prefix = "r1".into();
    })?;
    let report = well_formed(&engine)?;
    let row = report
        .rows
        .iter()
        .find(|r| r.group.code == "component.designator_prefix")
        .ok_or("no designator finding")?;
    assert_eq!(row.severity, Severity::Error);
    assert_eq!(row.blocks(), Some(Stage::Publish));
    assert_eq!(row.area, IssueArea::Component);
    assert!(report.counts.error >= 1);
    Ok(())
}

#[test]
fn a_pad_and_the_package_terminal_it_designates_are_one_issue() -> Fallible<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(
        &mut engine,
        f.footprint,
        footprint_body,
        |fp: &mut FootprintDefinition| {
            for pad in &mut fp.pads {
                pad.terminal = None;
            }
        },
    )?;
    let report = well_formed(&engine)?;
    let pads: Vec<&IssueRow> = report
        .rows
        .iter()
        .filter(|r| r.group.code == "footprint.terminal_required")
        .collect();
    assert_eq!(pads.len(), 2, "{:?}", titles(&report));
    for row in &pads {
        assert_eq!(row.also.len(), 1, "{}", row.title);
        assert_eq!(row.also[0].code, "footprint.terminal_without_pad");
        assert_eq!(row.severity, Severity::Error);
        assert!(
            row.objects()
                .any(|o| matches!(o, ObjectRef::LibraryObject { definition, .. } if *definition == f.package)),
            "the package terminal is located too"
        );
    }
    assert_eq!(
        pads.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        [
            "Pad 1 lands no package terminal",
            "Pad 2 lands no package terminal"
        ]
    );
    assert!(
        !report
            .rows
            .iter()
            .any(|r| r.group.code == "footprint.terminal_without_pad"),
        "no side is shown twice"
    );
    Ok(())
}

#[test]
fn a_pad_without_a_counterpart_keeps_its_own_issue() -> Fallible<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(
        &mut engine,
        f.footprint,
        footprint_body,
        |fp: &mut FootprintDefinition| {
            fp.pads[0].terminal = None;
            fp.pads[0].number = "EP".into();
        },
    )?;
    let report = well_formed(&engine)?;
    let codes: Vec<&str> = report.rows.iter().map(|r| r.group.code.as_str()).collect();
    assert!(codes.contains(&"footprint.terminal_required"), "{codes:?}");
    assert!(
        codes.contains(&"footprint.terminal_without_pad"),
        "{codes:?}"
    );
    assert!(titles(&report).contains(&"Package terminal ‘1’ has no pad"));
    Ok(())
}

#[test]
fn an_unmapped_terminal_and_its_package_terminal_are_one_issue() -> Fallible<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(
        &mut engine,
        f.binding,
        binding_body,
        |b: &mut BindingDefinition| {
            b.mappings.retain(|m| m.terminal.0 != 2);
        },
    )?;
    let report = well_formed(&engine)?;
    let row = report
        .rows
        .iter()
        .find(|r| r.group.code == "component.mapping_missing")
        .ok_or("no mapping finding")?;
    assert_eq!(row.title, "Package terminal ‘2’ has no electrical terminal");
    assert_eq!(row.also.len(), 1);
    assert_eq!(row.also[0].code, "binding.mapping_missing");
    assert!(
        !report
            .rows
            .iter()
            .any(|r| r.group.code == "binding.mapping_missing"),
        "{:?}",
        titles(&report)
    );
    Ok(())
}

#[test]
fn clearance_between_two_placements_is_one_issue() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let r2 = by_designator(&engine, "R2")?;
    for (instance, x) in [(r1, 0.0), (r2, 0.1)] {
        apply(
            &mut engine,
            Operation::Layout(LayoutCommand::Move {
                instance,
                position: [x, 0.0],
                rotation_deg: 0.0,
                layer: Layer::Top,
            }),
        )?;
    }
    let report = well_formed(&engine)?;
    let clearance: Vec<&IssueRow> = report
        .rows
        .iter()
        .filter(|r| r.group.code == "board.clearance")
        .collect();
    assert_eq!(clearance.len(), 1, "{:?}", titles(&report));
    let row = clearance[0];
    assert!(row.findings() > 1);
    assert_eq!(row.severity, Severity::Error);
    assert!(row.title.starts_with("R1 ↔ R2: "), "{}", row.title);
    assert!(row.title.ends_with("pads too close"), "{}", row.title);
    let courtyards: Vec<&str> = report
        .rows
        .iter()
        .filter(|r| r.group.code == "board.courtyard_overlap")
        .map(|r| r.title.as_str())
        .collect();
    assert!(
        courtyards.is_empty() || courtyards == ["R1 ↔ R2 courtyards overlap"],
        "{courtyards:?}"
    );
    Ok(())
}

#[test]
fn per_instance_copies_of_one_part_finding_are_one_issue() {
    let part = crate::ids::DefinitionId(40);
    let finding = |instance: u64, designator: &str| {
        Finding::new(
            "part.stale_component",
            format!(
                "{designator}: Part EX-1 is for Resistor r1, older than the instance's Resistor r2."
            ),
            vec![
                ObjectRef::Instance { id: instance },
                ObjectRef::Definition { id: part },
            ],
        )
    };
    let merged = merge::schematic(vec![finding(1, "R1"), finding(2, "R2")]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].len(), 2);
}

#[test]
fn area_scopes_partition_the_project_and_count_the_rest() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let f = draft(&mut engine, &two("Pair"))?;
    with(&mut engine, f.component, component_body, |c| {
        c.designator_prefix = String::new();
    })?;
    let every = all(&engine)?;
    let mut total = 0;
    for area in [IssueArea::Component, IssueArea::Schematic, IssueArea::Board] {
        let one = issues(&engine, IssueScope::Area { area })?;
        assert!(one.rows.iter().all(|r| r.area == area));
        total += one.rows.len();
        let outside: usize = one
            .elsewhere
            .iter()
            .map(|a| a.counts.error + a.counts.warning + a.counts.to_do + a.counts.info)
            .sum();
        assert_eq!(one.rows.len() + outside, every.rows.len(), "{area:?}");
        assert!(one.elsewhere.iter().all(|a| a.area != area));
    }
    assert_eq!(total, every.rows.len());
    // The schematic sees that the component area blocks publishing.
    let schematic = issues(
        &engine,
        IssueScope::Area {
            area: IssueArea::Schematic,
        },
    )?;
    let component = schematic
        .elsewhere
        .iter()
        .find(|a| a.area == IssueArea::Component)
        .ok_or("component blockers are out of sight")?;
    assert!(component.blocking >= 1);
    Ok(())
}

#[test]
fn a_selection_scope_lists_the_issues_of_those_objects() -> Fallible<()> {
    let engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let selected = issues(
        &engine,
        IssueScope::Selection {
            objects: vec![ObjectRef::Instance { id: r1 }],
        },
    )?;
    assert!(!selected.rows.is_empty());
    for row in &selected.rows {
        assert!(
            row.objects().any(|o| match *o {
                ObjectRef::Instance { id } | ObjectRef::Pad { instance: id, .. } => id == r1,
                ObjectRef::Terminal { endpoint } => endpoint.instance == r1,
                _ => false,
            }),
            "{}",
            row.title
        );
    }
    assert!(selected.rows.iter().any(|r| r.title == "R1: no Part"));
    assert!(!selected.rows.iter().any(|r| r.title == "R2: no Part"));
    let too_many = IssueScope::Selection {
        objects: vec![ObjectRef::Instance { id: r1 }; MAX_SCOPE_OBJECTS + 1],
    };
    assert!(matches!(
        engine.query(Query::Issues { scope: too_many }),
        Err(Error::Limit(_))
    ));
    Ok(())
}

/// Broken drafts of every kind: each finding's title follows the style
/// and each raw message names objects by their names.
#[test]
fn findings_of_broken_drafts_follow_the_style() -> Fallible<()> {
    type Breaker = fn(&mut Engine, &Fixture) -> crate::Result<()>;
    let cases: Vec<Breaker> = vec![
        |e, f| {
            with(e, f.component, component_body, |c| {
                c.terminals[0].name = String::new()
            })
        },
        |e, f| {
            with(e, f.component, component_body, |c| {
                c.representations.clear()
            })
        },
        |e, f| {
            with(e, f.symbol, symbol_body, |s| {
                s.units[0].pins.retain(|p| p.terminal.0 != 2)
            })
        },
        |e, f| {
            with(e, f.symbol, symbol_body, |s| {
                s.units[0].label = String::new()
            })
        },
        |e, f| {
            with(e, f.package, package_body, |p| {
                p.terminals[1].designation = p.terminals[0].designation.clone()
            })
        },
        |e, f| {
            with(e, f.footprint, footprint_body, |fp| {
                fp.pads[0].kind = crate::library::PadKind::Fiducial
            })
        },
        |e, f| {
            with(e, f.footprint, footprint_body, |fp| {
                fp.pads[1].terminal = None
            })
        },
        |e, f| {
            with(e, f.footprint, footprint_body, |fp| {
                fp.pads[0].kind = crate::library::PadKind::PlatedThrough
            })
        },
        |e, f| with(e, f.binding, binding_body, |b| b.mappings.clear()),
        |e, f| with(e, f.binding, binding_body, |b| b.footprints.clear()),
    ];
    for breaker in cases {
        let mut engine = Engine::default();
        let f = draft(&mut engine, &two("Pair"))?;
        breaker(&mut engine, &f)?;
        let report = well_formed(&engine)?;
        assert!(report.counts.error >= 1, "{:?}", titles(&report));
    }
    Ok(())
}

/// Refusals the editors meet name objects by their names: no IDs, rule
/// codes or debug output in what people read.
#[test]
fn refusals_name_objects_by_their_names() -> Fallible<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(&mut engine, f.component, component_body, |c| {
        c.designator_prefix = "r".into();
    })?;
    let mut refused = Vec::new();
    refused.push(publish(&mut engine, f.all()).err().ok_or("published")?);
    refused.push(
        edit(
            &mut engine,
            vec![DraftEdit {
                id: crate::ids::DefinitionId(999),
                meta: meta("Gone"),
                definition: Definition::empty(crate::ids::DefinitionKind::Symbol),
            }],
        )
        .err()
        .ok_or("edited a missing draft")?,
    );
    let body = engine
        .drafts()
        .get(f.symbol)
        .map(|d| d.meta.clone())
        .ok_or("symbol draft")?;
    refused.push(
        edit(
            &mut engine,
            vec![DraftEdit {
                id: f.symbol,
                meta: body,
                definition: Definition::empty(crate::ids::DefinitionKind::Package),
            }],
        )
        .err()
        .ok_or("changed a draft's kind")?,
    );
    let mut fresh = Engine::default();
    refused.push(fresh.undo().err().ok_or("undo")?);
    refused.push(fresh.redo().err().ok_or("redo")?);
    refused.push(
        engine
            .apply(
                engine.revision() + 5,
                Operation::Library(crate::LibraryCommand::Publish {
                    drafts: f.all(),
                    close: false,
                }),
            )
            .err()
            .ok_or("stale apply")?,
    );
    for error in &refused {
        let text = error.to_string();
        assert_eq!(naming(&text), Vec::<Problem>::new(), "{text}");
        assert_eq!(error.diagnostic().message, text);
    }
    // The publish refusal names the component, not its lineage ID.
    assert!(
        refused[0].to_string().contains("Component ‘Pair’"),
        "{}",
        refused[0]
    );
    Ok(())
}

/// A refusal that names its rule keeps the code out of the text people
/// read and offers it to hosts through `rule()` and the diagnostic.
#[test]
fn a_rule_code_moves_from_the_text_to_the_diagnostic() {
    let error = Error::Conflict("footprint.explicit: this footprint has explicit pads".into());
    assert_eq!(error.rule(), Some("footprint.explicit"));
    assert_eq!(error.to_string(), "This footprint has explicit pads");
    let diagnostic = error.diagnostic();
    assert_eq!(diagnostic.rule.as_deref(), Some("footprint.explicit"));
    assert_eq!(diagnostic.message, "This footprint has explicit pads");
    let plain = Error::Invalid("pad_l: the expression ends with an operator".into());
    assert_eq!(plain.rule(), None);
    assert_eq!(
        plain.to_string(),
        "pad_l: the expression ends with an operator"
    );
}

/// Every finding code the core sources emit is classified in
/// [`KNOWN_CODES`], so a new rule gets a severity and a group title.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn every_finding_code_in_the_sources_is_known() -> Fallible<()> {
    let mut found = std::collections::BTreeSet::new();
    for (path, text) in sources()? {
        let mut rest = text.as_str();
        while let Some(at) = rest.find(['(', ',']) {
            rest = &rest[at + 1..];
            let call = rest.trim_start();
            let Some(literal) = call.strip_prefix('"') else {
                continue;
            };
            let Some(end) = literal.find('"') else {
                continue;
            };
            let code = &literal[..end];
            let before = &text[..text.len() - rest.len() - 1];
            let callee = before
                .trim_end()
                .rsplit(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                .next()
                .unwrap_or_default();
            let finding_call = matches!(
                callee,
                "Finding::new" | "Finding::blocking" | "issue" | "publish_issue"
            );
            if finding_call && code.contains('.') && !code.contains(' ') {
                found.insert((code.to_owned(), path.clone()));
            }
        }
    }
    assert!(found.len() > 50, "the scan finds the rule owners");
    for (code, path) in &found {
        assert!(KNOWN_CODES.contains(&code.as_str()), "{code} ({path})");
    }
    Ok(())
}

/// Every code in [`KNOWN_CODES`] except the synthetic ones `project.issues`
/// adds is still written in a production source (as a literal outside the
/// classification table), so a code whose last emitter is gone is removed
/// with it.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn every_known_code_is_still_emitted() -> Fallible<()> {
    let texts: Vec<String> = sources()?
        .into_iter()
        .filter(|(path, _)| path != "issues/rules.rs")
        .map(|(_, text)| text)
        .collect();
    let synthetic = [
        super::super::rules::NO_PART,
        super::super::rules::CHECK_FAILED,
    ];
    for code in KNOWN_CODES.iter().filter(|c| !synthetic.contains(c)) {
        let literal = format!("\"{code}\"");
        assert!(
            texts.iter().any(|text| text.contains(&literal)),
            "{code} is classified but nothing emits it"
        );
    }
    Ok(())
}

/// No message or refusal in the core formats a value with `Debug`.
/// Purchasing group keys are internal sort keys and never shown.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn no_message_formats_debug_output() -> Fallible<()> {
    for (path, text) in sources()? {
        if path.ends_with("completeness/purchasing.rs") {
            continue;
        }
        for (n, line) in text.lines().enumerate() {
            let debug = line.contains(":?}") && line.contains('"');
            assert!(!debug, "{path}:{}: {line}", n + 1);
        }
    }
    Ok(())
}

/// `text` without its inline test module: everything before the first
/// `#[cfg(test)]` that opens a `mod … {` block.
#[cfg(not(target_arch = "wasm32"))]
fn production(text: &str) -> String {
    let mut at = 0;
    while let Some(found) = text[at..].find("#[cfg(test)]") {
        let start = at + found;
        let after = text[start + "#[cfg(test)]".len()..].trim_start();
        let line = after.lines().next().unwrap_or_default();
        if line.starts_with("mod ") && line.trim_end().ends_with('{') {
            return text[..start].to_owned();
        }
        at = start + 1;
    }
    text.to_owned()
}

/// The non-test sources of the core, except `updates.rs` and `updates/`
/// (slice C2 owns their wording): each file up to its test module.
#[cfg(not(target_arch = "wasm32"))]
fn sources() -> Fallible<Vec<(String, String)>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let name = path
                .strip_prefix(&root)?
                .to_string_lossy()
                .replace('\\', "/");
            if name == "tests" || name.ends_with("/tests") || name.starts_with("updates") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if name.ends_with(".rs") && !name.ends_with("tests.rs") && name != "tests.rs" {
                let text = std::fs::read_to_string(&path)?;
                out.push((name, production(&text)));
            }
        }
    }
    Ok(out)
}
