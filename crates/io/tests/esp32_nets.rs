//! The shipped ESP32-S31 dev board keeps its netlist, and its drawing is
//! honest: no wire runs over a connection point it does not connect to.
//! `examples/esp32-s31-devboard/nets.json` holds each net's name and its
//! members as `designator.terminal`, taken from the example before the
//! schematic connected what its wires run over.
use sparksmith_core::schematic;
use std::collections::BTreeMap;
use std::path::Path;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn the_example_keeps_its_nets_and_no_wire_runs_over_a_point() -> Fallible {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/esp32-s31-devboard");
    let project = sparksmith_io::read_project(&dir.join("esp32-s31-devboard.sparksmith"))?.project;
    let want: BTreeMap<String, Vec<String>> =
        serde_json::from_slice(&std::fs::read(dir.join("nets.json"))?)?;
    let designator = |id| {
        project
            .instance(id)
            .map(|i| i.designator.clone())
            .unwrap_or_default()
    };
    let got: BTreeMap<String, Vec<String>> = project
        .nets
        .iter()
        .map(|n| {
            let mut members: Vec<String> = n
                .terminals
                .iter()
                .map(|e| format!("{}.{}", designator(e.instance), e.terminal.0))
                .collect();
            members.sort();
            (n.name.clone(), members)
        })
        .collect();
    assert_eq!(got.len(), 75);
    for (name, members) in &want {
        assert_eq!(got.get(name), Some(members), "{name}");
    }
    assert_eq!(got, want);
    let mut again = project.clone();
    schematic::tidy(&mut again)?;
    assert_eq!(again.wires, project.wires, "a wire runs over a point");
    Ok(())
}
