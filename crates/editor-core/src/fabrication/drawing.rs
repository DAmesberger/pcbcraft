//! What the fabrication and assembly drawings print, in board millimetres,
//! curves already flattened: `pcbcraft-io` lays it out as PDF pages and
//! interprets nothing.
//!
//! CONTRACT STUB: filled by the drawings task.
use super::FabricationOptions;
use super::drill::{DrillHole, DrillTable};
use crate::diagnostics::Result;
use crate::geometry::{Bounds, Point};
use crate::ids::Id;
use crate::model::{Layer, Project};
use serde::Serialize;

/// One row of the stackup table, top to bottom.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct StackupRow {
    /// "Top mask", "Top copper", "Core 1", …
    pub name: String,
    /// "copper", "core", "prepreg", "mask", "legend".
    pub kind: String,
    /// "FR-4 7628", "35 µm Cu", …
    pub material: String,
    pub thickness_mm: f64,
}

/// One placed part on an assembly drawing.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct AssemblyPart {
    pub instance: Id,
    pub designator: String,
    pub value: String,
    /// The placement origin.
    pub at: Point,
    pub rotation_deg: f64,
    /// Body outline loops (assembly-layer drawing, else courtyard, else
    /// the pads' box), closed.
    pub body: Vec<Vec<Point>>,
    /// Pad outlines on this side, closed.
    pub pads: Vec<Vec<Point>>,
    /// Where the polarity (pin 1) mark goes, when the footprint has one.
    pub polarity: Option<Point>,
}

/// One side of the assembly drawing.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct AssemblySide {
    pub side: Layer,
    pub parts: Vec<AssemblyPart>,
}

/// Everything both drawings print.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct DrawingData {
    pub title: String,
    /// The board outline's exact bounds.
    pub bounds: Bounds,
    /// The outer outline (counter-clockwise), closed, flattened.
    pub outline: Vec<Point>,
    /// Cutouts and non-plated slots, closed, flattened.
    pub cutouts: Vec<Vec<Point>>,
    pub holes: Vec<DrillHole>,
    pub drills: DrillTable,
    pub stackup: Vec<StackupRow>,
    pub thickness_mm: f64,
    pub copper_layers: usize,
    /// Surface finish, mask and legend colours as words.
    pub finish: String,
    pub mask: String,
    pub legend: String,
    /// The rule profile's name and provenance line.
    pub profile: String,
    /// Fabrication notes from the effective rules (minimum track and gap,
    /// drill, annular ring, mask web, …), one sentence each.
    pub notes: Vec<String>,
    pub sides: Vec<AssemblySide>,
}

/// The drawing data of `project`.
pub fn data(project: &Project, _options: &FabricationOptions) -> Result<DrawingData> {
    let holes = super::drill::holes(project);
    Ok(DrawingData {
        title: project.name.clone(),
        bounds: project.board().bounds(),
        outline: Vec::new(),
        cutouts: Vec::new(),
        drills: super::drill::table(&holes),
        holes,
        stackup: Vec::new(),
        thickness_mm: 0.0,
        copper_layers: project.stackup.layers().len(),
        finish: String::new(),
        mask: String::new(),
        legend: String::new(),
        profile: String::new(),
        notes: Vec::new(),
        sides: Vec::new(),
    })
}
