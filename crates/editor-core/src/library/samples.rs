//! Original, unverified starter data for local editing. These are generic
//! two-terminal arrangements, not component categories; every value,
//! including the designator prefix and body style the caller passes, is
//! ordinary definition data a user then edits.
use super::graphics::{self, HAlign, Justify, PIN_TEXT_HEIGHT_MM, TextContent, VAlign};
use super::templates;
use super::{
    BindingDefinition, ComponentDefinition, Definition, DefinitionMeta, DraftEdit,
    FootprintDefinition, PackageBody, PackageDefinition, PackageTerminal, Pad, SymbolDefinition,
    Terminal, TerminalClass, TerminalMap, Unit,
};
use crate::fields::{FieldDefinition, QuantityKind, ValueType};
use crate::functions::{Direction, TerminalFunctions};
use crate::geometry::{Bounds, Point};
use crate::ids::{
    BindingRef, DefinitionId, FootprintRef, FunctionId, GraphicId, PackageRef, PackageTerminalId,
    PadId, QuantityKindRef, SymbolRef, TerminalId, UnitId,
};
use crate::quantity::{UnitOfMeasure, UnitSet};

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

/// What a sample is: its designator prefix and package body.
#[derive(Clone, Copy, Debug)]
pub struct Kind<'a> {
    pub prefix: &'a str,
    pub body: PackageBody,
}

impl Kind<'static> {
    pub const RESISTOR: Self = Self {
        prefix: "R",
        body: PackageBody::ChipResistor,
    };
    pub const CAPACITOR: Self = Self {
        prefix: "C",
        body: PackageBody::ChipCapacitor,
    };
    pub const CONNECTOR: Self = Self {
        prefix: "J",
        body: PackageBody::Moulded,
    };
}

struct Layout {
    /// The body rectangle, in the unit frame.
    body: Bounds,
    /// Pin connection ends.
    pins: [Point; 2],
    envelope: [f64; 3],
    offset: [f64; 3],
    pads: [Point; 2],
    land: Point,
}

fn sample(name: &str, kind: Kind<'_>, refs: SampleRefs, layout: Layout) -> Vec<DraftEdit> {
    let terminals = [TerminalId(1), TerminalId(2)];
    let package_terminals = [PackageTerminalId(1), PackageTerminalId(2)];
    let package = PackageDefinition {
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
        body: kind.body,
    };
    vec![
        DraftEdit {
            id: refs.component,
            meta: meta(name),
            definition: Definition::Component(ComponentDefinition {
                // Terminals 1–2, then their fixed passive functions 3–4.
                next_local: 5,
                designator_prefix: kind.prefix.into(),
                terminals: terminals
                    .iter()
                    .zip([FunctionId(3), FunctionId(4)])
                    .map(|(id, function)| Terminal {
                        id: *id,
                        name: id.to_string(),
                        class: TerminalClass::Electrical(TerminalFunctions::fixed(
                            function,
                            Direction::Passive,
                        )),
                    })
                    .collect(),
                resources: vec![],
                constraints: vec![],
                representations: vec![refs.symbol],
                implementations: vec![refs.binding],
                fields: vec![],
            }),
        },
        DraftEdit {
            id: refs.symbol.id,
            meta: meta(&format!("{name} symbol")),
            definition: Definition::Symbol(symbol(refs.component, &terminals, &layout)),
        },
        DraftEdit {
            id: refs.package.id,
            meta: meta(&format!("{name} package")),
            definition: Definition::Package(package.clone()),
        },
        DraftEdit {
            id: refs.footprint.id,
            meta: meta(&format!("{name} footprint")),
            definition: Definition::Footprint(footprint(refs.package, &package, &layout)),
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
                unavailable: vec![],
            }),
        },
    ]
}

/// Two rectangular top-side lands, pads 1–2 landing package terminals 1–2,
/// with the standard drawings (graphics from local ID 3).
fn footprint(
    package_ref: PackageRef,
    package: &PackageDefinition,
    layout: &Layout,
) -> FootprintDefinition {
    let pads = [PackageTerminalId(1), PackageTerminalId(2)]
        .iter()
        .zip(layout.pads)
        .zip([PadId(1), PadId(2)])
        .map(|((terminal, position), id)| {
            Pad::smd_rect(id, *terminal, id.0.to_string(), position, layout.land)
        })
        .collect();
    let mut footprint = FootprintDefinition {
        package: package_ref,
        next_local: 3,
        pads,
        ..FootprintDefinition::default()
    };
    redraw(&mut footprint, package);
    footprint
}

/// Replace a two-land sample footprint's drawings with the standard ones
/// for its current pads and `package` (graphics from local ID 3). A footprint
/// whose identities are exhausted keeps no drawings.
pub fn redraw(footprint: &mut FootprintDefinition, package: &PackageDefinition) {
    footprint.graphics.clear();
    footprint.polarity = None;
    footprint.next_local = 3;
    if templates::footprint::add_standard_graphics(footprint, package).is_err() {
        footprint.graphics.clear();
    }
}

/// One unit: a body rectangle, a pin per terminal pointing into it, the
/// designator above the body and the component name below it. Unit 1 and
/// graphics 2–4 use the symbol's local IDs. The samples are passives and
/// connectors whose terminal names are their pin numbers, so pins show
/// numbers only.
fn symbol(component: DefinitionId, terminals: &[TerminalId], layout: &Layout) -> SymbolDefinition {
    let body = layout.body;
    let gap = 0.5;
    SymbolDefinition {
        component,
        next_local: 5,
        pin_text_height: PIN_TEXT_HEIGHT_MM,
        units: vec![Unit {
            id: UnitId(1),
            label: "A".into(),
            graphics: vec![
                graphics::body_rect(GraphicId(2), body),
                graphics::text(
                    GraphicId(3),
                    TextContent::Designator,
                    [body.center()[0], body.max[1] + gap],
                    Justify {
                        h: HAlign::Center,
                        v: VAlign::Bottom,
                    },
                ),
                graphics::text(
                    GraphicId(4),
                    TextContent::ComponentName,
                    [body.center()[0], body.min[1] - gap],
                    Justify {
                        h: HAlign::Center,
                        v: VAlign::Top,
                    },
                ),
            ],
            pins: terminals
                .iter()
                .zip(layout.pins)
                .map(|(terminal, position)| {
                    let mut pin = graphics::pin_toward(*terminal, position, body);
                    pin.name.visible = false;
                    pin
                })
                .collect(),
        }],
    }
}

/// A two-terminal component with an axial symbol, a small two-land footprint
/// and a straight terminal mapping.
pub fn two_terminal(name: &str, kind: Kind<'_>, refs: SampleRefs) -> Vec<DraftEdit> {
    sample(
        name,
        kind,
        refs,
        Layout {
            body: Bounds {
                min: [-2.5, -1.0],
                max: [2.5, 1.0],
            },
            pins: [[-5.0, 0.0], [5.0, 0.0]],
            envelope: [2.0, 1.2, 0.6],
            offset: [0.0, 0.0, 0.05],
            pads: [[-1.2, 0.0], [1.2, 0.0]],
            land: [1.0, 1.4],
        },
    )
}

/// The same two terminals with both pins on one side and a 2.54 mm land
/// pitch.
pub fn two_terminal_header(name: &str, kind: Kind<'_>, refs: SampleRefs) -> Vec<DraftEdit> {
    sample(
        name,
        kind,
        refs,
        Layout {
            body: Bounds {
                min: [-2.5, -3.75],
                max: [2.5, 3.75],
            },
            pins: [[-5.0, 2.5], [-5.0, -2.5]],
            envelope: [5.0, 3.5, 4.0],
            offset: [0.0, 0.0, 0.05],
            pads: [[-1.27, 0.0], [1.27, 0.0]],
            land: [1.5, 2.0],
        },
    )
}

/// A resistance quantity kind (ohm, SI prefixes) and a non-negative
/// quantity field of that kind: ordinary library data, not a category.
pub fn resistance_field(kind: QuantityKindRef, field: DefinitionId) -> Vec<DraftEdit> {
    vec![
        DraftEdit {
            id: kind.id,
            meta: meta("Resistance"),
            definition: Definition::QuantityKind(QuantityKind {
                units: UnitSet {
                    dimension: [2, 1, -3, -2, 0, 0, 0],
                    units: vec![UnitOfMeasure {
                        symbol: "Ω".into(),
                        aliases: vec!["ohm".into(), "R".into()],
                        scale: 1.0,
                        si_prefixes: true,
                    }],
                },
                default_unit: "Ω".into(),
            }),
        },
        DraftEdit {
            id: field,
            meta: meta("Resistance"),
            definition: Definition::Field(FieldDefinition {
                next_local: 1,
                value_type: ValueType::Quantity {
                    kind,
                    min: Some(0.0),
                    max: None,
                },
                default: None,
                required_at: None,
            }),
        },
    ]
}
