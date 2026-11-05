//! Assembly tables: pick-and-place (one CSV per populated side) and the BOM
//! (one row per purchased line grouped as `project.bom` groups them, and
//! one row per instance).
//!
//! # Pick-and-place convention
//!
//! The default ([`PlacementConvention::default`]) is Pcbcraft's own:
//!
//! - **Point.** Each row gives the placement origin, which is the
//!   footprint datum (`Placement::position`), not the body or pad centre.
//! - **Origin and units.** Millimetres from the board origin with y up, the
//!   coordinates the Gerber files use. [`PlacementOrigin::OutlineLowerLeft`]
//!   measures from the lower-left corner of the board outline's bounding
//!   box; [`PlacementOrigin::Point`] from any board point.
//! - **Rotation.** Degrees counter-clockwise as seen from the top, in
//!   [0, 360) rounded to 0.001°. 0° is the footprint as drawn in the
//!   library.
//! - **Bottom side.** By default a bottom part is written with its board
//!   angle ([`BottomRotation::ViewedFromTop`]), seen through the board.
//!   [`BottomRotation::ViewedFromBottom`] writes the angle seen from below
//!   with the board flipped about its y axis, `(180 − rotation) mod 360`.
//!   The bottom placement mirrors the footprint and the flip mirrors it
//!   back, so the part shows unmirrored at that angle.
//!   [`PlacementConvention::mirror_bottom_x`] then also negates bottom x.
//!
//! A fabricator that expects another convention is configured with these
//! options: its origin, `ViewedFromBottom` with `mirror_bottom_x` for
//! bottom-view machines, and [`RotationOffset`]s. Each offset adds degrees
//! to every part whose package name matches a `*`/`?` pattern; the first
//! matching offset applies, after the bottom-side rule. This is for a house
//! whose zero orientation differs from the library's.
//!
//! # CSV
//!
//! Each file is UTF-8 without a byte-order mark, comma-separated, one
//! header row and LF line ends. A field that holds a comma, a double quote,
//! CR or LF is quoted, with inner quotes doubled (RFC 4180). Lengths have
//! 4 decimals.
//!
//! # Fitted parts
//!
//! The design model has no do-not-populate flag or assembly variants yet.
//! Every instance is fitted, and `bom-parts.csv` says so in its Fitted
//! column. A mechanical part placed from a part file carries its own
//! `bom` and `pick_and_place` flags (docs/mechanical-parts.md M1.6): it is
//! left out of the BOM files or the pick-and-place rows when they are off.
use super::{FabFile, FabricationOptions, FileGroup};
use crate::completeness::{self, BomLine, PurchasingState};
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use crate::ids::Id;
use crate::instances;
use crate::model::{Layer, Project};
use crate::rules::glob_matches;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Most rotation offsets one convention carries.
pub const MAX_ROTATION_OFFSETS: usize = 256;
/// Bytes of one rotation-offset pattern.
pub const MAX_PATTERN: usize = 128;

/// Where pick-and-place coordinates are measured from.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PlacementOrigin {
    /// The board origin of the design (Gerber coordinates are the same).
    #[default]
    BoardOrigin,
    /// The lower-left corner of the board outline's bounding box.
    OutlineLowerLeft,
    /// An explicit point in board millimetres.
    Point { at: Point },
}

/// How bottom-side rotations are written.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BottomRotation {
    /// Counter-clockwise as seen from the top, the board's own angle (the
    /// placement's `rotation_deg`).
    #[default]
    ViewedFromTop,
    /// Counter-clockwise as seen from below (the board flipped about its y
    /// axis): `(180 - rotation) mod 360`, x mirrored when
    /// [`PlacementConvention::mirror_bottom_x`] is set.
    ViewedFromBottom,
}

/// A rotation added to every part whose package label matches `package`
/// (`*` and `?` wildcards), for a fabricator whose zero orientation
/// differs from the library's.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RotationOffset {
    pub package: String,
    pub degrees: f64,
}

/// The pick-and-place convention. The default is Pcbcraft's: coordinates
/// of the placement origin (the footprint datum) from the board origin in
/// mm, rotation counter-clockwise in degrees as seen from the top with the
/// footprint as drawn in the library at 0°, bottom parts at their board
/// angle.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlacementConvention {
    #[serde(default)]
    pub origin: PlacementOrigin,
    #[serde(default)]
    pub bottom: BottomRotation,
    /// Write bottom-side x as seen from below (negated).
    #[serde(default)]
    pub mirror_bottom_x: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotation_offsets: Vec<RotationOffset>,
}

impl PlacementConvention {
    /// Finite origin and offsets, bounded patterns.
    pub fn validate(&self) -> Result<()> {
        let origin_ok = match self.origin {
            PlacementOrigin::Point { at } => crate::geometry::point_valid(at),
            _ => true,
        };
        let offsets_ok = self.rotation_offsets.len() <= MAX_ROTATION_OFFSETS
            && self.rotation_offsets.iter().all(|o| {
                o.degrees.is_finite()
                    && o.degrees.abs() <= 360.0
                    && !o.package.is_empty()
                    && o.package.len() <= MAX_PATTERN
            });
        if origin_ok && offsets_ok {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "fabrication.placement_convention: the origin must be a finite board point; at most {MAX_ROTATION_OFFSETS} rotation offsets, each a pattern of 1–{MAX_PATTERN} bytes and at most 360°"
            )))
        }
    }
}

/// One pick-and-place row.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PlacementRow {
    pub instance: Id,
    pub designator: String,
    pub value: String,
    pub package: String,
    pub x: f64,
    pub y: f64,
    pub rotation_deg: f64,
    pub side: Layer,
}

/// The BOM line of each instance.
fn line_index(report: &completeness::BomReport) -> BTreeMap<Id, &BomLine> {
    let mut index = BTreeMap::new();
    for line in &report.lines {
        for id in &line.instances {
            index.insert(*id, line);
        }
    }
    index
}

/// The Value of an instance in the BOM and pick-and-place files: its value
/// text ([`instances::value_text`], what the schematic shows, "100 nF"),
/// else the chosen Part's MPN, else the component name for a component of
/// more than two terminals (an IC, whose name is its identity), else empty.
/// A two-terminal part's name is a package description ("Chip capacitor
/// 0603 (1608 metric)") the Package column already carries, so a passive
/// with no value set and no Part leaves Value empty; its Status reads
/// "no part". The schematic falls back the same way.
fn instance_value(
    project: &Project,
    instance: &crate::model::ComponentInstance,
    line: Option<&BomLine>,
) -> Result<String> {
    if let Some(value) = instances::value_text(&project.library, instance)?
        .or_else(|| line.and_then(|l| l.mpn.clone()))
    {
        return Ok(value);
    }
    Ok(project
        .library
        .component(instance.component)
        .filter(|c| c.definition.terminals.len() > 2)
        .map(|c| c.meta.name.clone())
        .unwrap_or_default())
}

/// The package name of an instance (the package definition's name, else
/// the footprint's), or `None` without a physical choice.
fn package_name(project: &Project, instance: &crate::model::ComponentInstance) -> Option<String> {
    let resolved = instances::resolve(&project.library, instance).ok()?;
    let physical = resolved.physical?;
    let name = physical.package.meta.name.trim();
    Some(if name.is_empty() {
        physical.footprint.meta.name.trim().to_string()
    } else {
        name.to_string()
    })
}

/// `R10` after `R9`: the letter prefix, then the number, then the text (as
/// `project.bom` orders designators).
fn designator_key(designator: &str) -> (String, u64, String) {
    let digits = designator
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map_or(designator.len(), |(i, _)| i);
    let (prefix, number) = designator.split_at(digits);
    (
        prefix.to_owned(),
        number.parse().unwrap_or(u64::MAX),
        designator.to_owned(),
    )
}

/// An angle in [0, 360), rounded to 0.001°.
fn normal_angle(deg: f64) -> f64 {
    let r = (deg.rem_euclid(360.0) * 1000.0).round() / 1000.0;
    if r >= 360.0 { r - 360.0 } else { r }
}

/// Every placed part under `convention`, in designator order.
pub fn rows(project: &Project, convention: &PlacementConvention) -> Result<Vec<PlacementRow>> {
    convention.validate()?;
    let origin = match convention.origin {
        PlacementOrigin::BoardOrigin => [0.0, 0.0],
        PlacementOrigin::OutlineLowerLeft => project.board().bounds().min,
        PlacementOrigin::Point { at } => at,
    };
    let bom = completeness::bom(project)?;
    let lines = line_index(&bom);
    let mut out = Vec::with_capacity(project.placements.len());
    for placement in &project.placements {
        let Some(instance) = project.instance(placement.instance) else {
            continue;
        };
        if !crate::library::mechanical::in_pick_and_place(&project.library, instance.component) {
            continue;
        }
        let Some(package) = package_name(project, instance) else {
            continue;
        };
        let value = instance_value(project, instance, lines.get(&instance.id).copied())?;
        let mut x = placement.position[0] - origin[0];
        let y = placement.position[1] - origin[1];
        let mut rotation = placement.rotation_deg;
        if placement.layer == Layer::Bottom {
            if convention.bottom == BottomRotation::ViewedFromBottom {
                rotation = 180.0 - rotation;
            }
            if convention.mirror_bottom_x {
                x = -x;
            }
        }
        if let Some(offset) = convention
            .rotation_offsets
            .iter()
            .find(|o| glob_matches(&o.package, &package))
        {
            rotation += offset.degrees;
        }
        out.push(PlacementRow {
            instance: instance.id,
            designator: instance.designator.clone(),
            value,
            package,
            // Adding 0.0 turns −0 into +0, so the CSV never prints "-0.0000".
            x: x + 0.0,
            y: y + 0.0,
            rotation_deg: normal_angle(rotation),
            side: placement.layer,
        });
    }
    out.sort_by(|a, b| {
        designator_key(&a.designator)
            .cmp(&designator_key(&b.designator))
            .then_with(|| a.instance.cmp(&b.instance))
    });
    Ok(out)
}

/// One CSV field, quoted when it holds a comma, a quote, CR or LF.
pub fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// One CSV record with an LF line end.
fn record(out: &mut String, fields: &[&str]) {
    let quoted: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
    out.push_str(&quoted.join(","));
    out.push('\n');
}

fn mm(value: f64) -> String {
    let text = format!("{value:.4}");
    if text == "-0.0000" {
        "0.0000".into()
    } else {
        text
    }
}

fn angle(value: f64) -> String {
    let mut text = format!("{value:.3}");
    if text == "-0.000" {
        text = "0.000".into();
    }
    text
}

fn side_name(side: Layer) -> &'static str {
    match side {
        Layer::Top => "Top",
        Layer::Bottom => "Bottom",
    }
}

/// The pick-and-place CSV of one side's rows.
pub fn placement_csv(rows: &[PlacementRow]) -> String {
    let mut out = String::new();
    record(
        &mut out,
        &[
            "Designator",
            "Value",
            "Package",
            "X (mm)",
            "Y (mm)",
            "Rotation (deg)",
            "Side",
        ],
    );
    for row in rows {
        record(
            &mut out,
            &[
                &row.designator,
                &row.value,
                &row.package,
                &mm(row.x),
                &mm(row.y),
                &angle(row.rotation_deg),
                side_name(row.side),
            ],
        );
    }
    out
}

fn status(state: PurchasingState) -> &'static str {
    match state {
        PurchasingState::Resolved => "resolved",
        PurchasingState::Unresolved => "unresolved",
        PurchasingState::NoPart => "no part",
    }
}

/// The package name of a BOM line's footprint.
fn line_package(project: &Project, line: &BomLine) -> String {
    line.instances
        .first()
        .and_then(|id| project.instance(*id))
        .and_then(|i| package_name(project, i))
        .unwrap_or_default()
}

/// The grouped BOM: one row per `project.bom` line.
pub fn bom_csv(project: &Project) -> Result<String> {
    let report = completeness::bom(project)?;
    let mut out = String::new();
    record(
        &mut out,
        &[
            "Line",
            "Quantity",
            "Designators",
            "Value",
            "Package",
            "Manufacturer",
            "MPN",
            "Status",
        ],
    );
    for (k, line) in report.lines.iter().enumerate() {
        let value = match line.instances.first().and_then(|id| project.instance(*id)) {
            Some(first) => instance_value(project, first, Some(line))?,
            None => String::new(),
        };
        record(
            &mut out,
            &[
                &(k + 1).to_string(),
                &line.quantity.to_string(),
                &line.designators.join(" "),
                &value,
                &line_package(project, line),
                line.manufacturer.as_deref().unwrap_or(""),
                line.mpn.as_deref().unwrap_or(""),
                status(line.state),
            ],
        );
    }
    Ok(out)
}

/// One row per instance, in designator order, every instance fitted.
pub fn bom_parts_csv(project: &Project) -> Result<String> {
    let report = completeness::bom(project)?;
    let lines = line_index(&report);
    let mut instances: Vec<&crate::model::ComponentInstance> = project
        .instances
        .iter()
        .filter(|i| crate::library::mechanical::in_bom(&project.library, i.component))
        .collect();
    instances.sort_by(|a, b| {
        designator_key(&a.designator)
            .cmp(&designator_key(&b.designator))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut out = String::new();
    record(
        &mut out,
        &[
            "Designator",
            "Value",
            "Package",
            "Manufacturer",
            "MPN",
            "Fitted",
        ],
    );
    for instance in instances {
        let line = lines.get(&instance.id).copied();
        let value = instance_value(project, instance, line)?;
        record(
            &mut out,
            &[
                &instance.designator,
                &value,
                &package_name(project, instance).unwrap_or_default(),
                line.and_then(|l| l.manufacturer.as_deref()).unwrap_or(""),
                line.and_then(|l| l.mpn.as_deref()).unwrap_or(""),
                "yes",
            ],
        );
    }
    Ok(out)
}

/// The pick-and-place and BOM files: `{stem}-pos-top.csv` and
/// `{stem}-pos-bottom.csv` (each only when that side has parts),
/// `{stem}-bom.csv` (grouped) and `{stem}-bom-parts.csv` (per instance).
pub fn files(project: &Project, options: &FabricationOptions, stem: &str) -> Result<Vec<FabFile>> {
    let rows = rows(project, &options.placement)?;
    let mut out = Vec::new();
    for (side, suffix, description) in [
        (Layer::Top, "pos-top", "Pick-and-place, top side"),
        (Layer::Bottom, "pos-bottom", "Pick-and-place, bottom side"),
    ] {
        let side_rows: Vec<PlacementRow> =
            rows.iter().filter(|r| r.side == side).cloned().collect();
        if side_rows.is_empty() {
            continue;
        }
        out.push(FabFile {
            name: format!("{stem}-{suffix}.csv"),
            group: FileGroup::Assembly,
            description: description.into(),
            bytes: placement_csv(&side_rows).into_bytes(),
        });
    }
    out.push(FabFile {
        name: format!("{stem}-bom.csv"),
        group: FileGroup::Assembly,
        description: "Bill of materials, grouped by purchased part".into(),
        bytes: bom_csv(project)?.into_bytes(),
    });
    out.push(FabFile {
        name: format!("{stem}-bom-parts.csv"),
        group: FileGroup::Assembly,
        description: "Bill of materials, one row per part".into(),
        bytes: bom_parts_csv(project)?.into_bytes(),
    });
    Ok(out)
}

#[cfg(test)]
mod tests;
