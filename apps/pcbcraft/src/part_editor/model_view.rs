//! The Component 3D view's placement gestures over Solidcraft's renderer:
//! the model shown at its (preview) placement through the viewport's
//! display transform, display-only lands and seating plane, Solidcraft's
//! move/rotate triad, point-to-point picks and markers on leads that miss
//! their lands. A drag changes only the display transform; its release
//! sends one `library.edit_package`.
use super::PartEditor;
use super::model_place::{self, Drag, PointToPoint};
use crate::viewport::Viewport;
use egui::{Color32, Pos2, Stroke, Ui};
use pcbcraft_core::geometry::{Point3, Transform3};
use pcbcraft_core::library::package_fit;
use pcbcraft_core::{Engine, Operation};
use pcbcraft_solidcraft::geom::Vec3;
use solidcraft_ui_egui::gizmo::{self, Triad};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::viewport::Proj;
use std::sync::Arc;

/// How near a snap point the pointer takes it (px).
const SNAP_PX: f32 = 14.0;
/// The triad's distance step is a round value near this many pixels.
const STEP_PX: f64 = 8.0;

fn vec3(p: Point3) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

/// Millimetres per screen pixel at `c`.
fn mm_per_px(proj: &Proj, c: Vec3) -> Option<f64> {
    let a = proj.to_screen(c)?;
    let b = proj.to_screen(c + Vec3::X)?;
    let px = f64::from(a.distance(b));
    (px > 1e-9).then(|| 1.0 / px)
}

impl PartEditor {
    /// Before the 3D view draws: the model's display transform and the
    /// lands overlay. Outside the Part view the viewport is reset by
    /// [`reset_model_view`].
    pub fn model_view_prepare(&mut self, viewport: &mut Viewport, engine: &Engine) {
        let synced = self
            .source
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &engine.document().drafts));
        let Some(set) = &self.draft else {
            reset_model_view(viewport);
            return;
        };
        let tab = &mut self.model;
        tab.settle(&set.package, engine.revision(), synced);
        let shown = tab.shown(&set.package);
        viewport.set_model_transform(if shown.finite() {
            shown.transform()
        } else {
            Transform3::IDENTITY
        });
        if tab.hide_lands {
            viewport.set_overlay(None);
            tab.overlay_key = None;
            return;
        }
        let key = model_place::overlay_key(&set.footprint, &set.package);
        if !viewport.has_overlay() || tab.overlay_key.as_ref() != Some(&key) {
            viewport.set_overlay(Some(model_place::overlay(&set.footprint, &set.package)));
            tab.overlay_key = Some(key);
        }
    }

    /// After the 3D view drew: the triad or point-to-point picks, and lead
    /// markers. Returns the command a finished gesture sends.
    pub fn model_view(
        &mut self,
        ui: &mut Ui,
        viewport: &Viewport,
        engine: &Engine,
    ) -> Option<Operation> {
        let rect = viewport.rect()?;
        let set = self.draft.as_ref()?;
        let (draft, package, footprint) =
            (set.ids.package, set.package.clone(), set.footprint.clone());
        let proj = viewport.proj(rect);
        let painter = ui.painter_at(rect);
        let tk = Tokens::get();
        let revision = engine.revision();
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let tab = &mut self.model;
        let shown = tab.shown(&package);
        let to_screen = |p: Point3| proj.to_screen(vec3(p));
        // Leads that miss their lands, while nothing is being dragged.
        if tab.drag.is_none() {
            for lead in package_fit::lead_states(&package, &footprint, &shown) {
                if lead.miss_mm.is_some_and(|m| m > 1e-9)
                    && let Some(p) = to_screen(lead.at)
                {
                    painter.circle(p, 4.0, tk.warning, Stroke::new(1.0, Color32::from_gray(30)));
                }
            }
        }
        if tab.point_to_point != PointToPoint::Off {
            if escape {
                tab.point_to_point = PointToPoint::Off;
                return None;
            }
            let points = match tab.point_to_point {
                PointToPoint::Target(_) => model_place::board_points(&footprint),
                _ => model_place::model_points(&package, &shown),
            };
            for p in points.iter().filter_map(|p| to_screen(*p)) {
                painter.circle_filled(p, 2.0, tk.text_dim);
            }
            let pointer = ui
                .input(|i| i.pointer.hover_pos())
                .filter(|p| rect.contains(*p));
            let hovered =
                pointer.and_then(|p| model_place::nearest(&points, to_screen, p, SNAP_PX));
            if let PointToPoint::Target(source) = tab.point_to_point
                && let Some(from) = to_screen(source)
            {
                painter.circle(
                    from,
                    5.0,
                    tk.accent,
                    Stroke::new(1.5, Color32::from_gray(30)),
                );
                if let Some(to) = hovered.and_then(to_screen) {
                    dashed(&painter, from, to, tk.accent);
                }
            }
            if let Some(at) = hovered.and_then(to_screen) {
                painter.circle_stroke(at, 7.0, Stroke::new(2.0, tk.accent));
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if ui.input(|i| i.pointer.primary_clicked())
                && let Some(point) = hovered
            {
                match tab.point_to_point {
                    PointToPoint::Target(source) => {
                        tab.point_to_point = PointToPoint::Off;
                        let moved = model_place::point_to_point(&shown, source, point)?;
                        return tab.set(draft, &package.placement, moved, revision);
                    }
                    _ => tab.point_to_point = PointToPoint::Target(point),
                }
            }
            return None;
        }
        let centre = vec3(package_fit::body_centre(&package, &shown));
        let step = mm_per_px(&proj, centre).map_or(0.0, |m| model_place::snap_step(m, STEP_PX));
        if viewport.display_limited() {
            painter.text(
                rect.center_top() + egui::vec2(0.0, tk.space_8),
                egui::Align2::CENTER_TOP,
                "This model is too large to follow the triad live; it moves when the drag ends.",
                egui::FontId::proportional(tk.font_small),
                tk.warning,
            );
        }
        // The triad's snap step, shown where it applies.
        crate::kit::canvas_hint(
            &painter,
            rect,
            tk.space_8,
            &format!(
                "Triad snaps {} mm · arrows nudge {} mm (Shift ×10)",
                crate::kit::number_text(step),
                model_place::NUDGE_MM
            ),
        );
        // Arrow-key nudges over the view while no field has the keyboard:
        // each press is one command.
        let hovered = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| rect.contains(p));
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if hovered && !typing && tab.drag.is_none() {
            let pressed = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => model_place::nudge(*key, modifiers.shift),
                    _ => None,
                })
            });
            if let Some(shift_by) = pressed {
                let moved = package_fit::moved(&shown, &Transform3::translation(shift_by)).ok()?;
                return tab.set(draft, &package.placement, moved, revision);
            }
        }
        let triad = Triad {
            center: centre,
            translate: true,
            rotate: true,
        };
        let id = ui.id().with("model-triad");
        let Some(drag) = gizmo::show(ui, &painter, &proj, id, &triad, step) else {
            // The button went up outside the view: end the drag as released.
            if !ui.input(|i| i.pointer.any_down()) {
                tab.cancelled = false;
                if let Some(drag) = tab.drag.take() {
                    return tab.set(draft, &package.placement, drag.preview, revision);
                }
            }
            return None;
        };
        ui.ctx().request_repaint();
        if drag.started {
            tab.cancelled = false;
            tab.fields = None;
            tab.drag = Some(Drag {
                start: shown,
                preview: shown,
            });
        }
        if tab.cancelled {
            tab.cancelled = !drag.done;
            return None;
        }
        let Some(current) = &mut tab.drag else {
            return None;
        };
        if escape {
            tab.drag = None;
            tab.cancelled = !drag.done;
            return None;
        }
        if let Some(preview) = model_place::dragged(&package, &current.start, drag.change) {
            current.preview = preview;
        }
        if drag.done {
            let preview = current.preview;
            tab.drag = None;
            return tab.set(draft, &package.placement, preview, revision);
        }
        None
    }
}

/// Clear the Part view's display transform and overlay from a viewport
/// about to show a board.
pub fn reset_model_view(viewport: &mut Viewport) {
    viewport.set_model_transform(Transform3::IDENTITY);
    if viewport.has_overlay() {
        viewport.set_overlay(None);
    }
}

fn dashed(painter: &egui::Painter, a: Pos2, b: Pos2, color: Color32) {
    let length = a.distance(b);
    if length < 1.0 {
        return;
    }
    let dash = 6.0;
    let steps = ((length / dash) as usize).min(512);
    for k in (0..steps).step_by(2) {
        let t0 = k as f32 / steps as f32;
        let t1 = ((k + 1) as f32 / steps as f32).min(1.0);
        painter.line_segment([a.lerp(b, t0), a.lerp(b, t1)], Stroke::new(1.5, color));
    }
}
