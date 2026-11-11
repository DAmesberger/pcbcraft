//! Typed physical package parameters: the only source of a package's
//! `pkg_*` values and of generated 3D models. Stored in whole nanometres
//! (`_nm` fields); the millimetre accessors of the same names (`pitch()`
//! for `pitch_nm`) are what the methods here, the footprint templates and
//! the model builders compute with, and command payloads carry the
//! millimetre wire form ([`PackageFormParams`], `#[derive(units::Params)]`).
//! The seating plane is z = 0 (the top of the land copper), x along the
//! package length.
//!
//! Lead order (the order of [`PackageForm::lead_points`], matched to the
//! package's terminals by position): counter-clockwise seen from above,
//! starting at the −x end of the −y row. Two-sided forms run along −y
//! (left to right), then +y (right to left); four-sided forms continue
//! −y, +x (upward), +y, −x (downward). A no-lead exposed pad, when it is a
//! terminal, comes last. A chip's terminal 1 is at −x; a header's pins run
//! column by column from −x, row 0 at −y.
use super::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use crate::diagnostics::Result;
use crate::geometry::Point3;
use crate::units::{self, Nm, PointNm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Millimetre accessors of a form's stored nanometre fields: what the form
/// methods, footprint templates and model builders compute with
/// ([`Nm::to_mm_f64`], the one crossing out). `name: field => kind`, where
/// `kind` is `len` (`Nm`), `pair` (`[Nm; 2]`), `triple` (`[Nm; 3]`),
/// `point` (`PointNm`, as `[x, y]`), `opt_len` or `opt_point`.
macro_rules! mm_accessors {
    ($ty:ty { $($name:ident: $field:ident => $kind:ident),* $(,)? }) => {
        impl $ty {
            $(mm_accessors!(@$kind $name $field);)*
        }
    };
    (@len $name:ident $field:ident) => {
        #[doc = concat!("`", stringify!($field), "` in millimetres.")]
        pub fn $name(&self) -> f64 {
            self.$field.to_mm_f64()
        }
    };
    (@pair $name:ident $field:ident) => {
        #[doc = concat!("`", stringify!($field), "` in millimetres.")]
        pub fn $name(&self) -> [f64; 2] {
            self.$field.map($crate::units::Nm::to_mm_f64)
        }
    };
    (@triple $name:ident $field:ident) => {
        #[doc = concat!("`", stringify!($field), "` in millimetres.")]
        pub fn $name(&self) -> [f64; 3] {
            self.$field.map($crate::units::Nm::to_mm_f64)
        }
    };
    (@point $name:ident $field:ident) => {
        #[doc = concat!("`", stringify!($field), "` in millimetres, `[x, y]`.")]
        pub fn $name(&self) -> [f64; 2] {
            self.$field.to_mm_f64()
        }
    };
    (@opt_len $name:ident $field:ident) => {
        #[doc = concat!("`", stringify!($field), "` in millimetres.")]
        pub fn $name(&self) -> Option<f64> {
            self.$field.map($crate::units::Nm::to_mm_f64)
        }
    };
    (@opt_point $name:ident $field:ident) => {
        #[doc = concat!("`", stringify!($field), "` in millimetres, `[x, y]`.")]
        pub fn $name(&self) -> Option<[f64; 2]> {
            self.$field.map($crate::units::PointNm::to_mm_f64)
        }
    };
}

pub mod grid;
pub mod module;
pub mod molded;
pub mod tabbed;
pub mod tht;
pub mod usb_c;

pub use grid::{GridForm, GridFormParams};
pub use module::{ModuleForm, ModuleFormParams};
pub use molded::{MoldedForm, MoldedFormParams};
pub use tabbed::{TabbedForm, TabbedFormParams};
pub use tht::{ThtForm, ThtFormParams};
pub use usb_c::{UsbCForm, UsbCFormParams};

/// What every family added after the first four implements, in its own
/// file (`package_form/<family>.rs`), so a family slice never edits the
/// [`PackageForm`] dispatch. Semantics are those of the methods of the same
/// name on [`PackageForm`].
pub trait FamilyForm {
    fn label(&self) -> &'static str;
    fn lengths(&self) -> Vec<f64>;
    fn envelope(&self) -> Option<[f64; 3]>;
    fn lead_count(&self) -> u64;
    fn terminal_counts(&self) -> Vec<u64> {
        vec![self.lead_count()]
    }
    fn lead_points(&self) -> Vec<Point3>;
    fn values(&self) -> Vec<PackageValue>;
    /// Range and consistency rules together.
    fn range_problems(&self) -> Vec<String>;
}

/// Largest lead or pin count of one typed form.
pub const MAX_FORM_LEADS: u32 = 512;

/// `nm` as a constant (within bounds by construction; else 0).
const fn const_nm(nm: i64) -> Nm {
    match Nm::try_new(nm) {
        Some(v) => v,
        None => Nm::ZERO,
    }
}

/// Largest length a typed form stores: 1000 mm.
pub const FORM_LENGTH_LIMIT_NM: Nm = const_nm(1_000_000_000);

/// Smallest positive length a typed form stores (the envelope's smallest
/// size): 0.01 mm.
pub const FORM_LENGTH_MIN_NM: Nm = const_nm(10_000);

/// The range finding of a stored form length `v` named `name`, when it lies
/// outside [`FORM_LENGTH_MIN_NM`]..=[`FORM_LENGTH_LIMIT_NM`].
pub(crate) fn length_problem(name: &str, v: Nm) -> Option<String> {
    (!(FORM_LENGTH_MIN_NM..=FORM_LENGTH_LIMIT_NM).contains(&v)).then(|| {
        format!(
            "{name} must be {}–{} mm (is {}).",
            FORM_LENGTH_MIN_NM.to_mm_f64(),
            FORM_LENGTH_LIMIT_NM.to_mm_f64(),
            v.to_mm_f64()
        )
    })
}

/// A built-in typical value in millimetres, as stored. Every built-in value
/// lies far inside `Nm`'s ±2 m; an impossible miss reads 0, which the range
/// rules report.
pub fn typical_nm(mm: f64) -> Nm {
    Nm::from_mm_f64_rounded(mm).unwrap_or(Nm::ZERO)
}

/// [`typical_nm`] for an `[x, y]` point or size.
pub fn typical_point(mm: [f64; 2]) -> PointNm {
    PointNm::new(typical_nm(mm[0]), typical_nm(mm[1]))
}

/// Serde adapter: a [`PackageForm`] in a command payload or reply takes
/// the millimetre wire form ([`PackageFormParams`]).
pub mod form_mm {
    use super::{PackageForm, PackageFormParams};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(form: &PackageForm, s: S) -> Result<S::Ok, S::Error> {
        PackageFormParams::from(form.clone()).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PackageForm, D::Error> {
        PackageFormParams::deserialize(d).map(PackageForm::from)
    }
}

/// [`form_mm`] for an optional form.
pub mod option_form_mm {
    use super::{PackageForm, PackageFormParams};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(form: &Option<PackageForm>, s: S) -> Result<S::Ok, S::Error> {
        form.clone().map(PackageFormParams::from).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<PackageForm>, D::Error> {
        Option::<PackageFormParams>::deserialize(d).map(|f| f.map(PackageForm::from))
    }
}

/// A package's physical form. `Generic` has only the authored envelope.
/// Family parameters are boxed so definitions stay small in commands.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(tag = "family", rename_all = "snake_case", deny_unknown_fields)]
#[params(default)]
pub enum PackageForm {
    #[default]
    Generic,
    /// Two-terminal rectangular chip (resistor, capacitor, inductor).
    Chip(#[params] Box<ChipForm>),
    /// Gull-wing leads on two (SOIC, SOP, SSOP) or four (QFP) sides.
    GullWing(#[params] Box<GullWingForm>),
    /// No-lead with side terminals on two (DFN, SON) or four (QFN) sides,
    /// optionally with an exposed pad.
    NoLead(#[params] Box<NoLeadForm>),
    /// Straight through-hole pin header.
    Header(#[params] Box<HeaderForm>),
    /// B2a: gull-wing with unequal sides and an optional tab (SOT, DPAK).
    Tabbed(#[params] Box<TabbedForm>),
    /// B2b: molded two-terminal (SOD, SMA/SMB/SMC, tantalum, MELF).
    Molded(#[params] Box<MoldedForm>),
    /// B2c: grid array (BGA, LGA, WLCSP).
    Grid(#[params] Box<GridForm>),
    /// B2d: through-hole axial, radial, DIP and TO.
    Tht(#[params] Box<ThtForm>),
    /// A small board with a shield can, edge pad rows, a ground-pad array
    /// and an optional antenna end (radio modules).
    Module(#[params] Box<ModuleForm>),
    /// A surface-mount USB Type-C receptacle for USB 2.0 with through-hole
    /// shell stakes.
    UsbC(#[params] Box<UsbCForm>),
}

/// Which chip construction the body is: sets default materials.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChipKind {
    /// Thick-film resistor: alumina substrate, black overcoat on top.
    Resistor,
    /// Multilayer ceramic capacitor.
    Capacitor,
    /// Ferrite chip inductor or bead.
    Inductor,
    /// Chip LED: a white substrate between castellated end terminations,
    /// under a clear silicone lens.
    Led,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(deny_unknown_fields)]
pub struct ChipForm {
    pub kind: ChipKind,
    /// Body length (terminal to terminal), width and height.
    pub length_nm: Nm,
    pub width_nm: Nm,
    pub height_nm: Nm,
    /// Length of each wrapped end termination along the body.
    pub termination_nm: Nm,
}

mm_accessors!(ChipForm {
    length: length_nm => len,
    width: width_nm => len,
    height: height_nm => len,
    termination: termination_nm => len,
});

/// The common chip termination band's limit: 0.5 mm.
const CHIP_TERMINATION_MAX_NM: Nm = const_nm(500_000);

impl ChipForm {
    /// A chip of `size` (length, width, height) with the common
    /// termination band: a fifth of the length, at most 0.5 mm (EIA
    /// chip-resistor bands, 0.3–0.5 mm from 0603 to 1206).
    pub fn new(kind: ChipKind, size: [Nm; 3]) -> Self {
        // A fifth, rounded half away from zero: within bounds as `size` is.
        let fifth =
            Nm::try_new((size[0].get() * 2 + size[0].get().signum() * 5) / 10).unwrap_or(Nm::ZERO);
        Self {
            kind,
            length_nm: size[0],
            width_nm: size[1],
            height_nm: size[2],
            termination_nm: fifth.min(CHIP_TERMINATION_MAX_NM),
        }
    }
}

/// Lead rows: two sides (along ±y edges, leads pointing ±y) or four.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeadSides {
    Two,
    Four,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(deny_unknown_fields)]
pub struct GullWingForm {
    pub sides: LeadSides,
    /// Leads on each long side (the ±y edges) and, for `Four`, on each
    /// short side (the ±x edges; 0 for `Two`).
    pub leads_per_side: [u32; 2],
    pub pitch_nm: Nm,
    /// Moulded body length (x), width (y) and height above its underside.
    pub body_length_nm: Nm,
    pub body_width_nm: Nm,
    pub body_height_nm: Nm,
    /// Gap between the seating plane and the body underside.
    pub standoff_nm: Nm,
    /// Tip-to-tip lead span across y and (for `Four`) across x.
    pub span_nm: [Nm; 2],
    pub lead_width_nm: Nm,
    pub lead_thickness_nm: Nm,
    /// Flat foot length on the seating plane.
    pub foot_length_nm: Nm,
}

mm_accessors!(GullWingForm {
    pitch: pitch_nm => len,
    body_length: body_length_nm => len,
    body_width: body_width_nm => len,
    body_height: body_height_nm => len,
    standoff: standoff_nm => len,
    span: span_nm => pair,
    lead_width: lead_width_nm => len,
    lead_thickness: lead_thickness_nm => len,
    foot_length: foot_length_nm => len,
});

impl GullWingForm {
    /// Share of the body height above the standoff at which the leads
    /// leave the moulding (mid-height); the generated model uses it too.
    pub const LEAD_EXIT_SHARE: f64 = 0.5;

    /// Height of the lead's centre line where it leaves the body.
    pub fn lead_exit(&self) -> f64 {
        self.standoff() + Self::LEAD_EXIT_SHARE * self.body_height()
    }

    /// Room outside the body for the lead to bend down to its foot, across
    /// y and (four sides) across x: half span − foot − half body.
    pub fn bend_room(&self) -> [f64; 2] {
        let span = self.span();
        [
            span[0] / 2.0 - self.foot_length() - self.body_width() / 2.0,
            span[1] / 2.0 - self.foot_length() - self.body_length() / 2.0,
        ]
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(deny_unknown_fields)]
pub struct NoLeadForm {
    pub sides: LeadSides,
    /// Terminals on each long side (±y edges) and short side (±x; 0 for `Two`).
    pub terminals_per_side: [u32; 2],
    pub pitch_nm: Nm,
    pub body_length_nm: Nm,
    pub body_width_nm: Nm,
    pub body_height_nm: Nm,
    /// Terminal length inward from the body edge, and width along it.
    pub terminal_length_nm: Nm,
    pub terminal_width_nm: Nm,
    /// Exposed-pad length (x) and width (y), centred, if any.
    pub exposed_pad: Option<PointNm>,
}

mm_accessors!(NoLeadForm {
    pitch: pitch_nm => len,
    body_length: body_length_nm => len,
    body_width: body_width_nm => len,
    body_height: body_height_nm => len,
    terminal_length: terminal_length_nm => len,
    terminal_width: terminal_width_nm => len,
    exposed_pad: exposed_pad => opt_point,
});

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, units::Params)]
#[serde(deny_unknown_fields)]
pub struct HeaderForm {
    pub rows: u32,
    pub pins_per_row: u32,
    /// Pin pitch along a row and between rows.
    pub pitch_nm: Nm,
    pub row_pitch_nm: Nm,
    /// Plastic body height above the seating plane. The body is one pitch
    /// per pin long and one row pitch per row wide.
    pub body_height_nm: Nm,
    /// Square pin side.
    pub pin_side_nm: Nm,
    /// Pin length above the body top and below the seating plane.
    pub pin_above_nm: Nm,
    pub pin_below_nm: Nm,
}

mm_accessors!(HeaderForm {
    pitch: pitch_nm => len,
    row_pitch: row_pitch_nm => len,
    body_height: body_height_nm => len,
    pin_side: pin_side_nm => len,
    pin_above: pin_above_nm => len,
    pin_below: pin_below_nm => len,
});

impl PackageForm {
    /// The family-file implementation of a later family.
    pub fn family_form(&self) -> Option<&dyn FamilyForm> {
        match self {
            Self::Tabbed(f) => Some(f.as_ref()),
            Self::Molded(f) => Some(f.as_ref()),
            Self::Grid(f) => Some(f.as_ref()),
            Self::Tht(f) => Some(f.as_ref()),
            Self::Module(f) => Some(f.as_ref()),
            Self::UsbC(f) => Some(f.as_ref()),
            Self::Generic
            | Self::Chip(_)
            | Self::GullWing(_)
            | Self::NoLead(_)
            | Self::Header(_) => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Generic => "Generic",
            Self::Chip(_) => "Chip",
            Self::GullWing(_) => "Gull-wing",
            Self::NoLead(_) => "No-lead",
            Self::Header(_) => "Pin header",
            Self::Tabbed(_)
            | Self::Molded(_)
            | Self::Grid(_)
            | Self::Tht(_)
            | Self::Module(_)
            | Self::UsbC(_) => self.family_form().map_or("", |f| f.label()),
        }
    }

    /// Every length the form stores, for finiteness checks.
    pub fn lengths(&self) -> Vec<f64> {
        match self {
            Self::Generic => Vec::new(),
            Self::Chip(c) => vec![c.length(), c.width(), c.height(), c.termination()],
            Self::GullWing(g) => vec![
                g.pitch(),
                g.body_length(),
                g.body_width(),
                g.body_height(),
                g.standoff(),
                g.span()[0],
                g.span()[1],
                g.lead_width(),
                g.lead_thickness(),
                g.foot_length(),
            ],
            Self::NoLead(n) => {
                let mut v = vec![
                    n.pitch(),
                    n.body_length(),
                    n.body_width(),
                    n.body_height(),
                    n.terminal_length(),
                    n.terminal_width(),
                ];
                v.extend(n.exposed_pad().iter().flatten());
                v
            }
            Self::Header(h) => vec![
                h.pitch(),
                h.row_pitch(),
                h.body_height(),
                h.pin_side(),
                h.pin_above(),
                h.pin_below(),
            ],
            Self::Tabbed(_)
            | Self::Molded(_)
            | Self::Grid(_)
            | Self::Tht(_)
            | Self::Module(_)
            | Self::UsbC(_) => self.family_form().map(|f| f.lengths()).unwrap_or_default(),
        }
    }

    /// The typed form's envelope (length, width, height above the seating
    /// plane), centred on the model origin from z = 0 up; `None` for
    /// `Generic`, whose envelope is authored. Leads below the seating plane
    /// (header pins) are not part of the envelope.
    pub fn envelope(&self) -> Option<[f64; 3]> {
        match self {
            Self::Generic => None,
            Self::Chip(c) => Some([c.length(), c.width(), c.height()]),
            Self::GullWing(g) => Some([
                if g.sides == LeadSides::Four {
                    g.span()[1].max(g.body_length())
                } else {
                    g.body_length()
                },
                g.span()[0].max(g.body_width()),
                g.standoff() + g.body_height(),
            ]),
            Self::NoLead(n) => Some([n.body_length(), n.body_width(), n.body_height()]),
            Self::Header(h) => Some([
                h.pitch() * f64::from(h.pins_per_row),
                h.row_pitch() * f64::from(h.rows),
                h.body_height() + h.pin_above(),
            ]),
            Self::Tabbed(_)
            | Self::Molded(_)
            | Self::Grid(_)
            | Self::Tht(_)
            | Self::Module(_)
            | Self::UsbC(_) => self.family_form().and_then(|f| f.envelope()),
        }
    }

    /// [`Self::envelope`] as stored: each size through the rounding
    /// crossing.
    pub fn envelope_nm(&self) -> Result<Option<[Nm; 3]>> {
        self.envelope()
            .map(|[l, w, h]| {
                Ok([
                    Nm::from_mm_f64_rounded(l)?,
                    Nm::from_mm_f64_rounded(w)?,
                    Nm::from_mm_f64_rounded(h)?,
                ])
            })
            .transpose()
    }

    /// Leads (or terminations, terminals, pins) the form has, not counting
    /// an exposed pad; 0 for `Generic`.
    pub fn lead_count(&self) -> u64 {
        let rows = |sides: LeadSides, per: [u32; 2]| match sides {
            LeadSides::Two => 2 * u64::from(per[0]),
            LeadSides::Four => 2 * (u64::from(per[0]) + u64::from(per[1])),
        };
        match self {
            Self::Generic => 0,
            Self::Chip(_) => 2,
            Self::GullWing(g) => rows(g.sides, g.leads_per_side),
            Self::NoLead(n) => rows(n.sides, n.terminals_per_side),
            Self::Header(h) => u64::from(h.rows) * u64::from(h.pins_per_row),
            Self::Tabbed(_)
            | Self::Molded(_)
            | Self::Grid(_)
            | Self::Tht(_)
            | Self::Module(_)
            | Self::UsbC(_) => self.family_form().map_or(0, |f| f.lead_count()),
        }
    }

    /// Package terminal counts the form accepts: its leads, and for a
    /// no-lead form with an exposed pad also one more (the pad as a
    /// terminal). Empty for `Generic` (any count).
    pub fn terminal_counts(&self) -> Vec<u64> {
        let leads = self.lead_count();
        match self {
            Self::Generic => Vec::new(),
            Self::NoLead(n) if n.exposed_pad().is_some() => vec![leads, leads + 1],
            Self::Tabbed(_)
            | Self::Molded(_)
            | Self::Grid(_)
            | Self::Tht(_)
            | Self::Module(_)
            | Self::UsbC(_) => self
                .family_form()
                .map(|f| f.terminal_counts())
                .unwrap_or_default(),
            _ => vec![leads],
        }
    }

    /// Where each lead touches the seating plane (z = 0) in the model frame,
    /// in lead order (module docs), then a terminal exposed pad's centre.
    /// Empty for `Generic` and for counts beyond [`MAX_FORM_LEADS`].
    pub fn lead_points(&self) -> Vec<Point3> {
        if self.lead_count() > u64::from(MAX_FORM_LEADS) {
            return Vec::new();
        }
        match self {
            Self::Generic => Vec::new(),
            Self::Chip(c) => {
                let x = c.length() / 2.0 - c.termination() / 2.0;
                vec![[-x, 0.0, 0.0], [x, 0.0, 0.0]]
            }
            Self::GullWing(g) => {
                let reach = [
                    g.span()[0] / 2.0 - g.foot_length() / 2.0,
                    g.span()[1] / 2.0 - g.foot_length() / 2.0,
                ];
                ring(g.sides, g.leads_per_side, g.pitch(), reach)
            }
            Self::NoLead(n) => {
                let reach = [
                    n.body_width() / 2.0 - n.terminal_length() / 2.0,
                    n.body_length() / 2.0 - n.terminal_length() / 2.0,
                ];
                let mut points = ring(n.sides, n.terminals_per_side, n.pitch(), reach);
                if n.exposed_pad().is_some() {
                    points.push([0.0; 3]);
                }
                points
            }
            Self::Header(h) => (0..h.pins_per_row)
                .flat_map(|c| (0..h.rows).map(move |r| (c, r)))
                .map(|(c, r)| {
                    [
                        centred(c, h.pins_per_row, h.pitch()),
                        centred(r, h.rows, h.row_pitch()),
                        0.0,
                    ]
                })
                .collect(),
            Self::Tabbed(_)
            | Self::Molded(_)
            | Self::Grid(_)
            | Self::Tht(_)
            | Self::Module(_)
            | Self::UsbC(_) => self
                .family_form()
                .map(|f| f.lead_points())
                .unwrap_or_default(),
        }
    }

    /// The form's own `pkg_*` values, beyond the envelope (`pkg_L`, `pkg_W`,
    /// `pkg_H`) and terminal count (`pkg_n`) that
    /// `footprint_source::package_values` always offers. Names follow JEDEC
    /// outline letters where one exists.
    pub fn values(&self) -> Vec<PackageValue> {
        let length = |name: &str, v: f64| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Length(v),
        };
        let count = |name: &str, v: u32| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Count(v),
        };
        match self {
            Self::Generic => Vec::new(),
            Self::Chip(c) => vec![length("T", c.height()), length("a", c.termination())],
            Self::GullWing(g) => {
                let mut v = vec![
                    length("e", g.pitch()),
                    count("nx", g.leads_per_side[0]),
                    count("ny", g.leads_per_side[1]),
                    length("E1", g.body_width()),
                    length("E", g.span()[0]),
                    length("A1", g.standoff()),
                    length("A2", g.body_height()),
                    length("b", g.lead_width()),
                    length("c", g.lead_thickness()),
                    length("Lf", g.foot_length()),
                ];
                match g.sides {
                    LeadSides::Two => v.push(length("D", g.body_length())),
                    LeadSides::Four => {
                        v.push(length("D1", g.body_length()));
                        v.push(length("D", g.span()[1]));
                    }
                }
                v
            }
            Self::NoLead(n) => {
                let mut v = vec![
                    length("e", n.pitch()),
                    count("nx", n.terminals_per_side[0]),
                    count("ny", n.terminals_per_side[1]),
                    length("D", n.body_length()),
                    length("E", n.body_width()),
                    length("A", n.body_height()),
                    length("b", n.terminal_width()),
                    length("Lt", n.terminal_length()),
                ];
                if let Some([d2, e2]) = n.exposed_pad() {
                    v.push(length("D2", d2));
                    v.push(length("E2", e2));
                }
                v
            }
            Self::Header(h) => vec![
                length("e", h.pitch()),
                count("rows", h.rows),
                count("per_row", h.pins_per_row),
                length("row_pitch", h.row_pitch()),
                length("body_h", h.body_height()),
                length("pin", h.pin_side()),
                length("pin_above", h.pin_above()),
                length("pin_below", h.pin_below()),
            ],
            Self::Tabbed(_)
            | Self::Molded(_)
            | Self::Grid(_)
            | Self::Tht(_)
            | Self::Module(_)
            | Self::UsbC(_) => self.family_form().map(|f| f.values()).unwrap_or_default(),
        }
    }

    /// Lead and terminal counts within [`MAX_FORM_LEADS`]: the persistence
    /// bound (a refusal, unlike the range findings).
    pub fn within_bounds(&self) -> bool {
        self.lead_count() <= u64::from(MAX_FORM_LEADS)
    }

    /// Range rules of the typed parameters, as finding messages (each one
    /// `package.form_range`). Empty when the form is physically consistent.
    pub fn range_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut positive = |name: &str, v: Nm| out.extend(length_problem(name, v));
        match self {
            Self::Generic => {}
            Self::Chip(c) => {
                positive("Chip length", c.length_nm);
                positive("Chip width", c.width_nm);
                positive("Chip height", c.height_nm);
                positive("Termination length", c.termination_nm);
            }
            Self::GullWing(g) => {
                positive("Pitch", g.pitch_nm);
                positive("Body length", g.body_length_nm);
                positive("Body width", g.body_width_nm);
                positive("Body height", g.body_height_nm);
                positive("Lead span", g.span_nm[0]);
                if g.sides == LeadSides::Four {
                    positive("Lead span across x", g.span_nm[1]);
                }
                positive("Lead width", g.lead_width_nm);
                positive("Lead thickness", g.lead_thickness_nm);
                positive("Foot length", g.foot_length_nm);
            }
            Self::NoLead(n) => {
                positive("Pitch", n.pitch_nm);
                positive("Body length", n.body_length_nm);
                positive("Body width", n.body_width_nm);
                positive("Body height", n.body_height_nm);
                positive("Terminal length", n.terminal_length_nm);
                positive("Terminal width", n.terminal_width_nm);
                if let Some(pad) = n.exposed_pad {
                    positive("Exposed-pad length", pad.x_nm);
                    positive("Exposed-pad width", pad.y_nm);
                }
            }
            Self::Header(h) => {
                positive("Pitch", h.pitch_nm);
                positive("Row pitch", h.row_pitch_nm);
                positive("Body height", h.body_height_nm);
                positive("Pin side", h.pin_side_nm);
                positive("Pin length above", h.pin_above_nm);
                positive("Pin length below", h.pin_below_nm);
            }
            Self::Tabbed(_)
            | Self::Molded(_)
            | Self::Grid(_)
            | Self::Tht(_)
            | Self::Module(_)
            | Self::UsbC(_) => {
                if let Some(f) = self.family_form() {
                    out.extend(f.range_problems());
                }
            }
        }
        out.extend(self.consistency_problems());
        out
    }

    /// Rules between parameters: leads fit their body edge, do not touch,
    /// reach past the body, and so on.
    fn consistency_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut rule = |ok: bool, message: &str| {
            if !ok {
                out.push(message.to_owned());
            }
        };
        // A row of `n` features `width` wide at `pitch` must fit in `edge`.
        let fits = |n: u32, pitch: f64, width: f64, edge: f64| {
            f64::from(n.saturating_sub(1)) * pitch + width <= edge + 1e-9
        };
        match self {
            Self::Generic => {}
            Self::Chip(c) => rule(
                2.0 * c.termination() < c.length(),
                "The two terminations must leave body between them (2 × termination < length).",
            ),
            Self::GullWing(g) => {
                let [n, m] = g.leads_per_side;
                rule(n >= 1, "Each long side needs at least one lead.");
                rule(
                    g.pitch() > g.lead_width(),
                    "Leads must not touch (pitch > lead width).",
                );
                rule(
                    fits(n, g.pitch(), g.lead_width(), g.body_length()),
                    "The leads of a long side must fit along the body length.",
                );
                rule(
                    g.span()[0] > g.body_width(),
                    "The lead span must reach past the body width.",
                );
                rule(
                    g.foot_length() <= (g.span()[0] - g.body_width()) / 2.0,
                    "The foot must lie outside the body (foot length ≤ (span − body width) / 2).",
                );
                rule(g.standoff() >= 0.0, "The standoff must not be negative.");
                // The generated lead leaves the body at `lead_exit`, a
                // thickness above the seating plane at least, and inside
                // the moulding; then needs a thickness of room to bend.
                let t = g.lead_thickness();
                rule(
                    g.lead_exit() - t / 2.0 > t && t < g.body_height(),
                    "Leads must fit the body height: thinner than the body, and leaving it (at mid-height) more than 1.5 × lead thickness above the seating plane.",
                );
                rule(
                    g.bend_room()[0] > t,
                    "The span must leave more than a lead thickness outside the body for the lead to bend down to its foot ((span − body width) / 2 − foot length > lead thickness).",
                );
                match g.sides {
                    LeadSides::Two => rule(
                        m == 0 && g.span()[1] == 0.0,
                        "A two-sided form has no short-side leads or span (0).",
                    ),
                    LeadSides::Four => {
                        rule(m >= 1, "Each short side of a four-sided form needs a lead.");
                        rule(
                            fits(m, g.pitch(), g.lead_width(), g.body_width()),
                            "The leads of a short side must fit along the body width.",
                        );
                        rule(
                            g.span()[1] > g.body_length(),
                            "The lead span across x must reach past the body length.",
                        );
                        rule(
                            g.foot_length() <= (g.span()[1] - g.body_length()) / 2.0,
                            "The foot must lie outside the body across x.",
                        );
                        rule(
                            g.bend_room()[1] > t,
                            "The span across x must leave more than a lead thickness outside the body for the lead to bend.",
                        );
                    }
                }
            }
            Self::NoLead(n) => {
                let [nx, ny] = n.terminals_per_side;
                rule(nx >= 1, "Each long side needs at least one terminal.");
                rule(
                    n.pitch() > n.terminal_width(),
                    "Terminals must not touch (pitch > terminal width).",
                );
                rule(
                    fits(nx, n.pitch(), n.terminal_width(), n.body_length()),
                    "The terminals of a long side must fit along the body length.",
                );
                rule(
                    2.0 * n.terminal_length() < n.body_width(),
                    "Opposite terminals must not meet (2 × terminal length < body width).",
                );
                let (inner_l, inner_w) = match n.sides {
                    LeadSides::Two => {
                        rule(ny == 0, "A two-sided form has no short-side terminals (0).");
                        (n.body_length(), n.body_width() - 2.0 * n.terminal_length())
                    }
                    LeadSides::Four => {
                        rule(
                            ny >= 1,
                            "Each short side of a four-sided form needs a terminal.",
                        );
                        rule(
                            fits(ny, n.pitch(), n.terminal_width(), n.body_width()),
                            "The terminals of a short side must fit along the body width.",
                        );
                        rule(
                            2.0 * n.terminal_length() < n.body_length(),
                            "Opposite terminals must not meet across x.",
                        );
                        (
                            n.body_length() - 2.0 * n.terminal_length(),
                            n.body_width() - 2.0 * n.terminal_length(),
                        )
                    }
                };
                if let Some([l, w]) = n.exposed_pad() {
                    rule(
                        l < inner_l && w < inner_w,
                        "The exposed pad must lie inside the terminals.",
                    );
                }
            }
            Self::Header(h) => {
                rule(
                    h.rows >= 1 && h.pins_per_row >= 1,
                    "A header needs at least one pin.",
                );
                rule(
                    h.pin_side() < h.pitch(),
                    "Pins must not touch along a row (pin side < pitch).",
                );
                rule(
                    h.pin_side() < h.row_pitch(),
                    "Pins must fit the body width (pin side < row pitch).",
                );
            }
            // Later families report consistency in `FamilyForm::range_problems`.
            Self::Tabbed(_)
            | Self::Molded(_)
            | Self::Grid(_)
            | Self::Tht(_)
            | Self::Module(_)
            | Self::UsbC(_) => {}
        }
        out
    }
}

/// The position of item `i` of `n` at `pitch`, centred on 0.
fn centred(i: u32, n: u32, pitch: f64) -> f64 {
    (f64::from(i) - f64::from(n.saturating_sub(1)) / 2.0) * pitch
}

/// Lead points of a two- or four-sided row layout in lead order. `reach`
/// is the contact's distance from the centre across y (long sides) and
/// across x (short sides).
fn ring(sides: LeadSides, per: [u32; 2], pitch: f64, reach: [f64; 2]) -> Vec<Point3> {
    let [n, m] = per;
    let along = |i: u32, count: u32| centred(i, count, pitch);
    let mut out: Vec<Point3> = (0..n).map(|i| [along(i, n), -reach[0], 0.0]).collect();
    if sides == LeadSides::Four {
        out.extend((0..m).map(|i| [reach[1], along(i, m), 0.0]));
    }
    out.extend((0..n).rev().map(|i| [along(i, n), reach[0], 0.0]));
    if sides == LeadSides::Four {
        out.extend((0..m).rev().map(|i| [-reach[1], along(i, m), 0.0]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::support::{nm, pt};

    fn soic8() -> GullWingForm {
        GullWingForm {
            sides: LeadSides::Two,
            leads_per_side: [4, 0],
            pitch_nm: nm(1.27),
            body_length_nm: nm(4.9),
            body_width_nm: nm(3.9),
            body_height_nm: nm(1.45),
            standoff_nm: nm(0.1),
            span_nm: [nm(6.0), nm(0.0)],
            lead_width_nm: nm(0.41),
            lead_thickness_nm: nm(0.2),
            foot_length_nm: nm(0.8),
        }
    }

    fn qfn16() -> NoLeadForm {
        NoLeadForm {
            sides: LeadSides::Four,
            terminals_per_side: [4, 4],
            pitch_nm: nm(0.5),
            body_length_nm: nm(3.0),
            body_width_nm: nm(3.0),
            body_height_nm: nm(0.85),
            terminal_length_nm: nm(0.4),
            terminal_width_nm: nm(0.25),
            exposed_pad: Some(pt([1.7, 1.7])),
        }
    }

    fn header() -> HeaderForm {
        HeaderForm {
            rows: 2,
            pins_per_row: 3,
            pitch_nm: nm(2.54),
            row_pitch_nm: nm(2.54),
            body_height_nm: nm(2.5),
            pin_side_nm: nm(0.64),
            pin_above_nm: nm(6.0),
            pin_below_nm: nm(3.0),
        }
    }

    fn chip(kind: ChipKind, size: [f64; 3]) -> ChipForm {
        ChipForm::new(kind, size.map(nm))
    }

    fn names(form: &PackageForm) -> Vec<String> {
        form.values().into_iter().map(|v| v.name.0).collect()
    }

    #[test]
    fn typical_forms_have_no_range_problems() {
        for form in [
            PackageForm::Chip(Box::new(chip(ChipKind::Resistor, [1.6, 0.8, 0.45]))),
            PackageForm::GullWing(Box::new(soic8())),
            PackageForm::NoLead(Box::new(qfn16())),
            PackageForm::Header(Box::new(header())),
        ] {
            assert_eq!(
                form.range_problems(),
                Vec::<String>::new(),
                "{}",
                form.label()
            );
            assert!(form.within_bounds());
        }
    }

    #[test]
    fn envelopes_and_lead_counts() {
        let soic = PackageForm::GullWing(Box::new(soic8()));
        assert_eq!(soic.envelope(), Some([4.9, 6.0, 1.55]));
        assert_eq!(soic.lead_count(), 8);
        let qfn = PackageForm::NoLead(Box::new(qfn16()));
        assert_eq!(qfn.envelope(), Some([3.0, 3.0, 0.85]));
        assert_eq!(qfn.terminal_counts(), vec![16, 17]);
        let h = PackageForm::Header(Box::new(header()));
        assert_eq!(h.envelope(), Some([3.0 * 2.54, 2.0 * 2.54, 8.5]));
        assert_eq!(h.lead_count(), 6);
        assert_eq!(PackageForm::Generic.envelope(), None);
        assert!(PackageForm::Generic.terminal_counts().is_empty());
    }

    #[test]
    fn lead_points_run_counter_clockwise_from_the_minus_y_row() {
        let soic = PackageForm::GullWing(Box::new(soic8()));
        let p = soic.lead_points();
        assert_eq!(p.len(), 8);
        let y = 3.0 - 0.4;
        // Pin 1 at the −x end of the −y row; pin 4 at its +x end; pin 5
        // opposite pin 4; pin 8 opposite pin 1.
        assert!((p[0][0] + 1.905).abs() < 1e-12 && (p[0][1] + y).abs() < 1e-12);
        assert!((p[3][0] - 1.905).abs() < 1e-12);
        assert_eq!(p[4], [p[3][0], -p[3][1], 0.0]);
        assert_eq!(p[7], [p[0][0], -p[0][1], 0.0]);
        let qfn = PackageForm::NoLead(Box::new(qfn16()));
        let q = qfn.lead_points();
        assert_eq!(q.len(), 17, "16 terminals, then the exposed pad");
        // Pin 5 starts the +x side at its lowest y; pin 16 ends −x lowest.
        assert!(q[4][0] > 1.0 && q[4][1] < 0.0);
        assert!(q[15][0] < -1.0 && q[15][1] < 0.0);
        assert_eq!(q[16], [0.0; 3]);
        let chip = PackageForm::Chip(Box::new(chip(ChipKind::Capacitor, [2.0, 1.25, 0.6])));
        assert_eq!(chip.lead_points(), vec![[-0.8, 0.0, 0.0], [0.8, 0.0, 0.0]]);
        let h = PackageForm::Header(Box::new(header()));
        let hp = h.lead_points();
        // Column-major: pin 2 is beside pin 1 across the rows.
        assert_eq!(hp[0], [-2.54, -1.27, 0.0]);
        assert_eq!(hp[1], [-2.54, 1.27, 0.0]);
        assert_eq!(hp[5], [2.54, 1.27, 0.0]);
    }

    #[test]
    fn values_follow_the_family() {
        assert!(names(&PackageForm::Generic).is_empty());
        assert_eq!(
            names(&PackageForm::GullWing(Box::new(soic8()))),
            [
                "pkg_e", "pkg_nx", "pkg_ny", "pkg_E1", "pkg_E", "pkg_A1", "pkg_A2", "pkg_b",
                "pkg_c", "pkg_Lf", "pkg_D"
            ]
        );
        let mut qfp = soic8();
        qfp.sides = LeadSides::Four;
        qfp.leads_per_side = [3, 3];
        qfp.body_width_nm = nm(4.9);
        qfp.span_nm = [nm(6.9), nm(6.9)];
        let qfp = PackageForm::GullWing(Box::new(qfp));
        assert!(
            qfp.range_problems().is_empty(),
            "{:?}",
            qfp.range_problems()
        );
        let n = names(&qfp);
        assert!(n.contains(&"pkg_D1".to_owned()) && n.contains(&"pkg_D".to_owned()));
        let qfn = names(&PackageForm::NoLead(Box::new(qfn16())));
        assert!(qfn.contains(&"pkg_D2".to_owned()) && qfn.contains(&"pkg_E2".to_owned()));
        // Every name is a Solidcraft identifier under the reserved prefix.
        for name in names(&PackageForm::Header(Box::new(header())))
            .iter()
            .chain(&qfn)
        {
            assert!(name.starts_with(PACKAGE_PREFIX));
            assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        }
    }

    #[test]
    fn inconsistent_forms_are_found() {
        let mut c = chip(ChipKind::Resistor, [1.0, 0.5, 0.35]);
        c.termination_nm = nm(0.6);
        assert_eq!(PackageForm::Chip(Box::new(c)).range_problems().len(), 1);
        let mut g = soic8();
        g.pitch_nm = nm(0.3); // narrower than the lead
        g.span_nm[0] = nm(3.5); // inside the body
        let problems = PackageForm::GullWing(Box::new(g)).range_problems();
        assert!(problems.iter().any(|p| p.contains("touch")), "{problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("reach past")),
            "{problems:?}"
        );
        let mut g = soic8();
        g.leads_per_side = [4, 2];
        assert!(
            !PackageForm::GullWing(Box::new(g))
                .range_problems()
                .is_empty()
        );
        let mut n = qfn16();
        n.exposed_pad = Some(pt([2.5, 2.5]));
        assert!(!PackageForm::NoLead(Box::new(n)).range_problems().is_empty());
        let mut h = header();
        h.pin_side_nm = nm(3.0);
        h.pin_below_nm = nm(0.0);
        assert_eq!(PackageForm::Header(Box::new(h)).range_problems().len(), 3);
        let mut h = header();
        // Out of range (a stored length cannot be NaN).
        h.pitch_nm = nm(2000.0);
        assert!(!PackageForm::Header(Box::new(h)).range_problems().is_empty());
    }

    #[test]
    fn counts_beyond_the_bound_are_refused() {
        let mut h = header();
        h.rows = 100;
        h.pins_per_row = 100;
        let form = PackageForm::Header(Box::new(h));
        assert!(!form.within_bounds());
        assert!(form.lead_points().is_empty());
        let mut h = header();
        h.rows = u32::MAX;
        h.pins_per_row = u32::MAX;
        assert!(
            !PackageForm::Header(Box::new(h)).within_bounds(),
            "no overflow"
        );
    }
}
