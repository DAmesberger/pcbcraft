//! The USB-C receptacle family: a surface-mount USB Type-C receptacle for
//! USB 2.0 with 16 contacts on 12 lands (the four GND and four VBUS
//! contacts share a land per pair) and a shell held by four plated
//! through-hole stakes, with two plastic locating pegs. The form, its
//! range rules, lead order, lands and the electrical terminals the A and
//! B contact rows share live here; the model builder is
//! `pcbcraft-solidcraft` `models/usb_c.rs`.
//!
//! Frame: seen from above, x across the receptacle (`width`), y along its
//! depth with the mating face at −y (the board edge side) and the solder
//! tails at +y, the origin at the outline centre, z = 0 the seating
//! plane. Lands are numbered in x order from −x, as the receptacle's
//! recommended layout prints them seen from the component side:
//! `A1B12`, `A4B9`, `B8`, `A5`, `B7`, `A6`, `A7`, `B6`, `A8`, `B5`,
//! `B4A9`, `B1A12`; then `S`, the shell, landed by all four stakes. The
//! locating-peg holes are non-plated and land nothing.
//!
//! The contact order and the pairing of the A and B rows are the USB
//! Type-C pinout; the dimensions are the manufacturer's recommended PCB
//! layout and outline drawing (the built-in row cites it).
use super::{FORM_LENGTH_LIMIT_MM, FORM_LENGTH_MIN_MM, FamilyForm};
use crate::functions::Direction;
use crate::geometry::Point3;
use crate::ids::{PackageTerminalId, PadId};
use crate::library::footprint::{Hole, HoleShape, MaskRule, Pad, PadKind, PadShape};
use crate::library::footprint_source::{PACKAGE_PREFIX, PackageValue, ParamName, ParamValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The twelve contact lands in x order from −x, then the shell.
pub const DESIGNATIONS: [&str; 13] = [
    "A1B12", "A4B9", "B8", "A5", "B7", "A6", "A7", "B6", "A8", "B5", "B4A9", "B1A12", "S",
];

/// The electrical terminals of a USB 2.0 receptacle in symbol order, each
/// with its pin type and the lands (indices into [`DESIGNATIONS`]) that
/// carry it. The A and B rows' duplicates (VBUS, GND, D+, D−) are one
/// terminal carried by several package terminals, so both plug
/// orientations are joined by the netlist. VBUS is the board's supply
/// (power output); a connector drives nothing else, so the rest are
/// passive.
pub const TERMINALS: [(&str, Direction, &[usize]); 9] = [
    ("VBUS", Direction::PowerOutput, &[1, 10]),
    ("CC1", Direction::Passive, &[3]),
    ("CC2", Direction::Passive, &[9]),
    ("D+", Direction::Passive, &[5, 7]),
    ("D-", Direction::Passive, &[6, 4]),
    ("SBU1", Direction::Passive, &[8]),
    ("SBU2", Direction::Passive, &[2]),
    ("GND", Direction::Passive, &[0, 11]),
    ("SHIELD", Direction::Passive, &[12]),
];

/// Slack when the shell exactly fills the outline, mm.
const FIT_EPS_MM: f64 = 1e-6;

/// One pair of shell stakes (at −x and +x): its plated slot and land.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ShellTab {
    /// Centre along y in the receptacle frame.
    pub y: f64,
    /// The milled slot: width (across, x) and length (along y).
    pub slot: [f64; 2],
    /// The land round it: width (x) and length (y).
    pub land: [f64; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UsbCForm {
    /// Outline across (x, over the shell stakes), deep (y) and seated
    /// height.
    pub width: f64,
    pub depth: f64,
    pub height: f64,
    /// The mating opening across and high, and its centre height.
    pub opening: [f64; 2],
    pub opening_z: f64,
    /// Shell sheet thickness: the outline less the opening. The drawing's
    /// (8.94 − 8.34) / 2 and 3.31 − 1.73 − 2.56 / 2 agree on 0.30 mm, and
    /// its stakes at ±4.32 lie mid-sheet.
    pub shell_thickness: f64,
    /// Contact land pitch, the lands' centre line (y) and length.
    pub pitch: f64,
    pub land_y: f64,
    pub land_length: f64,
    /// Land width of the eight signal contacts and of the four shared
    /// VBUS and GND lands.
    pub signal_land_width: f64,
    pub power_land_width: f64,
    /// Centre of the VBUS and of the GND lands from x = 0.
    pub vbus_x: f64,
    pub gnd_x: f64,
    /// Centre of the shell stakes from x = 0, and the front and rear
    /// pair.
    pub tab_x: f64,
    pub tabs: [ShellTab; 2],
    /// How far the stakes reach below the seating plane.
    pub stake_length: f64,
    /// Locating pegs: centre from x = 0, y, peg diameter and the
    /// non-plated hole's.
    pub peg_x: f64,
    pub peg_y: f64,
    pub peg_diameter: f64,
    pub peg_hole: f64,
}

impl UsbCForm {
    /// The GCT USB4105 (top mount, USB 2.0) from its drawing, revision B4
    /// sheet 1: the built-in row's values and the family's custom entry.
    /// The drawing dimensions from the board edge are moved to the
    /// outline centre (the body runs 7.35 mm in from the edge, so y is the
    /// drawing's distance less 3.675): lands 7.355, stakes 2.60 and 6.78,
    /// pegs 6.28 from the edge.
    pub fn typical() -> Self {
        Self {
            width: 8.94,
            depth: 7.35,
            height: 3.31,
            opening: [8.34, 2.56],
            opening_z: 1.73,
            shell_thickness: 0.3,
            pitch: 0.5,
            land_y: 3.68,
            land_length: 1.15,
            signal_land_width: 0.3,
            power_land_width: 0.6,
            vbus_x: 2.4,
            gnd_x: 3.2,
            tab_x: 4.32,
            tabs: [
                ShellTab {
                    y: -1.075,
                    slot: [0.6, 1.4],
                    land: [1.0, 1.8],
                },
                ShellTab {
                    y: 3.105,
                    slot: [0.6, 1.7],
                    land: [1.0, 2.1],
                },
            ],
            stake_length: 0.95,
            peg_x: 2.89,
            peg_y: 2.605,
            peg_diameter: 0.5,
            peg_hole: 0.65,
        }
    }

    /// Each contact land's centre x and width, in lead order.
    pub fn contact_lands(&self) -> Vec<(f64, f64)> {
        let signal = (0..8).map(|k| ((f64::from(k) - 3.5) * self.pitch, self.signal_land_width));
        let mut out = vec![
            (-self.gnd_x, self.power_land_width),
            (-self.vbus_x, self.power_land_width),
        ];
        out.extend(signal);
        out.push((self.vbus_x, self.power_land_width));
        out.push((self.gnd_x, self.power_land_width));
        out
    }

    /// The four stake centres: front −x, front +x, rear −x, rear +x.
    pub fn stake_centres(&self) -> [[f64; 2]; 4] {
        let [front, rear] = self.tabs;
        [
            [-self.tab_x, front.y],
            [self.tab_x, front.y],
            [-self.tab_x, rear.y],
            [self.tab_x, rear.y],
        ]
    }

    /// The lands landing `terminals` (one per [`DESIGNATIONS`] entry):
    /// twelve rectangular contact lands, four oblong plated lands with a
    /// milled slot along y for the shell, and the two non-plated peg
    /// holes. Pad IDs count from 1. `None` when the count differs or the
    /// values are out of range.
    pub fn lands(&self, terminals: &[PackageTerminalId]) -> Option<Vec<Pad>> {
        if terminals.len() != DESIGNATIONS.len() || !self.range_problems().is_empty() {
            return None;
        }
        let mut pads = Vec::new();
        for ((x, width), (terminal, number)) in self
            .contact_lands()
            .into_iter()
            .zip(terminals.iter().zip(DESIGNATIONS))
        {
            let id = PadId(u32::try_from(pads.len() + 1).ok()?);
            pads.push(Pad::smd_rect(
                id,
                *terminal,
                number,
                [x, self.land_y],
                [width, self.land_length],
            ));
        }
        let shell = *terminals.last()?;
        let tabs = [self.tabs[0], self.tabs[0], self.tabs[1], self.tabs[1]];
        for (centre, tab) in self.stake_centres().into_iter().zip(tabs) {
            let id = PadId(u32::try_from(pads.len() + 1).ok()?);
            let mut pad = Pad::smd_rect(id, shell, "S", centre, tab.land);
            pad.kind = PadKind::PlatedThrough;
            pad.shape = PadShape::Oblong { size: tab.land };
            pad.hole = Some(Hole {
                shape: HoleShape::Slot {
                    width: tab.slot[0],
                    length: tab.slot[1],
                    angle_deg: 90.0,
                },
                plated: true,
                offset: [0.0, 0.0],
            });
            pads.push(pad);
        }
        for x in [-self.peg_x, self.peg_x] {
            let id = PadId(u32::try_from(pads.len() + 1).ok()?);
            let mut pad = Pad::smd_rect(id, shell, "", [x, self.peg_y], [self.peg_hole; 2]);
            pad.kind = PadKind::NonPlatedHole;
            pad.terminal = None;
            pad.shape = PadShape::Circle {
                diameter: self.peg_hole,
            };
            // No mask growth: the drawing leaves 0.175 mm of board between
            // the hole and the GND land beside it, too little for a mask
            // web after growing both openings.
            pad.mask = MaskRule::Expansion(0.0);
            pad.hole = Some(Hole {
                shape: HoleShape::Round {
                    diameter: self.peg_hole,
                },
                plated: false,
                offset: [0.0, 0.0],
            });
            pads.push(pad);
        }
        Some(pads)
    }
}

impl FamilyForm for UsbCForm {
    fn label(&self) -> &'static str {
        "USB-C receptacle"
    }
    fn lengths(&self) -> Vec<f64> {
        let mut v = vec![
            self.width,
            self.depth,
            self.height,
            self.opening[0],
            self.opening[1],
            self.opening_z,
            self.shell_thickness,
            self.pitch,
            self.land_y,
            self.land_length,
            self.signal_land_width,
            self.power_land_width,
            self.vbus_x,
            self.gnd_x,
            self.tab_x,
            self.stake_length,
            self.peg_x,
            self.peg_y,
            self.peg_diameter,
            self.peg_hole,
        ];
        for t in &self.tabs {
            v.push(t.y);
            v.extend(t.slot);
            v.extend(t.land);
        }
        v
    }
    fn envelope(&self) -> Option<[f64; 3]> {
        Some([self.width, self.depth, self.height])
    }
    /// Twelve contact lands and the shell.
    fn lead_count(&self) -> u64 {
        DESIGNATIONS.len() as u64
    }
    /// Each contact land's centre, then the rear −x stake for the shell.
    fn lead_points(&self) -> Vec<Point3> {
        let mut out: Vec<Point3> = self
            .contact_lands()
            .into_iter()
            .map(|(x, _)| [x, self.land_y, 0.0])
            .collect();
        out.push([-self.tab_x, self.tabs[1].y, 0.0]);
        out
    }
    /// `W`, `D`, `A` (outline and seated height), `e` (contact pitch),
    /// `Lp` (contact land length).
    fn values(&self) -> Vec<PackageValue> {
        let length = |name: &str, v: f64| PackageValue {
            name: ParamName(format!("{PACKAGE_PREFIX}{name}")),
            value: ParamValue::Length(v),
        };
        vec![
            length("W", self.width),
            length("D", self.depth),
            length("A", self.height),
            length("e", self.pitch),
            length("Lp", self.land_length),
        ]
    }
    fn range_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let in_range =
            |v: f64| v.is_finite() && (FORM_LENGTH_MIN_MM..=FORM_LENGTH_LIMIT_MM).contains(&v);
        let finite = |v: f64| v.is_finite() && v.abs() <= FORM_LENGTH_LIMIT_MM;
        if !self.lengths().into_iter().all(finite) {
            out.push("Every length must be finite.".to_owned());
            return out;
        }
        let mut positive = |name: &str, v: f64| {
            if !in_range(v) {
                out.push(format!(
                    "{name} must be {FORM_LENGTH_MIN_MM}–{FORM_LENGTH_LIMIT_MM} mm (is {v})."
                ));
            }
        };
        positive("Receptacle width", self.width);
        positive("Receptacle depth", self.depth);
        positive("Height", self.height);
        positive("Opening width", self.opening[0]);
        positive("Opening height", self.opening[1]);
        positive("Opening centre height", self.opening_z);
        positive("Shell thickness", self.shell_thickness);
        positive("Pitch", self.pitch);
        positive("Land length", self.land_length);
        positive("Signal land width", self.signal_land_width);
        positive("Power land width", self.power_land_width);
        positive("VBUS land offset", self.vbus_x);
        positive("GND land offset", self.gnd_x);
        positive("Stake offset", self.tab_x);
        positive("Stake length", self.stake_length);
        positive("Peg offset", self.peg_x);
        positive("Peg diameter", self.peg_diameter);
        positive("Peg hole", self.peg_hole);
        for t in &self.tabs {
            positive("Stake slot width", t.slot[0]);
            positive("Stake slot length", t.slot[1]);
            positive("Stake land width", t.land[0]);
            positive("Stake land length", t.land[1]);
        }
        let mut rule = |ok: bool, message: &str| {
            if !ok {
                out.push(message.to_owned());
            }
        };
        let t = self.shell_thickness;
        let [ow, oh] = self.opening;
        rule(
            ow + 2.0 * t <= self.width + FIT_EPS_MM && oh > 2.0 * FIT_EPS_MM && ow > oh,
            "The opening is wider than high, and it and its shell must fit within the width.",
        );
        rule(
            self.opening_z - oh / 2.0 - t >= -FIT_EPS_MM
                && self.opening_z + oh / 2.0 + t <= self.height + FIT_EPS_MM,
            "The shell round the opening must lie between the seating plane and the height.",
        );
        rule(
            self.signal_land_width < self.pitch,
            "Signal lands must not touch (width < pitch).",
        );
        rule(
            3.5 * self.pitch + self.signal_land_width / 2.0
                < self.vbus_x - self.power_land_width / 2.0
                && self.vbus_x + self.power_land_width / 2.0
                    < self.gnd_x - self.power_land_width / 2.0,
            "Contact lands must not touch: signals inside VBUS, VBUS inside GND.",
        );
        rule(
            self.gnd_x + self.power_land_width / 2.0
                < self.tab_x - self.tabs[0].land[0].max(self.tabs[1].land[0]) / 2.0,
            "The contact lands must lie inside the shell stakes.",
        );
        rule(
            self.tabs
                .iter()
                .all(|t| t.slot[0] < t.land[0] && t.slot[1] < t.land[1] && t.slot[0] <= t.slot[1]),
            "Each stake slot must lie within its land and run along y.",
        );
        rule(
            self.tab_x <= self.width / 2.0,
            "The shell stakes must lie within the outline width.",
        );
        let [front, rear] = self.tabs;
        rule(
            front.y + front.land[1] / 2.0 < rear.y - rear.land[1] / 2.0,
            "The front stakes must lie in front of (−y) the rear ones, clear of them.",
        );
        rule(
            self.peg_diameter < self.peg_hole && self.peg_x + self.peg_hole / 2.0 < self.tab_x,
            "Each peg must be narrower than its hole and lie inside the stakes.",
        );
        let half = self.depth / 2.0;
        rule(
            [front.y, rear.y, self.peg_y]
                .iter()
                .all(|y| (-half..=half).contains(y))
                && self.land_y > -half,
            "Stakes and pegs must lie within the depth, the contact lands behind the front.",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typical_values_are_in_range_and_land_thirteen_terminals() {
        let f = UsbCForm::typical();
        assert_eq!(f.range_problems(), Vec::<String>::new());
        assert_eq!(f.lead_points().len(), 13);
        let terminals: Vec<PackageTerminalId> = (1..=13).map(PackageTerminalId).collect();
        let pads = f.lands(&terminals).unwrap_or_default();
        assert_eq!(pads.len(), 18, "12 contacts, 4 stakes, 2 peg holes");
        assert_eq!(pads.iter().filter(|p| p.number == "S").count(), 4);
        assert!(
            pads.iter()
                .filter(|p| p.kind == PadKind::NonPlatedHole)
                .all(|p| p.terminal.is_none())
        );
        // The drawing's pitch: B8 at −1.75, GND at ±3.2, VBUS at ±2.4.
        let x = |n: &str| pads.iter().find(|p| p.number == n).map(|p| p.position[0]);
        assert_eq!(x("B8"), Some(-1.75));
        assert_eq!(x("A1B12"), Some(-3.2));
        assert_eq!(x("B4A9"), Some(2.4));
    }

    #[test]
    fn every_land_carries_exactly_one_electrical_terminal() {
        let mut seen = [0usize; 13];
        for (_, _, leads) in TERMINALS {
            for k in leads {
                if let Some(n) = seen.get_mut(*k) {
                    *n += 1;
                }
            }
        }
        assert_eq!(seen, [1; 13]);
    }

    #[test]
    fn touching_lands_are_refused() {
        let mut f = UsbCForm::typical();
        f.vbus_x = 2.0;
        assert!(!f.range_problems().is_empty());
        let mut f = UsbCForm::typical();
        f.tabs[0].slot = [1.2, 1.4];
        assert!(!f.range_problems().is_empty());
    }
}
