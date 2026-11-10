//! The flows VM runs broke most often, driven as a person or the VM driver
//! drives them: through the accessibility tree ([`Ax`]), by role and name,
//! with real pointer and key events on the whole app's frame.
use super::ax_harness::{Ax, Fallible};
use super::*;
use egui::{Key, Modifiers, Role};
use pcbcraft_core::{Point, schematic};

/// Where the sheet point `p` is on the schematic canvas now.
fn on_sheet(ax: &Ax, p: Point) -> Fallible<egui::Pos2> {
    let canvas = ax.find(Role::Canvas, "Schematic")?;
    let at = ax.app.schematic.screen_point(canvas.rect, p);
    if canvas.rect.contains(at) {
        Ok(at)
    } else {
        Err(format!("{p:?} is at {at:?}, off the canvas {:?}", canvas.rect).into())
    }
}

/// A point on the demo's one wire, half way along it.
fn wire_middle(ax: &Ax) -> Fallible<(pcbcraft_core::Id, egui::Pos2)> {
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
    let saved = pcbcraft_io::read_project(&ax.path())?;
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
    let saved = pcbcraft_io::read_project(&ax.path())?;
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
            Operation::Instance(pcbcraft_core::InstanceCommand::Place(
                pcbcraft_core::instances::NewInstance {
                    sheet: None,
                    component: first.component,
                    designator: Some("R77".into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: pcbcraft_core::UnitId(1),
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
