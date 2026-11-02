//! The assisted run's drafts, published by a person (here the test) and
//! placed on a board: the antenna keep-out keeps fill copper out on every
//! layer, and the board exports to a native model with the module's
//! generated body.
use super::*;

fn send(s: &mut Session, request: Value) -> Fallible<Value> {
    let reply = s.dispatch(request);
    if reply["ok"] != true {
        return Err(format!("{}", reply["error"]).into());
    }
    Ok(reply["result"].clone())
}

fn apply(s: &mut Session, editor: &str, command: &str, params: Value) -> Fallible<Value> {
    let revision = s.engine().revision();
    send(
        s,
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": editor, "operation": {"command": command, "params": params}}}}),
    )
}

/// Whether point `q` lies in a filled island (even-odd over its rings).
fn covered(copper: &pcbcraft_core::model::fill::FilledCopper, q: [f64; 2]) -> bool {
    let inside = |ring: &[[f64; 2]]| {
        let mut c = false;
        for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
            if (a[1] > q[1]) != (b[1] > q[1])
                && q[0] < (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                c = !c;
            }
        }
        c
    };
    copper
        .regions
        .iter()
        .any(|r| inside(&r.outer) && !r.holes.iter().any(|h| inside(h)))
}

#[test]
fn the_module_keeps_copper_out_of_its_antenna_area_on_the_board() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut transport = ReplayTransport::new(script()?);
    transport.prompt_tokens = Some(60_000);
    let mut s = Session::default();
    let outcome = run(
        &goal(datasheet(&dir)?),
        &Settings::default(),
        &mut transport,
        &mut s,
        &mut |_| {},
        &AtomicBool::new(false),
    )?;
    assert_eq!(outcome.stop, Stop::Submitted);
    let ids: Vec<u64> = s.engine().drafts().items.iter().map(|d| d.id.0).collect();
    let published = apply(&mut s, "Library", "library.publish", json!({"drafts": ids}))?;
    let revision = |kind: &str| -> Fallible<Value> {
        published["outcome"]["revisions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["kind"] == kind)
            .cloned()
            .ok_or_else(|| format!("no {kind} revision: {}", published["outcome"]).into())
    };
    let (component, symbol, binding, footprint) = (
        revision("component")?,
        revision("symbol")?,
        revision("binding")?,
        revision("footprint")?,
    );
    let placed = apply(
        &mut s,
        "Instance",
        "component.place",
        json!({"component": {"id": component["id"], "revision": component["revision"]},
            "designator": "U1", "properties": [], "representation": symbol["id"], "unit": 1,
            "position": [0.0, 0.0],
            "physical": {"binding": binding["id"], "footprint": footprint["id"]}}),
    )?;
    let instance = placed["outcome"]["instance"].clone();
    apply(
        &mut s,
        "Layout",
        "layout.place",
        json!({"instance": instance, "position": [30.0, 15.0], "rotation_deg": 0.0, "layer": "Top"}),
    )?;
    // A pour over the whole board on the bottom layer.
    let board = json!({"start": [0.0, 0.0], "segments": [
        {"line": {"to": [60.0, 0.0]}}, {"line": {"to": [60.0, 40.0]}},
        {"line": {"to": [0.0, 40.0]}}, {"line": {"to": [0.0, 0.0]}}]});
    apply(
        &mut s,
        "Fill",
        "fill.add",
        json!({"fill": {"name": "pour", "net": null, "layer": "Bottom", "outline": board}}),
    )?;
    apply(&mut s, "Fill", "fill.refill", json!({"target": {"fills": "all"}}))?;
    let project = s.engine().project();
    // The module (20 mm long, 5 mm antenna end) at (30, 15): keep-out
    // x 23..37, y 20..25.
    let keepouts = pcbcraft_core::layout::placed_keepouts(project);
    assert_eq!(keepouts.len(), 1);
    let copper = project
        .fills
        .first()
        .and_then(|f| f.computed.as_ref())
        .ok_or("computed fill copper")?;
    assert!(covered(copper, [5.0, 5.0]), "the pour fills the free board");
    assert!(
        !covered(copper, [30.0, 22.5]) && !covered(copper, [24.0, 20.5]),
        "but not the antenna area, even on the bottom layer"
    );
    assert!(covered(copper, [30.0, 26.0]), "copper resumes past the keep-out");
    // The board exports with the module's generated model.
    let path = dir.path().join("board.solidcraft");
    send(
        &mut s,
        json!({"command": "project.export_native", "params": {"path": path.to_string_lossy()}}),
    )?;
    assert!(std::fs::metadata(&path)?.len() > 0);
    Ok(())
}
