//! `library.set_marking` placements, finishes, codes and decals, and
//! `library.marking_surfaces`, over the ESP32-S31-WROOM-3 example drafts.
use super::super::Session;
use pcbcraft_core::Engine;
use pcbcraft_core::library::Definition;
use pcbcraft_core::library::marking::{Finish, MarkingItem, SurfaceRef};
use serde_json::{Value, json};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

const EXAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/esp32-s31-wroom-3/esp32-s31-wroom-3.pcbcraft"
);

/// The example's session and its component and package draft ids.
fn example() -> std::result::Result<(Session, u64, u64), Box<dyn std::error::Error>> {
    let mut session = Session::new(Engine::default());
    let opened = session.dispatch(json!({"command": "project.open", "params": {"path": EXAMPLE}}));
    assert_eq!(opened["ok"], true, "{opened}");
    let id = |kind: fn(&Definition) -> bool| {
        session
            .engine()
            .drafts()
            .items
            .iter()
            .find(|d| kind(&d.definition))
            .map(|d| d.id.0)
    };
    let component = id(|d| matches!(d, Definition::Component(_))).ok_or("component")?;
    let package = id(|d| matches!(d, Definition::Package(_))).ok_or("package")?;
    Ok((session, component, package))
}

fn revision(session: &Session) -> u64 {
    session.engine().revision()
}

/// The can marking of the example: etched model text, a placeholder logo
/// box (original artwork), a QR code of the designator and serial, and a
/// silkscreen line on the board beyond the can.
pub fn wroom_marking() -> Value {
    json!({
        "items": [
            {"item": "artwork", "anchor": "pin1_corner", "size_mm": [4.0, 4.0],
             "licence": "CC0-1.0",
             "svg": "<svg xmlns='http://www.w3.org/2000/svg' width='10mm' height='10mm' viewBox='0 0 10 10'><rect x='0.5' y='0.5' width='9' height='9' rx='1.2' fill='none' stroke='black' stroke-width='0.8'/><path d='M3 7 L5 3 L7 7 Z' fill='black'/></svg>"},
            {"item": "text", "template": "{mpn}"},
            {"item": "text", "template": "MODULE {var:lot}"},
            {"item": "code2d", "payload": "{designator} {var:serial}", "size_mm": 4.5, "anchor": "end"}
        ],
        "finish": "laser_etch_metal",
        "placement": {"on": {"surface": "anchor", "name": "can_top"}},
        "decals": [{
            "items": [{"item": "text", "template": "{designator}", "height_mm": 1.2}],
            "finish": "ink_white",
            "placement": {"on": {"surface": "anchor", "name": "pcb_top"}}
        }]
    })
}

#[test]
fn a_module_marking_goes_on_its_can_and_board_in_one_step() -> TestResult {
    let (mut session, component, _) = example()?;
    let before = revision(&session);
    let reply = session.dispatch(json!({"command": "library.set_marking", "params": {
        "expected_revision": before, "draft": component, "marking": wroom_marking()}}));
    assert_eq!(reply["ok"], true, "{reply}");
    let draft = session
        .engine()
        .drafts()
        .items
        .iter()
        .find(|d| d.id.0 == component)
        .ok_or("draft")?;
    let Definition::Component(c) = &draft.definition else {
        return Err("not a component".into());
    };
    let spec = c.marking.as_deref().ok_or("marked")?;
    assert_eq!(spec.finish, Finish::LaserEtchMetal);
    assert_eq!(
        spec.placement.on,
        SurfaceRef::Anchor {
            name: "can_top".into()
        }
    );
    assert!(matches!(spec.items[3], MarkingItem::Code2d { .. }));
    assert_eq!(spec.decals.len(), 1);
    assert_eq!(spec.decals[0].finish, Finish::InkWhite);
    // The artwork is packed with the edit: one undo step takes both.
    assert_eq!(spec.artwork().count(), 1);
    let undone = session.dispatch(json!({"command": "project.undo", "params": {}}));
    assert_eq!(undone["ok"], true, "{undone}");
    let marked = session.engine().drafts().items.iter().any(|d| {
        d.id.0 == component
            && matches!(&d.definition, Definition::Component(c) if c.marking.is_some())
    });
    assert!(!marked, "undo takes the marking back");
    Ok(())
}

#[test]
fn bad_marking_placements_are_refused_whole() -> TestResult {
    let (mut session, component, _) = example()?;
    let before = revision(&session);
    let mut bad_anchor = wroom_marking();
    bad_anchor["placement"]["on"]["name"] = json!("Can Top");
    let mut many = wroom_marking();
    many["decals"] = json!(vec![many["decals"][0].clone(); 5]);
    let mut tiny_code = wroom_marking();
    tiny_code["items"][3]["size_mm"] = json!(0.0);
    let mut bad_wrap = wroom_marking();
    bad_wrap["placement"]["wrap"] = json!({"wrap_deg": 400.0});
    for marking in [bad_anchor, many, tiny_code, bad_wrap] {
        let reply = session.dispatch(json!({"command": "library.set_marking", "params": {
            "expected_revision": before, "draft": component, "marking": marking}}));
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(revision(&session), before);
    }
    Ok(())
}

#[test]
fn marking_surfaces_list_a_module_s_anchors() -> TestResult {
    let (mut session, component, package) = example()?;
    let reply = session
        .dispatch(json!({"command": "library.marking_surfaces", "params": {"package": package}}));
    assert_eq!(reply["ok"], true, "{reply}");
    let surfaces = reply["result"]["surfaces"].as_array().ok_or("surfaces")?;
    let names: Vec<&str> = surfaces
        .iter()
        .filter_map(|s| s["on"]["name"].as_str())
        .collect();
    for n in ["can_top", "pcb_top", "antenna_area"] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
    assert!(surfaces.iter().any(|s| s["on"]["face"] == "pos_x"));
    // The example can is 21 × 23 mm; its top keeps 0.3 mm from the edge.
    let can = surfaces
        .iter()
        .find(|s| s["on"]["name"] == "can_top")
        .ok_or("can")?;
    assert_eq!(can["size"], json!([20.4, 22.4]));
    // A component is not a package.
    let reply = session
        .dispatch(json!({"command": "library.marking_surfaces", "params": {"package": component}}));
    assert_eq!(reply["ok"], false, "{reply}");
    // The request is in the catalog, with its schema.
    assert!(
        super::super::schemas()
            .to_string()
            .contains("library.marking_surfaces")
    );
    Ok(())
}
