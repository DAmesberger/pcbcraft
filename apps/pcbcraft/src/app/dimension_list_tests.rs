//! The Dimensions list of the part Properties card, through the whole
//! app's frame and its accessibility tree, as a person and a screen reader
//! use it: on the ESP32-S31 dev board with J1 selected in PCB 3D, each of
//! J1's dimensions is one compact row ("Front face → bottom edge" and its
//! value); a driven value reads `fx` and its result inline; the value
//! commits as one `layout.dimension_edit`; the row's delete icon removes
//! the dimension in one undo step; and hovering or selecting a row lights
//! its dimension in 3D, as selecting it in 3D selects its row.
use super::ax_harness::{Ax, Fallible};
use super::*;
use egui::accesskit::Role;
use pcbcraft_core::library::footprint_source::DimValue;
use pcbcraft_core::model::dimension::{BoardRef, FeatureRef};

fn esp32() -> Fallible<Engine> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    Ok(Engine::from_file(pcbcraft_io::read_project(&path)?)?)
}

/// Frames until the board's model is shown.
fn until_shown(ax: &mut Ax) -> Fallible {
    for _ in 0..24_000 {
        ax.frame(vec![]);
        let key = crate::scene::ModelKey::Project(Arc::clone(ax.app.engine.project_arc()));
        if let crate::scene::Status::Failed(e) = ax.app.scene.status() {
            return Err(e.clone().into());
        }
        if ax.app.scene.display(&key) == crate::scene::Display::Current {
            ax.frame(vec![]);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Err("the board's model was never shown".into())
}

const FACE: &str = "Front face \u{2192} bottom edge";
const ORIGIN: &str = "Origin \u{2192} left edge";

#[test]
fn j1s_dimension_rows_edit_delete_and_follow_3d() -> Fallible {
    let engine = esp32()?;
    let project = engine.project();
    let j1 = project
        .instances
        .iter()
        .find(|i| i.designator == "J1")
        .map(|i| i.id)
        .ok_or("no J1")?;
    let is_face = |d: &&pcbcraft_core::model::dimension::PartDimension| {
        d.refs.iter().any(
            |r| matches!(r, BoardRef::Part(FeatureRef::Face { placement, .. }) if *placement == j1),
        )
    };
    let face = project
        .part_dimensions
        .iter()
        .find(is_face)
        .ok_or("J1's face dimension")?
        .id;
    let origin = project
        .part_dimensions
        .iter()
        .find(|d| d.id != face && !is_face(d))
        .ok_or("J1's origin dimension")?
        .clone();
    let Some(DimValue::Expr(expr)) = &origin.value else {
        return Err(format!("the origin dimension is driven: {:?}", origin.value).into());
    };
    let expr = expr.0.clone();
    let placement = project.placement_of(j1).ok_or("J1 placed")?.clone();

    let mut ax = Ax::with_engine(engine, Workspace::Pcb)?;
    ax.app.pcb.set_3d(true);
    until_shown(&mut ax)?;
    ax.app.pcb.set_selection(Some(j1), None, None);
    let items = ax.app.scene.selected_items(&crate::pcb_editor::Group {
        parts: [j1].into(),
        ..Default::default()
    });
    ax.app
        .scene
        .viewport
        .view_from(pcbcraft_solidcraft::geom::Vec3::new(0.15, -1.0, 0.6));
    assert!(ax.app.scene.viewport.fit_items(&items));
    ax.settle();

    // One compact row each: the name on one line, half the height the
    // name, value and Delete rows took.
    let face_row = ax.find(Role::ListBoxOption, FACE)?;
    let origin_row = ax.find(Role::ListBoxOption, ORIGIN)?;
    let t = solidcraft_theme::Tokens::get();
    for row in [&face_row, &origin_row] {
        assert!(
            row.rect.height() <= t.control_height + t.space_4 + 0.5,
            "{row:?}"
        );
        assert_eq!(row.toggled, Some(false), "{row:?}");
    }
    assert!(
        (origin_row.rect.top() - face_row.rect.bottom()).abs() < 1.0
            || (face_row.rect.top() - origin_row.rect.bottom()).abs() < 1.0,
        "the rows follow each other: {face_row:?} {origin_row:?}"
    );
    // Each value is read-only text until edited; the driven one reads `fx`
    // and its result, decimal-aligned with a dim unit (with its expression
    // where the column has room), and the whole on hover.
    assert!(
        ax.texts.iter().any(|s| s == "fx") && ax.texts.iter().any(|s| s == "mm"),
        "fx and a unit: {:?}",
        ax.texts
    );
    assert!(!ax.has(Role::TextInput, FACE), "no field until edited");
    let whole = format!("{expr} = ");
    // The pointer rests on the value until its tooltip shows.
    let driven_value = egui::pos2(origin_row.rect.right() - 60.0, origin_row.rect.center().y);
    ax.hover(driven_value);
    for _ in 0..4 {
        ax.frame(vec![]);
    }
    assert!(
        ax.texts
            .iter()
            .any(|s| s.starts_with(&whole) && s.ends_with(" mm")),
        "{whole}…: {:?}",
        ax.texts
    );
    // Each delete is in the tree before the pointer comes, for the
    // keyboard and screen readers, named by what it deletes.
    let delete = format!("Delete dimension {FACE}");
    ax.find(Role::Button, &delete)?;

    // Hovering a row lights its dimension in 3D; leaving it does not.
    assert_eq!(ax.app.pcb.dimensions_lit(), vec![origin.id], "its field");
    ax.hover(egui::pos2(4.0, 450.0));
    ax.frame(vec![]);
    assert!(ax.app.pcb.dimensions_lit().is_empty());
    let name_at = egui::pos2(face_row.rect.left() + 12.0, face_row.rect.center().y);
    ax.hover(name_at);
    ax.frame(vec![]);
    assert_eq!(ax.app.pcb.dimensions_lit(), vec![face]);
    ax.hover(egui::pos2(4.0, 450.0));
    ax.frame(vec![]);
    assert!(ax.app.pcb.dimensions_lit().is_empty());

    // A click on a row selects its dimension, lit in 3D.
    let origin_at = egui::pos2(origin_row.rect.left() + 12.0, origin_row.rect.center().y);
    ax.click_at(origin_at);
    assert_eq!(ax.app.pcb.selected_dimension(), Some(origin.id));
    assert_eq!(ax.find(Role::ListBoxOption, ORIGIN)?.toggled, Some(true));
    assert_eq!(ax.find(Role::ListBoxOption, FACE)?.toggled, Some(false));
    ax.hover(egui::pos2(4.0, 450.0));
    assert_eq!(ax.app.pcb.dimensions_lit(), vec![origin.id]);
    // Selecting the face dimension in 3D (a click on its label in the
    // view) selects its row instead.
    let view = ax.app.scene.viewport.rect().ok_or("the 3D view")?;
    let label = ax
        .app
        .pcb
        .dimensions_drawn()
        .into_iter()
        .find(|d| d.0 == face)
        .map(|d| d.2)
        .filter(|r| view.contains_rect(*r))
        .ok_or("the face dimension's label is in the view")?;
    ax.click_at(label.center());
    assert_eq!(ax.app.pcb.selected_dimension(), Some(face));
    assert_eq!(ax.find(Role::ListBoxOption, FACE)?.toggled, Some(true));
    assert_eq!(ax.find(Role::ListBoxOption, ORIGIN)?.toggled, Some(false));
    assert_eq!(ax.app.pcb.dimensions_lit(), vec![face]);

    // The value commits as one edit of the same dimension, which moves J1;
    // one undo puts both back.
    // Enter on the selected row opens its value in place, all selected.
    let revision = ax.app.engine.revision();
    let face_row = ax.find(Role::ListBoxOption, FACE)?;
    ax.click_at(egui::pos2(
        face_row.rect.left() + 30.0,
        face_row.rect.center().y,
    ));
    ax.key(egui::Key::Enter);
    ax.settle();
    ax.find(Role::TextInput, FACE)?;
    ax.type_text("1 mm");
    ax.key(egui::Key::Enter);
    ax.settle();
    assert!(!ax.has(Role::TextInput, FACE), "the edit closed");
    let project = ax.app.engine.project();
    let edited = project
        .part_dimensions
        .iter()
        .find(|d| d.id == face)
        .ok_or("the dimension keeps its id")?;
    assert_eq!(edited.value, Some(DimValue::Fixed(1.0)));
    assert!(ax.app.engine.revision() > revision);
    let moved = project.placement_of(j1).ok_or("J1")?;
    assert!(
        (moved.position[1] - placement.position[1] - 1.0).abs() < 1e-6,
        "{:?} -> {:?}",
        placement.position,
        moved.position
    );
    let ctx = ax.ctx.clone();
    ax.app.run_command(&ctx, "edit.undo");
    ax.settle();
    let project = ax.app.engine.project();
    assert!(
        project
            .part_dimensions
            .iter()
            .any(|d| d.id == face && d.value == Some(DimValue::Fixed(0.0)))
    );
    assert_eq!(project.placement_of(j1), Some(&placement));

    // The delete icon at the row's end removes the dimension and leaves J1
    // in place; one undo brings it back.
    let row = ax.find(Role::ListBoxOption, FACE)?;
    ax.hover(egui::pos2(row.rect.left() + 12.0, row.rect.center().y));
    ax.button(&delete)?;
    let project = ax.app.engine.project();
    assert!(!project.part_dimensions.iter().any(|d| d.id == face));
    assert_eq!(project.placement_of(j1), Some(&placement));
    assert!(!ax.has(Role::ListBoxOption, FACE), "its row is gone");
    ax.app.run_command(&ctx, "edit.undo");
    ax.settle();
    assert!(
        ax.app
            .engine
            .project()
            .part_dimensions
            .iter()
            .any(|d| d.id == face)
    );
    ax.find(Role::ListBoxOption, FACE)?;
    Ok(())
}

/// While the board's model updates after an edit that raises a finding,
/// the "Updating" chip at the top right of the 3D view sits left of the
/// issues badge, not under it.
#[test]
fn the_updating_chip_sits_beside_the_issues_badge() -> Fallible {
    let engine = esp32()?;
    let project = engine.project();
    let u1 = project
        .instances
        .iter()
        .find(|i| i.designator == "U1")
        .map(|i| i.id)
        .ok_or("no U1")?;
    let placement = project.placement_of(u1).ok_or("U1 placed")?.clone();
    let mut ax = Ax::with_engine(engine, Workspace::Pcb)?;
    ax.app.pcb.set_3d(true);
    until_shown(&mut ax)?;
    // U1 a millimetre right (its courtyard meets SW2's, a finding): the
    // model updates.
    let revision = ax.app.engine.revision();
    ax.app.engine.apply(
        revision,
        Operation::Layout(pcbcraft_core::layout::LayoutCommand::Move {
            instance: u1,
            position: [placement.position[0] + 1.0, placement.position[1]],
            rotation_deg: placement.rotation_deg,
            layer: placement.layer,
        }),
    )?;
    ax.frame(vec![]);
    ax.frame(vec![]);
    let now = crate::scene::ModelKey::Project(Arc::clone(ax.app.engine.project_arc()));
    assert_eq!(
        ax.app.scene.display(&now),
        crate::scene::Display::Stale,
        "the model is updating"
    );
    let chip = ax.find(Role::Label, "Updating")?;
    // The canvas badge: the "Issues: …" button in the chip's band (the
    // ribbon's toggle and the Issues card's bar are the others).
    let badge = ax
        .all_prefix(Role::Button, "Issues: ")
        .into_iter()
        .find(|b| b.rect.top() <= chip.rect.bottom() && b.rect.bottom() >= chip.rect.top())
        .cloned()
        .ok_or_else(|| format!("no canvas badge:\n{}", ax.dump()))?;
    assert!(
        chip.rect.right() <= badge.rect.left() && chip.rect.top() < badge.rect.bottom(),
        "chip {:?} badge {:?}",
        chip.rect,
        badge.rect
    );
    Ok(())
}

/// J1 selected in Layout. The Dimensions list rows are focused by a
/// click.
fn j1_in_layout() -> Fallible<(Ax, pcbcraft_core::Id)> {
    let engine = esp32()?;
    let j1 = engine
        .project()
        .instances
        .iter()
        .find(|i| i.designator == "J1")
        .map(|i| i.id)
        .ok_or("no J1")?;
    let mut ax = Ax::with_engine(engine, Workspace::Pcb)?;
    ax.app.pcb.set_selection(Some(j1), None, None);
    ax.settle();
    Ok((ax, j1))
}

fn order(ax: &Ax) -> Vec<pcbcraft_core::Id> {
    ax.app
        .engine
        .project()
        .part_dimensions
        .iter()
        .map(|d| d.id)
        .collect()
}

/// ⌥↓ on a focused row moves it one place down as one
/// `layout.dimension_order`: the rows swap, J1 stays where it is, and one
/// undo restores the order.
#[test]
fn option_down_reorders_dimensions_as_one_undo_step() -> Fallible {
    let (mut ax, j1) = j1_in_layout()?;
    let before = order(&ax);
    let placement = ax.app.engine.project().placement_of(j1).cloned();
    let first = ax
        .all(Role::ListBoxOption, FACE)
        .into_iter()
        .chain(ax.all(Role::ListBoxOption, ORIGIN))
        .min_by(|a, b| a.rect.top().total_cmp(&b.rect.top()))
        .cloned()
        .ok_or("a dimension row")?;
    let other = if first.name == FACE { ORIGIN } else { FACE };
    ax.click_at(egui::pos2(first.rect.left() + 30.0, first.rect.center().y));
    ax.key_with(egui::Key::ArrowDown, egui::Modifiers::ALT);
    ax.settle();
    let after = order(&ax);
    assert_ne!(after, before, "the order changed");
    let mut swapped = before.clone();
    swapped.swap(0, 1);
    assert_eq!(after, swapped);
    assert_eq!(ax.app.engine.project().placement_of(j1).cloned(), placement);
    // The row moved down: the other row is now above it.
    let moved = ax.find(Role::ListBoxOption, &first.name)?;
    let above = ax.find(Role::ListBoxOption, other)?;
    assert!(above.rect.top() < moved.rect.top(), "{above:?} {moved:?}");
    let ctx = ax.ctx.clone();
    ax.app.run_command(&ctx, "edit.undo");
    ax.settle();
    assert_eq!(order(&ax), before, "one undo step");
    Ok(())
}

/// Esc drops an inline edit and sends nothing; invalid text shows why
/// under the list and leaves the document unchanged.
#[test]
fn esc_cancels_and_invalid_text_is_refused_in_place() -> Fallible {
    let (mut ax, _) = j1_in_layout()?;
    let row = ax.find(Role::ListBoxOption, FACE)?;
    ax.click_at(egui::pos2(row.rect.left() + 30.0, row.rect.center().y));
    let revision = ax.app.engine.revision();
    const NOTE: &str = "Follows the board when it changes";
    let note = ax.find(Role::Label, NOTE)?.rect;
    ax.key(egui::Key::F2);
    ax.settle();
    ax.find(Role::TextInput, FACE)?;
    // The field opens in its row: nothing under the list moves.
    assert_eq!(ax.find(Role::Label, NOTE)?.rect, note);
    ax.type_text("7 mm");
    ax.key(egui::Key::Escape);
    ax.settle();
    assert!(!ax.has(Role::TextInput, FACE));
    assert_eq!(ax.app.engine.revision(), revision, "Esc sends nothing");
    // Enter opens it again; text that is not a length is refused in place.
    ax.key(egui::Key::Enter);
    ax.settle();
    ax.find(Role::TextInput, FACE)?;
    ax.type_text("wobble");
    ax.key(egui::Key::Enter);
    ax.settle();
    assert_eq!(ax.app.engine.revision(), revision, "nothing sent");
    assert!(
        ax.texts
            .iter()
            .any(|t| t.starts_with(FACE) && t.contains(':')),
        "the reason names the row: {:?}",
        ax.texts
    );
    ax.find(Role::TextInput, FACE)?;
    Ok(())
}
