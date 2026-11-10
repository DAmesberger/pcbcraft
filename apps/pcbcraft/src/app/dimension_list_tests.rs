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
    let items = ax.app.scene.selected_items(Some(j1), None, None);
    ax.app
        .scene
        .viewport
        .view_from(pcbcraft_solidcraft::geom::Vec3::new(0.15, -1.0, 0.6));
    assert!(ax.app.scene.viewport.fit_items(&items));
    ax.settle();

    // One compact row each: the name on one line, half the height the
    // name, value and Delete rows took.
    let face_row = ax.find(Role::Button, FACE)?;
    let origin_row = ax.find(Role::Button, ORIGIN)?;
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
    // Each value is a field named by its row; the driven one reads `fx` and
    // its result inline (with its expression where the column has room),
    // and the whole on hover.
    let field = ax.find(Role::TextInput, FACE)?;
    let driven_field = ax.find(Role::TextInput, ORIGIN)?;
    let whole = format!("fx{expr} = ");
    let inline = |s: &String| s.starts_with("fx") && s.ends_with(" mm") && !s.contains(':');
    assert!(ax.texts.iter().any(inline), "fx…: {:?}", ax.texts);
    // The pointer rests on it until its tooltip shows.
    ax.hover(driven_field.center());
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
    assert_eq!(ax.find(Role::Button, ORIGIN)?.toggled, Some(true));
    assert_eq!(ax.find(Role::Button, FACE)?.toggled, Some(false));
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
    assert_eq!(ax.find(Role::Button, FACE)?.toggled, Some(true));
    assert_eq!(ax.find(Role::Button, ORIGIN)?.toggled, Some(false));
    assert_eq!(ax.app.pcb.dimensions_lit(), vec![face]);

    // The value commits as one edit of the same dimension, which moves J1;
    // one undo puts both back.
    let revision = ax.app.engine.revision();
    ax.click_at(field.center());
    ax.command_key(egui::Key::A);
    ax.type_text("1 mm");
    ax.key(egui::Key::Enter);
    ax.settle();
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
    let row = ax.find(Role::Button, FACE)?;
    ax.hover(egui::pos2(row.rect.left() + 12.0, row.rect.center().y));
    ax.button(&delete)?;
    let project = ax.app.engine.project();
    assert!(!project.part_dimensions.iter().any(|d| d.id == face));
    assert_eq!(project.placement_of(j1), Some(&placement));
    assert!(!ax.has(Role::Button, FACE), "its row is gone");
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
    ax.find(Role::Button, FACE)?;
    Ok(())
}
