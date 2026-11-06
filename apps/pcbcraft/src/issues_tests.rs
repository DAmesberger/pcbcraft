//! The Issues surface over the demo project: groups keep every issue,
//! fixes name only known rules, locate routes to the workspace that shows
//! the objects, the card lists and locates, and the pill steps through the
//! listed issues.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use egui::{Event, PointerButton, Pos2, RawInput};
use pcbcraft_core::issues::{AreaCount, KNOWN_CODES};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn report(engine: &Engine, scope: IssueScope) -> Fallible<Arc<IssueReport>> {
    match Answer::of(engine.query(Query::Issues { scope })) {
        Answer::Report(report) => Ok(report),
        Answer::Refused(why) => Err(why.into()),
    }
}

/// A locate target that records what it was asked and answers `shows`.
struct Probe {
    shows: bool,
    asked: usize,
}

impl Locate for Probe {
    fn locate(&mut self, _: &Engine, _: &[ObjectRef]) -> bool {
        self.asked += 1;
        self.shows
    }
}

fn probe(shows: bool) -> Probe {
    Probe { shows, asked: 0 }
}

#[test]
fn every_fix_names_a_rule_core_knows() {
    for (code, label, _) in FIXES {
        assert!(KNOWN_CODES.contains(code), "{code}");
        assert!(
            label.ends_with('…'),
            "a Fix that opens a dialog says so: {label}"
        );
    }
}

/// Grouping is presentation only: every row of the report is in exactly one
/// group, groups come most severe first, and a filter keeps exactly the
/// rows of its severity.
#[test]
fn groups_keep_every_issue_and_the_filter_only_narrows() -> Fallible<()> {
    let engine = Engine::demo()?;
    let report = report(&engine, IssueScope::All)?;
    assert!(!report.rows.is_empty());
    let all = groups(&report, None);
    let mut listed = flat(&all);
    listed.sort_unstable();
    assert_eq!(listed, (0..report.rows.len()).collect::<Vec<_>>());
    assert!(all.windows(2).all(|w| w[0].level <= w[1].level));
    for group in &all {
        assert!(!group.title.is_empty());
        assert!(
            group
                .rows
                .iter()
                .all(|&i| group_key(&report.rows[i]) == group.key)
        );
    }
    for severity in [
        Severity::Error,
        Severity::Warning,
        Severity::ToDo,
        Severity::Info,
    ] {
        let kept = flat(&groups(&report, Some(severity))).len();
        let expected = report
            .rows
            .iter()
            .filter(|r| r.severity == severity)
            .count();
        assert_eq!(kept, expected, "{severity:?}");
    }
    // The demo's instances have no Part: one group-wide Resolve parts…
    let no_part = all
        .iter()
        .find(|g| g.key.starts_with("purchasing.no_part"))
        .ok_or("no-part group")?;
    assert_eq!(no_part.level, Level::ToDo);
    assert!(!opens_by_default(no_part.level), "to do starts folded");
    assert_eq!(no_part.fix.as_ref().map(Fix::label), Some("Resolve parts…"));
    Ok(())
}

#[test]
fn locate_prefers_the_issues_workspace_then_tries_the_others() -> Fallible<()> {
    let engine = Engine::default();
    let refs = [ObjectRef::Net { id: 1 }];
    let (mut part, mut schematic, mut pcb) = (probe(true), probe(true), probe(true));
    let mut editors = Editors {
        part: &mut part,
        schematic: &mut schematic,
        pcb: &mut pcb,
    };
    assert_eq!(
        route(&mut editors, &engine, &refs, Workspace::Pcb),
        Some(Workspace::Pcb)
    );
    assert_eq!((part.asked, schematic.asked, pcb.asked), (0, 0, 1));

    let (mut part, mut schematic, mut pcb) = (probe(false), probe(true), probe(false));
    let mut editors = Editors {
        part: &mut part,
        schematic: &mut schematic,
        pcb: &mut pcb,
    };
    assert_eq!(
        route(&mut editors, &engine, &refs, Workspace::Pcb),
        Some(Workspace::Schematic),
        "an object the preferred workspace cannot show is found elsewhere"
    );
    assert_eq!(pcb.asked, 1);

    let (mut part, mut schematic, mut pcb) = (probe(false), probe(false), probe(false));
    let mut editors = Editors {
        part: &mut part,
        schematic: &mut schematic,
        pcb: &mut pcb,
    };
    assert_eq!(route(&mut editors, &engine, &refs, Workspace::Part), None);
    assert_eq!(route(&mut editors, &engine, &[], Workspace::Part), None);
    assert_eq!(
        (part.asked, schematic.asked, pcb.asked),
        (1, 1, 1),
        "each asked once; nothing to locate asks no editor"
    );
    Ok(())
}

/// Through the editors' own selection: a Board issue selects in the PCB, a
/// Schematic issue selects its instance on the sheet.
#[test]
fn every_demo_issue_locates_in_its_workspace() -> Fallible<()> {
    let engine = Engine::demo()?;
    let report = report(&engine, IssueScope::All)?;
    for row in &report.rows {
        let mut part = crate::part_editor::PartEditor::default();
        let mut schematic = crate::schematic_editor::SchematicEditor::default();
        let mut pcb = crate::pcb_editor::PcbEditor::default();
        let refs: Vec<ObjectRef> = row.objects().copied().collect();
        let shown = {
            let mut editors = Editors {
                part: &mut part,
                schematic: &mut schematic,
                pcb: &mut pcb,
            };
            route(&mut editors, &engine, &refs, workspace(row.area))
        };
        assert_eq!(shown, Some(workspace(row.area)), "{}", row.title);
        match row.area {
            IssueArea::Board => assert!(
                pcb.selected_instance().is_some()
                    || pcb.selected_track().is_some()
                    || pcb.highlighted_net().is_some(),
                "{}",
                row.title
            ),
            IssueArea::Schematic => assert!(
                schematic.selected_instance().is_some()
                    || schematic.selected_net(&engine).is_some(),
                "{}",
                row.title
            ),
            IssueArea::Component => assert!(part.lineage().is_some(), "{}", row.title),
        }
    }
    Ok(())
}

#[test]
fn a_refused_query_is_not_counted_and_blockers_elsewhere_are_named() {
    let refused = Answer::of(Err(pcbcraft_core::Error::Unsupported("issues".into())));
    assert!(matches!(Summary::of(&refused), Summary::NotCounted(_)));
    assert_eq!(Summary::of(&refused).counts(), None);
    let counts = IssueCounts {
        error: 1,
        to_do: 3,
        ..Default::default()
    };
    let report = IssueReport {
        counts,
        elsewhere: vec![
            AreaCount {
                area: IssueArea::Component,
                counts,
                blocking: 2,
            },
            AreaCount {
                area: IssueArea::Schematic,
                counts,
                blocking: 0,
            },
        ],
        ..Default::default()
    };
    assert_eq!(
        Summary::from_report(&report),
        Summary::Counted {
            counts,
            elsewhere: Some("2 blocking in Component".into()),
        }
    );
}

/// The bar always counts the workspace, whatever scope the card lists.
#[test]
fn the_bar_counts_the_workspace_in_every_scope() -> Fallible<()> {
    let engine = Engine::demo()?;
    let board = report(
        &engine,
        IssueScope::Area {
            area: IssueArea::Board,
        },
    )?;
    let mut surface = Surface::default();
    for scope in [Scope::Workspace, Scope::All, Scope::Selection] {
        surface.scope = scope;
        assert_eq!(
            surface.summary(&engine, Workspace::Pcb).counts(),
            Some(board.counts)
        );
    }
    Ok(())
}

#[test]
fn browser_badges_mark_the_objects_of_every_issue() -> Fallible<()> {
    let engine = Engine::demo()?;
    let all = report(&engine, IssueScope::All)?;
    let badges = Badges::of(&engine, &all);
    let project = engine.project();
    for instance in &project.instances {
        let badge = badges.instances.get(&instance.id).ok_or("instance badge")?;
        assert!(badge.level <= Level::ToDo, "{}", instance.designator);
        assert!(!badge.tip().is_empty());
    }
    let board = badges.board.as_ref().ok_or("board badge")?;
    let board_rows = all
        .rows
        .iter()
        .filter(|r| r.area == IssueArea::Board)
        .count();
    assert_eq!(board.count, board_rows);
    assert!(!badges.nets.is_empty(), "unrouted nets are marked");
    Ok(())
}

#[test]
fn copy_report_lists_every_raw_finding() -> Fallible<()> {
    let engine = Engine::demo()?;
    let all = report(&engine, IssueScope::All)?;
    let text = report_text(&all);
    for row in &all.rows {
        assert!(text.contains(&row.title));
        for finding in std::iter::once(&row.finding).chain(&row.also) {
            assert!(text.contains(&finding.message), "{}", finding.message);
        }
    }
    let findings: usize = all.rows.iter().map(IssueRow::findings).sum();
    assert_eq!(text.lines().count(), all.rows.len() + findings + 1);
    Ok(())
}

/// One egui context driving the Issues card or pill.
struct Harness {
    ctx: egui::Context,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        Self { ctx, time: 0.0 }
    }

    fn frame<R>(
        &mut self,
        events: Vec<Event>,
        mut add: impl FnMut(&mut Ui) -> R,
    ) -> (R, Vec<(String, Rect)>) {
        self.time += 0.05;
        let mut result = None;
        let mut output = self.ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(420.0, 900.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| result = Some(add(ui)),
        );
        output.textures_delta.clear();
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
            match shape {
                egui::Shape::Text(t) => out.push((
                    t.galley.text().to_owned(),
                    t.galley.rect.translate(t.pos.to_vec2()),
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut painted = Vec::new();
        for shape in &output.shapes {
            walk(&shape.shape, &mut painted);
        }
        (result.expect("the frame ran"), painted)
    }

    fn click<R>(&mut self, at: Pos2, mut add: impl FnMut(&mut Ui) -> R) -> R {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(vec![Event::PointerMoved(at)], &mut add);
        self.frame(vec![button(true)], &mut add);
        self.frame(vec![button(false)], &mut add).0
    }
}

fn at(painted: &[(String, Rect)], text: &str) -> Option<Rect> {
    painted.iter().find(|(t, _)| t == text).map(|(_, r)| *r)
}

/// Expanded, the card lists the workspace's groups under the bar and the
/// coverage in its footer; a row click locates the row's objects in its
/// area and the pill then names it.
#[test]
fn the_card_lists_groups_and_a_row_click_locates_it() -> Fallible<()> {
    let engine = Engine::demo()?;
    let board = report(
        &engine,
        IssueScope::Area {
            area: IssueArea::Board,
        },
    )?;
    let first = groups(&board, None).into_iter().next().ok_or("a group")?;
    let mut surface = Surface::default();
    let mut h = Harness::new();
    let (_, collapsed) = h.frame(Vec::new(), |ui| {
        surface.card(ui, &engine, Workspace::Pcb, &[])
    });
    assert!(at(&collapsed, "Issues").is_some());
    assert!(
        at(&collapsed, &first.title).is_none(),
        "collapsed is only the bar"
    );

    surface.open();
    // Open the first group whatever its default, so its rows show.
    if !surface.is_open(&first) {
        surface.toggled.insert(first.key.clone());
    }
    let (_, painted) = h.frame(Vec::new(), |ui| {
        surface.card(ui, &engine, Workspace::Pcb, &[])
    });
    assert!(at(&painted, &first.title).is_some(), "{painted:?}");
    assert!(at(&painted, &board.coverage).is_some(), "{painted:?}");
    assert!(at(&painted, "Copy report").is_some());
    let row = &board.rows[first.rows[0]];
    let title = at(&painted, &row.title).ok_or("row title")?;
    let request = h.click(title.center(), |ui| {
        surface.card(ui, &engine, Workspace::Pcb, &[])
    });
    match request {
        Some(Request::Locate { refs, area, .. }) => {
            assert_eq!(area, IssueArea::Board);
            assert_eq!(refs, row.objects().copied().collect::<Vec<_>>());
        }
        other => panic!("expected locate, got {other:?}"),
    }
    let canvas = Rect::from_min_size(Pos2::ZERO, vec2(420.0, 300.0));
    let (_, painted) = h.frame(Vec::new(), |ui| {
        surface.pill(ui, canvas, &engine, Workspace::Pcb, &[])
    });
    let text = crate::refusal::clip(&format!("Showing: {}", row.title), 80);
    assert!(at(&painted, &text).is_some(), "{painted:?}");
    Ok(())
}

/// The pill's › locates the next listed issue and × ends the pill.
#[test]
fn the_pill_steps_to_the_next_issue_and_ends() -> Fallible<()> {
    let engine = Engine::demo()?;
    let board = report(&engine, IssueScope::All)?;
    let order = flat(&groups(&board, None));
    assert!(order.len() >= 2, "the demo has several issues");
    let mut surface = Surface {
        scope: Scope::All,
        ..Default::default()
    };
    surface.show(&board.rows[order[0]]);
    let canvas = Rect::from_min_size(Pos2::ZERO, vec2(420.0, 300.0));
    let mut h = Harness::new();
    let pill =
        |ui: &mut Ui, surface: &mut Surface| surface.pill(ui, canvas, &engine, Workspace::Pcb, &[]);
    let (_, painted) = h.frame(Vec::new(), |ui| pill(ui, &mut surface));
    let next = at(&painted, "›").ok_or("next")?;
    let request = h.click(next.center(), |ui| pill(ui, &mut surface));
    match request {
        Some(Request::Locate { refs, .. }) => {
            assert_eq!(
                refs,
                board.rows[order[1]].objects().copied().collect::<Vec<_>>()
            );
        }
        other => panic!("expected the next issue, got {other:?}"),
    }
    assert_eq!(
        surface.shown.as_ref().map(|s| s.key.clone()),
        Some(row_key(&board.rows[order[1]]))
    );
    let (_, painted) = h.frame(Vec::new(), |ui| pill(ui, &mut surface));
    let close = at(&painted, "×").ok_or("close")?;
    h.click(close.center(), |ui| pill(ui, &mut surface));
    assert!(surface.shown.is_none());
    Ok(())
}

#[test]
fn the_selection_scope_without_a_selection_says_so() -> Fallible<()> {
    let engine = Engine::demo()?;
    let mut surface = Surface::default();
    surface.show_selection();
    let mut h = Harness::new();
    let (_, painted) = h.frame(Vec::new(), |ui| {
        surface.card(ui, &engine, Workspace::Pcb, &[])
    });
    assert!(
        at(&painted, "Select an object to list its issues").is_some(),
        "{painted:?}"
    );
    Ok(())
}

/// Expanded in a column as narrow as at an 860 pt window, the card keeps to
/// the column: its scope and filter share the width instead of pushing the
/// card over the canvas.
#[test]
fn the_expanded_card_keeps_to_a_narrow_column() -> Fallible<()> {
    let engine = Engine::demo()?;
    let mut surface = Surface::default();
    surface.open();
    let column = Rect::from_min_size(pos2(100.0, 0.0), vec2(250.0, 700.0));
    let mut h = Harness::new();
    let (used, painted) = h.frame(Vec::new(), |ui| {
        let mut child = ui.new_child(UiBuilder::new().max_rect(column));
        child.set_max_width(column.width());
        surface.card(&mut child, &engine, Workspace::Pcb, &[]);
        child.min_rect()
    });
    assert!(at(&painted, "PCB only").is_some(), "{painted:?}");
    assert!(at(&painted, "Every kind").is_some(), "{painted:?}");
    assert!(
        used.left() >= column.left() - 0.5 && used.right() <= column.right() + 0.5,
        "the card took {used:?} in a column of {column:?}"
    );
    Ok(())
}

/// However long the shown issue's title, the pill keeps clear of the
/// badge's corner at the canvas's top right and keeps its × and arrows.
#[test]
fn the_pill_keeps_clear_of_the_badge() -> Fallible<()> {
    let engine = Engine::demo()?;
    let all = report(&engine, IssueScope::All)?;
    let longest = all
        .rows
        .iter()
        .max_by_key(|r| r.title.chars().count())
        .ok_or("an issue")?;
    let mut surface = Surface {
        scope: Scope::All,
        ..Default::default()
    };
    surface.show(longest);
    let canvas = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0));
    let mut h = Harness::new();
    let (_, painted) = h.frame(Vec::new(), |ui| {
        surface.pill(ui, canvas, &engine, Workspace::Pcb, &[])
    });
    let next = at(&painted, "›").ok_or("next")?;
    let previous = at(&painted, "‹").ok_or("previous")?;
    assert!(next.right() <= canvas.right() - BADGE_RESERVE, "{next:?}");
    assert!(previous.left() >= canvas.left(), "{previous:?}");
    let close = at(&painted, "×").ok_or("close")?;
    assert!(
        close.right() <= next.left(),
        "the × {close:?} sits inside the pill, before › {next:?}"
    );
    for (text, rect) in &painted {
        assert!(
            rect.right() <= canvas.right() - BADGE_RESERVE + 0.5,
            "{text} at {rect:?}"
        );
    }
    Ok(())
}

/// Every kind of object an issue can name locates: routed through
/// `issues::route` with no workspace preferred over the one that shows it,
/// it opens that workspace (and tab) and selects or frames the object.
#[test]
fn every_object_kind_locates_in_the_workspace_that_shows_it() -> Fallible<()> {
    use pcbcraft_core::layout::{LayoutCommand, RouteRequest};
    use pcbcraft_core::{Endpoint, Layer, Operation, TerminalId};
    let mut engine = Engine::demo()?;
    let r1 = engine
        .project()
        .instance_by_designator("R1")
        .ok_or("R1")?
        .id;
    let net = engine
        .project()
        .net_of(Endpoint {
            instance: r1,
            terminal: TerminalId(2),
        })
        .ok_or("R1 pin 2's net")?
        .id;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net,
                start: [9.2, 8.0],
                end: [14.8, 8.0],
                width: 0.3,
                layer: Layer::Top.into(),
            },
        }),
    )?;
    let project = engine.project();
    let track = project.tracks.first().ok_or("a track")?.id;
    let pad = pcbcraft_core::layout::BoardCopper::of(project)
        .pads()
        .iter()
        .find(|p| p.instance == r1)
        .ok_or("an R1 pad")?
        .pad;
    let occurrence = project.occurrences_of(r1).next().ok_or("R1's symbol")?;
    let set = crate::part_editor::WorkingSet::find(engine.drafts(), None).ok_or("a draft")?;
    let ids = set.ids;
    let terminal = set.component.terminals.first().ok_or("a terminal")?.id;
    let footprint_pad = set.footprint.pads.first().ok_or("a footprint pad")?.id;
    let package_terminal = set
        .package
        .terminals
        .first()
        .ok_or("a package terminal")?
        .id;
    let local = |definition, local| ObjectRef::LibraryObject { definition, local };
    // (object, workspace, tab or what is selected)
    let cases: Vec<(ObjectRef, Workspace, &str)> = vec![
        (ObjectRef::Instance { id: r1 }, Workspace::Schematic, "R1"),
        (
            ObjectRef::Terminal {
                endpoint: Endpoint {
                    instance: r1,
                    terminal: TerminalId(1),
                },
            },
            Workspace::Schematic,
            "R1",
        ),
        (
            ObjectRef::Occurrence { id: occurrence.id },
            Workspace::Schematic,
            "R1",
        ),
        (
            ObjectRef::Unit {
                instance: r1,
                unit: occurrence.unit,
            },
            Workspace::Schematic,
            "R1",
        ),
        (
            ObjectRef::Property {
                instance: r1,
                field: ids.component,
            },
            Workspace::Schematic,
            "R1",
        ),
        (ObjectRef::Net { id: net }, Workspace::Schematic, "net"),
        (ObjectRef::Pad { instance: r1, pad }, Workspace::Pcb, "R1"),
        (ObjectRef::Track { id: track }, Workspace::Pcb, "track"),
        (ObjectRef::Board, Workspace::Pcb, "board"),
        (
            ObjectRef::Definition { id: ids.component },
            Workspace::Part,
            "details",
        ),
        (
            ObjectRef::Definition { id: ids.symbol },
            Workspace::Part,
            "symbol",
        ),
        (local(ids.component, terminal.0), Workspace::Part, "symbol"),
        (
            ObjectRef::Definition { id: ids.footprint },
            Workspace::Part,
            "footprint",
        ),
        (
            local(ids.footprint, footprint_pad.0),
            Workspace::Part,
            "footprint",
        ),
        (
            ObjectRef::Definition { id: ids.binding },
            Workspace::Part,
            "mapping",
        ),
        (
            local(ids.package, package_terminal.0),
            Workspace::Part,
            "mapping",
        ),
        (
            ObjectRef::Definition { id: ids.package },
            Workspace::Part,
            "3d",
        ),
    ];
    for (object, expected, what) in cases {
        let mut part = crate::part_editor::PartEditor::default();
        let mut schematic = crate::schematic_editor::SchematicEditor::default();
        let mut pcb = crate::pcb_editor::PcbEditor::default();
        let shown = {
            let mut editors = Editors {
                part: &mut part,
                schematic: &mut schematic,
                pcb: &mut pcb,
            };
            route(&mut editors, &engine, &[object], expected)
        };
        assert_eq!(shown, Some(expected), "{object:?}");
        match (expected, what) {
            (Workspace::Schematic, "net") => {
                assert_eq!(schematic.selected_net(&engine), Some(net), "{object:?}");
            }
            (Workspace::Schematic, _) => {
                assert_eq!(schematic.selected_instance(), Some(r1), "{object:?}");
            }
            (Workspace::Pcb, "track") => assert_eq!(pcb.selected_track(), Some(track)),
            (Workspace::Pcb, "board") => {
                assert_eq!(pcb.selected_instance(), None);
                assert!(pcb.has_frame_request(), "the board is framed");
            }
            (Workspace::Pcb, _) => assert_eq!(pcb.selected_instance(), Some(r1), "{object:?}"),
            (Workspace::Part, tab) => {
                let (shown, selected) = part.located();
                assert_eq!(shown, tab, "{object:?}");
                // A definition is its tab's subject; its objects are selected.
                if matches!(object, ObjectRef::LibraryObject { .. }) {
                    assert!(selected, "{object:?}");
                }
            }
        }
    }
    Ok(())
}

/// A clearance issue between two parts selects both and frames them, at a
/// zoom that keeps the parts and their neighbours on screen.
#[test]
fn a_two_part_issue_selects_and_frames_both() -> Fallible<()> {
    let engine = Engine::demo()?;
    let project = engine.project();
    let id = |d: &str| {
        project
            .instance_by_designator(d)
            .map(|i| i.id)
            .ok_or_else(|| d.to_owned())
    };
    let (r1, r2) = (id("R1")?, id("R2")?);
    let mut pcb = crate::pcb_editor::PcbEditor::default();
    let refs = [
        ObjectRef::Instance { id: r1 },
        ObjectRef::Instance { id: r2 },
    ];
    assert!(Locate::locate(&mut pcb, &engine, &refs));
    assert_eq!(pcb.selected_instance(), Some(r1));
    assert!(pcb.is_located(r2), "the second part is selected too");
    Ok(())
}

/// e2e UX run 2 finding 1: an unconnected pin's row offers Mark no-connect,
/// one typed `instance.set_no_connect` that clears the issue and one undo
/// restores; a wired no-connect pin's row offers Clear no-connect.
#[test]
fn an_unconnected_pin_row_marks_it_no_connect_with_one_undo() -> Fallible<()> {
    use pcbcraft_core::instances::NewInstance;
    use pcbcraft_core::schematic::SchematicCommand;
    use pcbcraft_core::{Endpoint, InstanceCommand, UnitId};
    let mut engine = Engine::demo()?;
    let first = engine.project().instances[0].clone();
    let place = |engine: &mut Engine, name: &str, at: [f64; 2]| -> Fallible<Id> {
        engine.apply(
            engine.revision(),
            Operation::Instance(InstanceCommand::Place(NewInstance {
                sheet: None,
                component: first.component,
                designator: Some(name.into()),
                properties: vec![],
                representation: first.representation,
                unit: UnitId(1),
                position: at,
                physical: None,
            })),
        )?;
        Ok(engine
            .project()
            .instance_by_designator(name)
            .map(|i| i.id)
            .ok_or("placed")?)
    };
    let r90 = place(&mut engine, "R90", [0.0, -400.0])?;
    let pin = |id, t| Endpoint {
        instance: id,
        terminal: TerminalId(t),
    };
    let row_of = |engine: &Engine, code: &str| -> Fallible<Option<IssueRow>> {
        Ok(report(engine, IssueScope::All)?
            .rows
            .iter()
            .find(|r| {
                r.group.code == code
                    && r.objects().any(
                        |o| matches!(o, ObjectRef::Terminal { endpoint } if *endpoint == pin(r90, 1)),
                    )
            })
            .cloned())
    };
    let row = row_of(&engine, "instance.terminal_unaccounted")?.ok_or("no unconnected row")?;
    let Some(Fix::Apply {
        label, operation, ..
    }) = fix(&row)
    else {
        return Err("no Mark no-connect".into());
    };
    assert_eq!(label, "Mark no-connect");
    let before = engine.document().clone();
    engine.apply(engine.revision(), *operation)?;
    assert!(row_of(&engine, "instance.terminal_unaccounted")?.is_none());
    engine.undo()?;
    assert_eq!(engine.document(), &before);

    // Marked, then wired: the row clears the mark.
    let Some(Fix::Apply { operation, .. }) = fix(&row) else {
        return Err("no fix".into());
    };
    engine.apply(engine.revision(), *operation)?;
    let r91 = place(&mut engine, "R91", [40.0, -400.0])?;
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::Connect {
            from: pin(r90, 1).into(),
            to: pin(r91, 2).into(),
        }),
    )?;
    let row = row_of(&engine, "instance.no_connect_connected")?.ok_or("no connected row")?;
    let Some(Fix::Apply {
        label, operation, ..
    }) = fix(&row)
    else {
        return Err("no Clear no-connect".into());
    };
    assert_eq!(label, "Clear no-connect");
    engine.apply(engine.revision(), *operation)?;
    assert!(row_of(&engine, "instance.no_connect_connected")?.is_none());
    Ok(())
}
