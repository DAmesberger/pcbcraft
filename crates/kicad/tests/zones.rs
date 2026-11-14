//! Zones of Rust-built synthetic boards: fills defined from the zone and
//! filled by our engine, rule areas as keepouts.
mod support;

use sparksmith_core::CopperLayer;
use sparksmith_core::model::fill::IslandRemoval;
use sparksmith_core::rules::PadConnection;
use sparksmith_kicad::report::Case;
use support::{Board, Footprint, Pad, import};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

const ZONE: &str = "
  (net 1 \"GND\")
  (gr_rect (start 0 0) (end 30 20) (stroke (width 0.1) (type default)) (layer \"Edge.Cuts\"))
  (zone (net 1) (net_name \"GND\") (layers \"F&B.Cu\") (hatch edge 0.5) (priority 2)
    (connect_pads yes (clearance 0.3)) (min_thickness 0.25) (filled_areas_thickness no)
    (fill yes (thermal_gap 0.5) (thermal_bridge_width 0.4) (island_removal_mode 1))
    (polygon (pts (xy 1 1) (xy 29 1) (xy 29 19) (xy 1 19))))
  (zone (net 0) (net_name \"\") (layer \"F.Cu\") (hatch edge 0.5)
    (connect_pads (clearance 0)) (min_thickness 0.25)
    (keepout (tracks not_allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed))
    (fill (thermal_gap 0.5) (thermal_bridge_width 0.5))
    (polygon (pts (xy 20 5) (xy 25 5) (xy 25 10) (xy 20 10))))
";

fn zoned() -> Board {
    let gnd = |n: &'static str, x: f64| {
        Pad::smd(n, "rect", [x, 0.0, 0.0], [0.8, 0.6]).with("(net 1 \"GND\")")
    };
    let mut fp = Footprint::new("R1", [8.0, 10.0, 0.0], false, vec![gnd("1", -1.0), gnd("2", 1.0)]);
    // A footprint's own rule area: KiCad writes its outline in board
    // coordinates.
    fp.extra = " (zone (net 0) (net_name \"\") (layer \"F.Cu\") (hatch edge 0.5) \
        (connect_pads (clearance 0)) (min_thickness 0.25) \
        (keepout (tracks allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed)) \
        (fill (thermal_gap 0.5) (thermal_bridge_width 0.5)) \
        (polygon (pts (xy 4 14) (xy 6 14) (xy 6 16) (xy 4 16))))"
        .into();
    Board {
        copper: 2,
        origin: None,
        setup: "(pad_to_mask_clearance 0)".into(),
        footprints: vec![fp],
        items: ZONE.into(),
    }
}

#[test]
fn a_zone_becomes_a_fill_per_layer_filled_by_our_engine() -> Fallible<()> {
    let imported = import(zoned().text().as_bytes())?;
    let project = imported.engine.project();
    assert_eq!(imported.report.count(Case::Zone), 2);
    assert_eq!(project.fills.len(), 2);
    let layers: Vec<CopperLayer> = project.fills.iter().map(|f| f.layer).collect();
    assert_eq!(layers, [CopperLayer::Top, CopperLayer::Bottom]);
    let r1 = project.instance_by_designator("R1").ok_or("R1")?;
    let gnd = project
        .net_of(sparksmith_core::Endpoint {
            instance: r1.id,
            terminal: sparksmith_core::TerminalId(1),
        })
        .ok_or("GND")?
        .id;
    for fill in project.fills.iter() {
        assert_eq!(fill.net, Some(gnd));
        assert_eq!(fill.priority, 2);
        assert_eq!(fill.settings.clearance, Some(0.3));
        assert_eq!(fill.settings.width_min, Some(0.25));
        assert_eq!(fill.settings.connections.smd, Some(PadConnection::Solid));
        assert_eq!(fill.settings.islands, IslandRemoval::Never);
        let thermal = fill.settings.thermal.ok_or("thermal")?;
        assert!((thermal.gap - 0.5).abs() < 1e-9 && (thermal.spoke_width - 0.4).abs() < 1e-9);
        // Our engine filled it: copper of the 28 × 18 mm outline.
        let copper = fill.computed.as_ref().ok_or("not filled")?;
        assert!(!copper.regions.is_empty());
    }
    Ok(())
}

#[test]
fn rule_areas_become_keepouts_on_their_layers() -> Fallible<()> {
    let imported = import(zoned().text().as_bytes())?;
    let project = imported.engine.project();
    assert_eq!(imported.report.count(Case::Keepout), 2);
    assert_eq!(project.keepouts.len(), 2);
    let board = &project.keepouts[0];
    assert!(board.forbid.tracks && board.forbid.fills && !board.forbid.vias);
    // The footprint's area forbids only pour, at KiCad's (4, 14)–(6, 16):
    // ours (4, 6)–(6, 4) about the outline's corner (0, 20).
    let own = &project.keepouts[1];
    assert!(own.forbid.fills && !own.forbid.tracks);
    let b = sparksmith_core::geometry::Bounds::of(own.outline.flatten(1e-3)).ok_or("box")?;
    assert!((b.min[0] - 4.0).abs() < 1e-9 && (b.min[1] - 4.0).abs() < 1e-9);
    assert!((b.max[0] - 6.0).abs() < 1e-9 && (b.max[1] - 6.0).abs() < 1e-9);
    Ok(())
}
