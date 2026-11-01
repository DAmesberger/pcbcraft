//! Original, unverified starter data for local editing. These are generic
//! two-terminal arrangements, not component categories; every value,
//! including the designator prefix and body style the caller passes, is
//! ordinary definition data a user then edits.
use super::footprint::{Hole, HoleShape, PadKind, PadShape};
use super::graphics::{self, HAlign, Justify, PIN_TEXT_HEIGHT_MM, TextContent, VAlign};
use super::templates;
use super::{
    BindingDefinition, ChipForm, ChipKind, ComponentDefinition, Definition, DefinitionMeta,
    DraftEdit, FootprintDefinition, GeneratedMaterials, GullWingForm, HeaderForm, LeadSides,
    ModelPlacement, NoLeadForm, PackageDefinition, PackageForm, PackageModel, PackageTerminal, Pad,
    SymbolDefinition, Terminal, TerminalClass, TerminalMap, Unit,
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

/// What a sample is: its designator prefix and, for a chip, its chip
/// construction (`None` is a generic moulded envelope).
#[derive(Clone, Copy, Debug)]
pub struct Kind<'a> {
    pub prefix: &'a str,
    pub chip: Option<ChipKind>,
}

impl Kind<'static> {
    pub const RESISTOR: Self = Self {
        prefix: "R",
        chip: Some(ChipKind::Resistor),
    };
    pub const CAPACITOR: Self = Self {
        prefix: "C",
        chip: Some(ChipKind::Capacitor),
    };
    pub const CONNECTOR: Self = Self {
        prefix: "J",
        chip: None,
    };
}

/// The builder revision a sample's generated model is authored against
/// (`models::family_rev` in the integration crate).
const SAMPLE_FAMILY_REV: u16 = 1;

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
        form: kind.chip.map_or(PackageForm::Generic, |chip| {
            PackageForm::Chip(Box::new(ChipForm::new(chip, layout.envelope)))
        }),
        envelope: layout.envelope,
        model: match kind.chip {
            Some(_) => PackageModel::Generated {
                family_rev: SAMPLE_FAMILY_REV,
                materials: GeneratedMaterials::default(),
            },
            None => PackageModel::default(),
        },
        // A generated model is seated on the lands (z = 0, the seating
        // plane); only an envelope keeps the layout's lift.
        placement: match kind.chip {
            Some(_) => ModelPlacement::default(),
            None => ModelPlacement {
                translation: layout.offset,
                rotation_deg: [0.0; 3],
            },
        },
    };
    vec![
        DraftEdit {
            id: refs.component,
            meta: meta(name),
            definition: Definition::Component(ComponentDefinition {
                pin_rules: Vec::new(),
                marking: None,
                review: None,
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
            // Land centres on the chip's termination centres (length / 2 −
            // termination / 2 = 0.8 mm), so the leads fit their pads.
            pads: [[-0.8, 0.0], [0.8, 0.0]],
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

/// A typed package preset: original, typical outline values (not a
/// supplier's drawing), the form a user then edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// 0603 (1608 metric) chip resistor.
    Chip0603,
    /// 0805 (2012 metric) chip resistor.
    Chip0805,
    /// 8-lead, 1.27 mm pitch gull-wing on two sides.
    Soic8,
    /// 32-lead, 0.8 mm pitch gull-wing on four sides.
    Tqfp32,
    /// 16-terminal, 0.5 mm pitch no-lead with an exposed pad.
    Qfn16,
    /// 8-terminal, 0.5 mm pitch no-lead on two sides.
    Dfn8,
    /// 2 × 5 pins at 2.54 mm.
    Header2x5,
    /// 1 × 2 pins at 2.54 mm.
    Header1x2,
}

impl Preset {
    pub const ALL: [Self; 8] = [
        Self::Chip0603,
        Self::Chip0805,
        Self::Soic8,
        Self::Tqfp32,
        Self::Qfn16,
        Self::Dfn8,
        Self::Header2x5,
        Self::Header1x2,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chip0603 => "0603 chip",
            Self::Chip0805 => "0805 chip",
            Self::Soic8 => "SOIC-8",
            Self::Tqfp32 => "TQFP-32",
            Self::Qfn16 => "QFN-16",
            Self::Dfn8 => "DFN-8",
            Self::Header2x5 => "Header 2×5",
            Self::Header1x2 => "Header 1×2",
        }
    }

    /// The preset's typed form (millimetres, typical values).
    pub fn form(self) -> PackageForm {
        let gull =
            |sides, leads_per_side, pitch, body: [f64; 3], span: [f64; 2], lead: [f64; 3]| {
                PackageForm::GullWing(Box::new(GullWingForm {
                    sides,
                    leads_per_side,
                    pitch,
                    body_length: body[0],
                    body_width: body[1],
                    body_height: body[2],
                    standoff: 0.1,
                    span,
                    lead_width: lead[0],
                    lead_thickness: lead[1],
                    foot_length: lead[2],
                }))
            };
        let header = |rows| {
            PackageForm::Header(Box::new(HeaderForm {
                rows,
                pins_per_row: if rows == 2 { 5 } else { 2 },
                pitch: 2.54,
                row_pitch: 2.54,
                body_height: 2.5,
                pin_side: 0.64,
                pin_above: 6.0,
                pin_below: 3.0,
            }))
        };
        match self {
            Self::Chip0603 => PackageForm::Chip(Box::new(ChipForm::new(
                ChipKind::Resistor,
                [1.6, 0.8, 0.45],
            ))),
            Self::Chip0805 => PackageForm::Chip(Box::new(ChipForm::new(
                ChipKind::Resistor,
                [2.0, 1.25, 0.5],
            ))),
            Self::Soic8 => gull(
                LeadSides::Two,
                [4, 0],
                1.27,
                [4.9, 3.9, 1.45],
                [6.0, 0.0],
                [0.41, 0.2, 0.6],
            ),
            Self::Tqfp32 => gull(
                LeadSides::Four,
                [8, 8],
                0.8,
                [7.0, 7.0, 1.0],
                [9.0, 9.0],
                [0.37, 0.15, 0.6],
            ),
            Self::Qfn16 => PackageForm::NoLead(Box::new(NoLeadForm {
                sides: LeadSides::Four,
                terminals_per_side: [4, 4],
                pitch: 0.5,
                body_length: 3.0,
                body_width: 3.0,
                body_height: 0.85,
                terminal_length: 0.4,
                terminal_width: 0.25,
                exposed_pad: Some([1.7, 1.7]),
            })),
            Self::Dfn8 => PackageForm::NoLead(Box::new(NoLeadForm {
                sides: LeadSides::Two,
                terminals_per_side: [4, 0],
                pitch: 0.5,
                body_length: 2.0,
                body_width: 2.0,
                body_height: 0.75,
                terminal_length: 0.35,
                terminal_width: 0.25,
                exposed_pad: None,
            })),
            Self::Header2x5 => header(2),
            Self::Header1x2 => header(1),
        }
    }

    /// The designator prefix a component of this package usually takes.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Chip0603 | Self::Chip0805 => "R",
            Self::Header2x5 | Self::Header1x2 => "J",
            _ => "U",
        }
    }
}

/// Lands for the leads of a typed `form`, in lead order, landing
/// `terminals[k]` (pad IDs and numbers from 1): each centred on its lead
/// contact so the leads fit their pads, sized from the lead (original
/// rules of thumb, not a land-pattern standard), plated-through for header
/// pins, and the exposed pad last when the package lists it as a terminal.
/// `None` for `Generic` or when the terminal count does not match.
pub fn form_lands(form: &PackageForm, terminals: &[PackageTerminalId]) -> Option<Vec<Pad>> {
    let points = form.lead_points();
    let leads = usize::try_from(form.lead_count()).ok()?;
    if points.is_empty() || !form.terminal_counts().contains(&(terminals.len() as u64)) {
        return None;
    }
    // Lead k's land size [along x, along y].
    let size = |k: usize| -> [f64; 2] {
        match form {
            PackageForm::Chip(c) => [c.termination + 0.6, c.width + 0.2],
            PackageForm::GullWing(g) => {
                let land = [g.lead_width + 0.2, g.foot_length + 0.6];
                if across_x(g.sides, g.leads_per_side, k) {
                    [land[1], land[0]]
                } else {
                    land
                }
            }
            PackageForm::NoLead(n) => {
                let land = [n.terminal_width + 0.05, n.terminal_length + 0.4];
                if across_x(n.sides, n.terminals_per_side, k) {
                    [land[1], land[0]]
                } else {
                    land
                }
            }
            PackageForm::Header(h) => {
                let d = h.pin_side * std::f64::consts::SQRT_2 + 0.8;
                [d, d]
            }
            PackageForm::Generic
            | PackageForm::Tabbed(_)
            | PackageForm::Molded(_)
            | PackageForm::Grid(_)
            | PackageForm::Tht(_) => [1.0, 1.0],
        }
    };
    let mut pads = Vec::with_capacity(terminals.len());
    for (k, (terminal, point)) in terminals.iter().zip(&points).enumerate() {
        let id = PadId(u32::try_from(k + 1).ok()?);
        let at = [point[0], point[1]];
        let mut pad = Pad::smd_rect(id, *terminal, id.0.to_string(), at, size(k));
        if k >= leads {
            // The exposed pad.
            if let PackageForm::NoLead(n) = form
                && let Some(ep) = n.exposed_pad
            {
                pad.shape = PadShape::Rect { size: ep };
                pad.number = "EP".into();
            }
        } else if let PackageForm::Header(h) = form {
            let diameter = size(k)[0];
            pad.kind = PadKind::PlatedThrough;
            // Pin 1 square, the others round.
            if k > 0 {
                pad.shape = PadShape::Circle { diameter };
            }
            pad.hole = Some(Hole {
                shape: HoleShape::Round {
                    diameter: h.pin_side * std::f64::consts::SQRT_2 + 0.1,
                },
                plated: true,
                offset: [0.0, 0.0],
            });
        }
        pads.push(pad);
    }
    Some(pads)
}

/// Lead `k` of a row layout lies on a short (±x) side.
fn across_x(sides: LeadSides, per: [u32; 2], k: usize) -> bool {
    let n = per[0] as usize;
    let m = per[1] as usize;
    sides == LeadSides::Four && ((n..n + m).contains(&k) || (2 * n + m..2 * (n + m)).contains(&k))
}

/// A component of `preset`'s package: one passive terminal per lead (and
/// the exposed pad), a rectangular symbol with pins down both sides,
/// form-derived lands and a straight terminal mapping. The package model
/// is generated.
pub fn packaged(name: &str, preset: Preset, refs: SampleRefs) -> Vec<DraftEdit> {
    packaged_form(name, preset.form(), preset.prefix(), refs)
}

/// [`packaged`] for any typed `form` and designator `prefix`.
pub fn packaged_form(
    name: &str,
    form: PackageForm,
    prefix: &str,
    refs: SampleRefs,
) -> Vec<DraftEdit> {
    let count = form
        .terminal_counts()
        .last()
        .copied()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    let leads = u32::try_from(form.lead_count()).unwrap_or(0);
    let terminals: Vec<TerminalId> = (1..=count).map(TerminalId).collect();
    let package_terminals: Vec<PackageTerminalId> = (1..=count).map(PackageTerminalId).collect();
    let package = PackageDefinition {
        next_local: count + 1,
        terminals: package_terminals
            .iter()
            .map(|id| PackageTerminal {
                id: *id,
                designation: if id.0 > leads {
                    "EP".into()
                } else {
                    id.to_string()
                },
            })
            .collect(),
        envelope: form.envelope().unwrap_or([1.0; 3]),
        form,
        model: PackageModel::Generated {
            family_rev: SAMPLE_FAMILY_REV,
            materials: GeneratedMaterials::default(),
        },
        placement: ModelPlacement::default(),
    };
    let pads = form_lands(&package.form, &package_terminals).unwrap_or_default();
    let mut footprint = FootprintDefinition {
        package: refs.package,
        next_local: u32::try_from(pads.len() + 1).unwrap_or(1),
        pads,
        ..FootprintDefinition::default()
    };
    footprint.graphics.clear();
    let first_graphic = footprint.next_local;
    if templates::footprint::add_standard_graphics(&mut footprint, &package).is_err() {
        footprint.graphics.clear();
        footprint.next_local = first_graphic;
    }
    vec![
        DraftEdit {
            id: refs.component,
            meta: meta(name),
            definition: Definition::Component(ComponentDefinition {
                pin_rules: Vec::new(),
                marking: None,
                review: None,
                next_local: 2 * count + 1,
                designator_prefix: prefix.into(),
                terminals: terminals
                    .iter()
                    .map(|id| Terminal {
                        id: *id,
                        name: if id.0 > leads {
                            "EP".into()
                        } else {
                            id.to_string()
                        },
                        class: TerminalClass::Electrical(TerminalFunctions::fixed(
                            FunctionId(id.0 + count),
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
            definition: Definition::Symbol(block_symbol(refs.component, &terminals)),
        },
        DraftEdit {
            id: refs.package.id,
            meta: meta(&format!("{name} package")),
            definition: Definition::Package(package),
        },
        DraftEdit {
            id: refs.footprint.id,
            meta: meta(&format!("{name} footprint")),
            definition: Definition::Footprint(footprint),
        },
        DraftEdit {
            id: refs.binding.id,
            meta: meta(&format!("{name} binding")),
            definition: Definition::Binding(BindingDefinition {
                component: refs.component,
                package: refs.package,
                mappings: package_terminals
                    .iter()
                    .zip(&terminals)
                    .map(|(package_terminal, terminal)| TerminalMap {
                        package_terminal: *package_terminal,
                        terminal: *terminal,
                    })
                    .collect(),
                footprints: vec![refs.footprint],
                unavailable: vec![],
            }),
        },
    ]
}

/// Silkscreen pin-1 dot radius, in mm.
pub const PIN_ONE_DOT_RADIUS_MM: f64 = 0.2;

/// Mark pad 1 of the footprint among `edits` as the polarity pad, with a
/// filled silkscreen dot just beyond it on the footprint's −x side (where
/// the presets start numbering), clear of its mask opening. Edits without
/// a footprint, or a footprint without pad 1, are left unchanged.
pub fn mark_pin_one(edits: &mut [DraftEdit]) {
    for edit in edits {
        let Definition::Footprint(footprint) = &mut edit.definition else {
            continue;
        };
        let Some(pad) = footprint.pad(PadId(1)) else {
            continue;
        };
        let at = pad.position;
        let half = pad.bounds().size().map(|v| v / 2.0);
        let gap = templates::footprint::SILK_PAD_CLEARANCE_MM
            + pad
                .mask_expansion(&footprint.defaults)
                .unwrap_or(0.0)
                .max(0.0)
            + 2.0 * PIN_ONE_DOT_RADIUS_MM;
        let center = [at[0] - half[0] - gap, at[1]];
        let id = GraphicId(footprint.next_local);
        let Some(next) = footprint.next_local.checked_add(1) else {
            continue;
        };
        footprint.next_local = next;
        footprint.graphics.push(graphics::FootprintGraphic {
            id,
            layer: graphics::DrawingLayer::Silkscreen,
            side: graphics::Side::Component,
            item: graphics::Item::Shape {
                shape: crate::geometry::Shape::Circle {
                    center,
                    radius: PIN_ONE_DOT_RADIUS_MM,
                },
                width: 0.0,
                fill: graphics::Fill::Solid,
            },
        });
        footprint.polarity = Some(graphics::Polarity {
            pad: PadId(1),
            markers: vec![id],
        });
    }
}

/// Shorten every rectangular land but the exposed pad of the footprint
/// among `edits` by `mm` at its inner end (the heel, towards the footprint
/// centre along the land's long axis), keeping its outer end in place.
pub fn trim_heels(edits: &mut [DraftEdit], mm: f64) {
    if !(mm.is_finite() && mm > 0.0) {
        return;
    }
    for edit in edits {
        let Definition::Footprint(footprint) = &mut edit.definition else {
            continue;
        };
        for pad in &mut footprint.pads {
            let PadShape::Rect { size } = &mut pad.shape else {
                continue;
            };
            if pad.number == "EP" {
                continue;
            }
            let axis = usize::from(size[1] > size[0]);
            if size[axis] <= mm {
                continue;
            }
            size[axis] -= mm;
            let outward = if pad.position[axis] < 0.0 { -1.0 } else { 1.0 };
            pad.position[axis] += outward * mm / 2.0;
        }
    }
}

/// A rectangle with the first half of `terminals` down its left side and
/// the rest up its right side, on a 2.54 mm grid, with the designator
/// above and the name below.
fn block_symbol(component: DefinitionId, terminals: &[TerminalId]) -> SymbolDefinition {
    const STEP: f64 = 2.54;
    let left = terminals.len().div_ceil(2);
    let rows = left.max(1) as f64;
    let top = ((rows - 1.0) / 2.0).floor() * STEP;
    let body = Bounds {
        min: [-2.0 * STEP, top - rows * STEP],
        max: [2.0 * STEP, top + STEP],
    };
    let gap = 0.5;
    let pins = terminals
        .iter()
        .enumerate()
        .map(|(k, terminal)| {
            let position = if k < left {
                [-3.0 * STEP, top - k as f64 * STEP]
            } else {
                let j = terminals.len() - 1 - k;
                [3.0 * STEP, top - j as f64 * STEP]
            };
            graphics::pin_toward(*terminal, position, body)
        })
        .collect();
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
            pins,
        }],
    }
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

/// One component of the PCB realism showcase: its name, typed package
/// form, designator prefix and whether its footprint marks pin 1.
pub struct ShowcasePart {
    pub name: &'static str,
    pub form: PackageForm,
    pub prefix: &'static str,
    pub pin_one: bool,
    /// How much of each perimeter land's inner end to trim, in mm (see
    /// [`trim_heels`]).
    pub heel_trim_mm: f64,
}

/// The showcase's components, in `SHOWCASE_INSTANCES` part order: an
/// SOIC-8, a QFN-16, a 2×5 header, 0603 resistors, 0805 capacitors and a
/// polarised 3216 tantalum capacitor. Original typical values.
pub fn showcase_parts() -> Vec<ShowcasePart> {
    vec![
        ShowcasePart {
            name: "Interface bridge (SOIC-8)",
            form: Preset::Soic8.form(),
            prefix: "U",
            pin_one: true,
            heel_trim_mm: 0.0,
        },
        ShowcasePart {
            name: "Controller (QFN-16)",
            form: Preset::Qfn16.form(),
            prefix: "U",
            pin_one: true,
            // The preset's lands reach 0.05 mm from the exposed pad and
            // meet at the corners; trimming their heels keeps 0.2 mm.
            heel_trim_mm: 0.2,
        },
        ShowcasePart {
            name: "Header 2×5",
            form: Preset::Header2x5.form(),
            prefix: "J",
            pin_one: true,
            heel_trim_mm: 0.0,
        },
        ShowcasePart {
            name: "Resistor 0603",
            form: Preset::Chip0603.form(),
            prefix: "R",
            pin_one: false,
            heel_trim_mm: 0.0,
        },
        ShowcasePart {
            name: "Capacitor 0805",
            form: PackageForm::Chip(Box::new(ChipForm::new(
                ChipKind::Capacitor,
                [2.0, 1.25, 0.85],
            ))),
            prefix: "C",
            pin_one: false,
            heel_trim_mm: 0.0,
        },
        ShowcasePart {
            name: "Tantalum capacitor 3216 (polarised)",
            form: PackageForm::Chip(Box::new(ChipForm::new(
                ChipKind::Capacitor,
                [3.2, 1.6, 1.6],
            ))),
            prefix: "C",
            pin_one: true,
            heel_trim_mm: 0.0,
        },
    ]
}

/// The showcase instances: (designator, index into [`showcase_parts`]).
pub const SHOWCASE_INSTANCES: &[(&str, usize)] = &[
    ("J1", 2),
    ("U1", 0),
    ("U2", 1),
    ("C1", 4),
    ("C2", 4),
    ("C3", 5),
    ("R1", 3),
    ("R2", 3),
    ("R3", 3),
];

/// The showcase netlist: (net name, terminals as (designator, terminal)).
/// Terminal numbers are the pad numbers; the QFN's exposed pad is 17 and
/// the tantalum's pad 1 is its positive terminal.
pub const SHOWCASE_NETS: &[(&str, &[(&str, u32)])] = &[
    (
        "VCC",
        &[
            ("J1", 1),
            ("C3", 1),
            ("U1", 8),
            ("C1", 2),
            ("U2", 13),
            ("C2", 2),
            ("R1", 2),
            ("R2", 2),
        ],
    ),
    (
        "GND",
        &[
            ("J1", 2),
            ("J1", 10),
            ("C3", 2),
            ("U1", 4),
            ("C1", 1),
            ("C2", 1),
            ("U2", 17),
        ],
    ),
    ("SDA", &[("J1", 7), ("U1", 5), ("R1", 1)]),
    ("SCL", &[("J1", 5), ("U1", 6), ("R2", 1)]),
    ("EN", &[("J1", 3), ("U1", 7)]),
    ("D0", &[("U1", 1), ("U2", 16)]),
    ("D1", &[("U1", 2), ("U2", 15)]),
    ("D2", &[("U1", 3), ("U2", 14)]),
    ("INT", &[("U2", 6), ("R3", 2)]),
    ("OUT", &[("R3", 1), ("J1", 9)]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{Error, Result};
    use crate::library::package_fit::{self, FitRotation};
    use crate::{Engine, LibraryCommand, Operation, Outcome};

    #[test]
    fn every_preset_sample_fits_its_lands_and_publishes() -> Result<()> {
        for preset in Preset::ALL {
            let mut engine = Engine::default();
            let refs = engine.reserve_lineages()?;
            let edits = packaged(preset.label(), preset, refs);
            let Some(Definition::Package(package)) = edits
                .iter()
                .find(|e| e.id == refs.package.id)
                .map(|e| e.definition.clone())
            else {
                return Err(Error::Invalid("no package".into()));
            };
            let Some(Definition::Footprint(footprint)) = edits
                .iter()
                .find(|e| e.id == refs.footprint.id)
                .map(|e| e.definition.clone())
            else {
                return Err(Error::Invalid("no footprint".into()));
            };
            assert!(package.form.range_problems().is_empty(), "{preset:?}");
            assert_eq!(
                footprint.pads.len(),
                package.terminals.len(),
                "{preset:?}: one land per terminal"
            );
            engine.apply(
                engine.revision(),
                Operation::Library(LibraryCommand::Edit { edits }),
            )?;
            let Outcome::LeadsFitted { fit, .. } = engine.apply(
                engine.revision(),
                Operation::Library(LibraryCommand::FitLeads {
                    draft: refs.package.id,
                    pairs: vec![],
                    rotation: FitRotation::Free,
                    confirm: false,
                }),
            )?
            else {
                return Err(Error::Invalid("expected a fit".into()));
            };
            assert!(fit.rms_mm < 1e-9, "{preset:?}: {} mm rms", fit.rms_mm);
            assert!(
                package_fit::placement_findings(&package, &footprint, refs.footprint.id).is_empty(),
                "{preset:?}"
            );
            engine.apply(
                engine.revision(),
                Operation::Library(LibraryCommand::Publish {
                    drafts: vec![
                        refs.component,
                        refs.symbol.id,
                        refs.package.id,
                        refs.footprint.id,
                        refs.binding.id,
                    ],
                }),
            )?;
        }
        Ok(())
    }

    #[test]
    fn the_resistor_sample_fits_its_own_footprint() -> Result<()> {
        let mut engine = Engine::default();
        let refs = engine.reserve_lineages()?;
        let edits = two_terminal("R", Kind::RESISTOR, refs);
        let (mut package, mut footprint) = (None, None);
        for edit in &edits {
            match &edit.definition {
                Definition::Package(p) => package = Some(p.clone()),
                Definition::Footprint(f) => footprint = Some(f.clone()),
                _ => {}
            }
        }
        let (Some(package), Some(footprint)) = (package, footprint) else {
            return Err(Error::Invalid("incomplete sample".into()));
        };
        let pairs = package_fit::form_pairs(&package, &footprint)?;
        let fit = package_fit::fit_leads(&package.placement, &pairs, FitRotation::Free)?;
        assert!(fit.rms_mm < 1e-9, "{} mm rms", fit.rms_mm);
        assert_eq!(fit.placement, package.placement, "already fitted");
        assert!(
            package_fit::placement_findings(&package, &footprint, refs.footprint.id).is_empty()
        );
        // The form's lands are the sample's lands.
        let ids: Vec<PackageTerminalId> = package.terminals.iter().map(|t| t.id).collect();
        let lands = form_lands(&package.form, &ids).unwrap_or_default();
        assert_eq!(
            lands
                .iter()
                .map(|p| (p.position, p.shape.clone()))
                .collect::<Vec<_>>(),
            footprint
                .pads
                .iter()
                .map(|p| (p.position, p.shape.clone()))
                .collect::<Vec<_>>()
        );
        Ok(())
    }
}
