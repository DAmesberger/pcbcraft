//! A footprint from a datasheet's pad table (`library.footprint_from_pads`):
//! rows of lands, slots and holes become an explicit footprint with the
//! standard drawings (`templates::footprint`). Pure; the command stores the
//! result. Pads land terminals only as the request says: a row's `terminal`,
//! or `assign_by_number` through the same rule as `library.assign_terminals`.
use super::footprint_edit::{self, AssignRule};
use super::package_fit;
use super::templates::footprint::add_standard_graphics;
use super::{
    CornerMask, FootprintDefinition, Hole, HoleShape, MAX_PADS, MaskRule, ModelPlacement,
    PackageDefinition, Pad, PadKind, PadShape, PadSide, PasteRule,
};
use crate::diagnostics::{Error, Finding, Result};
use crate::geometry::Point;
use crate::ids::{DefinitionId, GraphicId, MAX_LABEL, PackageTerminalId, PadId};
use crate::rules::ProjectRules;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Corner ratio of a `rounded` land (of the short side).
const ROUNDED_RATIO: f64 = 0.25;

/// A row's copper outline.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RowShape {
    #[default]
    Rect,
    /// Corners of a quarter of the short side.
    Rounded,
    /// Semicircular ends on the long axis.
    Oblong,
    /// `size` gives the diameter twice.
    Circle,
}

/// One land of the table.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadRow {
    /// The pad's number as the datasheet prints it.
    pub number: String,
    pub at: Point,
    /// Copper size `[along x, along y]` before `rotation_deg`.
    pub size: Point,
    #[serde(default)]
    pub shape: RowShape,
    #[serde(default)]
    pub rotation_deg: f64,
    #[serde(default)]
    pub side: PadSide,
    /// A plated round drill makes the land plated through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drill: Option<f64>,
    /// The package terminal's designation it lands. Several rows may land
    /// one terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// Copper that lands no terminal on purpose (a mounting tab). Surface
    /// lands only.
    #[serde(default)]
    pub mechanical: bool,
}

/// One slot of the table: an oblong land around a milled slot.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SlotRow {
    pub number: String,
    pub at: Point,
    /// The finished slot `[along x, along y]`; its round ends are on the
    /// long axis.
    pub slot: Point,
    /// Oblong copper `[along x, along y]`. Required when plated; an unplated
    /// slot has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land: Option<Point>,
    #[serde(default = "plated")]
    pub plated: bool,
    /// As for [`PadRow::terminal`]; an unplated slot lands nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

fn plated() -> bool {
    true
}

/// One unplated round hole of the table (a locating peg).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HoleRow {
    #[serde(default)]
    pub number: String,
    pub at: Point,
    pub diameter: f64,
}

/// The `library.footprint_from_pads` request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FootprintFromPads {
    /// The footprint draft.
    pub draft: DefinitionId,
    #[serde(default)]
    pub pads: Vec<PadRow>,
    #[serde(default)]
    pub slots: Vec<SlotRow>,
    #[serde(default)]
    pub holes: Vec<HoleRow>,
    /// Pads without a `terminal` take the one whose designation equals
    /// their number, as `library.assign_terminals` by designation does.
    #[serde(default)]
    pub assign_by_number: bool,
    /// Replace the draft's pads and drawings instead of refusing them.
    #[serde(default)]
    pub replace: bool,
}

/// What `library.footprint_from_pads` made.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PadTableReport {
    /// In order: pad rows, slots, then holes.
    pub pads: Vec<PadId>,
    pub graphics: Vec<GraphicId>,
    /// Numbers of the pads that must land a terminal and do not yet.
    pub unassigned: Vec<String>,
    /// Package terminals no pad lands yet.
    pub unlanded: Vec<String>,
    /// Where the envelope model moved, over these pads; `None` if it stayed.
    pub envelope_moved_to: Option<[f64; 3]>,
    /// Advisory lint of the result.
    pub findings: Vec<Finding>,
}

/// [`build`]'s result: the footprint, the envelope's new place and the report.
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltPads {
    pub footprint: FootprintDefinition,
    pub placement: Option<ModelPlacement>,
    pub report: PadTableReport,
}

fn invalid(message: String) -> Error {
    Error::Invalid(format!("footprint.pad_table: {message}"))
}

/// The terminal a row names; a designation must name exactly one.
fn named_terminal(
    package: &PackageDefinition,
    number: &str,
    terminal: Option<&str>,
) -> Result<Option<PackageTerminalId>> {
    let Some(designation) = terminal else {
        return Ok(None);
    };
    let mut found = package
        .terminals
        .iter()
        .filter(|t| t.designation == designation);
    match (found.next(), found.next()) {
        (Some(t), None) => Ok(Some(t.id)),
        (None, _) => Err(invalid(format!(
            "pad {number} names terminal ‘{designation}’, which the package does not have"
        ))),
        (Some(_), Some(_)) => Err(invalid(format!(
            "pad {number} names terminal ‘{designation}’, which is on several package terminals"
        ))),
    }
}

fn row_pad(id: PadId, row: &PadRow, package: &PackageDefinition) -> Result<Pad> {
    let [x, y] = row.size;
    let shape = match row.shape {
        RowShape::Rect => PadShape::Rect { size: row.size },
        RowShape::Rounded => PadShape::RoundRect {
            size: row.size,
            radius_ratio: ROUNDED_RATIO,
            max_radius: None,
            corners: CornerMask::ALL,
        },
        RowShape::Oblong => PadShape::Oblong { size: row.size },
        RowShape::Circle if (x - y).abs() <= 1e-9 => PadShape::Circle { diameter: x },
        RowShape::Circle => {
            return Err(invalid(format!(
                "pad {} is a circle, so its size needs two equal values",
                row.number
            )));
        }
    };
    let (kind, hole) = match (row.drill, row.mechanical) {
        (Some(_), true) => {
            return Err(invalid(format!(
                "pad {} is mechanical and drilled; a plated hole lands a terminal, an unplated one is a hole row",
                row.number
            )));
        }
        (Some(diameter), false) => (
            PadKind::PlatedThrough,
            Some(Hole {
                shape: HoleShape::Round { diameter },
                plated: true,
                offset: [0.0, 0.0],
            }),
        ),
        (None, true) => (PadKind::Mechanical, None),
        (None, false) => (PadKind::Smd, None),
    };
    let mut pad = Pad {
        id,
        kind,
        terminal: named_terminal(package, &row.number, row.terminal.as_deref())?,
        number: row.number.clone(),
        position: row.at,
        rotation_deg: row.rotation_deg,
        side: row.side,
        shape,
        hole,
        mask: MaskRule::Inherit,
        paste: PasteRule::Inherit,
        vias: Vec::new(),
    };
    pad.fold_rotation()?;
    Ok(pad)
}

/// A slot's hole: its round ends on the long axis.
fn slot_hole(slot: Point, plated: bool) -> Hole {
    let [x, y] = slot;
    Hole {
        shape: HoleShape::Slot {
            width: x.min(y),
            length: x.max(y),
            angle_deg: if y > x { 90.0 } else { 0.0 },
        },
        plated,
        offset: [0.0, 0.0],
    }
}

fn slot_pad(id: PadId, row: &SlotRow, package: &PackageDefinition) -> Result<Pad> {
    let (kind, land) = match (row.plated, row.land) {
        (true, Some(land)) => (PadKind::PlatedThrough, land),
        (true, None) => {
            return Err(invalid(format!(
                "plated slot {} needs its land size",
                row.number
            )));
        }
        (false, None) => (PadKind::NonPlatedHole, row.slot),
        (false, Some(_)) => {
            return Err(invalid(format!(
                "unplated slot {} has no copper, so no land",
                row.number
            )));
        }
    };
    if !row.plated && row.terminal.is_some() {
        return Err(invalid(format!(
            "unplated slot {} cannot land a terminal",
            row.number
        )));
    }
    Ok(Pad {
        id,
        kind,
        terminal: named_terminal(package, &row.number, row.terminal.as_deref())?,
        number: row.number.clone(),
        position: row.at,
        rotation_deg: 0.0,
        side: PadSide::Top,
        shape: PadShape::Oblong { size: land },
        hole: Some(slot_hole(row.slot, row.plated)),
        mask: MaskRule::Inherit,
        paste: PasteRule::Inherit,
        vias: Vec::new(),
    })
}

fn hole_pad(id: PadId, row: &HoleRow) -> Pad {
    Pad {
        id,
        kind: PadKind::NonPlatedHole,
        terminal: None,
        number: row.number.clone(),
        position: row.at,
        rotation_deg: 0.0,
        side: PadSide::Top,
        shape: PadShape::Circle {
            diameter: row.diameter,
        },
        hole: Some(Hole {
            shape: HoleShape::Round {
                diameter: row.diameter,
            },
            plated: false,
            offset: [0.0, 0.0],
        }),
        mask: MaskRule::Inherit,
        paste: PasteRule::Inherit,
        vias: Vec::new(),
    }
}

/// The footprint a request makes of the draft `footprint` (lineage `id`),
/// landing `package`. Any problem is an error and nothing else. A pad left
/// without a terminal is reported, not refused: `library.assign_terminals`
/// can finish it and publish still requires it.
pub fn build(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
    request: &FootprintFromPads,
    id: DefinitionId,
    rules: &ProjectRules,
) -> Result<BuiltPads> {
    let count = request.pads.len() + request.slots.len() + request.holes.len();
    if count == 0 {
        return Err(invalid("the table has no rows".into()));
    }
    if count > MAX_PADS {
        return Err(Error::Limit(format!(
            "footprint.pad_table: a footprint has at most {MAX_PADS} pads"
        )));
    }
    let authored = footprint
        .graphics
        .iter()
        .any(|g| matches!(g.item, super::graphics::Item::Shape { .. }));
    if !request.replace && (!footprint.pads.is_empty() || footprint.source.is_some() || authored) {
        return Err(Error::Conflict(
            "footprint.template_not_empty: the footprint already has content; pass replace to rebuild it from the table".into(),
        ));
    }
    let numbers = request
        .pads
        .iter()
        .map(|r| &r.number)
        .chain(request.slots.iter().map(|r| &r.number))
        .chain(request.holes.iter().map(|r| &r.number));
    if numbers.into_iter().any(|n| n.len() > MAX_LABEL) {
        return Err(Error::Limit(format!(
            "footprint.pad_table: a pad number has at most {MAX_LABEL} bytes"
        )));
    }
    if request.pads.iter().any(|r| r.number.trim().is_empty())
        || request.slots.iter().any(|r| r.number.trim().is_empty())
    {
        return Err(invalid("every pad and slot row needs a number".into()));
    }

    let mut next = super::templates::footprint_family::cleared(footprint);
    next.graphics.clear();
    next.next_local = next.next_local.max(1);
    let first = next.next_local;
    let last = u32::try_from(count)
        .ok()
        .and_then(|n| first.checked_add(n))
        .ok_or_else(|| Error::Limit("footprint local identities exhausted".into()))?;
    next.next_local = last;
    let mut ids = (first..last).map(PadId);
    let mut fresh = || {
        ids.next()
            .ok_or_else(|| Error::Limit("footprint local identities exhausted".into()))
    };
    for row in &request.pads {
        next.pads.push(row_pad(fresh()?, row, package)?);
    }
    for row in &request.slots {
        next.pads.push(slot_pad(fresh()?, row, package)?);
    }
    for row in &request.holes {
        next.pads.push(hole_pad(fresh()?, row));
    }
    let pads: Vec<PadId> = next.pads.iter().map(|p| p.id).collect();
    if !next.geometry_finite() {
        return Err(invalid(
            "every position and size must be finite and within ±10000 mm".into(),
        ));
    }
    if request.assign_by_number {
        next =
            footprint_edit::assign_terminals(&next, package, &AssignRule::ByPackageDesignation)?.0;
    }

    // The body first: the courtyard is drawn around it where it will sit.
    let placement = package_fit::envelope_default(package, &next)?;
    let mut placed = package.clone();
    if let Some(p) = placement {
        placed.placement = p;
    }
    let keepouts = next.keepouts.clone();
    let graphics = add_standard_graphics(&mut next, &placed)?;
    if next.keepouts.is_empty() {
        next.keepouts = keepouts;
    }

    let mut problems = Vec::new();
    next.local_findings(id, &mut problems);
    if let Some(problem) = problems
        .iter()
        .find(|f| f.code != "footprint.terminal_required")
    {
        return Err(Error::Invalid(format!(
            "{}: {}",
            problem.code, problem.message
        )));
    }

    let unassigned = next
        .pads
        .iter()
        .filter(|p| p.terminal.is_none() && matches!(p.kind, PadKind::Smd | PadKind::PlatedThrough))
        .map(|p| p.number.clone())
        .collect();
    let unlanded = package
        .terminals
        .iter()
        .filter(|t| !next.pads.iter().any(|p| p.terminal == Some(t.id)))
        .map(|t| t.designation.clone())
        .collect();
    let mut findings = next.lint_with(id, rules);
    findings.extend(package_fit::placement_findings(&placed, &next, id));
    let report = PadTableReport {
        pads,
        graphics,
        unassigned,
        unlanded,
        envelope_moved_to: placement.map(|p| p.translation),
        findings,
    };
    Ok(BuiltPads {
        footprint: next,
        placement,
        report,
    })
}
