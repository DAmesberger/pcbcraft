//! Properties for the Symbol view through the card kit: the armed tool's
//! options, the unit selector, grid and snap, and exact values for the
//! selection (shape points and corner radii, text content and layout, pin
//! direction, length and decorations). Widgets edit the working copy; the
//! editor commits once the field loses focus, one undo step per edit.
use super::PartEditor;
use super::drawing::Tool;
use crate::kit;
use egui::Ui;
use pcbcraft_core::geometry::{Shape, fit_corner_radii};
use pcbcraft_core::library::graphics::{
    Fill, HAlign, LINE_SPACING, MAX_PIN_LENGTH_MM, MAX_STROKE_MM, TEXT_HEIGHT_MM, TextContent,
    VAlign, printable_text,
};
use pcbcraft_core::library::{Grid, Item, PinDirection, Rotation};
use pcbcraft_core::{Engine, LibraryCommand, Operation, UnitId};
use solidcraft_ui_egui::ui_kit::{self, Button};

const COORD: std::ops::RangeInclusive<f64> = -10000.0..=10000.0;
const RADIUS: std::ops::RangeInclusive<f64> = 0.0..=10000.0;
/// Grid steps the selector offers, mm (metric; 2.5 mm is the default).
const GRID_STEPS: [f64; 6] = [0.5, 1.0, 1.25, 2.5, 5.0, 10.0];

fn fill_options() -> Vec<(Fill, String)> {
    vec![
        (Fill::None, "None".into()),
        (Fill::Body, "Body".into()),
        (Fill::Solid, "Solid".into()),
    ]
}

fn shape_name(shape: &Shape) -> &'static str {
    match shape {
        Shape::Line { .. } => "Line",
        Shape::Polyline { .. } => "Polyline",
        Shape::Rect { .. } => "Rectangle",
        Shape::Circle { .. } => "Circle",
        Shape::Arc { .. } => "Arc",
        Shape::Polygon { .. } => "Polygon",
    }
}

/// Points and per-vertex corner radii of a path, one row each.
fn path_rows(ui: &mut Ui, points: &mut [[f64; 2]], radii: &mut Vec<f64>, closed: bool) {
    let n = points.len();
    if radii.len() != n {
        *radii = vec![0.0; n];
    }
    for i in 0..n {
        kit::pair_row(ui, &format!("Point {}", i + 1), &mut points[i], "mm", COORD);
        let end = !closed && (i == 0 || i + 1 == n);
        if !end {
            kit::number_row(
                ui,
                &format!("Radius {}", i + 1),
                &mut radii[i],
                "mm",
                RADIUS,
            );
        }
    }
    if radii.iter().all(|r| *r == 0.0) {
        radii.clear();
    }
}

impl PartEditor {
    /// Options of the armed drawing tool.
    pub(super) fn tool_section(&mut self, ui: &mut Ui, engine: &Engine) {
        let tool = self.drawing.tool();
        if tool == Tool::Select {
            return;
        }
        let field_options = self.field_options(engine);
        let field_now = self.field_content(engine);
        let pending = self.drawing.pending().len();
        let mut finish = false;
        let mut cancel = false;
        kit::section(
            ui,
            "draw-tool",
            &format!("{} tool", tool.label()),
            true,
            None,
            |ui| {
                kit::note(ui, tool.tip());
                if !matches!(tool, Tool::Text | Tool::Field | Tool::Pin) {
                    kit::number_row(
                        ui,
                        "Stroke",
                        &mut self.stroke_width,
                        "mm",
                        0.0..=MAX_STROKE_MM,
                    );
                }
                if matches!(
                    tool,
                    Tool::RoundedRect | Tool::Polyline | Tool::Polygon | Tool::Rect
                ) {
                    kit::number_row(
                        ui,
                        "Corner radius",
                        &mut self.drawing.corner_radius,
                        "mm",
                        0.0..=50.0,
                    );
                }
                if matches!(
                    tool,
                    Tool::Rect | Tool::RoundedRect | Tool::Circle | Tool::Polygon
                ) {
                    kit::choice_row(ui, "Fill", &mut self.fill, &fill_options());
                }
                if tool == Tool::Field {
                    let mut chosen = field_now.clone();
                    if kit::choice_row(ui, "Shows", &mut chosen, &field_options) {
                        self.field_choice = Some(chosen);
                    }
                }
                if tool.is_chain() {
                    kit::value_row(ui, "Points", &pending.to_string());
                    kit::actions(ui, |ui| {
                        let needed = if tool == Tool::Polygon { 3 } else { 2 };
                        if kit::primary(ui, "Finish", pending >= needed).clicked() {
                            finish = true;
                        }
                        if ui_kit::button(ui, "Cancel", Button::Normal).clicked() {
                            cancel = true;
                        }
                    });
                }
            },
        );
        if finish {
            self.finish_chain();
        }
        if cancel {
            self.drawing.cancel();
        }
    }

    /// The unit being drawn, and adding one.
    pub(super) fn unit_section(&mut self, ui: &mut Ui) {
        let Some(index) = self.unit_index() else {
            return;
        };
        let Some(set) = self.draft.as_mut() else {
            return;
        };
        let options: Vec<(UnitId, String)> = set
            .symbol
            .units
            .iter()
            .map(|u| (u.id, format!("Unit {}", u.label)))
            .collect();
        let mut current = set.symbol.units.get(index).map(|u| u.id);
        let mut add = false;
        let mut changed = false;
        kit::section(ui, "symbol-unit", "Unit", true, Some(options.len()), |ui| {
            if let Some(id) = &mut current {
                changed = kit::choice_row(ui, "Drawing", id, &options);
            }
            if let Some(unit) = set.symbol.units.get_mut(index) {
                kit::text_row(ui, "Label", &mut unit.label);
            }
            kit::actions(ui, |ui| {
                if ui_kit::button(ui, "Add unit", Button::Normal).clicked() {
                    add = true;
                }
            });
        });
        if changed {
            self.unit = current;
            self.selection.clear();
            self.symbol_view = None;
        }
        if add {
            self.add_unit();
        }
    }

    /// Grid step (an undoable library setting) and snapping.
    pub(super) fn grid_section(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let grid = engine.drafts().grid;
        let mut step = grid.symbol_mm;
        let mut options: Vec<(f64, String)> = GRID_STEPS
            .iter()
            .map(|s| (*s, format!("{} mm", kit::number_text(*s))))
            .collect();
        if !GRID_STEPS.contains(&step) {
            options.push((step, format!("{} mm", kit::number_text(step))));
        }
        let mut operation = None;
        kit::section(ui, "symbol-grid", "Grid and snap", false, None, |ui| {
            if kit::choice_row(ui, "Symbol grid", &mut step, &options) {
                operation = Some(Operation::Library(LibraryCommand::SetGrid {
                    grid: Grid {
                        symbol_mm: step,
                        ..grid
                    },
                }));
            }
            kit::check_row(ui, "Snap to points, alignment and grid", &mut self.snap);
            kit::check_row(ui, "Show grid", &mut self.show_grid);
            kit::note(ui, "Hold Cmd/Ctrl or Alt to place without snapping.");
        });
        operation
    }

    /// Exact values of the selection.
    pub(super) fn selection_section(&mut self, ui: &mut Ui, engine: &Engine) {
        let count = self.selection.len();
        if count == 0 {
            kit::note(
                ui,
                "Click to select; Shift adds. Drag right for a window, left for a crossing box.",
            );
            return;
        }
        if count > 1 {
            kit::section(
                ui,
                "symbol-selection",
                "Selection",
                true,
                Some(count),
                |ui| {
                    kit::value_row(ui, "Objects", &count.to_string());
                    kit::note(
                        ui,
                        "Drag to move; Rotate, Mirror, Duplicate and Delete act on all.",
                    );
                },
            );
            return;
        }
        if let Some(id) = self.selected_graphic() {
            let field_options = self.field_options(engine);
            let focus = std::mem::take(&mut self.focus_text);
            let Some(index) = self.unit_index() else {
                return;
            };
            let Some(graphic) = self
                .draft
                .as_mut()
                .and_then(|s| s.symbol.units.get_mut(index))
                .and_then(|u| u.graphics.iter_mut().find(|g| g.id == id))
            else {
                return;
            };
            match &mut graphic.item {
                Item::Shape { shape, width, fill } => {
                    let title = shape_name(shape);
                    kit::section(ui, "symbol-shape", title, true, None, |ui| {
                        shape_rows(ui, shape);
                        kit::number_row(ui, "Stroke", width, "mm", 0.0..=MAX_STROKE_MM);
                        if shape.is_closed() {
                            kit::choice_row(ui, "Fill", fill, &fill_options());
                        }
                        if !shape.corners_fit() {
                            kit::status(
                                ui,
                                "A corner radius is larger than its edges allow",
                                kit::Tone::Error,
                            );
                            kit::actions(ui, |ui| {
                                if ui_kit::button(ui, "Fit radii", Button::Normal).clicked() {
                                    fit_radii(shape);
                                }
                            });
                        }
                    });
                }
                Item::Text(text) => {
                    kit::section(ui, "symbol-text", "Text", true, None, |ui| {
                        let mut kind = match &text.content {
                            TextContent::Literal(_) => None,
                            other => Some(other.clone()),
                        };
                        let mut kinds: Vec<(Option<TextContent>, String)> =
                            vec![(None, "Text".into())];
                        kinds.extend(field_options.into_iter().map(|(c, l)| (Some(c), l)));
                        if kit::choice_row(ui, "Shows", &mut kind, &kinds) {
                            text.content = kind.unwrap_or(TextContent::Literal("Text".into()));
                        }
                        if let TextContent::Literal(s) = &mut text.content {
                            let invalid = !printable_text(s);
                            let response = kit::multiline_row(ui, "Content", s, invalid);
                            if focus {
                                response.request_focus();
                            }
                            kit::note(ui, "Enter breaks the line; up to 16 lines.");
                        }
                        kit::pair_row(ui, "Anchor", &mut text.at, "mm", COORD);
                        kit::number_row(ui, "Height", &mut text.height, "mm", TEXT_HEIGHT_MM);
                        kit::number_row(
                            ui,
                            "Line spacing",
                            &mut text.line_spacing,
                            "",
                            LINE_SPACING,
                        );
                        let rotations = [
                            (Rotation::R0, "0°".to_owned()),
                            (Rotation::R90, "90°".into()),
                            (Rotation::R180, "180°".into()),
                            (Rotation::R270, "270°".into()),
                        ];
                        kit::choice_row(ui, "Rotation", &mut text.rotation, &rotations);
                        let h = [
                            (HAlign::Left, "Left".to_owned()),
                            (HAlign::Center, "Centre".into()),
                            (HAlign::Right, "Right".into()),
                        ];
                        kit::choice_row(ui, "Justify", &mut text.justify.h, &h);
                        let v = [
                            (VAlign::Top, "Top".to_owned()),
                            (VAlign::Middle, "Middle".into()),
                            (VAlign::Bottom, "Bottom".into()),
                        ];
                        kit::choice_row(ui, "Vertical", &mut text.justify.v, &v);
                        kit::check_row(ui, "Visible", &mut text.visible);
                    });
                }
            }
            return;
        }
        if let Some(terminal) = self.selected_pin()
            && let Some(pin) = self.draft.as_mut().and_then(|s| {
                s.symbol
                    .units
                    .iter_mut()
                    .flat_map(|u| u.pins.iter_mut())
                    .find(|p| p.terminal == terminal)
            })
        {
            kit::section(ui, "symbol-pin", "Pin", true, None, |ui| {
                kit::pair_row(ui, "Pin end", &mut pin.position, "mm", COORD);
                let directions = [
                    (PinDirection::Right, "Right (body to the right)".to_owned()),
                    (PinDirection::Left, "Left".into()),
                    (PinDirection::Up, "Up".into()),
                    (PinDirection::Down, "Down".into()),
                ];
                kit::choice_row(ui, "Direction", &mut pin.direction, &directions);
                kit::number_row(ui, "Length", &mut pin.length, "mm", 0.0..=MAX_PIN_LENGTH_MM);
                kit::check_row(ui, "Inverted (bubble)", &mut pin.decorations.inverted);
                kit::check_row(ui, "Clock (wedge)", &mut pin.decorations.clock);
                kit::check_row(ui, "Show name", &mut pin.name.visible);
                kit::check_row(ui, "Show number", &mut pin.number.visible);
            });
        }
    }
}

/// Rows for a shape's points and size.
fn shape_rows(ui: &mut Ui, shape: &mut Shape) {
    match shape {
        Shape::Line { a, b } => {
            kit::pair_row(ui, "Start", a, "mm", COORD);
            kit::pair_row(ui, "End", b, "mm", COORD);
        }
        Shape::Rect { a, b, radius } => {
            kit::pair_row(ui, "Corner", a, "mm", COORD);
            kit::pair_row(ui, "Opposite corner", b, "mm", COORD);
            kit::number_row(ui, "Corner radius", radius, "mm", RADIUS);
        }
        Shape::Circle { center, radius } => {
            kit::pair_row(ui, "Centre", center, "mm", COORD);
            kit::number_row(ui, "Radius", radius, "mm", RADIUS);
        }
        Shape::Arc {
            center,
            radius,
            start_deg,
            sweep_deg,
        } => {
            kit::pair_row(ui, "Centre", center, "mm", COORD);
            kit::number_row(ui, "Radius", radius, "mm", RADIUS);
            kit::number_row(ui, "Start", start_deg, "°", -360.0..=360.0);
            kit::number_row(ui, "Sweep", sweep_deg, "°", -360.0..=360.0);
        }
        Shape::Polyline { points, radii } => path_rows(ui, points, radii, false),
        Shape::Polygon { points, radii } => path_rows(ui, points, radii, true),
    }
}

/// Reduce a shape's corner radii to what fits.
fn fit_radii(shape: &mut Shape) {
    match shape {
        Shape::Rect { a, b, radius } => {
            let half = (a[0] - b[0]).abs().min((a[1] - b[1]).abs()) / 2.0;
            *radius = radius.min(half * (1.0 - 1e-9));
        }
        Shape::Polyline { points, radii } => *radii = fit_corner_radii(points, radii, false),
        Shape::Polygon { points, radii } => *radii = fit_corner_radii(points, radii, true),
        _ => {}
    }
}
