//! The module package family: a small printed circuit board carrying a
//! shield can, with edge pads (castellated half-holes or LGA lands) in
//! rows along its sides, an optional bottom ground-pad array, optional
//! single pads (corner ground pads) and an optional on-board antenna at
//! one end (Wi-Fi/BLE modules and similar). The form, its range rules,
//! lead order and the host keep-out over the antenna live here; the lands
//! are `samples::form_lands_with`, the footprint keep-out
//! [`ModuleForm::antenna_keepout`] and the model builder
//! `pcbcraft-solidcraft` `models/module.rs`.
//!
//! Frame: seen from above, x across the module (`width`), y along it
//! (`length`), the antenna end at +y, the origin at the outline centre.
//! Pads are numbered from 1 through the `rows` in the order listed; each
//! row runs counter-clockwise seen from above (a −x row downwards, a −y
//! row towards +x, a +x row upwards, a +y row towards −x) from its
//! `first` pad, `pitch` apart, so rows listed −x, −y, +x, +y number the
//! way module datasheets do. Each row pad is `pad_size[0]` across its
//! edge and `pad_size[1]` along it. After the rows comes the ground-pad
//! array as one terminal (`ground.number`, landed by every pad of the
//! array), then each single pad as its own terminal (`singles`).
//!
//! Values are the module datasheet's mechanical drawing and recommended
//! land pattern; the typical values below are an original synthetic
//! module in round numbers, not any product's.
use super::{FORM_LENGTH_LIMIT_MM, FORM_LENGTH_MIN_MM, FamilyForm, MAX_FORM_LEADS};
use crate::geometry::Point3;
use crate::ids::{PackageTerminalId, PadId};
use crate::library::footprint::Pad;
use crate::library::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Most pads of one ground-pad array.
pub const MAX_GROUND_PADS: u32 = 256;
/// Most pad rows and single pads of one module.
pub const MAX_MODULE_ROWS: usize = 16;
pub const MAX_MODULE_SINGLES: usize = 64;
/// Bytes of a terminal number the form names.
pub const MAX_PAD_NUMBER: usize = 16;
/// Slack when a pad may touch the outline (a castellated pad), mm.
const EDGE_EPS_MM: f64 = 1e-6;

/// The side of the module a pad row runs along.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModuleSide {
    /// −x, numbered downwards (−y).
    Left,
    /// −y, numbered towards +x.
    Bottom,
    /// +x, numbered upwards (+y).
    Right,
    /// +y, numbered towards −x.
    Top,
}

impl ModuleSide {
    /// The unit step from one pad of a row to the next.
    pub fn step(self) -> [f64; 2] {
        match self {
            Self::Left => [0.0, -1.0],
            Self::Bottom => [1.0, 0.0],
            Self::Right => [0.0, 1.0],
            Self::Top => [-1.0, 0.0],
        }
    }

    /// The outward normal of the side.
    pub fn outward(self) -> [f64; 2] {
        match self {
            Self::Left => [-1.0, 0.0],
            Self::Bottom => [0.0, -1.0],
            Self::Right => [1.0, 0.0],
            Self::Top => [0.0, 1.0],
        }
    }

    /// Whether the row's pads are long in x (across a ±x edge).
    pub fn across_x(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// One row of edge pads.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadRow {
    pub side: ModuleSide,
    pub count: u32,
    /// Centre of the row's first pad (lowest number) in the module frame.
    pub first: [f64; 2],
}

/// The bottom ground-pad array: `rows` × `cols` square-cornered pads at
/// `pitch`, each `size`, in a grid turned `rotation_deg` counter-clockwise
/// about `centre`, all landing one terminal numbered `number`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GroundPads {
    pub rows: u32,
    pub cols: u32,
    /// Centre-to-centre pitch along the grid's x and y.
    pub pitch: [f64; 2],
    /// Pad size along the grid's x and y.
    pub size: [f64; 2],
    pub centre: [f64; 2],
    #[serde(default)]
    pub rotation_deg: f64,
    /// The terminal's number (the datasheet's pin number, e.g. "95").
    pub number: String,
}

/// One single pad that is its own terminal (a corner ground pad).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SinglePad {
    pub number: String,
    pub centre: [f64; 2],
    pub size: [f64; 2],
}

/// The host-board keep-out over the antenna: the antenna region (full
/// width, `antenna_length` deep from the +y edge) grown beyond the
/// module's −x, +x and +y edges. It applies to every copper layer and
/// forbids fills, tracks and vias.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AntennaKeepout {
    /// Growth beyond the −x edge, the +x edge and the +y edge, mm.
    pub beyond: [f64; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModuleForm {
    /// Module outline across (x) and along (y; the antenna end is +y).
    pub width: f64,
    pub length: f64,
    /// Seated height: the board and the shield can.
    pub height: f64,
    /// The module board's thickness.
    pub pcb_thickness: f64,
    /// Depth of the antenna region from the +y edge; 0 for a module
    /// without an on-board antenna (connector or external antenna).
    pub antenna_length: f64,
    /// Shield can size (x, y) and centre in the module frame.
    pub can_size: [f64; 2],
    pub can_centre: [f64; 2],
    /// Pitch within every row.
    pub pitch: f64,
    /// Row pad size: across its edge, along it.
    pub pad_size: [f64; 2],
    /// Pad rows in numbering order.
    pub rows: Vec<PadRow>,
    /// Half-hole (castellation) diameter where a row pad meets the
    /// outline; 0 for LGA pads.
    #[serde(default)]
    pub castellation: f64,
    /// The recommended host land of a row pad: its width along the edge
    /// and its growth beyond the module pad's outer and inner ends
    /// (0, 0 when the land pattern equals the module pads).
    pub land_width: f64,
    #[serde(default)]
    pub land_extend: [f64; 2],
    #[serde(default)]
    pub ground: Option<GroundPads>,
    #[serde(default)]
    pub singles: Vec<SinglePad>,
    #[serde(default)]
    pub keepout: Option<AntennaKeepout>,
}

impl ModuleForm {
    /// Original typical values (a synthetic 18 × 25 mm castellated module
    /// in round numbers: 14 + 10 + 14 pads at 1.27 mm, a 3 × 3 ground
    /// array and a 6 mm antenna end): the family's custom entry.
    pub fn typical() -> Self {
        let (hw, hl) = (9.0, 12.5);
        Self {
            width: 18.0,
            length: 25.0,
            height: 3.2,
            pcb_thickness: 0.8,
            antenna_length: 6.0,
            can_size: [16.6, 17.4],
            can_centre: [0.0, -2.5],
            pitch: 1.27,
            pad_size: [1.2, 0.9],
            rows: vec![
                PadRow {
                    side: ModuleSide::Left,
                    count: 14,
                    first: [-hw + 0.6, hl - 7.0],
                },
                PadRow {
                    side: ModuleSide::Bottom,
                    count: 10,
                    first: [-5.715, -hl + 0.6],
                },
                PadRow {
                    side: ModuleSide::Right,
                    count: 14,
                    first: [hw - 0.6, hl - 7.0 - 13.0 * 1.27],
                },
            ],
            castellation: 0.6,
            land_width: 0.9,
            land_extend: [0.3, 0.0],
            ground: Some(GroundPads {
                rows: 3,
                cols: 3,
                pitch: [1.6, 1.6],
                size: [1.1, 1.1],
                centre: [0.0, -2.0],
                rotation_deg: 0.0,
                number: "39".into(),
            }),
            singles: Vec::new(),
            keepout: Some(AntennaKeepout {
                beyond: [0.0, 0.0, 0.0],
            }),
        }
    }

    /// Row pads in all.
    pub fn row_pads(&self) -> u64 {
        self.rows.iter().map(|r| u64::from(r.count)).sum()
    }

    /// Pads of the ground array.
    pub fn ground_pads(&self) -> u64 {
        self.ground
            .as_ref()
            .map_or(0, |g| u64::from(g.rows) * u64::from(g.cols))
    }

    /// Each row pad's side and centre, in numbering order. Empty beyond
    /// [`MAX_FORM_LEADS`].
    pub fn row_centres(&self) -> Vec<(ModuleSide, [f64; 2])> {
        if self.row_pads() > u64::from(MAX_FORM_LEADS) || self.rows.len() > MAX_MODULE_ROWS {
            return Vec::new();
        }
        let mut out = Vec::new();
        for row in &self.rows {
            let d = row.side.step();
            for k in 0..row.count {
                let t = f64::from(k) * self.pitch;
                out.push((row.side, [row.first[0] + d[0] * t, row.first[1] + d[1] * t]));
            }
        }
        out
    }

    /// A row pad's size in the module frame (x, y).
    pub fn row_pad_size(&self, side: ModuleSide) -> [f64; 2] {
        let [across, along] = self.pad_size;
        if side.across_x() {
            [across, along]
        } else {
            [along, across]
        }
    }

    /// The host land centre and size (x, y) of each row pad, in numbering
    /// order: the module pad grown by `land_extend` at its outer and inner
    /// ends, `land_width` wide.
    pub fn row_lands(&self) -> Vec<([f64; 2], [f64; 2])> {
        let [outer, inner] = self.land_extend;
        let across = self.pad_size[0] + outer + inner;
        let shift = (outer - inner) / 2.0;
        self.row_centres()
            .into_iter()
            .map(|(side, [x, y])| {
                let n = side.outward();
                let centre = [x + n[0] * shift, y + n[1] * shift];
                let size = if side.across_x() {
                    [across, self.land_width]
                } else {
                    [self.land_width, across]
                };
                (centre, size)
            })
            .collect()
    }

    /// The ground-pad centres, row by row from the grid's +y, columns from
    /// its −x, turned about the array centre.
    pub fn ground_centres(&self) -> Vec<[f64; 2]> {
        let Some(g) = &self.ground else {
            return Vec::new();
        };
        if self.ground_pads() > u64::from(MAX_GROUND_PADS) {
            return Vec::new();
        }
        let along = |i: u32, n: u32, pitch: f64| {
            (f64::from(i) - f64::from(n.saturating_sub(1)) / 2.0) * pitch
        };
        let (c, s) = crate::geometry::cos_sin(g.rotation_deg);
        (0..g.rows)
            .flat_map(|r| (0..g.cols).map(move |col| (r, col)))
            .map(|(r, col)| {
                let u = along(col, g.cols, g.pitch[0]);
                let v = -along(r, g.rows, g.pitch[1]);
                [g.centre[0] + c * u - s * v, g.centre[1] + s * u + c * v]
            })
            .collect()
    }

    /// Terminal numbers in lead order: rows from 1, the ground number, then
    /// each single pad's.
    pub fn designations(&self) -> Vec<String> {
        let rows = usize::try_from(self.row_pads()).unwrap_or(0);
        let mut out: Vec<String> = (1..=rows).map(|k| k.to_string()).collect();
        if let Some(g) = &self.ground {
            out.push(g.number.trim().to_owned());
        }
        out.extend(self.singles.iter().map(|p| p.number.trim().to_owned()));
        out
    }

    /// [`Self::designations`] for a package of `count` terminals: the
    /// form's numbers, then numbers past the last for any extra terminal.
    pub fn designations_for(&self, count: usize) -> Vec<String> {
        let mut out = self.designations();
        out.truncate(count);
        while out.len() < count {
            out.push((out.len() + 1).to_string());
        }
        out
    }

    /// The host lands in lead order, landing `terminals[k]`: one
    /// rectangular land per row pad ([`Self::row_lands`]), every pad of the
    /// ground array (turned with it) landing the ground terminal, then the
    /// single pads at their module size. Pad IDs count from 1. `None` when
    /// the terminal count differs from the form's.
    pub fn lands(&self, terminals: &[PackageTerminalId]) -> Option<Vec<Pad>> {
        let expected = self.terminal_counts().first().copied()?;
        if u64::try_from(terminals.len()).ok()? != expected || !self.range_problems().is_empty() {
            return None;
        }
        let numbers = self.designations();
        let mut terminal = terminals.iter().zip(numbers);
        let mut pads = Vec::new();
        let mut push = |pad: Pad| -> Option<()> {
            let mut pad = pad;
            pad.id = PadId(u32::try_from(pads.len() + 1).ok()?);
            pads.push(pad);
            Some(())
        };
        for (centre, size) in self.row_lands() {
            let (t, number) = terminal.next()?;
            push(Pad::smd_rect(PadId(0), *t, number, centre, size))?;
        }
        if let Some(g) = &self.ground {
            let (t, number) = terminal.next()?;
            for centre in self.ground_centres() {
                let mut pad = Pad::smd_rect(PadId(0), *t, number.clone(), centre, g.size);
                pad.rotation_deg = pad.shape.canonical_rotation(g.rotation_deg);
                push(pad)?;
            }
        }
        for single in &self.singles {
            let (t, number) = terminal.next()?;
            push(Pad::smd_rect(
                PadId(0),
                *t,
                number,
                single.centre,
                single.size,
            ))?;
        }
        Some(pads)
    }

    /// The antenna keep-out rectangle (min, max) in the module frame, or
    /// `None` without an antenna or a keep-out.
    pub fn antenna_keepout(&self) -> Option<[[f64; 2]; 2]> {
        let k = self.keepout?;
        if self.antenna_length <= 0.0 {
            return None;
        }
        let (hw, hl) = (self.width / 2.0, self.length / 2.0);
        Some([
            [-hw - k.beyond[0], hl - self.antenna_length],
            [hw + k.beyond[1], hl + k.beyond[2]],
        ])
    }

    /// The shield can's height above the module board.
    pub fn can_height(&self) -> f64 {
        self.height - self.pcb_thickness
    }

    /// Whether a row pad at `centre` on `side` reaches the outline (gets a
    /// castellation).
    pub fn at_edge(&self, side: ModuleSide, centre: [f64; 2]) -> bool {
        let half = self.pad_size[0] / 2.0;
        let (hw, hl) = (self.width / 2.0, self.length / 2.0);
        let reach = match side {
            ModuleSide::Left => -centre[0] + half - hw,
            ModuleSide::Right => centre[0] + half - hw,
            ModuleSide::Bottom => -centre[1] + half - hl,
            ModuleSide::Top => centre[1] + half - hl,
        };
        reach >= -0.05
    }
}

fn valid_number(n: &str) -> bool {
    let n = n.trim();
    !n.is_empty() && n.len() <= MAX_PAD_NUMBER && n.chars().all(|c| c.is_ascii_graphic())
}

impl FamilyForm for ModuleForm {
    fn label(&self) -> &'static str {
        "Module"
    }
    fn lengths(&self) -> Vec<f64> {
        let mut v = vec![
            self.width,
            self.length,
            self.height,
            self.pcb_thickness,
            self.antenna_length,
            self.can_size[0],
            self.can_size[1],
            self.can_centre[0],
            self.can_centre[1],
            self.pitch,
            self.pad_size[0],
            self.pad_size[1],
            self.castellation,
            self.land_width,
            self.land_extend[0],
            self.land_extend[1],
        ];
        for r in &self.rows {
            v.extend(r.first);
        }
        if let Some(g) = &self.ground {
            v.extend(g.pitch);
            v.extend(g.size);
            v.extend(g.centre);
        }
        for p in &self.singles {
            v.extend(p.centre);
            v.extend(p.size);
        }
        if let Some(k) = &self.keepout {
            v.extend(k.beyond);
        }
        v
    }
    fn envelope(&self) -> Option<[f64; 3]> {
        Some([self.width, self.length, self.height])
    }
    fn lead_count(&self) -> u64 {
        self.row_pads()
    }
    /// Row pads, the ground terminal and the single pads.
    fn terminal_counts(&self) -> Vec<u64> {
        let singles = u64::try_from(self.singles.len()).unwrap_or(u64::MAX);
        vec![self.row_pads() + u64::from(self.ground.is_some()) + singles]
    }
    fn lead_points(&self) -> Vec<Point3> {
        let mut out: Vec<Point3> = self
            .row_centres()
            .into_iter()
            .map(|(_, [x, y])| [x, y, 0.0])
            .collect();
        if let Some(g) = &self.ground {
            out.push([g.centre[0], g.centre[1], 0.0]);
        }
        out.extend(self.singles.iter().map(|p| [p.centre[0], p.centre[1], 0.0]));
        out
    }
    /// `W`, `Lm`, `A` (outline and seated height), `A1` (board), `e`
    /// (pitch), `Lp` and `b` (row pad across and along its edge), `Lant`
    /// (antenna depth).
    fn values(&self) -> Vec<PackageValue> {
        let length = |name: &str, v: f64| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Length(v),
        };
        vec![
            length("W", self.width),
            length("Lm", self.length),
            length("A", self.height),
            length("A1", self.pcb_thickness),
            length("e", self.pitch),
            length("Lp", self.pad_size[0]),
            length("b", self.pad_size[1]),
            length("Lant", self.antenna_length),
        ]
    }
    fn range_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let in_range =
            |v: f64| v.is_finite() && (FORM_LENGTH_MIN_MM..=FORM_LENGTH_LIMIT_MM).contains(&v);
        let mut positive = |name: &str, v: f64| {
            if !in_range(v) {
                out.push(format!(
                    "{name} must be {FORM_LENGTH_MIN_MM}–{FORM_LENGTH_LIMIT_MM} mm (is {v})."
                ));
            }
        };
        positive("Module width", self.width);
        positive("Module length", self.length);
        positive("Height", self.height);
        positive("Board thickness", self.pcb_thickness);
        positive("Can width", self.can_size[0]);
        positive("Can length", self.can_size[1]);
        positive("Pitch", self.pitch);
        positive("Pad length", self.pad_size[0]);
        positive("Pad width", self.pad_size[1]);
        positive("Land width", self.land_width);
        let mut rule = |ok: bool, message: String| {
            if !ok {
                out.push(message);
            }
        };
        let finite = |v: f64| v.is_finite() && v.abs() <= FORM_LENGTH_LIMIT_MM;
        if !self.lengths().into_iter().all(finite) {
            rule(false, "Every length must be finite.".into());
            return out;
        }
        rule(
            self.antenna_length >= 0.0 && self.antenna_length < self.length,
            "The antenna depth is 0 (no antenna) or positive and shorter than the module.".into(),
        );
        rule(
            self.castellation >= 0.0 && self.castellation < self.pad_size[1],
            "The castellation diameter is 0 (LGA pads) or narrower than its pad.".into(),
        );
        rule(
            self.height > self.pcb_thickness,
            format!(
                "The height ({}) must exceed the board thickness ({}) by the can.",
                self.height, self.pcb_thickness
            ),
        );
        rule(
            self.rows.len() <= MAX_MODULE_ROWS && self.singles.len() <= MAX_MODULE_SINGLES,
            format!(
                "A module has at most {MAX_MODULE_ROWS} pad rows and {MAX_MODULE_SINGLES} single pads."
            ),
        );
        let n = self.row_pads();
        rule(
            (1..=u64::from(MAX_FORM_LEADS)).contains(&n) && self.rows.iter().all(|r| r.count > 0),
            format!("A module has 1–{MAX_FORM_LEADS} row pads and no empty row (has {n})."),
        );
        rule(
            self.pad_size[1] < self.pitch && self.land_width < self.pitch,
            "Row pads and lands must not touch (width < pitch).".into(),
        );
        rule(
            self.land_extend
                .iter()
                .all(|v| *v > -self.pad_size[0] / 2.0),
            "A land may shrink by less than half the pad length at each end.".into(),
        );
        let (hw, hl) = (self.width / 2.0, self.length / 2.0);
        let inside = |c: [f64; 2], s: [f64; 2]| {
            c[0] - s[0] / 2.0 >= -hw - EDGE_EPS_MM
                && c[0] + s[0] / 2.0 <= hw + EDGE_EPS_MM
                && c[1] - s[1] / 2.0 >= -hl - EDGE_EPS_MM
                && c[1] + s[1] / 2.0 <= hl + EDGE_EPS_MM
        };
        rule(
            self.row_centres()
                .iter()
                .all(|(side, c)| inside(*c, self.row_pad_size(*side))),
            "Every row pad must lie within the module outline.".into(),
        );
        let [cw, cl] = self.can_size;
        let [cx, cy] = self.can_centre;
        rule(
            inside(self.can_centre, self.can_size)
                && cy + cl / 2.0 <= hl - self.antenna_length + EDGE_EPS_MM
                && cx.abs() <= hw
                && cw <= self.width,
            "The shield can must lie within the module, clear of the antenna region.".into(),
        );
        if let Some(g) = &self.ground {
            let count = u64::from(g.rows) * u64::from(g.cols);
            rule(
                (1..=u64::from(MAX_GROUND_PADS)).contains(&count),
                format!("A ground array has 1–{MAX_GROUND_PADS} pads."),
            );
            rule(
                g.pitch.iter().chain(&g.size).all(|v| in_range(*v)) && g.rotation_deg.is_finite(),
                "Ground pad sizes and pitches must be positive and finite.".into(),
            );
            rule(
                (g.cols < 2 || g.size[0] < g.pitch[0]) && (g.rows < 2 || g.size[1] < g.pitch[1]),
                "Ground pads must not touch (size < pitch).".into(),
            );
            let r = g.size[0].hypot(g.size[1]) / 2.0;
            rule(
                self.ground_centres()
                    .iter()
                    .all(|c| inside(*c, [2.0 * r, 2.0 * r])),
                "The ground array must lie within the module.".into(),
            );
            rule(
                valid_number(&g.number),
                format!(
                    "The ground terminal needs a number of 1–{MAX_PAD_NUMBER} printable characters."
                ),
            );
        }
        rule(
            self.singles.iter().all(|p| {
                valid_number(&p.number)
                    && p.size.iter().all(|v| in_range(*v))
                    && inside(p.centre, p.size)
            }),
            "Single pads need a number, a positive size and a place within the module.".into(),
        );
        let numbers = self.designations();
        let unique: std::collections::BTreeSet<&String> = numbers.iter().collect();
        rule(
            unique.len() == numbers.len(),
            "Terminal numbers must be unique (rows count from 1).".into(),
        );
        if let Some(k) = &self.keepout {
            rule(
                k.beyond.iter().all(|v| *v >= 0.0),
                "Keep-out growth beyond the edges is 0 or positive.".into(),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typical_values_are_in_range_and_count_their_terminals() {
        let m = ModuleForm::typical();
        assert_eq!(m.range_problems(), Vec::<String>::new());
        assert_eq!(m.lead_count(), 38);
        assert_eq!(m.terminal_counts(), vec![39]);
        assert_eq!(m.lead_points().len(), 39);
        assert_eq!(m.ground_centres().len(), 9);
        assert_eq!(m.designations().last().map(String::as_str), Some("39"));
    }

    #[test]
    fn rows_run_counter_clockwise_from_their_first_pad() {
        let m = ModuleForm::typical();
        let e = m.row_centres();
        assert_eq!(e[0], (ModuleSide::Left, [-8.4, 5.5]));
        assert!((e[13].1[1] - (5.5 - 13.0 * 1.27)).abs() < 1e-9);
        assert_eq!(e[14].0, ModuleSide::Bottom);
        assert!((e[23].1[0] - 5.715).abs() < 1e-9, "{:?}", e[23]);
        // The right column climbs back level with pin 1.
        assert!((e[37].1[1] - 5.5).abs() < 1e-9);
        assert!(m.at_edge(e[0].0, e[0].1));
    }

    #[test]
    fn lands_grow_outward_by_the_extension() {
        let m = ModuleForm::typical();
        let (c, s) = m.row_lands()[0];
        assert!((c[0] - s[0] / 2.0 - (-9.3)).abs() < 1e-9);
        assert_eq!(s, [1.5, 0.9]);
        let (c, s) = m.row_lands()[14];
        assert!((c[1] - s[1] / 2.0 - (-12.8)).abs() < 1e-9);
    }

    #[test]
    fn a_turned_ground_array_keeps_its_pitch() {
        let mut m = ModuleForm::typical();
        if let Some(g) = &mut m.ground {
            g.rotation_deg = 45.0;
        }
        let c = m.ground_centres();
        let d = (c[1][0] - c[0][0]).hypot(c[1][1] - c[0][1]);
        assert!((d - 1.6).abs() < 1e-9);
        // The centre pad stays put.
        assert!((c[4][0]).abs() < 1e-9 && (c[4][1] + 2.0).abs() < 1e-9);
    }

    #[test]
    fn the_keepout_covers_the_antenna_end() {
        let mut m = ModuleForm::typical();
        assert_eq!(m.antenna_keepout(), Some([[-9.0, 6.5], [9.0, 12.5]]));
        m.keepout = Some(AntennaKeepout {
            beyond: [1.0, 2.0, 3.0],
        });
        assert_eq!(m.antenna_keepout(), Some([[-10.0, 6.5], [11.0, 15.5]]));
        m.antenna_length = 0.0;
        assert_eq!(m.antenna_keepout(), None);
    }

    #[test]
    fn inconsistent_values_are_named() {
        let mut m = ModuleForm::typical();
        m.can_centre = [0.0, 3.0];
        assert!(m.range_problems().iter().any(|p| p.contains("shield can")));
        let mut m = ModuleForm::typical();
        m.rows[0].count = 30;
        assert!(
            m.range_problems()
                .iter()
                .any(|p| p.contains("row pad must"))
        );
        let mut m = ModuleForm::typical();
        m.height = 0.5;
        assert!(!m.range_problems().is_empty());
        let mut m = ModuleForm::typical();
        if let Some(g) = &mut m.ground {
            g.number = "3".into();
        }
        assert!(m.range_problems().iter().any(|p| p.contains("unique")));
    }
}
