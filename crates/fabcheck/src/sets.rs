//! Two fabrication sets compared layer by layer: Gerber layers paired by
//! their X2 `.FileFunction` (never by file name), drill files by plated or
//! non-plated function.
//!
//! With [`Scope::Pads`], the reference set is filtered to pad objects: on
//! copper the objects carrying a pad (`.P`) object attribute or flashed
//! with a pad aperture function (not a via's), on mask and
//! paste the flashes carrying a component (`.C`) attribute, in the drill
//! files every tool but via drills. That is how a footprint-only import is
//! held to a full board's output. [`Scope::Routed`] adds tracks and vias:
//! on copper every object but zone fills (regions without a pad
//! attribute) and non-conductor graphics, on mask and paste every flash,
//! and every drill.
use crate::compare::{LayerDiff, compare};
use crate::drills::{DrillDiff, hits, pair};
use crate::gerber::{GerberFile, Kind, Object};
use crate::polygons::{self, Shapes};
use crate::{Fallible, excellon};
use std::collections::BTreeMap;

/// One file of a set.
#[derive(Clone, Debug)]
pub struct File {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// What of the reference set is compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Everything.
    All,
    /// Pad objects and component drills only.
    Pads,
    /// Pads, tracks and vias: everything but zone fills and drawings.
    Routed,
}

/// A circle of the design whose copper the reference may approximate by
/// chords (KiCad approximates round custom-pad primitives within its
/// `max_error`), mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    pub center: [f64; 2],
    pub radius: f64,
}

/// How differences are judged: `tolerance_nm` everywhere, and up to
/// `arc_tolerance_nm` for a piece lying wholly within that distance of
/// one of `arcs` (labelled an arc piece, never silent).
#[derive(Clone, Debug)]
pub struct Judge {
    pub tolerance_nm: i32,
    pub arc_tolerance_nm: i32,
    pub arcs: Vec<Arc>,
    /// Added to every reference coordinate before comparing, nm (when the
    /// two sets use different origins).
    pub shift_nm: [i64; 2],
}

impl Judge {
    pub fn exact(tolerance_nm: i32) -> Self {
        Self {
            tolerance_nm,
            arc_tolerance_nm: tolerance_nm,
            arcs: Vec::new(),
            shift_nm: [0, 0],
        }
    }

    /// Whether every point of `outline` lies within the arc tolerance of
    /// one circle.
    pub fn on_arc(&self, outline: &[[f64; 2]]) -> bool {
        let band = f64::from(self.arc_tolerance_nm) / 1e6;
        self.arcs.iter().any(|a| {
            outline
                .iter()
                .all(|p| ((p[0] - a.center[0]).hypot(p[1] - a.center[1]) - a.radius).abs() <= band)
        })
    }
}

/// One Gerber layer compared.
#[derive(Clone, Debug)]
pub struct LayerResult {
    /// The `.FileFunction` value, e.g. `Copper,L1,Top`.
    pub function: String,
    pub diff: LayerDiff,
    /// Pieces over the tolerance lying on an arc, and the widest of them, µm.
    pub arc_pieces: usize,
    pub arc_worst_um: f64,
    /// The widest piece not on an arc, µm.
    pub worst_um: f64,
    /// Whether the layer is within the judge's tolerances.
    pub passes: bool,
    /// Objects of the reference file, and how many the scope kept.
    pub reference_objects: usize,
    pub kept_objects: usize,
    /// Our image and the reference image (for locating differences).
    pub ours: Shapes,
    pub reference: Shapes,
}

/// One drill function compared.
#[derive(Clone, Debug)]
pub struct DrillResult {
    /// `Plated` or `NonPlated`.
    pub function: String,
    pub diff: DrillDiff,
    /// Reference hits the scope left out (via drills).
    pub excluded: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SetDiff {
    pub layers: Vec<LayerResult>,
    pub drills: Vec<DrillResult>,
}

impl SetDiff {
    pub fn passes(&self) -> bool {
        self.layers.iter().all(|l| l.passes) && self.drills.iter().all(|d| d.diff.passes())
    }
}

/// Layer functions compared: copper, mask and paste.
fn compared(function: &str) -> bool {
    ["Copper,", "Soldermask,", "Paste,"]
        .iter()
        .any(|p| function.starts_with(p))
}

fn gerbers(files: &[File]) -> Fallible<BTreeMap<String, GerberFile>> {
    let mut out = BTreeMap::new();
    for file in files.iter().filter(|f| f.name.ends_with(".gbr")) {
        let parsed = crate::gerber::parse(std::str::from_utf8(&file.bytes)?)?;
        let Some(function) = parsed.file_attribute(".FileFunction") else {
            continue;
        };
        let function = function.join(",");
        if compared(&function) && out.insert(function.clone(), parsed).is_some() {
            return Err(format!("sets: two layers of function {function}").into());
        }
    }
    Ok(out)
}

fn drill_files(files: &[File]) -> Fallible<BTreeMap<String, excellon::DrillFile>> {
    let mut out = BTreeMap::new();
    for file in files.iter().filter(|f| f.name.ends_with(".drl")) {
        let parsed = excellon::parse(std::str::from_utf8(&file.bytes)?)?;
        let Some(function) = parsed.attribute(".FileFunction") else {
            continue;
        };
        let Some(kind) = function.first().cloned() else {
            continue;
        };
        if out.insert(kind.clone(), parsed).is_some() {
            return Err(format!("sets: two drill files of function {kind}").into());
        }
    }
    Ok(out)
}

/// Whether a reference object is a pad object of a layer of `function`:
/// on copper, an object with a pad (`.P`) attribute, or a flash whose
/// aperture function names a pad other than a via's (inner layers carry no
/// `.P`); on mask and paste, a flash with a component (`.C`) attribute.
fn pad_object(function: &str, object: &Object) -> bool {
    if function.starts_with("Copper,") {
        let pad_function = object
            .aperture_attributes
            .get(".AperFunction")
            .and_then(|v| v.first())
            .is_some_and(|f| f.ends_with("Pad") && f != "ViaPad");
        object.object_attributes.contains_key(".P")
            || (matches!(object.kind, Kind::Flash { .. }) && pad_function)
    } else {
        matches!(object.kind, Kind::Flash { .. }) && object.object_attributes.contains_key(".C")
    }
}

/// Whether a reference object is a pad, track or via of a layer of
/// `function` ([`Scope::Routed`]).
fn routed_object(function: &str, object: &Object) -> bool {
    if function.starts_with("Copper,") {
        let aperture = object
            .aperture_attributes
            .get(".AperFunction")
            .and_then(|v| v.first())
            .map(String::as_str);
        if object.object_attributes.contains_key(".P") {
            return true;
        }
        !matches!(object.kind, Kind::Region { .. }) && aperture != Some("NonConductor")
    } else {
        matches!(object.kind, Kind::Flash { .. })
    }
}

/// `shapes` moved by `shift` nm.
fn shifted(mut shapes: Shapes, shift: [i64; 2]) -> Fallible<Shapes> {
    if shift == [0, 0] {
        return Ok(shapes);
    }
    let (dx, dy) = (i32::try_from(shift[0])?, i32::try_from(shift[1])?);
    for ring in shapes.iter_mut().flatten() {
        for p in ring.iter_mut() {
            p.x = p.x.checked_add(dx).ok_or("sets: a shifted point overflows")?;
            p.y = p.y.checked_add(dy).ok_or("sets: a shifted point overflows")?;
        }
    }
    Ok(shapes)
}

/// Drill hits moved by `shift` nm.
fn shifted_hits(mut hits: Vec<crate::drills::DrillHit>, shift: [i64; 2]) -> Vec<crate::drills::DrillHit> {
    let d = [shift[0] as f64 / 1e6, shift[1] as f64 / 1e6];
    let m = |p: &mut crate::Point| {
        p[0] += d[0];
        p[1] += d[1];
    };
    for hit in &mut hits {
        match &mut hit.feature {
            crate::drills::Feature::Round { at } => m(at),
            crate::drills::Feature::Slot { from, to } => {
                m(from);
                m(to);
            }
        }
    }
    hits
}

/// Compare `ours` with `reference` under `judge`.
pub fn compare_sets(
    ours: &[File],
    reference: &[File],
    scope: Scope,
    judge: &Judge,
) -> Fallible<SetDiff> {
    let tolerance_nm = judge.tolerance_nm;
    let mine = gerbers(ours)?;
    let theirs = gerbers(reference)?;
    let mut functions: Vec<&String> = mine.keys().chain(theirs.keys()).collect();
    functions.sort();
    functions.dedup();
    let mut out = SetDiff::default();
    for function in functions {
        let a = match mine.get(function) {
            Some(file) => polygons::render(file, |_| true)?,
            None => Vec::new(),
        };
        let (b, total, kept) = match theirs.get(function) {
            Some(file) => {
                let keep = |o: &Object| match scope {
                    Scope::All => true,
                    Scope::Pads => pad_object(function, o),
                    Scope::Routed => routed_object(function, o),
                };
                let kept = file.objects.iter().filter(|o| keep(o)).count();
                let shapes = shifted(polygons::render(file, keep)?, judge.shift_nm)?;
                (shapes, file.objects.len(), kept)
            }
            None => (Vec::new(), 0, 0),
        };
        let diff = compare(&a, &b, tolerance_nm)?;
        let mut arc_pieces = 0;
        let mut arc_worst_um: f64 = 0.0;
        let mut worst_um: f64 = if diff.pieces.is_empty() {
            diff.worst_width_um
        } else {
            0.0
        };
        for piece in &diff.pieces {
            if judge.on_arc(&piece.outline) {
                arc_pieces += 1;
                arc_worst_um = arc_worst_um.max(piece.width_um);
            } else {
                worst_um = worst_um.max(piece.width_um);
            }
        }
        let passes = worst_um <= f64::from(tolerance_nm) / 1e3
            && arc_worst_um <= f64::from(judge.arc_tolerance_nm) / 1e3;
        out.layers.push(LayerResult {
            function: function.clone(),
            diff,
            arc_pieces,
            arc_worst_um,
            worst_um,
            passes,
            reference_objects: total,
            kept_objects: kept,
            ours: a,
            reference: b,
        });
    }
    let mine = drill_files(ours)?;
    let theirs = drill_files(reference)?;
    let mut kinds: Vec<&String> = mine.keys().chain(theirs.keys()).collect();
    kinds.sort();
    kinds.dedup();
    let tolerance_mm = f64::from(tolerance_nm) / 1e6;
    for kind in kinds {
        let a = match mine.get(kind) {
            Some(file) => hits(file, |_| true)?,
            None => Vec::new(),
        };
        let (b, excluded) = match theirs.get(kind) {
            Some(file) => {
                let all = hits(file, |_| true)?;
                let kept = hits(file, |f| {
                    scope != Scope::Pads || !f.iter().any(|v| v == "ViaDrill")
                })?;
                let excluded = all.len() - kept.len();
                (shifted_hits(kept, judge.shift_nm), excluded)
            }
            None => (Vec::new(), 0),
        };
        out.drills.push(DrillResult {
            function: kind.clone(),
            diff: pair(&a, &b, tolerance_mm),
            excluded,
        });
    }
    Ok(out)
}

/// A pad of the design, for naming where pieces lie: its box (mm) and a
/// class name that carries no names or values from the design.
#[derive(Clone, Debug)]
pub struct PadBox {
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub class: String,
}

/// What kind of piece a difference piece is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PieceKind {
    /// On a round primitive the reference draws as chords.
    Arc,
    /// Anything else.
    Other,
}

/// Pieces over the tolerance grouped by kind and by the class of the pad
/// they lie on (or "no pad").
#[derive(Clone, Debug, PartialEq)]
pub struct ClassRow {
    pub kind: PieceKind,
    pub class: String,
    pub pieces: usize,
    pub widest_um: f64,
    /// Where the widest lies, mm (for the local report only).
    pub at: [f64; 2],
}

/// Group a layer's pieces by kind and pad class.
pub fn attribute(layer: &LayerResult, judge: &Judge, pads: &[PadBox]) -> Vec<ClassRow> {
    let mut rows: BTreeMap<(PieceKind, String), ClassRow> = BTreeMap::new();
    for piece in &layer.diff.pieces {
        let kind = if judge.on_arc(&piece.outline) {
            PieceKind::Arc
        } else {
            PieceKind::Other
        };
        let c = piece.center();
        let class = pads
            .iter()
            .find(|p| {
                p.min[0] - 0.2 <= c[0]
                    && c[0] <= p.max[0] + 0.2
                    && p.min[1] - 0.2 <= c[1]
                    && c[1] <= p.max[1] + 0.2
            })
            .map_or_else(|| "no pad".to_owned(), |p| p.class.clone());
        let row = rows.entry((kind, class.clone())).or_insert(ClassRow {
            kind,
            class,
            pieces: 0,
            widest_um: 0.0,
            at: c,
        });
        row.pieces += 1;
        if piece.width_um > row.widest_um {
            row.widest_um = piece.width_um;
            row.at = c;
        }
    }
    rows.into_values().collect()
}
