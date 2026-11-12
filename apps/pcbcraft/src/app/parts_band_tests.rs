//! The Parts and Vias band under the PCB canvas, through the whole app's
//! frame and accessibility tree, on the ESP32-S31 dev board: ⇧⌘L shows it;
//! its rows list the parts in natural order; a filter term narrows them
//! and shows its chip; the star saves the filter in the user settings; a
//! row click selects the part in the PCB editor and a board selection
//! selects its row; ↓ and Enter move and locate; hovering a row lights the
//! part in Layout and in 3D without a model rebuild. The narrow widths
//! (230 and 420 pt) are checked on the band itself
//! (`pcb_editor::parts_table::tests`).
use super::ax_harness::{Ax, Fallible, Node};
use super::*;
use crate::kit::list::{ListState, RowKey, natural_cmp};
use egui::accesskit::Role;

fn esp32() -> Fallible<Engine> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    Ok(Engine::from_file(pcbcraft_io::read_project(&path)?)?)
}

fn open_band(ax: &mut Ax) {
    ax.key_with(
        egui::Key::L,
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
    );
}

/// The parts rows shown, top to bottom.
fn rows(ax: &Ax) -> Vec<Node> {
    ax.all_prefix(Role::Row, "Ref: ")
        .into_iter()
        .cloned()
        .collect()
}

/// A row's designator: its accessible name reads "Ref: R1, Value: …".
fn reference(row: &Node) -> String {
    row.name
        .strip_prefix("Ref: ")
        .and_then(|n| n.split(", ").next())
        .unwrap_or_default()
        .to_owned()
}

fn instance(ax: &Ax, designator: &str) -> Fallible<pcbcraft_core::Id> {
    ax.app
        .engine
        .project()
        .instance_by_designator(designator)
        .map(|i| i.id)
        .ok_or_else(|| format!("no {designator}").into())
}

fn table_state(ax: &Ax) -> ListState {
    ListState::load(
        &ax.ctx,
        egui::Id::unique(("pcb-parts-table", crate::pcb_editor::TableTab::Parts)),
    )
}

#[test]
fn the_band_lists_parts_naturally_filters_and_saves_the_filter() -> Fallible {
    let engine = esp32()?;
    let placed = engine.project().placements.len();
    let mut xs: Vec<f64> = engine
        .project()
        .placements
        .iter()
        .map(|p| p.position[0])
        .collect();
    xs.sort_by(f64::total_cmp);
    let threshold = xs.get(xs.len() / 2).ok_or("parts")?.floor();
    let mut ax = Ax::with_engine(engine, Workspace::Pcb)?;
    assert!(rows(&ax).is_empty(), "the band starts hidden");
    open_band(&mut ax);
    ax.find(Role::Grid, &format!("Parts, {placed} rows"))?;
    let shown: Vec<String> = rows(&ax).iter().map(reference).collect();
    assert!(shown.len() >= 4, "{}", ax.dump());
    assert!(
        shown
            .windows(2)
            .all(|w| natural_cmp(&w[0], &w[1]) != std::cmp::Ordering::Greater),
        "natural order: {shown:?}"
    );
    // A term narrows the rows and shows as a chip.
    // The field runs left of its star (the field's text input carries no
    // accessible name of its own).
    let star = ax.find(Role::Button, "Save this filter")?;
    ax.click_at(star.center() - egui::vec2(120.0, 0.0));
    let query = format!("x>={threshold}");
    ax.type_text(&query);
    ax.frame(vec![]);
    assert!(
        ax.texts.iter().any(|t| t.starts_with("x \u{2265}")),
        "the term's chip: {:?}",
        ax.texts
    );
    let grid = ax.find_prefix(Role::Grid, "Parts, ")?;
    assert!(
        !grid.name.contains(&format!(" {placed} rows")),
        "narrowed: {}",
        grid.name
    );
    for row in rows(&ax) {
        let id = instance(&ax, &reference(&row))?;
        let x = ax
            .app
            .engine
            .project()
            .placement_of(id)
            .map_or(f64::NAN, |p| p.position[0]);
        assert!(x >= threshold, "{} at x {x}", reference(&row));
    }
    // The star keeps it in the user settings.
    ax.button("Save this filter")?;
    assert!(
        ax.app
            .sources
            .list_settings()
            .saved_filters
            .iter()
            .any(|f| f.list == "pcb.parts" && f.query == query),
        "saved filters: {:?}",
        ax.app.sources.list_settings().saved_filters
    );
    Ok(())
}

#[test]
fn rows_and_board_share_the_selection_and_down_and_enter_locate() -> Fallible {
    let mut ax = Ax::with_engine(esp32()?, Workspace::Pcb)?;
    open_band(&mut ax);
    let shown = rows(&ax);
    let (first, second) = match shown.as_slice() {
        [a, b, ..] => (a.clone(), b.clone()),
        _ => return Err(format!("rows: {}", ax.dump()).into()),
    };
    // A row click selects the part as a canvas click does.
    ax.click_at(first.center());
    assert_eq!(
        ax.app.pcb.selected_instance(),
        Some(instance(&ax, &reference(&first))?)
    );
    // ↓ selects the next row; Enter locates it.
    ax.key(egui::Key::ArrowDown);
    let next = instance(&ax, &reference(&second))?;
    assert_eq!(ax.app.pcb.selected_instance(), Some(next));
    ax.key(egui::Key::Enter);
    assert_eq!(ax.app.pcb.table_located(), Some(next));
    assert_eq!(ax.app.pcb.selected_instance(), Some(next));
    // A board selection selects its row and reveals it: the last part.
    let last = ax
        .app
        .engine
        .project()
        .instances
        .iter()
        .map(|i| i.designator.clone())
        .max_by(|a, b| natural_cmp(a, b))
        .ok_or("parts")?;
    let id = instance(&ax, &last)?;
    ax.app.pcb.set_selection(Some(id), None, None);
    ax.frame(vec![]);
    ax.frame(vec![]);
    assert_eq!(
        table_state(&ax).selection,
        [RowKey::of(("part", id))].into(),
        "the board selection selected its row"
    );
    assert!(
        rows(&ax).iter().any(|r| reference(r) == last),
        "{last}'s row was scrolled into view"
    );
    Ok(())
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

#[test]
fn hovering_a_row_lights_its_part_in_layout_and_3d_without_a_rebuild() -> Fallible {
    let mut ax = Ax::with_engine(esp32()?, Workspace::Pcb)?;
    open_band(&mut ax);
    let row = rows(&ax).first().cloned().ok_or("a row")?;
    let id = instance(&ax, &reference(&row))?;
    ax.hover(row.center());
    ax.frame(vec![]);
    assert!(ax.app.pcb.lit().parts.contains(&id), "Layout lights it");
    // PCB 3D: the hovered row lights the part's bodies; the model stays.
    ax.app.pcb.set_3d(true);
    until_shown(&mut ax)?;
    let requests = ax.app.scene.requests();
    let bodies = ax.app.scene.selected_items(&crate::pcb_editor::Group {
        parts: [id].into(),
        ..Default::default()
    });
    assert!(!bodies.is_empty(), "the part has bodies in 3D");
    let row = rows(&ax)
        .into_iter()
        .find(|r| reference(r) == reference(&row))
        .ok_or("the row in 3D")?;
    ax.hover(row.center());
    ax.frame(vec![]);
    let lit = ax.app.scene.viewport.highlighted().to_vec();
    assert!(
        bodies.iter().all(|b| lit.contains(b)),
        "3D lights the part: {bodies:?} in {lit:?}"
    );
    assert_eq!(ax.app.scene.requests(), requests, "no model was rebuilt");
    // Off the row, the light goes.
    ax.hover(egui::pos2(4.0, 450.0));
    ax.frame(vec![]);
    assert!(
        !bodies
            .iter()
            .any(|b| ax.app.scene.viewport.highlighted().contains(b)),
        "the light goes with the hover"
    );
    Ok(())
}
