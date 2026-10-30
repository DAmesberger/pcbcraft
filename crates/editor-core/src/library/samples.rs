//! Original, unverified starter data for local editing. These are generic
//! two-terminal arrangements, not component categories; every value is
//! ordinary definition data a user then edits.
use super::{
    Anchor, BindingDefinition, ComponentDefinition, Definition, DefinitionMeta, DraftEdit,
    FootprintDefinition, PackageDefinition, PackageTerminal, Pad, PinKind, SymbolDefinition,
    Terminal, TerminalMap, Unit,
};
use crate::geometry::Point;
use crate::ids::{
    BindingRef, DefinitionId, FootprintRef, PackageRef, PackageTerminalId, PadId, SymbolRef,
    TerminalId, UnitId,
};

/// The five lineages of one sample and the revisions they pin of each other
/// (normally the drafts' pending revisions).
#[derive(Clone, Copy, Debug)]
pub struct SampleRefs {
    pub component: DefinitionId,
    pub symbol: SymbolRef,
    pub package: PackageRef,
    pub footprint: FootprintRef,
    pub binding: BindingRef,
}

const PROVENANCE: &str =
    "Locally authored generic sample; verify against the selected component datasheet.";

fn meta(name: &str) -> DefinitionMeta {
    DefinitionMeta {
        name: name.into(),
        description: String::new(),
        provenance: PROVENANCE.into(),
    }
}

struct Layout {
    body: Point,
    anchors: [Point; 2],
    envelope: [f64; 3],
    offset: [f64; 3],
    pads: [Point; 2],
    land: Point,
}

fn sample(name: &str, refs: SampleRefs, layout: Layout) -> Vec<DraftEdit> {
    let terminals = [TerminalId(1), TerminalId(2)];
    let package_terminals = [PackageTerminalId(1), PackageTerminalId(2)];
    vec![
        DraftEdit {
            id: refs.component,
            meta: meta(name),
            definition: Definition::Component(ComponentDefinition {
                next_local: 3,
                terminals: terminals
                    .iter()
                    .map(|id| Terminal {
                        id: *id,
                        name: id.to_string(),
                        kind: PinKind::Passive,
                    })
                    .collect(),
                representations: vec![refs.symbol],
                implementations: vec![refs.binding],
            }),
        },
        DraftEdit {
            id: refs.symbol.id,
            meta: meta(&format!("{name} symbol")),
            definition: Definition::Symbol(SymbolDefinition {
                component: refs.component,
                next_local: 2,
                units: vec![Unit {
                    id: UnitId(1),
                    label: "A".into(),
                    body_size: layout.body,
                    anchors: terminals
                        .iter()
                        .zip(layout.anchors)
                        .map(|(terminal, position)| Anchor {
                            terminal: *terminal,
                            position,
                        })
                        .collect(),
                }],
            }),
        },
        DraftEdit {
            id: refs.package.id,
            meta: meta(&format!("{name} package")),
            definition: Definition::Package(PackageDefinition {
                next_local: 3,
                terminals: package_terminals
                    .iter()
                    .map(|id| PackageTerminal {
                        id: *id,
                        designation: id.to_string(),
                    })
                    .collect(),
                body_size: layout.envelope,
                model_offset: layout.offset,
            }),
        },
        DraftEdit {
            id: refs.footprint.id,
            meta: meta(&format!("{name} footprint")),
            definition: Definition::Footprint(FootprintDefinition {
                package: refs.package,
                next_local: 3,
                pads: package_terminals
                    .iter()
                    .zip(layout.pads)
                    .enumerate()
                    .map(|(i, (terminal, position))| Pad {
                        id: PadId(if i == 0 { 1 } else { 2 }),
                        terminal: *terminal,
                        position,
                        size: layout.land,
                    })
                    .collect(),
                datum_offset: [0.0, 0.0],
            }),
        },
        DraftEdit {
            id: refs.binding.id,
            meta: meta(&format!("{name} binding")),
            definition: Definition::Binding(BindingDefinition {
                component: refs.component,
                package: refs.package,
                mappings: package_terminals
                    .iter()
                    .zip(terminals)
                    .map(|(package_terminal, terminal)| TerminalMap {
                        package_terminal: *package_terminal,
                        terminal,
                    })
                    .collect(),
                footprints: vec![refs.footprint],
            }),
        },
    ]
}

/// A two-terminal component with an axial symbol, a small two-land footprint
/// and a straight terminal mapping.
pub fn two_terminal(name: &str, refs: SampleRefs) -> Vec<DraftEdit> {
    sample(
        name,
        refs,
        Layout {
            body: [6.0, 3.0],
            anchors: [[-5.0, 0.0], [5.0, 0.0]],
            envelope: [2.0, 1.2, 0.6],
            offset: [0.0, 0.0, 0.05],
            pads: [[-1.2, 0.0], [1.2, 0.0]],
            land: [1.0, 1.4],
        },
    )
}

/// The same two terminals with both anchors on one side and a 2.54 mm land
/// pitch.
pub fn two_terminal_header(name: &str, refs: SampleRefs) -> Vec<DraftEdit> {
    sample(
        name,
        refs,
        Layout {
            body: [4.0, 6.0],
            anchors: [[-5.0, 1.5], [-5.0, -1.5]],
            envelope: [5.0, 3.5, 4.0],
            offset: [0.0, 0.0, 0.05],
            pads: [[-1.27, 0.0], [1.27, 0.0]],
            land: [1.5, 2.0],
        },
    )
}
