//! The flows VM runs broke most often, driven as a person or the VM driver
//! drives them: through the accessibility tree ([`Ax`]), by role and name,
//! with real pointer and key events on the whole app's frame.
use super::ax_harness::{Ax, Fallible};
use super::*;
use egui::{Key, Modifiers, Role};
use sparksmith_core::{Point, schematic};

/// Where the sheet point `p` is on the schematic canvas now, in the part
/// no docked card covers.
fn on_sheet(ax: &Ax, p: Point) -> Fallible<egui::Pos2> {
    ax.find(Role::Canvas, "Schematic")?;
    let clear = ax.app.schematic.clear_rect();
    let at = ax.app.schematic.screen_point(p);
    if clear.contains(at) {
        Ok(at)
    } else {
        Err(format!("{p:?} is at {at:?}, off the clear canvas {clear:?}").into())
    }
}

/// A point on the demo's one wire, half way along it.
fn wire_middle(ax: &Ax) -> Fallible<(sparksmith_core::Id, egui::Pos2)> {
    let project = ax.app.engine.project();
    let wire = project.wires.first().ok_or("the demo has a wire")?;
    let (a, b) = schematic::wire_ends(project, wire).ok_or("wire ends")?;
    let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let on = schematic::nearest_on_wire(project, wire.id, mid).ok_or("on the wire")?;
    Ok((wire.id, on_sheet(ax, on)?))
}

/// R1's body on the demo sheet, 12 mm left of the origin.
const R1_BODY: Point = [-12.0, 0.0];

/// The in-window File menu opens from its button, lists its rows as
/// buttons, closes on Esc without running anything, and its Save row
/// saves.
#[test]
fn the_file_menu_opens_closes_on_esc_and_saves() -> Fallible {
    let mut ax = Ax::new(Workspace::Schematic)?;
    ax.button("File")?;
    for row in ["New project", "Open…", "Save", "Save as…", "Quit"] {
        assert!(ax.has(Role::Button, row), "{row} in:\n{}", ax.dump());
    }
    ax.key(Key::Escape);
    assert!(!ax.has(Role::Button, "Save as…"), "Esc closes the menu");
    assert!(!ax.path().exists(), "Esc ran nothing");
    ax.button("File")?;
    ax.button("Save")?;
    ax.until("the save", |ax| !ax.app.files.busy() && ax.path().exists())?;
    assert!(!ax.has(Role::Button, "Save as…"), "a chosen row closes it");
    let saved = sparksmith_io::read_project(&ax.path())?;
    assert_eq!(&saved.project, ax.app.engine.project(), "the design saved");
    Ok(())
}

/// A right-click on a wire opens its menu with its rows; Esc closes it
/// and changes nothing; its Delete… opens the disconnect review, which
/// removes the wire only when applied.
#[test]
fn a_wires_context_menu_closes_on_esc_and_deletes_through_the_review() -> Fallible {
    let mut ax = Ax::new(Workspace::Schematic)?;
    let before = ax.app.engine.revision();
    let (wire, at) = wire_middle(&ax)?;
    ax.right_click_at(at);
    ax.find(Role::Button, "Add junction here")?;
    ax.find(Role::Button, "Delete…")?;
    ax.key(Key::Escape);
    assert!(!ax.has(Role::Button, "Add junction here"), "Esc closes it");
    assert_eq!(ax.app.engine.revision(), before, "Esc changed the design");
    assert!(!ax.app.schematic.reviewing());

    ax.right_click_at(at);
    ax.button("Delete…")?;
    assert!(ax.app.schematic.reviewing(), "Delete… opens the review");
    assert_eq!(ax.app.engine.revision(), before, "nothing until applied");
    ax.button("Apply update")?;
    ax.settle();
    assert!(ax.app.engine.project().wire(wire).is_none(), "applied");
    assert!(!ax.app.schematic.reviewing());
    Ok(())
}

/// Delete and Backspace on a clicked wire open the disconnect review; the
/// review's Cancel leaves the wire.
#[test]
fn delete_on_a_selected_wire_opens_the_review() -> Fallible {
    for key in [Key::Delete, Key::Backspace] {
        let mut ax = Ax::new(Workspace::Schematic)?;
        let before = ax.app.engine.revision();
        let (wire, at) = wire_middle(&ax)?;
        ax.click_at(at);
        ax.key(key);
        assert!(ax.app.schematic.reviewing(), "{key:?} opens the review");
        assert_eq!(ax.app.engine.revision(), before, "{key:?} changed it");
        ax.button("Cancel")?;
        ax.settle();
        assert!(!ax.app.schematic.reviewing(), "{key:?}: Cancel closes it");
        assert!(ax.app.engine.project().wire(wire).is_some());
        assert_eq!(ax.app.engine.revision(), before);
    }
    Ok(())
}

/// Delete… on a part opens the removal review titled for it; its primary
/// removes the part in one undo step.
#[test]
fn the_removal_review_removes_the_part_once_applied() -> Fallible {
    let mut ax = Ax::new(Workspace::Schematic)?;
    let r1 = ax.app.engine.project().instances[0].id;
    let before = ax.app.engine.revision();
    let body = on_sheet(&ax, R1_BODY)?;
    ax.right_click_at(body);
    ax.button("Delete…")?;
    assert!(ax.app.schematic.reviewing());
    assert_eq!(ax.app.engine.revision(), before);
    ax.button("Remove R1")?;
    ax.settle();
    assert!(ax.app.engine.project().instance(r1).is_none(), "removed");
    assert!(!ax.app.schematic.reviewing());
    ax.command_key(Key::Z);
    assert!(ax.app.engine.project().instance(r1).is_some(), "one undo");
    Ok(())
}

/// ⌘S with a card field holding the keyboard saves what was typed, once.
#[test]
fn command_s_from_a_focused_field_saves_the_typed_text() -> Fallible {
    let mut ax = Ax::new(Workspace::Schematic)?;
    ax.click(Role::TextInput, "Title")?;
    assert!(ax.ctx.text_edit_focused(), "the field has the keyboard");
    ax.key_with(Key::A, Modifiers::COMMAND);
    ax.type_text("FIELD SAVE");
    ax.command_key(Key::S);
    ax.until("the save", |ax| !ax.app.files.busy() && ax.path().exists())?;
    let saved = sparksmith_io::read_project(&ax.path())?;
    assert_eq!(
        saved.project.properties.title, "FIELD SAVE",
        "the file holds the old value"
    );
    assert!(!ax.app.files.dirty(), "the unsaved dot stayed");
    Ok(())
}

/// The Place button opens the palette with its search field; a generic
/// part the project does not hold yet is listed for its words, a click on
/// its row arms it and a click on the sheet creates and places it.
#[test]
fn the_place_palette_places_a_generic_part() -> Fallible {
    let mut ax = Ax::new(Workspace::Schematic)?;
    let count = ax.app.engine.project().instances.len();
    let components = ax.app.engine.project().library.components.len();
    ax.button("Place")?;
    ax.settle();
    assert!(ax.ctx.text_edit_focused(), "the search has the keyboard");
    ax.type_text("capacitor 0603");
    ax.settle();
    ax.find(
        Role::Label,
        "Not a component yet: choosing it creates one, then you place it",
    )?;
    ax.button("Chip capacitor 0603 (1608 metric)")?;
    ax.settle();
    let free = on_sheet(&ax, [20.0, -30.0])?;
    ax.click_at(free);
    ax.settle();
    assert_eq!(
        ax.app.engine.project().instances.len(),
        count + 1,
        "placed:\n{}",
        ax.dump()
    );
    assert_eq!(
        ax.app.engine.project().library.components.len(),
        components + 1,
        "the generic part was created"
    );
    Ok(())
}

/// The Wire button arms the tool; its card's Arc box rounds the corners
/// of the wire drawn by clicks, ended with Enter.
#[test]
fn the_wire_tool_draws_arc_corners() -> Fallible {
    let mut ax = Ax::new(Workspace::Schematic)?;
    let count = ax.app.engine.project().wires.len();
    ax.button("Wire")?;
    let arc = ax.find_prefix(Role::CheckBox, "Arc")?;
    assert_eq!(arc.toggled, Some(false), "sharp by default");
    ax.click_at(arc.center());
    assert_eq!(ax.find_prefix(Role::CheckBox, "Arc")?.toggled, Some(true));
    // Empty sheet below the demo's two resistors.
    for p in [[-30.0, -15.0], [0.0, -15.0], [0.0, -25.0]] {
        let at = on_sheet(&ax, p)?;
        ax.click_at(at);
    }
    ax.key(Key::Enter);
    ax.settle();
    let wires = &ax.app.engine.project().wires;
    assert_eq!(wires.len(), count + 1, "one wire drawn");
    let drawn = wires.last().ok_or("wire")?;
    assert!(!drawn.corners.is_empty(), "it has a corner: {drawn:?}");
    assert!(
        drawn.corners.iter().all(|c| c.radius_mm() > 0.0),
        "rounded: {:?}",
        drawn.corners
    );
    Ok(())
}

/// Update PCB runs on the first press: its button on the PCB workspace,
/// and ⇧⌘U from the schematic straight after a workspace change.
#[test]
fn update_pcb_opens_its_review_on_the_first_press() -> Fallible {
    fn pending(ax: &mut Ax) -> Fallible {
        let first = ax.app.engine.project().instances[0].clone();
        ax.app.engine.apply(
            ax.app.engine.revision(),
            Operation::Instance(sparksmith_core::InstanceCommand::Place(
                sparksmith_core::instances::NewInstance {
                    sheet: None,
                    component: first.component,
                    designator: Some("R77".into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: sparksmith_core::UnitId(1),
                    position: crate::schematic_editor::pt([30.0, -150.0]),
                    physical: first.physical,
                },
            )),
        )?;
        ax.settle();
        Ok(())
    }
    let mut ax = Ax::new(Workspace::Pcb)?;
    pending(&mut ax)?;
    ax.button("Update PCB")?;
    assert!(ax.app.pcb.reviewing(), "one click opens the review");

    let mut ax = Ax::new(Workspace::Pcb)?;
    pending(&mut ax)?;
    ax.command_key(Key::Num2);
    assert_eq!(ax.app.workspace, Workspace::Schematic);
    ax.key_with(Key::U, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(ax.app.workspace, Workspace::Pcb);
    assert!(ax.app.pcb.reviewing(), "one ⇧⌘U opens the review");
    Ok(())
}

/// The canvases and the fields of a card are named for assistive tools.
#[test]
fn canvases_and_card_fields_have_names() -> Fallible {
    let ax = Ax::new(Workspace::Schematic)?;
    ax.find(Role::Canvas, "Schematic")?;
    for field in ["Name", "Title", "Revision", "Author"] {
        ax.find(Role::TextInput, field)?;
    }
    let ax = Ax::new(Workspace::Pcb)?;
    ax.find(Role::Canvas, "Board")?;
    ax.find(Role::TextInput, "Project")?;
    Ok(())
}

/// Demo run item 12: a ribbon tab is drawn before its click is handled, so
/// the frame of the click still shows the old tab chosen; the click asks
/// for the next frame, which shows the new one without waiting for input.
#[test]
fn a_ribbon_tab_click_redraws_the_ribbon_in_its_new_state() -> Fallible {
    let mut ax = Ax::new(Workspace::Pcb)?;
    ax.app.pcb.set_3d(true);
    ax.settle();
    let tab = ax.find(Role::Button, "Layout")?;
    assert_eq!(tab.toggled, Some(false));
    ax.hover(tab.center());
    let event = |pressed| egui::Event::PointerButton {
        pos: tab.center(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    ax.frame(vec![event(true)]);
    ax.frame(vec![event(false)]);
    assert!(!ax.app.pcb.is_3d(), "the click chose Layout");
    assert_eq!(ax.find(Role::Button, "Layout")?.toggled, Some(false));
    assert!(
        ax.ctx.has_requested_repaint(),
        "the ribbon waits for input to show Layout chosen"
    );
    ax.frame(vec![]);
    assert_eq!(ax.find(Role::Button, "Layout")?.toggled, Some(true));
    Ok(())
}

/// Whether `engine` holds an empty project: no parts, library, drafts,
/// wires or board objects, and no issues in any area.
fn assert_empty(engine: &Engine, what: &str) -> Fallible {
    let project = engine.project();
    assert!(project.instances.is_empty(), "{what}: instances");
    assert!(project.placements.is_empty(), "{what}: placements");
    assert!(project.wires.is_empty(), "{what}: wires");
    assert!(project.nets.is_empty(), "{what}: nets");
    assert!(
        project.tracks.is_empty() && project.vias.is_empty(),
        "{what}: copper"
    );
    assert_eq!(
        project.library,
        sparksmith_core::library::Library::default(),
        "{what}: library"
    );
    assert!(engine.drafts().items.is_empty(), "{what}: library drafts");
    let sparksmith_core::QueryResult::Issues(report) =
        engine.query(sparksmith_core::Query::Issues {
            scope: sparksmith_core::issues::IssueScope::All,
        })?
    else {
        return Err("the issues query answered something else".into());
    };
    assert!(report.rows.is_empty(), "{what}: issues {:?}", report.rows);
    Ok(())
}

/// A launch without a file opens an empty project with no issues; there
/// is no built-in sample design.
#[test]
fn a_launch_without_a_file_opens_an_empty_project() -> Fallible {
    let ax = Ax::with_engine(file_ui::new_project()?, Workspace::Schematic)?;
    assert_empty(&ax.app.engine, "launch")
}

/// A launch shows the empty project's whole page framed, not zoomed into
/// its top-left corner, where the first part landed on the border
/// (docs/e2e-run-4.md, finding 10).
#[test]
fn a_launch_frames_the_whole_empty_page() -> Fallible {
    let mut ax = Ax::with_engine(file_ui::new_project()?, Workspace::Schematic)?;
    ax.frame(vec![]);
    let project = ax.app.engine.project();
    let sheet = project.sheets.list.first().ok_or("a first sheet")?;
    let ([w, h], [x, y]) = (sheet.size_mm(), sheet.top_left());
    let (a, b) = ([x, y], [x + w, y - h]);
    let (a, b) = (on_sheet(&ax, a)?, on_sheet(&ax, b)?);
    let canvas = ax.app.schematic.clear_rect();
    // Fitted: the page fills the clear canvas's width or its height.
    assert!(
        (b.x - a.x) > canvas.width() * 0.8 || (b.y - a.y) > canvas.height() * 0.6,
        "page {a:?}–{b:?} in {canvas:?}"
    );
    Ok(())
}

/// Run 4, finding 6: the schematic runs under the docked Browser, which
/// floats over it, while Fit keeps the page in the part it leaves clear. A
/// part panned under the Browser's column, below its card, is drawn and
/// picked there.
#[test]
fn the_schematic_runs_under_the_docked_browser() -> Fallible {
    let mut ax = Ax::new(Workspace::Schematic)?;
    assert!(ax.app.browser_visible, "the Browser docks at this width");
    let canvas = ax.find(Role::Canvas, "Schematic")?.rect;
    let clear = ax.app.schematic.clear_rect();
    let card = ax
        .ctx
        .memory(|m| m.area_rect(egui::Id::unique("project-browser")))
        .ok_or("the Browser card")?;
    assert!(
        canvas.left() < card.left() && clear.left() >= card.right(),
        "canvas {canvas:?}, clear {clear:?}, Browser {card:?}"
    );
    // Middle-drag R1's body under the Browser's column, below its card.
    let to = egui::pos2(card.center().x, (card.bottom() + canvas.bottom()) / 2.0);
    assert!(to.y > card.bottom() + 20.0, "room below the card: {card:?}");
    let from = on_sheet(&ax, R1_BODY)?;
    let middle = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Middle,
        pressed,
        modifiers: Modifiers::NONE,
    };
    ax.hover(from);
    ax.frame(vec![middle(from, true)]);
    ax.frame(vec![egui::Event::PointerMoved(to)]);
    ax.frame(vec![middle(to, false)]);
    let at = ax.app.schematic.screen_point(R1_BODY);
    assert!((at - to).length() < 1.0, "R1 at {at:?}, dragged to {to:?}");
    ax.click_at(at);
    let r1 = ax.app.engine.project().instances[0].id;
    assert_eq!(ax.app.schematic.selected_instance(), Some(r1));
    Ok(())
}

/// The board's 2D canvas runs under the docked cards as the schematic
/// does.
#[test]
fn the_board_runs_under_the_docked_browser() -> Fallible {
    let ax = Ax::new(Workspace::Pcb)?;
    let canvas = ax.find(Role::Canvas, "Board")?.rect;
    let card = ax
        .ctx
        .memory(|m| m.area_rect(egui::Id::unique("project-browser")))
        .ok_or("the Browser card")?;
    assert!(
        canvas.left() < card.left(),
        "canvas {canvas:?}, Browser {card:?}"
    );
    Ok(())
}

/// File › New replaces a design with the same empty project a launch
/// opens.
#[test]
fn file_new_opens_the_same_empty_project() -> Fallible {
    let mut ax = Ax::new(Workspace::Schematic)?;
    assert!(
        !ax.app.engine.project().instances.is_empty(),
        "a design to replace"
    );
    ax.command_key(Key::N);
    ax.until("the new project", |ax| {
        ax.app.engine.project().instances.is_empty()
    })?;
    assert_empty(&ax.app.engine, "File › New")?;
    assert_eq!(ax.app.engine.to_file(), file_ui::new_project()?.to_file());
    Ok(())
}

/// Run 4, finding 1: File › New without a default fab profile (Settings)
/// asks for the board's profile on the PCB, JLCPCB 2-layer 1 oz
/// preselected; one click on Use profile sets it. An opened file is not
/// asked.
#[test]
fn a_new_project_asks_for_its_fab_profile_on_the_pcb() -> Fallible {
    let mut ax = Ax::new(Workspace::Pcb)?;
    assert!(!ax.has(Role::Button, "Use profile"), "an opened design");
    // The document File › New opens for a user without a default.
    ax.app.replace(Engine::default().to_file(), None, false);
    ax.settle();
    ax.button("Use profile")?;
    ax.until("the profile", |ax| {
        ax.app.engine.project().rules.profile.is_some()
    })?;
    let profile = ax.app.engine.project().rules.profile.as_ref();
    assert_eq!(
        profile.map(|p| p.id.as_str()),
        Some(sparksmith_io::rule_profiles::DEFAULT_BUILTIN)
    );
    ax.settle();
    assert!(!ax.has(Role::Button, "Use profile"), "asked once");
    Ok(())
}

/// The board selection filter is kept per user: a kind turned off in its
/// popover stays off in the next session.
#[test]
fn the_selection_filter_persists_per_user() -> Fallible {
    let config = tempfile::tempdir()?;
    let dirs = sparksmith_io::settings::UserDirs {
        config: config.path().join("config"),
        cache: config.path().join("cache"),
    };
    let sources = LibrarySources::new(Some(dirs.clone()));
    let mut ax = Ax::with_sources(Engine::demo()?, Workspace::Pcb, sources)?;
    ax.button("Selection filter")?;
    ax.click(Role::CheckBox, "Tracks")?;
    ax.settle();
    assert!(!ax.app.pcb.filter().tracks, "{}", ax.dump());
    ax.app.sources.wait()?;
    let again = Ax::with_sources(
        Engine::demo()?,
        Workspace::Pcb,
        LibrarySources::new(Some(dirs)),
    )?;
    let kept = again.app.pcb.filter();
    assert!(
        !kept.tracks && kept.parts && kept.vias && kept.fills,
        "{kept:?}"
    );
    Ok(())
}

/// The first issue row the Board's Issues card lists that offers Waive…:
/// one that blocks no stage.
fn waivable_board_row(engine: &Engine) -> Fallible<sparksmith_core::issues::IssueRow> {
    use sparksmith_core::issues::{IssueArea, IssueScope, Severity};
    let sparksmith_core::QueryResult::Issues(report) =
        engine.query(sparksmith_core::Query::Issues {
            scope: IssueScope::Area {
                area: IssueArea::Board,
            },
        })?
    else {
        return Err("the issues query answered something else".into());
    };
    report
        .rows
        .into_iter()
        .find(|r| {
            r.severity != Severity::Info
                && r.blocks().is_none()
                && !sparksmith_core::fabrication::preflight::NO_WAIVER
                    .contains(&r.finding.code.as_str())
        })
        .ok_or_else(|| "the demo board has no waivable issue".into())
}

/// Inline waive, as a person does it: Waive… on a row opens a reason
/// field in the list; with Settings › Production "Require a reason" on,
/// Enter on an empty reason is refused under the field and changes
/// nothing; a reason and Enter waive every finding of the row in one undo
/// step, the row moves to Waived, and one undo brings it back.
#[test]
fn an_issue_is_waived_inline_with_a_reason_and_one_undo_restores_it() -> Fallible {
    let mut ax = Ax::new(Workspace::Pcb)?;
    ax.app.engine.require_waiver_reason(true);
    let row = waivable_board_row(&ax.app.engine)?;
    let keys: Vec<String> = std::iter::once(&row.finding)
        .chain(&row.also)
        .map(|f| sparksmith_core::fabrication::preflight::key(&f.code, &f.objects))
        .collect();
    let before = ax.app.engine.document().clone();
    let bar = ax.find_prefix(Role::Button, "Issues:")?;
    ax.click_at(bar.center());
    ax.settle();
    // Its group may start folded (to do): unfold it.
    if !ax.has(Role::TreeItem, &row.title) {
        let header = ax.find_prefix(Role::TreeItem, &format!("{}, ", row.group.title))?;
        ax.click_at(header.center());
        ax.settle();
    }
    ax.click(Role::Button, &format!("Waive: {}", row.title))?;
    ax.settle();
    let field = format!("Reason to waive {}", row.title);
    ax.find(Role::TextInput, &field)?;
    ax.key(Key::Enter);
    ax.settle();
    assert_eq!(
        ax.app.engine.document(),
        &before,
        "the refused waive changed nothing"
    );
    assert!(
        ax.texts.iter().any(|t| t.contains("reason is required")),
        "the refusal shows under the field: {:?}",
        ax.texts
    );
    ax.find(Role::TextInput, &field)?;
    let reason = "Accepted for the prototype run";
    ax.type_text(reason);
    ax.key(Key::Enter);
    ax.settle();
    let waivers = &ax.app.engine.project().production.waivers;
    for key in &keys {
        assert!(
            waivers.iter().any(|w| &w.key == key && w.reason == reason),
            "{key} waived: {waivers:?}"
        );
    }
    assert!(!ax.has(Role::TextInput, &field), "the field closed");
    assert!(
        !ax.all_prefix(Role::TreeItem, "Waived").is_empty(),
        "the Waived group shows:\n{}",
        ax.dump()
    );
    ax.command_key(Key::Z);
    ax.settle();
    assert_eq!(ax.app.engine.document(), &before, "one undo restores it");
    Ok(())
}

/// Run 4, finding 15: at 1558 x 889 with a track selected and Issues
/// expanded over a long list, ticking a production group opens its reason
/// form inside the docked dialog's visible body, clear of the footer, and
/// "Waive n findings" waives that group with the dialog still open.
#[test]
fn the_production_waive_form_shows_beside_a_long_issues_list() -> Fallible {
    let mut engine = Engine::showcase()?;
    let tracks: Vec<_> = engine
        .project()
        .tracks
        .iter()
        .map(|t| (t.id, t.net))
        .collect();
    // Every third track removed: unrouted nets and dangling ends, two
    // groups of findings and a long Issues list.
    for (id, _) in tracks.iter().skip(1).step_by(3) {
        engine.apply(
            engine.revision(),
            sparksmith_core::Operation::Layout(
                sparksmith_core::layout::LayoutCommand::DeleteTrack { id: *id },
            ),
        )?;
    }
    let mut ax = Ax::with_engine(engine, Workspace::Pcb)?;
    ax.window = egui::vec2(1558.0, 889.0);
    let (track, net) = *tracks.first().ok_or("the showcase has tracks")?;
    ax.app.pcb.set_selection(None, Some(track), Some(net));
    ax.settle();
    let bar = ax.find_prefix(Role::Button, "Issues:")?;
    ax.click_at(bar.center());
    ax.settle();
    ax.button("Production")?;
    ax.until("the production check", |ax| {
        ax.has(Role::CheckBox, "Track ends joining nothing")
    })?;
    ax.click(Role::CheckBox, "Track ends joining nothing")?;
    ax.settle();
    let header = ax.find(Role::Label, "Production files")?;
    let footer = ax.find(Role::Button, "Export")?;
    let reason = ax.find(Role::TextInput, "Reason")?;
    let waive = ax
        .all_prefix(Role::Button, "Waive ")
        .into_iter()
        .find(|n| n.name.ends_with("findings"))
        .ok_or_else(|| format!("no Waive button:\n{}", ax.dump()))?
        .clone();
    let row = sparksmith_ui::theme::Tokens::get().control_height;
    assert!(
        header.rect.bottom() < reason.rect.top() && waive.rect.bottom() + row <= footer.rect.top(),
        "the form shows in the body, clear of the footer: {:?} {:?} {:?}\n{}",
        reason.rect,
        waive.rect,
        footer.rect,
        ax.dump()
    );
    ax.click_at(waive.center());
    ax.settle();
    assert!(ax.app.pcb.production_open(), "{}", ax.dump());
    let waivers = &ax.app.engine.project().production.waivers;
    assert!(
        waivers
            .iter()
            .any(|w| w.key.starts_with("board.track_dangling")),
        "the group is waived: {waivers:?}"
    );
    Ok(())
}
