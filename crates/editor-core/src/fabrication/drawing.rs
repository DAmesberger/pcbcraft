//! What the fabrication and assembly drawings print, in board millimetres,
//! curves already flattened: `pcbcraft-io` lays it out as PDF pages and
//! interprets nothing.
use super::FabricationOptions;
use super::drill::{DrillHole, DrillTable};
use crate::diagnostics::Result;
use crate::geometry::{Bounds, Point};
use crate::ids::Id;
use crate::layout::{self, BoardDrawing, PlacedItem};
use crate::library::PAD_ARC_TOLERANCE_MM;
use crate::model::DielectricKind;
use crate::model::board::{CutoutKind, flatten};
use crate::model::{Layer, MaskColour, PadFinish, Project, SilkscreenColour};
use crate::rules::CopperObject;
use serde::Serialize;
use std::collections::BTreeMap;

/// Chord tolerance of the drawn outlines, mm.
pub const DRAWING_CHORD_MM: f64 = 0.01;
/// Parts drawn per side (a bound on the drawing, not the board).
pub const MAX_DRAWN_PARTS: usize = 20_000;
/// Points of one drawn loop.
pub const MAX_LOOP_POINTS: usize = 4096;

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
    /// The fab profile's name and provenance line.
    pub profile: String,
    /// Fabrication notes from the effective rules (minimum track and gap,
    /// drill, annular ring, mask web, …), one sentence each.
    pub notes: Vec<String>,
    pub sides: Vec<AssemblySide>,
}

fn mm(v: f64) -> String {
    crate::layout::limits::mm(v)
}

fn finish_word(f: PadFinish) -> &'static str {
    match f {
        PadFinish::Enig => "ENIG",
        PadFinish::Hasl => "HASL (tin-lead)",
        PadFinish::LeadFreeHasl => "Lead-free HASL",
        PadFinish::Osp => "OSP",
        PadFinish::ImmersionSilver => "Immersion silver",
    }
}

fn mask_word(c: MaskColour) -> &'static str {
    match c {
        MaskColour::Green => "green",
        MaskColour::Blue => "blue",
        MaskColour::Red => "red",
        MaskColour::Purple => "purple",
        MaskColour::Yellow => "yellow",
        MaskColour::Black => "black",
        MaskColour::White => "white",
        MaskColour::Clear => "clear",
    }
}

fn legend_word(c: SilkscreenColour) -> &'static str {
    match c {
        SilkscreenColour::White => "white",
        SilkscreenColour::Black => "black",
        SilkscreenColour::Yellow => "yellow",
    }
}

fn bounded(mut ring: Vec<Point>) -> Vec<Point> {
    ring.truncate(MAX_LOOP_POINTS);
    ring
}

/// The stackup table rows, top to bottom: copper and dielectrics, the mask
/// at the stackup's mask thickness and the legend by side without a
/// thickness, which the stackup does not hold. Neither counts towards the
/// nominal board thickness.
fn stackup_rows(project: &Project) -> Vec<StackupRow> {
    let s = &project.stackup;
    let mut rows = Vec::new();
    if s.silkscreen_top {
        rows.push(StackupRow {
            name: "Top legend".into(),
            kind: "legend".into(),
            material: format!("{} ink", legend_word(project.fabrication.silkscreen_colour)),
            thickness_mm: 0.0,
        });
    }
    if s.mask_top {
        rows.push(StackupRow {
            name: "Top mask".into(),
            kind: "mask".into(),
            material: format!("{} solder mask", mask_word(project.fabrication.mask_colour)),
            thickness_mm: s.mask_um / 1000.0,
        });
    }
    let layers = s.layers();
    let (mut cores, mut prepregs) = (0, 0);
    for (i, foil) in s.copper.iter().enumerate() {
        let name = layers
            .get(i)
            .map(|l| l.name())
            .unwrap_or_else(|| format!("Copper {}", i + 1));
        rows.push(StackupRow {
            name: format!("{name} copper"),
            kind: "copper".into(),
            material: format!("{:.0} µm Cu", foil.thickness_um),
            thickness_mm: foil.thickness_um / 1000.0,
        });
        if let Some(d) = s.dielectrics.get(i) {
            let (kind, n) = match d.kind {
                DielectricKind::Core => {
                    cores += 1;
                    ("core", cores)
                }
                DielectricKind::Prepreg => {
                    prepregs += 1;
                    ("prepreg", prepregs)
                }
            };
            let glass = match d.glass {
                crate::model::GlassStyle::Style106 => "106",
                crate::model::GlassStyle::Style1080 => "1080",
                crate::model::GlassStyle::Style2116 => "2116",
                crate::model::GlassStyle::Style7628 => "7628",
            };
            let title = if kind == "core" { "Core" } else { "Prepreg" };
            rows.push(StackupRow {
                name: format!("{title} {n}"),
                kind: kind.into(),
                material: format!("FR-4 {glass}"),
                thickness_mm: d.thickness_mm,
            });
        }
    }
    if s.mask_bottom {
        rows.push(StackupRow {
            name: "Bottom mask".into(),
            kind: "mask".into(),
            material: format!("{} solder mask", mask_word(project.fabrication.mask_colour)),
            thickness_mm: s.mask_um / 1000.0,
        });
    }
    if s.silkscreen_bottom {
        rows.push(StackupRow {
            name: "Bottom legend".into(),
            kind: "legend".into(),
            material: format!("{} ink", legend_word(project.fabrication.silkscreen_colour)),
            thickness_mm: 0.0,
        });
    }
    rows
}

fn notes(project: &Project) -> Vec<String> {
    let rules = project.rules.effective();
    let gap = rules
        .copper
        .clearance
        .get(CopperObject::Track, CopperObject::Track);
    let via_hole = rules
        .copper
        .clearance
        .get(CopperObject::ViaHole, CopperObject::Track);
    let pth_hole = rules
        .copper
        .clearance
        .get(CopperObject::PthHole, CopperObject::Track);
    let weights: Vec<String> = project
        .stackup
        .copper
        .iter()
        .map(|c| format!("{:.0} µm", c.thickness_um))
        .collect();
    let s = &project.stackup;
    let masked = s.mask_top || s.mask_bottom;
    vec![
        format!(
            "Nominal board thickness {} over the outer copper, solder mask and legend \
             excluded; {} copper layers.",
            mm(s.thickness_mm()),
            s.copper.len()
        ),
        if masked {
            format!(
                "Solder mask {} µm thick over copper on each masked side.",
                trim_um(s.mask_um)
            )
        } else {
            "No solder mask.".into()
        },
        format!("Copper thickness top to bottom: {}.", weights.join(", ")),
        format!(
            "Minimum track width {}, minimum copper gap {}.",
            mm(rules.copper.track_width_min),
            mm(gap)
        ),
        format!(
            "Minimum finished drill {}, via hole to copper {}, plated hole to copper {}.",
            mm(rules.holes.drill_min),
            mm(via_hole),
            mm(pth_hole)
        ),
        format!(
            "Minimum annular ring: vias {}, plated holes {}.",
            mm(rules.holes.via_annular_ring_min),
            mm(rules.holes.pth_annular_ring_min)
        ),
        format!(
            "Solder mask: {} {}, expansion {}, minimum web {}.",
            mask_word(project.fabrication.mask_colour),
            match project.fabrication.mask_finish {
                crate::model::MaskFinish::Gloss => "gloss",
                crate::model::MaskFinish::Satin => "satin",
                crate::model::MaskFinish::Matte => "matte",
            },
            mm(rules.mask.expansion),
            mm(rules.mask.web_min)
        ),
        format!(
            "Legend: {} ink, minimum line {}, minimum text height {}.",
            legend_word(project.fabrication.silkscreen_colour),
            mm(rules.silkscreen.line_width_min),
            mm(rules.silkscreen.text_height_min)
        ),
        format!(
            "Surface finish: {}.",
            finish_word(project.fabrication.pad_finish)
        ),
        format!(
            "Dimensions in millimetres. {}",
            project.rules.profile.as_ref().map_or_else(
                || "Hole sizes are finished sizes; no fab profile says how the fabricator \
                    reads drill files."
                    .to_string(),
                |p| p.drill_files.note()
            )
        ),
        "Generated by Pcbcraft; the Gerber job file lists every layer.".into(),
    ]
}

fn profile_line(project: &Project) -> String {
    let Some(p) = &project.rules.profile else {
        return "No fab profile: no manufacturing rule was checked".into();
    };
    let mut source = p.source.publisher.clone();
    if !p.source.document.is_empty() {
        source = format!("{source}, {}", p.source.document);
    }
    format!("{} (from {source}, read {})", p.name, p.source.date)
}

/// `um` without trailing zeros.
fn trim_um(um: f64) -> String {
    let text = format!("{um:.1}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The value printed for each instance: its BOM values, else its MPN.
fn values(project: &Project) -> BTreeMap<Id, String> {
    let mut out = BTreeMap::new();
    if let Ok(bom) = crate::completeness::bom(project) {
        for line in bom.lines {
            let text = if line.values.is_empty() {
                line.mpn.clone().unwrap_or_default()
            } else {
                line.values
                    .iter()
                    .map(|v| v.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            for id in line.instances {
                out.insert(id, text.clone());
            }
        }
    }
    out
}

fn box_loop(b: Bounds) -> Vec<Point> {
    vec![b.min, [b.max[0], b.min[1]], b.max, [b.min[0], b.max[1]]]
}

fn assembly_sides(project: &Project) -> Vec<AssemblySide> {
    let values = values(project);
    let mut assembly: BTreeMap<(Id, bool), Vec<Vec<Point>>> = BTreeMap::new();
    let mut courtyard: BTreeMap<(Id, bool), Vec<Vec<Point>>> = BTreeMap::new();
    let bottom = |l: Layer| l == Layer::Bottom;
    for g in layout::placed_graphics(project) {
        let (map, side) = match g.layer {
            BoardDrawing::Assembly(side) => (&mut assembly, side),
            BoardDrawing::Courtyard(side) => (&mut courtyard, side),
            BoardDrawing::Silkscreen(_) => continue,
        };
        if let PlacedItem::Shape { shape, .. } = &g.item {
            let ring = shape.flatten(DRAWING_CHORD_MM);
            if ring.len() >= 2 {
                let mut ring = bounded(ring);
                if shape.is_closed()
                    && let Some(first) = ring.first().copied()
                {
                    ring.push(first);
                }
                map.entry((g.instance, bottom(side)))
                    .or_default()
                    .push(ring);
            }
        }
    }
    let mut pads: BTreeMap<(Id, bool), Vec<Vec<Point>>> = BTreeMap::new();
    let mut polarity: BTreeMap<Id, Point> = BTreeMap::new();
    for placed in layout::placed_pads(project) {
        let ring = bounded(placed.outline().flatten(PAD_ARC_TOLERANCE_MM.max(0.005)));
        if ring.len() < 3 {
            continue;
        }
        let sides: &[Layer] = if placed.pad.kind.through_hole() {
            &[Layer::Top, Layer::Bottom]
        } else {
            std::slice::from_ref(&placed.layer)
        };
        for side in sides {
            let mut closed = ring.clone();
            closed.push(ring[0]);
            pads.entry((placed.instance, bottom(*side)))
                .or_default()
                .push(closed);
        }
        let is_polarity = project
            .instance(placed.instance)
            .and_then(|i| crate::instances::resolve(&project.library, i).ok())
            .and_then(|r| r.physical)
            .and_then(|p| p.footprint.definition.polarity.as_ref().map(|q| q.pad))
            == Some(placed.pad.id);
        if is_polarity {
            polarity.insert(placed.instance, placed.center());
        }
    }
    let mut sides = Vec::new();
    for side in [Layer::Top, Layer::Bottom] {
        let mut parts = Vec::new();
        for placement in &project.placements {
            if placement.layer != side || parts.len() >= MAX_DRAWN_PARTS {
                continue;
            }
            let Some(instance) = project.instance(placement.instance) else {
                continue;
            };
            let key = (instance.id, bottom(side));
            let part_pads = pads.get(&key).cloned().unwrap_or_default();
            let body = assembly
                .get(&key)
                .cloned()
                .or_else(|| courtyard.get(&key).cloned())
                .or_else(|| {
                    let b = Bounds::of(part_pads.iter().flatten().copied())?;
                    let mut ring = box_loop(b);
                    ring.push(b.min);
                    Some(vec![ring])
                })
                .unwrap_or_default();
            parts.push(AssemblyPart {
                instance: instance.id,
                designator: instance.designator.clone(),
                value: values.get(&instance.id).cloned().unwrap_or_default(),
                at: placement.position,
                rotation_deg: placement.rotation_deg,
                body,
                pads: part_pads,
                polarity: polarity.get(&instance.id).copied(),
            });
        }
        if !parts.is_empty() {
            sides.push(AssemblySide { side, parts });
        }
    }
    sides
}

/// The drawing data of `project`.
pub fn data(project: &Project, _options: &FabricationOptions) -> Result<DrawingData> {
    let board = project.board();
    let holes = super::drill::holes(project);
    let mut outline = bounded(flatten(&board.outer, DRAWING_CHORD_MM));
    if let Some(first) = outline.first().copied() {
        outline.push(first);
    }
    let cutouts = board
        .cutouts
        .iter()
        .filter(|c| c.kind != (CutoutKind::Slot { plated: true }))
        .map(|c| {
            let mut ring = bounded(flatten(&c.outline, DRAWING_CHORD_MM));
            if let Some(first) = ring.first().copied() {
                ring.push(first);
            }
            ring
        })
        .collect();
    let f = &project.fabrication;
    Ok(DrawingData {
        title: project.name.clone(),
        bounds: board.bounds(),
        outline,
        cutouts,
        drills: super::drill::table(&holes),
        holes,
        stackup: stackup_rows(project),
        thickness_mm: project.stackup.thickness_mm(),
        copper_layers: project.stackup.copper.len(),
        finish: finish_word(f.pad_finish).into(),
        mask: mask_word(f.mask_colour).into(),
        legend: legend_word(f.silkscreen_colour).into(),
        profile: profile_line(project),
        notes: notes(project),
        sides: assembly_sides(project),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    #[test]
    fn the_showcase_drawing_data_matches_the_board() -> Result<()> {
        let engine = Engine::showcase()?;
        let project = engine.project();
        let d = data(project, &FabricationOptions::default())?;
        let size = project.board_size;
        assert!((d.bounds.max[0] - d.bounds.min[0] - size[0]).abs() < 1e-9);
        assert!((d.bounds.max[1] - d.bounds.min[1] - size[1]).abs() < 1e-9);
        let ob = Bounds::of(d.outline.iter().copied()).ok_or(crate::Error::Invalid("o".into()))?;
        assert_eq!(ob, d.bounds);
        assert_eq!(d.outline.first(), d.outline.last());
        // The nominal thickness is copper plus dielectrics; the mask rows
        // carry the stackup's mask thickness but are not part of it.
        let total: f64 = d
            .stackup
            .iter()
            .filter(|r| r.kind != "mask")
            .map(|r| r.thickness_mm)
            .sum();
        assert!((total - project.stackup.thickness_mm()).abs() < 1e-9);
        let masks: Vec<f64> = d
            .stackup
            .iter()
            .filter(|r| r.kind == "mask")
            .map(|r| r.thickness_mm)
            .collect();
        assert_eq!(masks.len(), 2);
        assert!(
            masks
                .iter()
                .all(|t| (t - project.stackup.mask_um / 1000.0).abs() < 1e-12)
        );
        assert!(
            d.notes
                .iter()
                .any(|n| n.starts_with("Nominal board thickness")
                    && n.contains("solder mask and legend excluded")),
            "{:?}",
            d.notes
        );
        assert!(
            d.notes.iter().any(|n| n
                == &format!(
                    "Solder mask {} µm thick over copper on each masked side.",
                    project.stackup.mask_um
                )),
            "{:?}",
            d.notes
        );
        // The test prototype reads drill files as finished sizes.
        assert!(
            d.notes.iter().any(|n| n
                .contains("Hole sizes are finished sizes; the drill file lists finished sizes.")),
            "{:?}",
            d.notes
        );
        assert_eq!(d.copper_layers, 4);
        assert_eq!(d.stackup.iter().filter(|r| r.kind == "copper").count(), 4);
        // Parts per side: every placement once, on its side.
        let count = |side: Layer| {
            d.sides
                .iter()
                .filter(|s| s.side == side)
                .map(|s| s.parts.len())
                .sum::<usize>()
        };
        let expect = |side: Layer| {
            project
                .placements
                .iter()
                .filter(|p| p.layer == side)
                .count()
        };
        assert_eq!(count(Layer::Top), expect(Layer::Top));
        assert_eq!(count(Layer::Bottom), expect(Layer::Bottom));
        assert!(
            count(Layer::Bottom) >= 1,
            "the showcase has a bottom resistor"
        );
        // Polarised parts carry a polarity mark at their pin-1 pad.
        let polarised: Vec<&AssemblyPart> = d
            .sides
            .iter()
            .flat_map(|s| &s.parts)
            .filter(|p| p.polarity.is_some())
            .collect();
        assert!(
            polarised.len() >= 3,
            "{:?}",
            polarised.iter().map(|p| &p.designator).collect::<Vec<_>>()
        );
        for part in d.sides.iter().flat_map(|s| &s.parts) {
            assert!(!part.body.is_empty(), "{}", part.designator);
            assert!(!part.pads.is_empty(), "{}", part.designator);
            if let Some(p) = part.polarity {
                let near = part
                    .pads
                    .iter()
                    .flatten()
                    .any(|q| (q[0] - p[0]).hypot(q[1] - p[1]) < 5.0);
                assert!(near, "{}", part.designator);
            }
        }
        // Drill table counts every hole.
        let counted: usize = d.drills.tools.iter().map(|t| t.count).sum();
        assert_eq!(counted, d.holes.len());
        assert!(d.notes.iter().any(|n| n.contains("Minimum track width")));
        assert!(!d.profile.is_empty());
        Ok(())
    }
}
