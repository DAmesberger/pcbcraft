//! Original, unverified starter data for local editing. These are generic
//! two-terminal arrangements, not component categories; every value,
//! including the designator prefix and body style the caller passes, is
//! ordinary definition data a user then edits.
use super::footprint::{Hole, HoleShape, PadKind, PadShape};
use super::graphics::{self, HAlign, Justify, PIN_TEXT_HEIGHT_MM, TextContent, VAlign};
use super::symbol_style::{self, SymbolConventions, SymbolKind};
use super::templates;
use super::templates::catalog::{LandValues, SkeletonHint};
use super::{
    BindingDefinition, ChipForm, ChipKind, ComponentDefinition, Definition, DefinitionMeta,
    DraftEdit, FootprintDefinition, GeneratedMaterials, GullWingForm, HeaderForm, LeadSides,
    ModelPlacement, NoLeadForm, PackageDefinition, PackageForm, PackageModel, PackageTerminal, Pad,
    SymbolDefinition, Terminal, TerminalClass, TerminalMap, Unit,
};
use crate::diagnostics::{Error, Result};
use crate::fields::{FieldDefinition, QuantityKind, ValueType};
use crate::functions::{Direction, TerminalFunctions};
use crate::geometry::{Bounds, Point};
use crate::ids::{
    BindingRef, DefinitionId, FootprintRef, FunctionId, GraphicId, PackageRef, PackageTerminalId,
    PadId, QuantityKindRef, SymbolRef, TerminalId, UnitId,
};
use crate::quantity::{UnitOfMeasure, UnitSet};
use crate::rules::DesignRules;

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

impl Kind<'_> {
    /// The drawn symbol of a two-terminal sample: its chip construction's
    /// (resistor, capacitor, inductor), else the one its prefix names;
    /// `None` keeps the plain body rectangle (a connector).
    pub fn symbol(&self) -> Option<SymbolKind> {
        match self.chip {
            Some(chip) => Some(chip_symbol(chip, false)),
            None => SymbolKind::for_prefix(self.prefix, false),
        }
    }
}

/// The symbol a chip construction draws: a resistor, a capacitor
/// (polarised when the package marks pin 1) or an inductor.
pub fn chip_symbol(chip: ChipKind, polarised: bool) -> SymbolKind {
    match chip {
        ChipKind::Resistor => SymbolKind::Resistor,
        ChipKind::Capacitor if polarised => SymbolKind::PolarisedCapacitor,
        ChipKind::Capacitor => SymbolKind::Capacitor,
        ChipKind::Inductor => SymbolKind::Inductor,
    }
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
    /// A kind-specific drawing in place of the body rectangle.
    drawn: Option<SymbolKind>,
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
                mechanical: None,
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
            definition: Definition::Symbol(
                layout
                    .drawn
                    .and_then(|kind| {
                        symbol_style::generate(
                            kind,
                            SymbolConventions::default(),
                            refs.component,
                            &terminals,
                        )
                        .ok()
                    })
                    .unwrap_or_else(|| symbol(refs.component, &terminals, &layout)),
            ),
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

/// A two-terminal component with an axial symbol (the kind-specific drawing
/// of [`Kind::symbol`], else a body rectangle), a small two-land footprint
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
            drawn: kind.symbol(),
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
            drawn: None,
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

/// The designation of each of `count` package terminals a typed `form`
/// gives, in lead order: a grid's ball names (A1, A2, …), else lead
/// numbers from 1 and `EP` for an exposed pad. The footprint's pad numbers
/// ([`form_lands`]) are the same text.
pub fn form_designations(form: &PackageForm, count: usize) -> Vec<String> {
    if let PackageForm::Module(m) = form {
        return m.designations_for(count);
    }
    let leads = usize::try_from(form.lead_count()).unwrap_or(0);
    let balls = match form {
        PackageForm::Grid(g) => g.ball_names(),
        _ => Vec::new(),
    };
    (0..count)
        .map(|k| {
            if k >= leads {
                "EP".to_owned()
            } else {
                balls.get(k).cloned().unwrap_or_else(|| (k + 1).to_string())
            }
        })
        .collect()
}

/// Outward offset of an estimated gull-wing land's centre from its foot's
/// centre: the toe gets 0.4 mm, the heel 0.2 mm (the B2 template rules).
const TOE_HEEL_SHIFT_MM: f64 = 0.1;

/// The least copper gap between two estimated no-lead lands: the `min
/// mask web` rule plus the footprint's default mask expansion on both
/// sides, so the lint's `footprint.mask_web` holds under the same rules.
fn no_lead_copper_gap(rules: &DesignRules) -> f64 {
    rules.mask.web_min + 2.0 * super::footprint::MASK_EXPANSION_MM
}

/// [`form_lands_with`] under the default design-rule profile.
pub fn form_lands(form: &PackageForm, terminals: &[PackageTerminalId]) -> Option<Vec<Pad>> {
    form_lands_with(form, terminals, &DesignRules::default())
}

/// Lands for the leads of a typed `form`, in lead order, landing
/// `terminals[k]` (pad IDs from 1, numbers from [`form_designations`]):
/// each centred on its lead contact (a toe-heavy land just outside a
/// gull-wing or molded foot) so the leads fit their pads, sized from the
/// lead (original rules of thumb, not a land-pattern standard, the same
/// estimates the family's footprint template uses), and the exposed pad
/// last when the package lists it as a terminal. Header, DIP, TO, axial
/// and radial leads are plated through with a round drill; a DIP's or TO's
/// pin 1 is a rounded square. A grid's lands are round, smaller than the
/// ball ([`GridForm::estimated_land_diameter`]). `None` for `Generic` or
/// when the terminal count does not match.
///
/// The land margins come from `rules` (`LandRules`: the gull-wing extra
/// width and the least chip land gap; the no-lead gap from the mask web).
///
/// [`GridForm::estimated_land_diameter`]: super::package_form::grid::GridForm::estimated_land_diameter
pub fn form_lands_with(
    form: &PackageForm,
    terminals: &[PackageTerminalId],
    rules: &DesignRules,
) -> Option<Vec<Pad>> {
    if let PackageForm::Module(m) = form {
        return m.lands(terminals);
    }
    let no_lead_gap = no_lead_copper_gap(rules);
    let points = form.lead_points();
    let leads = usize::try_from(form.lead_count()).ok()?;
    if points.is_empty() || !form.terminal_counts().contains(&(terminals.len() as u64)) {
        return None;
    }
    let numbers = form_designations(form, terminals.len());
    let family_sizes: Vec<[f64; 2]> = match form {
        PackageForm::Tabbed(t) => t.land_sizes(),
        PackageForm::Molded(m) => m.land_sizes(),
        _ => Vec::new(),
    };
    // Lead k's land size [along x, along y].
    let size = |k: usize| -> [f64; 2] {
        match form {
            PackageForm::Chip(c) => [c.termination + 0.6, c.width + 0.2],
            PackageForm::GullWing(g) => {
                let land = [
                    g.lead_width + rules.land.gull_wing_width_extra,
                    g.foot_length + 0.6,
                ];
                if across_x(g.sides, g.leads_per_side, k) {
                    [land[1], land[0]]
                } else {
                    land
                }
            }
            PackageForm::NoLead(n) => {
                // 0.05 mm wider than the terminal, but never so wide that
                // neighbours at the pitch lose the mask web; never
                // narrower than the terminal.
                let width = (n.terminal_width + 0.05)
                    .min(n.pitch - no_lead_gap)
                    .max(n.terminal_width);
                let land = [width, n.terminal_length + 0.4];
                if across_x(n.sides, n.terminals_per_side, k) {
                    [land[1], land[0]]
                } else {
                    land
                }
            }
            PackageForm::Header(h) => {
                // Within the pitch (as a through-hole land), keeping a
                // minimal ring round the drill.
                let drill = h.pin_side * std::f64::consts::SQRT_2 + 0.1;
                let pitch = if h.rows > 1 {
                    h.pitch.min(h.row_pitch)
                } else {
                    h.pitch
                };
                let d = (h.pin_side * std::f64::consts::SQRT_2 + 0.8)
                    .min(pitch - super::package_form::tht::LAND_GAP_MM)
                    .max(drill + super::package_form::tht::MIN_RING_MM);
                [d, d]
            }
            PackageForm::Tabbed(_) | PackageForm::Molded(_) => {
                family_sizes.get(k).copied().unwrap_or([1.0, 1.0])
            }
            PackageForm::Grid(g) => {
                let d = g.estimated_land_diameter();
                [d, d]
            }
            PackageForm::Tht(t) => {
                let (_, land) = t.estimated_drill_and_land();
                [land, land]
            }
            PackageForm::Generic | PackageForm::Module(_) => [1.0, 1.0],
        }
    };
    let tab_index = match form {
        PackageForm::Tabbed(t) if t.tab.is_some() => leads.checked_sub(1),
        _ => None,
    };
    let mut pads = Vec::with_capacity(terminals.len());
    for (k, (terminal, point)) in terminals.iter().zip(&points).enumerate() {
        let id = PadId(u32::try_from(k + 1).ok()?);
        let mut at = [point[0], point[1]];
        let number = numbers.get(k).cloned().unwrap_or_else(|| id.0.to_string());
        match form {
            // Molded lands lie along x, tabbed leads along y; a flange tab
            // sits on its solderable area, a tab lead like the others.
            PackageForm::Molded(_) => at[0] += at[0].signum() * TOE_HEEL_SHIFT_MM,
            PackageForm::Tabbed(t) => {
                let flange = t.tab.is_some_and(|tab| tab.flange.is_some());
                if !(Some(k) == tab_index && flange) {
                    at[1] += at[1].signum() * TOE_HEEL_SHIFT_MM;
                }
            }
            _ => {}
        }
        let mut land = size(k);
        if let PackageForm::Chip(_) = form {
            // Small chips: keep the lands' inner edges at least
            // the least chip land gap apart, trimming the heel, never the
            // toe.
            let outer = at[0].abs() + land[0] / 2.0;
            let inner = (at[0].abs() - land[0] / 2.0).max(rules.land.chip_gap_min / 2.0);
            if inner < outer {
                land[0] = outer - inner;
                at[0] = at[0].signum() * (outer + inner) / 2.0;
            }
        }
        let mut pad = Pad::smd_rect(id, *terminal, number, at, land);
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
        } else if let PackageForm::Tht(t) = form {
            let (drill, land) = t.estimated_drill_and_land();
            pad.kind = PadKind::PlatedThrough;
            pad.shape = if k == 0 && t.marks_pin_one() {
                PadShape::RoundRect {
                    size: [land, land],
                    radius_ratio: 0.25,
                    max_radius: None,
                    corners: super::footprint::CornerMask::ALL,
                }
            } else {
                PadShape::Circle { diameter: land }
            };
            pad.hole = Some(Hole {
                shape: HoleShape::Round { diameter: drill },
                plated: true,
                offset: [0.0, 0.0],
            });
        } else if let PackageForm::Grid(_) = form {
            pad.shape = PadShape::Circle {
                diameter: size(k)[0],
            };
        } else if let PackageForm::Molded(_) = form {
            pad.shape = PadShape::RoundRect {
                size: size(k),
                radius_ratio: 0.25,
                max_radius: None,
                corners: super::footprint::CornerMask::ALL,
            };
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
    build_packaged(
        &Packaged {
            name,
            form,
            prefix,
            symbol: SymbolStyle::Block,
            conventions: SymbolConventions::default(),
            lands: None,
            provenance: PROVENANCE,
            template: false,
            rules: &DesignRules::default(),
        },
        refs,
    )
}

/// [`packaged_form`] drawing the symbol of `kind` (the box kinds lay out
/// any count; a drawn kind whose terminal count differs from the form's
/// keeps the box).
pub fn packaged_symbol(
    name: &str,
    form: PackageForm,
    prefix: &str,
    kind: SymbolKind,
    refs: SampleRefs,
) -> Vec<DraftEdit> {
    build_packaged(
        &Packaged {
            name,
            form,
            prefix,
            symbol: SymbolStyle::of(kind),
            conventions: SymbolConventions::default(),
            lands: None,
            provenance: PROVENANCE,
            template: false,
            rules: &DesignRules::default(),
        },
        refs,
    )
}

/// How a generated component's symbol is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SymbolStyle {
    /// Pins down the left, then up the right, in package order.
    Block,
    /// A kind-specific drawing (`symbol_style`).
    Drawn(SymbolKind),
    /// Pins down the left side only, in package order.
    Column,
    /// The body and its texts; pins are drawn by the user.
    Empty,
}

impl SymbolStyle {
    fn of(kind: SymbolKind) -> Self {
        match kind {
            SymbolKind::IcBox => Self::Block,
            SymbolKind::Connector => Self::Column,
            kind => Self::Drawn(kind),
        }
    }
}

struct Packaged<'a> {
    name: &'a str,
    form: PackageForm,
    prefix: &'a str,
    symbol: SymbolStyle,
    /// How drawn kinds are styled (the project's conventions).
    conventions: SymbolConventions,
    lands: Option<&'a LandValues>,
    provenance: &'a str,
    /// Lands from [`template_lands`] (a template build) rather than the
    /// plain form lands of a sample.
    template: bool,
    /// The design rules the land estimates follow.
    rules: &'a DesignRules,
}

/// What [`from_template`] builds: a typed `form`, the skeleton the
/// symbol starts from, the designator prefix, optional cited land
/// overrides and the provenance text every draft records.
pub struct TemplateSpec<'a> {
    pub name: &'a str,
    pub form: PackageForm,
    pub skeleton: SkeletonHint,
    /// The symbol the request names; `None` chooses one with
    /// [`choose_symbol`].
    pub symbol: Option<SymbolKind>,
    /// The symbol the row suggests.
    pub row_symbol: Option<SymbolKind>,
    /// The project's drawing conventions.
    pub conventions: SymbolConventions,
    pub prefix: &'a str,
    pub lands: Option<&'a LandValues>,
    pub provenance: &'a str,
}

/// The default profile's clearance between a no-lead perimeter land's
/// heel and the exposed pad when the lands are estimated (original rule
/// of thumb); estimates read the `exposed pad clearance` rule.
pub const EXPOSED_PAD_CLEARANCE_MM: f64 = 0.2;

/// The five drafts of a component built from a typed `form` and a
/// skeleton (`library.new_from_template`): one passive terminal per lead
/// (and the exposed pad), the skeleton's symbol, form-derived lands with
/// the spec's land overrides, the standard drawings, a pin-1 marker on
/// packages of more than two terminals or a polarised two-terminal one,
/// and a straight terminal mapping. The package model is generated.
/// Refuses a form that has no terminals and a two-terminal skeleton on a
/// package of another count.
pub fn from_template(spec: &TemplateSpec<'_>, refs: SampleRefs) -> Result<Vec<DraftEdit>> {
    from_template_with(spec, refs, &DesignRules::default())
}

/// [`from_template`] with land estimates following `rules` (the project's
/// effective design rules).
pub fn from_template_with(
    spec: &TemplateSpec<'_>,
    refs: SampleRefs,
    rules: &DesignRules,
) -> Result<Vec<DraftEdit>> {
    let count = form_terminal_count(&spec.form);
    if count == 0 {
        return Err(Error::Invalid(format!(
            "template.values: the {} family values give no terminals",
            spec.form.label()
        )));
    }
    if spec.symbol.is_none()
        && matches!(spec.skeleton, SkeletonHint::TwoTerminal { .. })
        && count != 2
    {
        return Err(Error::Invalid(format!(
            "template.skeleton: a two-terminal skeleton needs a package of 2 terminals; this one has {count}"
        )));
    }
    let kind = choose_symbol(
        spec.skeleton,
        &spec.form,
        spec.prefix,
        spec.symbol,
        spec.row_symbol,
    );
    if let Some(needed) = kind.and_then(SymbolKind::terminal_count)
        && usize::try_from(count).ok() != Some(needed)
    {
        return Err(Error::Invalid(format!(
            "template.symbol: a {} symbol needs {needed} terminal{}; this package has {count}",
            kind.map_or("", SymbolKind::label).to_lowercase(),
            if needed == 1 { "" } else { "s" },
        )));
    }
    let polarised = matches!(spec.skeleton, SkeletonHint::TwoTerminal { polarised: true })
        || kind.is_some_and(SymbolKind::polarised);
    let symbol = kind.map_or(SymbolStyle::Empty, SymbolStyle::of);
    let mut edits = build_packaged(
        &Packaged {
            name: spec.name,
            form: spec.form.clone(),
            prefix: spec.prefix,
            symbol,
            conventions: spec.conventions,
            lands: spec.lands,
            provenance: spec.provenance,
            template: true,
            rules,
        },
        refs,
    );
    if polarised || count > 2 {
        mark_pin_one(&mut edits);
    }
    Ok(edits)
}

/// The symbol a template build draws; `None` for a blank skeleton
/// without an explicit choice. An `explicit` request wins. A two-terminal
/// skeleton takes the kind its designator `prefix` customarily names (so
/// a user's `D` on a tantalum row draws a diode), else the row's
/// suggestion, else its form's: a chip's construction, or a diode when
/// polarised and a resistor when not. The other skeletons keep their box.
/// A presentation default the author edits, never an electrical claim.
pub fn choose_symbol(
    skeleton: SkeletonHint,
    form: &PackageForm,
    prefix: &str,
    explicit: Option<SymbolKind>,
    row: Option<SymbolKind>,
) -> Option<SymbolKind> {
    if explicit.is_some() {
        return explicit;
    }
    Some(match skeleton {
        SkeletonHint::TwoTerminal { polarised } => SymbolKind::for_prefix(prefix, polarised)
            .or(row.filter(|k| k.terminal_count() == Some(2)))
            .unwrap_or(match form {
                PackageForm::Chip(chip) => chip_symbol(chip.kind, polarised),
                _ if polarised => SymbolKind::Diode,
                _ => SymbolKind::Resistor,
            }),
        SkeletonHint::IcBox => SymbolKind::IcBox,
        SkeletonHint::Connector => SymbolKind::Connector,
        SkeletonHint::Blank => return None,
    })
}

/// The terminal count a typed form gives a new package: its largest
/// accepted count (a no-lead exposed pad becomes a terminal).
pub fn form_terminal_count(form: &PackageForm) -> u32 {
    form.terminal_counts()
        .last()
        .copied()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n <= super::package_form::MAX_FORM_LEADS + 1)
        .unwrap_or(0)
}

/// [`heel_trim_with`] under the default design-rule profile
/// ([`EXPOSED_PAD_CLEARANCE_MM`]).
pub fn heel_trim(form: &PackageForm) -> f64 {
    heel_trim_with(form, &DesignRules::default())
}

/// How much to trim the heels of a no-lead form's estimated perimeter
/// lands so they keep the `exposed pad clearance` rule from its exposed
/// pad; 0 for other forms.
pub fn heel_trim_with(form: &PackageForm, rules: &DesignRules) -> f64 {
    let PackageForm::NoLead(n) = form else {
        return 0.0;
    };
    let Some(ep) = n.exposed_pad else {
        return 0.0;
    };
    // Estimated land length (see `form_lands`) and its centre.
    let length = n.terminal_length + 0.4;
    let gap = |body: f64, ep: f64| body / 2.0 - n.terminal_length / 2.0 - length / 2.0 - ep / 2.0;
    let mut gaps = vec![gap(n.body_width, ep[1])];
    if n.sides == LeadSides::Four {
        gaps.push(gap(n.body_length, ep[0]));
    }
    let worst = gaps.into_iter().fold(f64::INFINITY, f64::min);
    let trim = rules.land.exposed_pad_clearance - worst;
    // Never trim a land below half its estimated length.
    if trim.is_finite() && trim > 0.0 {
        trim.min(length / 2.0)
    } else {
        0.0
    }
}

/// Apply cited land overrides to form-derived `pads` of `form`: each
/// perimeter land's length along its lead and width across it, the
/// centre-to-centre distance between opposite lands (both pairs of sides
/// of a four-sided form), and the exposed pad's size. Plated-through pins
/// keep their lands. Values that are not positive and finite are ignored.
pub fn apply_lands(pads: &mut [Pad], form: &PackageForm, lands: &LandValues) {
    let ok = |v: Option<f64>| v.filter(|v| v.is_finite() && *v > 0.0);
    // Round lands (grid balls, through-hole rings) take one diameter.
    if let PackageForm::Grid(_) | PackageForm::Tht(_) = form {
        let Some(d) = ok(lands.pad_width).or(ok(lands.pad_length)) else {
            return;
        };
        for pad in pads.iter_mut() {
            match &mut pad.shape {
                PadShape::Circle { diameter } => *diameter = d,
                PadShape::RoundRect { size, .. } => *size = [d, d],
                _ => {}
            }
        }
        return;
    }
    let per = match form {
        PackageForm::GullWing(g) => Some((g.sides, g.leads_per_side)),
        PackageForm::NoLead(n) => Some((n.sides, n.terminals_per_side)),
        _ => None,
    };
    let chip = matches!(form, PackageForm::Chip(_) | PackageForm::Molded(_));
    // A tabbed form's leads run along y; its tab keeps its own land.
    let tab = match form {
        PackageForm::Tabbed(t) if t.tab.is_some() => usize::try_from(form.lead_count())
            .ok()
            .and_then(|n| n.checked_sub(1)),
        _ => None,
    };
    let tabbed = matches!(form, PackageForm::Tabbed(_));
    if !chip && !tabbed && per.is_none() {
        return;
    }
    for (k, pad) in pads.iter_mut().enumerate() {
        if pad.kind != PadKind::Smd || Some(k) == tab {
            continue;
        }
        let (PadShape::Rect { size } | PadShape::RoundRect { size, .. }) = &mut pad.shape else {
            continue;
        };
        if pad.number == "EP" {
            if let Some([x, y]) = lands.exposed_pad
                && x.is_finite()
                && y.is_finite()
                && x > 0.0
                && y > 0.0
            {
                *size = [x, y];
            }
            continue;
        }
        // The axis the lead runs along: x for a chip and the short sides
        // of a four-sided form, y for the long rows.
        let axis = match per {
            _ if tabbed => 1,
            Some((sides, per)) if !across_x(sides, per, k) => 1,
            _ => 0,
        };
        if let Some(l) = ok(lands.pad_length) {
            size[axis] = l;
        }
        if let Some(w) = ok(lands.pad_width) {
            size[1 - axis] = w;
        }
        if let Some(gap) = ok(lands.gap) {
            let sign = if pad.position[axis] < 0.0 { -1.0 } else { 1.0 };
            pad.position[axis] = sign * gap / 2.0;
        }
    }
}

/// The lands a template build gives a typed `form` landing `terminals`
/// (`library.new_from_template`, `library.package_from_template` and the
/// gallery preview, so the preview is exactly what Create builds):
/// [`form_lands`], then the row's cited land overrides, or without them
/// the estimated heel trim that keeps no-lead lands clear of an exposed
/// pad. `None` when the form gives no lands for that many terminals.
pub fn template_lands(
    form: &PackageForm,
    terminals: &[PackageTerminalId],
    lands: Option<&LandValues>,
) -> Option<Vec<Pad>> {
    template_lands_with(form, terminals, lands, &DesignRules::default())
}

/// [`template_lands`] with the estimates following `rules` (the project's
/// effective design rules, or the default profile in a library context).
pub fn template_lands_with(
    form: &PackageForm,
    terminals: &[PackageTerminalId],
    lands: Option<&LandValues>,
    rules: &DesignRules,
) -> Option<Vec<Pad>> {
    let mut pads = form_lands_with(form, terminals, rules)?;
    match lands {
        Some(lands) => apply_lands(&mut pads, form, lands),
        None => {
            trim_pads(&mut pads, heel_trim_with(form, rules));
            clear_corners(&mut pads, form, no_lead_copper_gap(rules));
        }
    }
    Some(pads)
}

/// Pull back the heels of the estimated lands that meet at a corner of a
/// four-sided no-lead form (pins 1 and N, and the other three corners):
/// the end lands of two sides run towards each other, so without this
/// their inner corners touch. Both lands of a pair give up the same
/// length, the least that leaves `copper_gap` (the no-lead copper gap) of copper
/// between them; the toes stay. A pair that cannot be cleared within half
/// of the shorter land is left as it is (the lint then reports it).
fn clear_corners(pads: &mut [Pad], form: &PackageForm, copper_gap: f64) {
    let PackageForm::NoLead(n) = form else {
        return;
    };
    if n.sides != LeadSides::Four {
        return;
    }
    // Perimeter lands: index, long axis, outward sign and extent.
    let lands: Vec<(usize, usize, f64, Bounds)> = pads
        .iter()
        .enumerate()
        .filter_map(|(k, pad)| {
            let PadShape::Rect { size } = pad.shape else {
                return None;
            };
            if pad.number == "EP" {
                return None;
            }
            let axis = usize::from(size[1] > size[0]);
            let outward = if pad.position[axis] < 0.0 { -1.0 } else { 1.0 };
            Some((k, axis, outward, pad.bounds()))
        })
        .collect();
    let pulled = |(_, axis, outward, bounds): &(usize, usize, f64, Bounds), by: f64| {
        let mut b = *bounds;
        if *outward > 0.0 {
            b.min[*axis] += by;
        } else {
            b.max[*axis] -= by;
        }
        b
    };
    let mut pull = vec![0.0_f64; pads.len()];
    for (i, a) in lands.iter().enumerate() {
        for b in lands.iter().skip(i + 1) {
            if a.1 == b.1 || rect_gap(&a.3, &b.3) >= copper_gap {
                continue;
            }
            let length = |l: &(usize, usize, f64, Bounds)| l.3.size()[l.1];
            let most = length(a).min(length(b)) / 2.0;
            let gap = |by: f64| rect_gap(&pulled(a, by), &pulled(b, by));
            if gap(most) < copper_gap {
                continue;
            }
            let (mut lo, mut hi) = (0.0, most);
            for _ in 0..48 {
                let mid = (lo + hi) / 2.0;
                if gap(mid) >= copper_gap {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            pull[a.0] = pull[a.0].max(hi);
            pull[b.0] = pull[b.0].max(hi);
        }
    }
    for (k, axis, outward, _) in lands {
        let by = pull[k];
        if by <= 0.0 {
            continue;
        }
        if let Some(pad) = pads.get_mut(k)
            && let PadShape::Rect { size } = &mut pad.shape
        {
            size[axis] -= by;
            pad.position[axis] += outward * by / 2.0;
        }
    }
}

/// The distance between two axis-aligned rectangles (0 when they touch or
/// overlap).
fn rect_gap(a: &Bounds, b: &Bounds) -> f64 {
    let dx = (a.min[0] - b.max[0]).max(b.min[0] - a.max[0]).max(0.0);
    let dy = (a.min[1] - b.max[1]).max(b.min[1] - a.max[1]).max(0.0);
    dx.hypot(dy)
}

fn build_packaged(spec: &Packaged<'_>, refs: SampleRefs) -> Vec<DraftEdit> {
    let Packaged {
        name,
        prefix,
        provenance,
        ..
    } = *spec;
    let form = spec.form.clone();
    let meta = |name: &str| DefinitionMeta {
        name: name.into(),
        description: String::new(),
        provenance: provenance.into(),
    };
    let count = form_terminal_count(&form);
    let terminals: Vec<TerminalId> = (1..=count).map(TerminalId).collect();
    let package_terminals: Vec<PackageTerminalId> = (1..=count).map(PackageTerminalId).collect();
    let designations = form_designations(&form, package_terminals.len());
    let package = PackageDefinition {
        next_local: count + 1,
        terminals: package_terminals
            .iter()
            .zip(&designations)
            .map(|(id, designation)| PackageTerminal {
                id: *id,
                designation: designation.clone(),
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
    let pads = if spec.template {
        template_lands_with(&package.form, &package_terminals, spec.lands, spec.rules)
    } else {
        form_lands_with(&package.form, &package_terminals, spec.rules).map(|mut pads| {
            if let Some(lands) = spec.lands {
                apply_lands(&mut pads, &package.form, lands);
            }
            pads
        })
    }
    .unwrap_or_default();
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
    let symbol = match spec.symbol {
        SymbolStyle::Block => block_symbol(refs.component, &terminals),
        SymbolStyle::Drawn(kind) => {
            symbol_style::generate(kind, spec.conventions, refs.component, &terminals)
                .unwrap_or_else(|_| block_symbol(refs.component, &terminals))
        }
        SymbolStyle::Column => column_symbol(refs.component, &terminals),
        SymbolStyle::Empty => block_symbol(refs.component, &[]),
    };
    vec![
        DraftEdit {
            id: refs.component,
            meta: meta(name),
            definition: Definition::Component(ComponentDefinition {
                pin_rules: Vec::new(),
                marking: None,
                review: None,
                mechanical: None,
                next_local: 2 * count + 1,
                designator_prefix: prefix.into(),
                terminals: terminals
                    .iter()
                    .map(|id| Terminal {
                        id: *id,
                        name: usize::try_from(id.0)
                            .ok()
                            .and_then(|k| designations.get(k.checked_sub(1)?))
                            .cloned()
                            .unwrap_or_else(|| id.to_string()),
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
            definition: Definition::Symbol(symbol),
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
        if let Definition::Footprint(footprint) = &mut edit.definition {
            mark_polarity(footprint, PadId(1));
        }
    }
}

/// Mark `pad` of `footprint` as the polarity pad, as [`mark_pin_one`]
/// does for pad 1. A missing pad or an exhausted identity counter leaves
/// the footprint unchanged.
pub fn mark_polarity(footprint: &mut FootprintDefinition, pad_id: PadId) {
    let Some(pad) = footprint.pad(pad_id) else {
        return;
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
        return;
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
        pad: pad_id,
        markers: vec![id],
    });
}

/// Shorten every rectangular land but the exposed pad of the footprint
/// among `edits` by `mm` at its inner end (the heel, towards the footprint
/// centre along the land's long axis), keeping its outer end in place.
pub fn trim_heels(edits: &mut [DraftEdit], mm: f64) {
    for edit in edits {
        if let Definition::Footprint(footprint) = &mut edit.definition {
            trim_pads(&mut footprint.pads, mm);
        }
    }
}

/// [`trim_heels`] on a pad list.
fn trim_pads(pads: &mut [Pad], mm: f64) {
    if !(mm.is_finite() && mm > 0.0) {
        return;
    }
    for pad in pads {
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

/// A rectangle with the first half of `terminals` down its left side and
/// the rest up its right side, on the default symbol grid
/// ([`graphics::SYMBOL_MODULE_MM`]), with the designator above and the
/// name below.
fn block_symbol(component: DefinitionId, terminals: &[TerminalId]) -> SymbolDefinition {
    const STEP: f64 = graphics::SYMBOL_MODULE_MM;
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

/// A narrow body with every terminal down its left side in package order,
/// on the default symbol grid (a connector's starting point).
fn column_symbol(component: DefinitionId, terminals: &[TerminalId]) -> SymbolDefinition {
    const STEP: f64 = graphics::SYMBOL_MODULE_MM;
    let mut symbol = block_symbol(component, &[]);
    let rows = terminals.len().max(1) as f64;
    let body = Bounds {
        min: [-STEP, -rows * STEP],
        max: [STEP, STEP],
    };
    let gap = 0.5;
    if let Some(unit) = symbol.units.first_mut() {
        unit.graphics = vec![
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
        ];
        unit.pins = terminals
            .iter()
            .enumerate()
            .map(|(k, terminal)| {
                graphics::pin_toward(*terminal, [-2.0 * STEP, -(k as f64) * STEP], body)
            })
            .collect();
    }
    symbol
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
    /// The schematic symbol: a box for the ICs and the header, the
    /// kind's drawing for the passives.
    pub symbol: SymbolKind,
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
            symbol: SymbolKind::IcBox,
            pin_one: true,
            heel_trim_mm: 0.0,
        },
        ShowcasePart {
            name: "Controller (QFN-16)",
            form: Preset::Qfn16.form(),
            prefix: "U",
            symbol: SymbolKind::IcBox,
            pin_one: true,
            // The preset's lands reach 0.05 mm from the exposed pad and
            // meet at the corners; trimming their heels keeps 0.2 mm.
            heel_trim_mm: 0.2,
        },
        ShowcasePart {
            name: "Header 2×5",
            form: Preset::Header2x5.form(),
            prefix: "J",
            symbol: SymbolKind::IcBox,
            pin_one: true,
            heel_trim_mm: 0.0,
        },
        ShowcasePart {
            name: "Resistor 0603",
            form: Preset::Chip0603.form(),
            prefix: "R",
            symbol: SymbolKind::Resistor,
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
            symbol: SymbolKind::Capacitor,
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
            symbol: SymbolKind::PolarisedCapacitor,
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

    /// The founder's check: the showcase's capacitors draw capacitor
    /// symbols (C3, the tantalum, polarised with pin 1 the + plate), its
    /// resistors resistor symbols and its ICs and header boxes.
    #[test]
    fn showcase_instances_draw_their_kinds_symbols() -> Result<()> {
        let engine = Engine::showcase()?;
        let project = engine.project();
        let expect = [
            ("C1", SymbolKind::Capacitor),
            ("C2", SymbolKind::Capacitor),
            ("C3", SymbolKind::PolarisedCapacitor),
            ("R1", SymbolKind::Resistor),
            ("R2", SymbolKind::Resistor),
            ("R3", SymbolKind::Resistor),
        ];
        for (designator, kind) in expect {
            let instance = project
                .instances
                .iter()
                .find(|i| i.designator == designator)
                .ok_or_else(|| Error::NotFound(designator.into()))?;
            let symbol = &crate::instances::resolve(&project.library, instance)?
                .symbol
                .definition;
            let drawn = symbol_style::generate(
                kind,
                SymbolConventions::default(),
                symbol.component,
                &[TerminalId(1), TerminalId(2)],
            )?;
            assert_eq!(symbol, &drawn, "{designator}");
            // Wires still meet the pins on the sheet: every terminal's
            // schematic position is a pin connection end.
            for terminal in [1, 2] {
                let end = crate::schematic::pin_position(
                    project,
                    crate::ids::Endpoint {
                        instance: instance.id,
                        terminal: TerminalId(terminal),
                    },
                );
                assert!(end.is_some(), "{designator}.{terminal}");
            }
        }
        for designator in ["U1", "U2", "J1"] {
            let instance = project
                .instances
                .iter()
                .find(|i| i.designator == designator)
                .ok_or_else(|| Error::NotFound(designator.into()))?;
            let symbol = &crate::instances::resolve(&project.library, instance)?
                .symbol
                .definition;
            let body = symbol.units[0].graphic(GraphicId(2)).map(|g| &g.item);
            assert!(
                matches!(
                    body,
                    Some(graphics::Item::Shape {
                        shape: crate::geometry::Shape::Rect { .. },
                        ..
                    })
                ),
                "{designator}"
            );
        }
        Ok(())
    }

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

    /// The land estimates follow the design rules they are given; the
    /// rule-free entry points are the default profile.
    #[test]
    fn land_estimates_follow_the_land_rules() -> Result<()> {
        let ids = |n: u32| (1..=n).map(PackageTerminalId).collect::<Vec<_>>();
        let width = |pads: &[Pad]| match pads.first().map(|p| &p.shape) {
            Some(PadShape::Rect { size }) => size[0],
            _ => f64::NAN,
        };
        let soic = Preset::Soic8.form();
        let base = DesignRules::default();
        let default = form_lands(&soic, &ids(8)).ok_or(Error::Invalid("lands".into()))?;
        assert_eq!(
            Some(&default),
            form_lands_with(&soic, &ids(8), &base).as_ref()
        );
        // 0.41 mm leads: 0.61 mm lands by default, 0.71 mm with 0.3 mm extra.
        assert!((width(&default) - 0.61).abs() < 1e-9, "{}", width(&default));
        let mut wide = base.clone();
        wide.land.gull_wing_width_extra = 0.3;
        let lands = form_lands_with(&soic, &ids(8), &wide).ok_or(Error::Invalid("lands".into()))?;
        assert!((width(&lands) - 0.71).abs() < 1e-9, "{}", width(&lands));

        // The chip lands' inner edges keep the least chip gap.
        let chip = Preset::Chip0603.form();
        let mut apart = base.clone();
        apart.land.chip_gap_min = 1.2;
        let lands =
            form_lands_with(&chip, &ids(2), &apart).ok_or(Error::Invalid("lands".into()))?;
        let inner = |p: &Pad| match p.shape {
            PadShape::Rect { size } => p.position[0].abs() - size[0] / 2.0,
            _ => f64::NAN,
        };
        for pad in &lands {
            assert!((inner(pad) - 0.6).abs() < 1e-9, "{}", inner(pad));
        }

        // A larger exposed-pad clearance trims the no-lead heels more.
        let qfn = Preset::Qfn16.form();
        assert_eq!(heel_trim(&qfn), heel_trim_with(&qfn, &base));
        let mut clear = base.clone();
        clear.land.exposed_pad_clearance = base.land.exposed_pad_clearance + 0.1;
        assert!(heel_trim_with(&qfn, &clear) > heel_trim(&qfn));
        let terminals = ids(form_terminal_count(&qfn));
        assert_eq!(
            template_lands(&qfn, &terminals, None),
            template_lands_with(&qfn, &terminals, None, &base)
        );
        assert_ne!(
            template_lands(&qfn, &terminals, None),
            template_lands_with(&qfn, &terminals, None, &clear)
        );
        Ok(())
    }
}
