//! Nets, tracks and vias of Rust-built synthetic boards, through the
//! engine: labels join the pads' terminals into named nets, and tracks and
//! vias land on those nets and layers.
mod support;

use sparksmith_core::{CopperLayer, ViaSpan};
use sparksmith_kicad::report::Case;
use support::{Board, Footprint, Pad, import};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Two resistors joined by a top track, a via and a bottom track, plus a
/// net with a space in its name, two names equal but for case, an arc
/// track and a track piece too short to import.
fn routed() -> Board {
    let pad = |number: &'static str, x: f64, net: (u32, &str)| {
        Pad::smd(number, "rect", [x, 0.0, 0.0], [0.8, 0.6])
            .with(&format!("(net {} \"{}\")", net.0, net.1))
    };
    let resistor = |reference: &str, x: f64, y: f64, nets: [(u32, &str); 2]| {
        Footprint::new(
            reference,
            [x, y, 0.0],
            false,
            vec![pad("1", -1.0, nets[0]), pad("2", 1.0, nets[1])],
        )
    };
    let items = "
  (net 1 \"SIG\") (net 2 \"my net\") (net 3 \"gnd\") (net 4 \"GND\")
  (gr_rect (start 0 0) (end 40 20) (stroke (width 0.1) (type default)) (layer \"Edge.Cuts\"))
  (segment (start 11 10) (end 20 10) (width 0.25) (layer \"F.Cu\") (net 1))
  (via (at 20 10) (size 0.6) (drill 0.3) (layers \"F.Cu\" \"B.Cu\") (net 1))
  (segment (start 20 10) (end 29 10) (width 0.25) (layer \"B.Cu\") (net 1))
  (segment (start 29 10) (end 29 10.004) (width 0.25) (layer \"B.Cu\") (net 1))
  (arc (start 9 10) (mid 8 9) (end 7 10) (width 0.2) (layer \"F.Cu\") (net 2))
  (segment (start 5 15) (end 6 15) (width 0.25) (layer \"F.Cu\") (net 9))
";
    Board {
        copper: 2,
        origin: None,
        setup: "(pad_to_mask_clearance 0)".into(),
        footprints: vec![
            resistor("R1", 10.0, 10.0, [(2, "my net"), (1, "SIG")]),
            resistor("R2", 30.0, 10.0, [(1, "SIG"), (3, "gnd")]),
            resistor("R3", 20.0, 16.0, [(4, "GND"), (4, "GND")]),
        ],
        items: items.into(),
    }
}

#[test]
fn pads_join_into_named_nets_through_labels() -> Fallible<()> {
    let imported = import(routed().text().as_bytes())?;
    let project = imported.engine.project();
    let report = &imported.report;
    assert_eq!(report.counts.nets, 4);
    let name = |designator: &str, terminal: u32| -> Option<String> {
        let instance = project.instance_by_designator(designator)?;
        let net = project.net_of(sparksmith_core::Endpoint {
            instance: instance.id,
            terminal: sparksmith_core::TerminalId(terminal),
        })?;
        Some(project.net_name(net.id).to_owned())
    };
    // R1.2 and R2.1 share SIG.
    assert_eq!(name("R1", 2).as_deref(), Some("SIG"));
    assert_eq!(name("R2", 1).as_deref(), Some("SIG"));
    let sig = |d: &str, t: u32| {
        let i = project.instance_by_designator(d)?;
        project.net_of(sparksmith_core::Endpoint {
            instance: i.id,
            terminal: sparksmith_core::TerminalId(t),
        })
    };
    assert_eq!(sig("R1", 2).map(|n| n.id), sig("R2", 1).map(|n| n.id));
    // A space is no label character; "gnd" and "GND" stay two nets.
    assert_eq!(name("R1", 1).as_deref(), Some("my_net"));
    assert_ne!(sig("R2", 2).map(|n| n.id), sig("R3", 1).map(|n| n.id));
    assert_eq!(report.count(Case::NetRenamed), 2);
    Ok(())
}

#[test]
fn tracks_and_vias_land_on_their_nets_and_layers() -> Fallible<()> {
    let imported = import(routed().text().as_bytes())?;
    let project = imported.engine.project();
    let report = &imported.report;
    let r1 = project.instance_by_designator("R1").ok_or("R1")?;
    let sig = project
        .net_of(sparksmith_core::Endpoint {
            instance: r1.id,
            terminal: sparksmith_core::TerminalId(2),
        })
        .ok_or("SIG")?
        .id;
    let on = |layer: CopperLayer| {
        project
            .tracks
            .iter()
            .filter(|t| t.net == sig && t.layer == layer)
            .count()
    };
    assert_eq!((on(CopperLayer::Top), on(CopperLayer::Bottom)), (1, 1));
    let via = project.vias.first().ok_or("no via")?;
    assert_eq!((via.net, via.span), (sig, ViaSpan::Through));
    assert!((via.drill - 0.3).abs() < 1e-9 && (via.diameter - 0.6).abs() < 1e-9);
    // The outline's lower-left corner, KiCad's (0, 20), is our origin:
    // KiCad's (20, 10) is (20, 20 − 10) here.
    assert!((via.position[0] - 20.0).abs() < 1e-9 && (via.position[1] - 10.0).abs() < 1e-9);
    // The arc became chords from (9, 10) over (8, 11) to (7, 10).
    assert_eq!(report.count(Case::ArcTrack), 1);
    let chords: Vec<_> = project
        .tracks
        .iter()
        .filter(|t| t.net != sig && (t.width - 0.2).abs() < 1e-9)
        .collect();
    assert!(chords.len() > 8, "{}", chords.len());
    let top = chords
        .iter()
        .flat_map(|t| [t.start[1], t.end[1]])
        .fold(f64::MIN, f64::max);
    assert!((top - 11.0).abs() < 1e-3, "{top}");
    // The 4 µm piece is below the engine's least length; the track on a
    // net without pads has nowhere to go. Both are counted.
    assert_eq!(report.count(Case::TrackTooShort), 1);
    assert_eq!(report.count(Case::TrackWithoutNet), 1);
    assert_eq!(report.count(Case::TrackRefused), 0);
    assert_eq!(report.counts.vias, 1);
    Ok(())
}

#[test]
fn a_track_the_checks_refuse_is_listed_and_the_rest_imported() -> Fallible<()> {
    let mut board = routed();
    // A SIG track across R3's GND pads: the clearance check refuses it.
    board
        .items
        .push_str("  (segment (start 18 16) (end 22 16) (width 0.25) (layer \"F.Cu\") (net 1))\n");
    let imported = import(board.text().as_bytes())?;
    assert_eq!(imported.report.count(Case::TrackRefused), 1);
    assert!(imported.report.counts.tracks > 2);
    Ok(())
}
