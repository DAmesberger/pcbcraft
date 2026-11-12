//! The Component 3D tab's placement state and the pure helpers its view and
//! inspector share: the shown (preview) placement, triad changes, snap
//! points for point-to-point moves and the display-only overlay of lands
//! and the seating plane. Every finished gesture is one typed
//! `library.edit_package` or `library.fit_leads` command; while a gesture
//! runs only the display transform changes and no geometry is rebuilt.
use pcbcraft_core::geometry::{Point, Point3, Transform3};
use pcbcraft_core::library::package_fit::{self, FitRotation};
use pcbcraft_core::library::{
    Axis, FootprintDefinition, ModelPlacement, PAD_ARC_TOLERANCE_MM, PackageDefinition,
    PackageEdit, PadKind,
};
use pcbcraft_core::{DefinitionId, LibraryCommand, Operation};
use pcbcraft_solidcraft::geom::{Aabb3, Vec3};
use pcbcraft_ui::gizmo::Change;
use solidcraft_viewport_gpu::view::GpuScene;

/// Copper thickness the overlay lands are drawn with (display only; the
/// board model uses the stack-up).
const LAND_MM: f64 = 0.035;
/// How far the overlay lands' tops lie below the seating plane: the solder
/// joint's bond line under a seated part (est.: a 100 to 125 µm stencil
/// print reflows to about half). A land top on the seating plane shared
/// that plane with every lead and pad bottom standing on it, and the two
/// flickered in black streaks (z-fighting).
pub const LAND_GAP_MM: f64 = 0.05;
/// How far the drawn seating plane reaches past the lands and body: a
/// share of their diagonal within these bounds, so Fit view frames the
/// part and its lands rather than a wide plane.
const PLANE_MARGIN_SHARE: f64 = 0.1;
const PLANE_MARGIN_MM: [f64; 2] = [0.2, 1.5];

/// The seating plane's margin around lands and body within `bounds`.
fn plane_margin(bounds: &Aabb3) -> f64 {
    let d = (bounds.max.x - bounds.min.x).hypot(bounds.max.y - bounds.min.y);
    (PLANE_MARGIN_SHARE * d).clamp(PLANE_MARGIN_MM[0], PLANE_MARGIN_MM[1])
}

/// One arrow-key nudge of the model, in mm (Shift: ten times).
pub const NUDGE_MM: f64 = 0.05;

/// The datum-frame shift of a nudge key, if `key` is one.
pub fn nudge(key: egui::Key, shift: bool) -> Option<[f64; 3]> {
    let step = if shift { 10.0 * NUDGE_MM } else { NUDGE_MM };
    Some(match key {
        egui::Key::ArrowLeft => [-step, 0.0, 0.0],
        egui::Key::ArrowRight => [step, 0.0, 0.0],
        egui::Key::ArrowDown => [0.0, -step, 0.0],
        egui::Key::ArrowUp => [0.0, step, 0.0],
        egui::Key::PageUp => [0.0, 0.0, step],
        egui::Key::PageDown => [0.0, 0.0, -step],
        _ => return None,
    })
}

/// A point-to-point move in progress.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PointToPoint {
    #[default]
    Off,
    /// Pick a point on the model.
    Source,
    /// Pick where it goes: a pad point or the datum.
    Target(Point3),
}

/// A triad drag: the placement it started from and the preview now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub start: ModelPlacement,
    pub preview: ModelPlacement,
}

#[derive(Default)]
pub struct ModelTab {
    /// Typed placement values not yet committed.
    pub fields: Option<ModelPlacement>,
    /// Typed form values not yet committed.
    pub form: Option<pcbcraft_core::library::PackageForm>,
    pub drag: Option<Drag>,
    /// Esc ended the triad drag; ignore it until the button is released.
    pub cancelled: bool,
    /// A placement sent to the engine, shown until the draft reflects it,
    /// with the engine revision it was sent at.
    pub pending: Option<(ModelPlacement, u64)>,
    pub point_to_point: PointToPoint,
    /// Fit with quarter turns only.
    pub quarter: bool,
    /// Hide the lands and seating plane.
    pub hide_lands: bool,
    /// What the overlay was last built from.
    pub overlay_key: Option<Vec<u8>>,
}

impl ModelTab {
    /// The placement the view shows: a drag preview, typed values, a sent
    /// placement, or the draft's.
    pub fn shown(&self, package: &PackageDefinition) -> ModelPlacement {
        if let Some(drag) = &self.drag {
            return drag.preview;
        }
        self.fields
            .or(self.pending.map(|(p, _)| p))
            .unwrap_or(package.placement)
    }

    pub fn rotation(&self) -> FitRotation {
        if self.quarter {
            FitRotation::Quarter
        } else {
            FitRotation::Free
        }
    }

    /// Drop a sent placement once the draft shows it or the engine moved on
    /// (applied or refused).
    pub fn settle(&mut self, package: &PackageDefinition, revision: u64, synced: bool) {
        if let Some((placement, sent)) = self.pending
            && (placement == package.placement || synced && revision != sent)
        {
            self.pending = None;
        }
    }

    /// The command storing `placement`, remembered as pending.
    pub fn set(
        &mut self,
        draft: DefinitionId,
        current: &ModelPlacement,
        placement: ModelPlacement,
        revision: u64,
    ) -> Option<Operation> {
        if placement == *current {
            return None;
        }
        self.pending = Some((placement, revision));
        Some(edit(draft, vec![PackageEdit::SetPlacement(placement)]))
    }
}

pub fn edit(draft: DefinitionId, edits: Vec<PackageEdit>) -> Operation {
    Operation::Library(LibraryCommand::EditPackage { draft, edits })
}

/// The confirmed fit of the draft's leads to its footprint's pads; the
/// engine derives the pairs from the typed form.
pub fn fit(draft: DefinitionId, rotation: FitRotation) -> Operation {
    Operation::Library(LibraryCommand::FitLeads {
        draft,
        pairs: vec![],
        rotation,
        confirm: true,
    })
}

fn axis(index: usize) -> Axis {
    match index {
        0 => Axis::X,
        1 => Axis::Y,
        _ => Axis::Z,
    }
}

/// The placement a triad change gives from `start`: a datum-frame shift,
/// or a turn about a datum axis through the body centre.
pub fn dragged(
    package: &PackageDefinition,
    start: &ModelPlacement,
    change: Change,
) -> Option<ModelPlacement> {
    let outer = match change {
        Change::Translate(v) => Transform3::translation([v.x, v.y, v.z]),
        Change::Rotate { axis: i, angle } => package_fit::turn_about(
            axis(i),
            angle.to_degrees(),
            package_fit::body_centre(package, start),
        ),
    };
    package_fit::moved(start, &outer).ok()
}

/// Model snap points under `placement`, in the datum frame: the body's
/// corners, edge midpoints and face centres, and the lead contacts.
pub fn model_points(package: &PackageDefinition, placement: &ModelPlacement) -> Vec<Point3> {
    let corners = package_fit::body_corners(package, placement);
    let mid = |a: Point3, b: Point3| std::array::from_fn(|i| (a[i] + b[i]) / 2.0);
    let mut out = corners.to_vec();
    for a in 0..8usize {
        for bit in [1usize, 2, 4] {
            if a & bit == 0 {
                out.push(mid(corners[a], corners[a | bit]));
            }
        }
    }
    for bit in [1usize, 2, 4] {
        for side in [0, bit] {
            let face: Vec<Point3> = (0..8)
                .filter(|k| k & bit == side)
                .map(|k| corners[k])
                .collect();
            let n = face.len().max(1) as f64;
            out.push(std::array::from_fn(|i| {
                face.iter().map(|p| p[i]).sum::<f64>() / n
            }));
        }
    }
    let t = placement.transform();
    out.extend(package.form.lead_points().into_iter().map(|p| t.apply(p)));
    out
}

fn copper(kind: PadKind) -> bool {
    matches!(kind, PadKind::Smd | PadKind::PlatedThrough)
}

/// A copper pad's outline on the seating plane, in the datum frame.
fn land(footprint: &FootprintDefinition, pad: &pcbcraft_core::library::Pad) -> Vec<Point> {
    pad.outline(PAD_ARC_TOLERANCE_MM)
        .into_iter()
        .map(|p| package_fit::to_datum(&footprint.datum, p))
        .collect()
}

/// Board snap points in the datum frame, on the seating plane: the datum
/// origin, and each copper pad's centre, outline vertices and edge
/// midpoints.
pub fn board_points(footprint: &FootprintDefinition) -> Vec<Point3> {
    let mut out = vec![[0.0; 3]];
    for pad in footprint.pads.iter().filter(|p| copper(p.kind)) {
        let c = package_fit::to_datum(&footprint.datum, pad.position());
        out.push([c[0], c[1], 0.0]);
        let ring = land(footprint, pad);
        for (i, a) in ring.iter().enumerate() {
            let b = ring[(i + 1) % ring.len()];
            out.push([a[0], a[1], 0.0]);
            out.push([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, 0.0]);
        }
    }
    out
}

/// The translation moving `source` onto `target`.
pub fn point_to_point(
    placement: &ModelPlacement,
    source: Point3,
    target: Point3,
) -> Option<ModelPlacement> {
    package_fit::moved(
        placement,
        &Transform3::translation(std::array::from_fn(|i| target[i] - source[i])),
    )
    .ok()
}

/// The nearest of `points` to `pointer` on screen within `radius` pixels.
pub fn nearest(
    points: &[Point3],
    to_screen: impl Fn(Point3) -> Option<egui::Pos2>,
    pointer: egui::Pos2,
    radius: f32,
) -> Option<Point3> {
    points
        .iter()
        .filter_map(|p| Some((*p, to_screen(*p)?.distance(pointer))))
        .filter(|(_, d)| *d <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| p)
}

/// Distance snap of the triad at the current zoom: a 1-2-5 step near
/// `px` pixels.
pub fn snap_step(mm_per_px: f64, px: f64) -> f64 {
    let raw = mm_per_px * px;
    if !(raw.is_finite() && raw > 0.0) {
        return 0.0;
    }
    let decade = 10f64.powf(raw.log10().floor());
    let m = raw / decade;
    let nice = if m < 1.5 {
        1.0
    } else if m < 3.5 {
        2.0
    } else if m < 7.5 {
        5.0
    } else {
        10.0
    };
    nice * decade
}

/// The overlay key of `footprint` and the committed body: the overlay is
/// rebuilt only when either changes, never during a drag.
pub fn overlay_key(footprint: &FootprintDefinition, package: &PackageDefinition) -> Vec<u8> {
    let mut key = serde_json::to_vec(&(&footprint.pads, &footprint.datum)).unwrap_or_default();
    key.extend(
        serde_json::to_vec(&package_fit::body_corners(package, &package.placement))
            .unwrap_or_default(),
    );
    key
}

fn v3(p: [f64; 3]) -> [f32; 3] {
    p.map(|v| v as f32)
}

/// Lands ([`LAND_GAP_MM`] under the seating plane, `LAND_MM` thick) and a
/// translucent board surface under them, display only: never saved, never
/// picked.
pub fn overlay(footprint: &FootprintDefinition, package: &PackageDefinition) -> (GpuScene, Aabb3) {
    let mut scene = GpuScene::default();
    let metal = scene.color([214, 178, 96, 255]);
    let edge = scene.color([150, 118, 52, 255]);
    let board = scene.color([28, 92, 58, 150]);
    let rim = scene.color([18, 64, 40, 255]);
    let mut bounds = Aabb3::EMPTY;
    let (top, low) = (-LAND_GAP_MM, -LAND_GAP_MM - LAND_MM);
    for pad in footprint.pads.iter().filter(|p| copper(p.kind)) {
        let ring = land(footprint, pad);
        if ring.len() < 3 {
            continue;
        }
        let n = ring.len() as f64;
        let c = ring
            .iter()
            .fold([0.0, 0.0], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
        let area: f64 = (0..ring.len())
            .map(|i| {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum();
        let ccw = area >= 0.0;
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let (a, b) = if ccw { (a, b) } else { (b, a) };
            for p in [[c[0], c[1]], a, b] {
                scene.tri(v3([p[0], p[1], top]), [0.0, 0.0, 1.0], metal);
            }
            let d = [b[0] - a[0], b[1] - a[1]];
            let len = d[0].hypot(d[1]).max(1e-12);
            let normal = [(d[1] / len) as f32, (-d[0] / len) as f32, 0.0];
            let (top_a, top_b) = ([a[0], a[1], top], [b[0], b[1], top]);
            let (low_a, low_b) = ([a[0], a[1], low], [b[0], b[1], low]);
            for p in [top_a, low_a, low_b, top_a, low_b, top_b] {
                scene.tri(v3(p), normal, edge);
            }
            bounds.add(Vec3::new(a[0], a[1], top));
        }
    }
    for corner in package_fit::body_corners(package, &package.placement) {
        bounds.add(Vec3::new(corner[0], corner[1], 0.0));
    }
    if bounds.is_empty() {
        bounds.add(Vec3::new(0.0, 0.0, 0.0));
    }
    let z = low;
    let margin = plane_margin(&bounds);
    let (x0, y0) = (bounds.min.x - margin, bounds.min.y - margin);
    let (x1, y1) = (bounds.max.x + margin, bounds.max.y + margin);
    let plane = [[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]];
    for k in [0, 1, 2, 0, 2, 3] {
        scene.trans_tri(v3(plane[k]), [0.0, 0.0, 1.0], board);
    }
    for k in 0..4 {
        scene.line(v3(plane[k]), v3(plane[(k + 1) % 4]), rim, 1.0, false);
    }
    bounds.add(Vec3::new(x0, y0, z));
    bounds.add(Vec3::new(x1, y1, z));
    (scene, bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schematic_editor::pt;
    use pcbcraft_core::library::package_form::typical_nm as nm;
    use pcbcraft_core::library::{GullWingForm, LeadSides, PackageForm, PackageTerminal, Pad};
    use pcbcraft_core::{PackageTerminalId, PadId};
    use pcbcraft_solidcraft::geom::Vec3;

    /// An original SOIC-8-like form whose rows run along x.
    pub fn soic() -> PackageDefinition {
        PackageDefinition {
            terminals: (1..=8)
                .map(|i| PackageTerminal {
                    id: PackageTerminalId(i),
                    designation: i.to_string(),
                })
                .collect(),
            form: PackageForm::GullWing(Box::new(GullWingForm {
                sides: LeadSides::Two,
                leads_per_side: [4, 0],
                pitch_nm: nm(1.27),
                body_length_nm: nm(4.9),
                body_width_nm: nm(3.9),
                body_height_nm: nm(1.5),
                standoff_nm: nm(0.1),
                span_nm: [nm(6.0), nm(0.0)],
                lead_width_nm: nm(0.4),
                lead_thickness_nm: nm(0.2),
                foot_length_nm: nm(0.6),
            })),
            envelope_nm: [nm(4.9), nm(6.0), nm(1.6)],
            ..PackageDefinition::default()
        }
    }

    /// Its lands down the −x column and up the +x column.
    pub fn soic_lands() -> FootprintDefinition {
        let y = |i: u32| 1.905 - 1.27 * f64::from(i);
        FootprintDefinition {
            pads: (0..8)
                .map(|k| {
                    let position = if k < 4 {
                        [-2.7, y(k)]
                    } else {
                        [2.7, -y(k - 4)]
                    };
                    Pad::smd_rect(
                        PadId(k + 1),
                        PackageTerminalId(k + 1),
                        (k + 1).to_string(),
                        pt(position),
                        pt([1.55, 0.6]),
                    )
                })
                .collect(),
            ..FootprintDefinition::default()
        }
    }

    #[test]
    fn a_triad_turn_keeps_the_body_centre_and_a_shift_adds() {
        let package = soic();
        let start = ModelPlacement::from_mm([1.0, 0.0, 0.0], [0.0; 3]).unwrap_or_default();
        let turned = dragged(
            &package,
            &start,
            Change::Rotate {
                axis: 2,
                angle: -std::f64::consts::FRAC_PI_2,
            },
        );
        // −90°, stored reduced to one turn.
        assert_eq!(turned.map(|p| p.rotation_deg()), Some([0.0, 0.0, 270.0]));
        assert_eq!(turned.map(|p| p.translation()), Some([1.0, 0.0, 0.0]));
        let shifted = dragged(
            &package,
            &start,
            Change::Translate(Vec3::new(0.5, -1.0, 0.25)),
        );
        assert_eq!(shifted.map(|p| p.translation()), Some([1.5, -1.0, 0.25]));
    }

    #[test]
    fn point_to_point_moves_a_lead_onto_a_pad_centre() {
        let package = soic();
        let fp = soic_lands();
        let placement = ModelPlacement::from_mm([0.0; 3], [0.0, 0.0, -90.0]).unwrap_or_default();
        let lead = model_points(&package, &placement)
            .into_iter()
            .last()
            .unwrap_or_default();
        let pad = board_points(&fp)[1];
        let moved = point_to_point(&placement, lead, pad);
        let t = moved.map(|p| p.transform());
        let landed = t.map(|t| t.apply(package.form.lead_points()[7]));
        assert!(landed.is_some_and(|q| q.iter().zip(pad).all(|(a, b)| (a - b).abs() < 1e-9)));
        // The datum origin is the first board point; 26 model points.
        assert_eq!(board_points(&fp)[0], [0.0; 3]);
        assert_eq!(model_points(&package, &placement).len(), 8 + 12 + 6 + 8);
    }

    #[test]
    fn the_shown_placement_prefers_the_drag_then_typed_then_pending() {
        let package = soic();
        let mut tab = ModelTab::default();
        assert_eq!(tab.shown(&package), package.placement);
        let sent = ModelPlacement::from_mm([0.0, 0.0, 1.0], [0.0; 3]).unwrap_or_default();
        assert!(
            tab.set(DefinitionId(3), &package.placement, sent, 5)
                .is_some()
        );
        assert_eq!(tab.shown(&package), sent);
        // Not yet reflected and the engine has not moved on: still pending.
        tab.settle(&package, 5, true);
        assert_eq!(tab.shown(&package), sent);
        // Refused (revision moved, draft unchanged): dropped.
        tab.settle(&package, 6, true);
        assert_eq!(tab.shown(&package), package.placement);
        // Storing the current placement sends nothing.
        assert!(
            tab.set(DefinitionId(3), &package.placement, package.placement, 6)
                .is_none()
        );
    }

    #[test]
    fn the_overlay_draws_every_land_and_a_plane_under_the_body() {
        let (scene, bounds) = overlay(&soic_lands(), &soic());
        assert!(!scene.tris.is_empty());
        assert!(!scene.trans.is_empty());
        // The plane reaches a little past the outermost land, not far.
        let past = -2.7 - 0.775 - bounds.min.x;
        assert!(
            (PLANE_MARGIN_MM[0] - 1e-6..=PLANE_MARGIN_MM[1] + 1e-6).contains(&past),
            "{past}"
        );
        assert!(past < 1.0, "a SOIC's plane stays near its lands: {past}");
        // The lands lie a bond line under the seating plane the leads
        // stand on, never on it.
        assert!((bounds.min.z + LAND_GAP_MM + LAND_MM).abs() < 1e-9);
    }

    #[test]
    fn snap_steps_are_round() {
        assert!((snap_step(0.01, 8.0) - 0.1).abs() < 1e-12);
        assert!((snap_step(0.03, 8.0) - 0.2).abs() < 1e-12);
        assert_eq!(snap_step(f64::NAN, 8.0), 0.0);
    }
}
