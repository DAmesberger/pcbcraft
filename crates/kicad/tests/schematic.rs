//! A Rust-built schematic over a Rust-built board (slice K5): sheets,
//! symbols at KiCad's places with KiCad's pins, wires, a power symbol and
//! a label drawn through the engine, and the schematic's nets equal to the
//! board's. All data here is original.
mod support;

use sparksmith_kicad::report::Case;
use std::collections::BTreeMap;
use support::{Board, Footprint, Pad};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Two resistors: R1.1 on GND, R1.2 and R2.1 on SIG, R2.2 on OUT, plus a
/// pin of type "not connected" on each that joins nothing.
fn board() -> Board {
    let pad = |number: &'static str, x: f64, net: (u32, &str)| {
        Pad::smd(number, "rect", [x, 0.0, 0.0], [0.8, 0.6])
            .with(&format!("(net {} \"{}\")", net.0, net.1))
    };
    let resistor = |reference: &str, x: f64, nets: [(u32, &str); 2]| {
        Footprint::new(
            reference,
            [x, 10.0, 0.0],
            false,
            vec![pad("1", -1.0, nets[0]), pad("2", 1.0, nets[1])],
        )
    };
    Board {
        copper: 2,
        origin: None,
        setup: "(pad_to_mask_clearance 0)".into(),
        footprints: vec![
            resistor("R1", 10.0, [(1, "GND"), (2, "SIG")]),
            resistor("R2", 20.0, [(2, "SIG"), (3, "OUT")]),
        ],
        items: "  (net 1 \"GND\") (net 2 \"SIG\") (net 3 \"OUT\")
  (gr_rect (start 0 0) (end 30 20) (stroke (width 0.1) (type default)) (layer \"Edge.Cuts\"))\n"
            .into(),
    }
}

/// The root sheet: R1 at (50, 50), R2 at (60, 50) turned 180°, a wire
/// with three corners from R1.2 to R2.1, a GND power symbol on R1.1 and a
/// local label OUT on R2.2. Pins are 3.81 mm above and below each
/// symbol's origin (library frame, y up; the sheet's y is down, so R1.2
/// and the turned R2's pin 1 are at y = 53.81).
fn schematic() -> String {
    let resistor_lib = "(symbol \"Synthetic:R\" (in_bom yes) (on_board yes)
      (symbol \"R_0_1\" (rectangle (start -1 2.54) (end 1 -2.54)))
      (symbol \"R_1_1\"
        (pin passive line (at 0 3.81 270) (length 1.27) (name \"~\") (number \"1\"))
        (pin passive line (at 0 -3.81 90) (length 1.27) (name \"~\") (number \"2\"))
        (pin no_connect line (at 2.54 0 180) (length 1.27) hide (name \"NC\") (number \"3\"))))";
    let gnd_lib = "(symbol \"Synthetic:GND\" (power) (in_bom yes) (on_board yes)
      (symbol \"GND_1_1\" (pin power_in line (at 0 0 270) (length 0) hide (name \"GND\") (number \"1\"))))";
    let symbol = |lib: &str, reference: &str, value: &str, at: &str| {
        format!(
            "(symbol (lib_id \"{lib}\") (at {at}) (unit 1)
               (property \"Reference\" \"{reference}\" (at 0 0 0))
               (property \"Value\" \"{value}\" (at 0 0 0))
               (instances (project \"synthetic\" (path \"/00000000-0000-4000-8000-000000000001\" (reference \"{reference}\") (unit 1)))))"
        )
    };
    format!(
        "(kicad_sch (version 20231120) (generator \"sparksmith_test\")
  (uuid \"00000000-0000-4000-8000-000000000001\")
  (paper \"A4\")
  (lib_symbols {resistor_lib} {gnd_lib})
  {r1} {r2} {gnd}
  (wire (pts (xy 50 53.81) (xy 55 53.81)))
  (wire (pts (xy 55 53.81) (xy 55 58) (xy 60 58)))
  (wire (pts (xy 60 58) (xy 60 53.81)))
  (label \"OUT\" (at 60 46.19 0))
)",
        r1 = symbol("Synthetic:R", "R1", "10k", "50 50 0"),
        // Turned half round: its pin 1 is below, pin 2 above.
        r2 = symbol("Synthetic:R", "R2", "10k", "60 50 180"),
        gnd = symbol("Synthetic:GND", "#PWR01", "GND", "50 46.19 0"),
    )
}

#[test]
fn the_schematic_is_drawn_and_its_nets_equal_the_boards() -> Fallible<()> {
    let mut files = BTreeMap::new();
    files.insert("synthetic.kicad_sch".to_owned(), schematic().into_bytes());
    let imported = support::import_with_schematic(
        board().text().as_bytes(),
        None,
        Some(("synthetic.kicad_sch", &files)),
    )?;
    let project = imported.engine.project();
    let report = &imported.report;
    assert_eq!(report.counts.sheets, 1);
    assert_eq!(
        (
            report.counts.schematic_nets_equal,
            report.counts.schematic_nets_split,
            report.counts.schematic_nets_merged
        ),
        (3, 0, 0)
    );
    // The wire with its corners, the power port and the label; no
    // labels made up per pin.
    assert_eq!(report.count(Case::SchematicWire), 1);
    assert_eq!(report.count(Case::SchematicLabel), 2);
    assert_eq!(project.labels.len(), 2);
    assert_eq!(report.count(Case::NoConnectPin), 2);
    // R1 sits where KiCad has it: (50, 50) is (20, −20) on our sheet.
    let r1 = project.instance_by_designator("R1").ok_or("R1")?;
    let occurrence = project.occurrences_of(r1.id).next().ok_or("on no sheet")?;
    let at = occurrence.position.to_mm_f64();
    assert!((at[0] - 20.0).abs() < 1e-9 && (at[1] + 20.0).abs() < 1e-9);
    // Its symbol has KiCad's pins: pin 1 at (0, 3.81), and the "not
    // connected" pin drawn too.
    let symbol = project
        .library
        .symbol(sparksmith_core::SymbolRef::new(
            occurrence_symbol(project, r1.id)?,
            sparksmith_core::Revision::FIRST,
        ))
        .ok_or("symbol")?;
    let pins: Vec<_> = symbol.definition.units.iter().flat_map(|u| &u.pins).collect();
    assert_eq!(pins.len(), 2);
    let p = pins[0].position.to_mm_f64();
    assert!(p[0].abs() < 1e-9 && (p[1] - 3.81).abs() < 1e-9, "{p:?}");
    // Both resistors share the SIG net through the wire.
    let net = |d: &str, t: u32| -> Option<sparksmith_core::Id> {
        let i = project.instance_by_designator(d)?;
        project
            .net_of(sparksmith_core::Endpoint {
                instance: i.id,
                terminal: sparksmith_core::TerminalId(t),
            })
            .map(|n| n.id)
    };
    assert!(net("R1", 2).is_some());
    assert_eq!(net("R1", 2), net("R2", 1));
    assert_ne!(net("R1", 1), net("R1", 2));
    Ok(())
}

fn occurrence_symbol(
    project: &sparksmith_core::Project,
    instance: sparksmith_core::Id,
) -> Fallible<sparksmith_core::DefinitionId> {
    Ok(project.instance(instance).ok_or("instance")?.representation)
}
