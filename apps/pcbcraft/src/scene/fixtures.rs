//! Boards for the 3D view's tests, made through the engine's typed
//! operations as the app makes them.
use pcbcraft_core::Engine;
use pcbcraft_core::library::Definition;
use serde_json::{Value, json};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

fn apply(engine: &mut Engine, operation: Value) -> Fallible<Value> {
    let operation: pcbcraft_core::Operation = serde_json::from_value(operation)?;
    let outcome = engine.apply(engine.revision(), operation)?;
    Ok(serde_json::to_value(outcome)?)
}

/// The ESP32-S31-WROOM-3 example (`examples/esp32-s31-wroom-3`) opened as
/// the app opens a file, its drafts published and U1 placed in the middle
/// of the board, top side: a board whose only part is a generated module
/// model with its marking.
pub(crate) fn wroom_board() -> Fallible<Engine> {
    let file = pcbcraft_io::read_project(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-wroom-3/esp32-s31-wroom-3.pcbcraft"
    )))?;
    let mut engine = Engine::default();
    engine.replace_document(file)?;
    let id = |kind: fn(&Definition) -> bool| {
        engine
            .drafts()
            .items
            .iter()
            .find(|d| kind(&d.definition))
            .map(|d| d.id.0)
            .ok_or("a draft of the example is missing")
    };
    let component = id(|d| matches!(d, Definition::Component(_)))?;
    let symbol = id(|d| matches!(d, Definition::Symbol(_)))?;
    let package = id(|d| matches!(d, Definition::Package(_)))?;
    let footprint = id(|d| matches!(d, Definition::Footprint(_)))?;
    let binding = id(|d| matches!(d, Definition::Binding(_)))?;
    let published = apply(
        &mut engine,
        json!({"editor": "Library", "operation": {"command": "library.publish",
            "params": {"drafts": [component, symbol, package, footprint, binding]}}}),
    )?;
    let revision = published["revisions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["kind"] == "component")
        .cloned()
        .ok_or("no component revision")?;
    let placed = apply(
        &mut engine,
        json!({"editor": "Instance", "operation": {"command": "component.place", "params": {
            "component": {"id": revision["id"], "revision": revision["revision"]},
            "designator": "U1", "properties": [], "representation": symbol, "unit": 1,
            "position": [60.0, 0.0],
            "physical": {"binding": binding, "footprint": footprint}}}}),
    )?;
    let size = engine.project().board_size;
    apply(
        &mut engine,
        json!({"editor": "Layout", "operation": {"command": "layout.place", "params": {
            "instance": placed["instance"], "position": [size[0] / 2.0, size[1] / 2.0],
            "rotation_deg": 0.0, "layer": "Top"}}}),
    )?;
    Ok(engine)
}
