//! Properties for the Symbol view through the card kit: the armed tool's
//! card rows, the SYMBOL unit selector, the grid and snap popover, and the
//! exact values of a selected pin's drawing (PIN · Graphic) or of one shape
//! or text (GRAPHIC, TEXT). Widgets edit the working copy; the editor
//! commits once the field loses focus, one undo step per edit.
use super::PartEditor;
use super::drawing::Tool;
use crate::kit;
use egui::Ui;
use pcbcraft_core::geometry::{Shape, fit_corner_radii};
use pcbcraft_core::library::graphics::{
    Fill, HAlign, LINE_SPACING, MAX_PIN_LENGTH_MM, MAX_STROKE_MM, TEXT_HEIGHT_MM, TextContent,
    VAlign, printable_text, upright,
};
use pcbcraft_core::library::symbol_style::{ResistorStyle, SymbolConventions};
use pcbcraft_core::library::{Grid, Item, PinDirection, Rotation, ShapeNm};
use pcbcraft_core::units::{Nm, PointNm};
use pcbcraft_core::{Engine, GraphicId, LibraryCommand, Operation, TerminalId, UnitId};
use solidcraft_ui_egui::ui_kit::{self, Button};

const COORD: std::ops::RangeInclusive<f64> = -10000.0..=10000.0;
const RADIUS: std::ops::RangeInclusive<f64> = 0.0..=10000.0;
/// Grid steps the selector offers, mm (metric; 2.5 mm is the default).
const GRID_STEPS: [f64; 6] = [0.5, 1.0, 1.25, 2.5, 5.0, 10.0];
/// The grid popover's width, in points.
const GRID_POPOVER_WIDTH: f32 = 260.0;

fn fill_options() -> Vec<(Fill, String)> {
    vec![
        (Fill::None, "None".into()),
        (Fill::Body, "Body".into()),
        (Fill::Solid, "Solid".into()),
    ]
}

/// The header icon of a shape, as its drawing tool shows it.
pub(super) fn shape_icon(shape: &Shape) -> &'static str {
    match shape {
        Shape::Line { .. } => "line",
        Shape::Polyline { .. } => "trace",
        Shape::Rect { .. } => "rect",
        Shape::Circle { .. } => "circle",
        Shape::Arc { .. } => "arc_center",
        Shape::Polygon { .. } => "polygon",
    }
}

pub(super) fn shape_name(shape: &Shape) -> &'static str {
    match shape {
        Shape::Line { .. } => "Line",
        Shape::Polyline { .. } => "Polyline",
        Shape::Rect { .. } => "Rectangle",
        Shape::Circle { .. } => "Circle",
        Shape::Arc { .. } => "Arc",
        Shape::Polygon { .. } => "Polygon",
    }
}

/// The rows `add` draws, as one hover response to mark with
/// [`kit::field_error`].
fn row_response(ui: &mut Ui, salt: &str, add: impl FnOnce(&mut Ui)) -> egui::Response {
    let top = ui.cursor().top();
    add(ui);
    let rect = egui::Rect::from_min_max(
        egui::pos2(ui.max_rect().left(), top),
        egui::pos2(ui.max_rect().right(), ui.cursor().top()),
    );
    ui.interact(rect, ui.scope_id().with(salt), egui::Sense::hover())
}

/// Points and per-vertex corner radii of a path, one row each. Returns the
/// last radius row, where a radius that does not fit is marked.
fn path_rows(
    ui: &mut Ui,
    points: &mut [[f64; 2]],
    radii: &mut Vec<f64>,
    closed: bool,
) -> Option<egui::Response> {
    let n = points.len();
    if radii.len() != n {
        *radii = vec![0.0; n];
    }
    let mut last = None;
    for i in 0..n {
        kit::pair_row(ui, &format!("Point {}", i + 1), &mut points[i], "mm", COORD);
        let end = !closed && (i == 0 || i + 1 == n);
        if !end {
            last = Some(row_response(ui, &format!("radius-{i}"), |ui| {
                kit::number_row(
                    ui,
                    &format!("Radius {}", i + 1),
                    &mut radii[i],
                    "mm",
                    RADIUS,
                );
            }));
        }
    }
    if radii.iter().all(|r| *r == 0.0) {
        radii.clear();
    }
    last
}

impl PartEditor {
    /// The armed drawing tool's options, the rows of its tool card:
    /// stroke, corner radius, fill, what a Field shows, the next pin, and
    /// a chain's points so far.
    pub(super) fn tool_rows(&mut self, ui: &mut Ui, engine: &Engine) {
        let tool = self.drawing.tool();
        if tool == Tool::Select {
            return;
        }
        let field_options = self.field_options(engine);
        let field_now = self.field_content(engine);
        let pending = self.drawing.pending().len();
        if !matches!(tool, Tool::Text | Tool::Field | Tool::Pin) {
            kit::number_row(
                ui,
                "Stroke",
                &mut self.stroke_width,
                "mm",
                0.0..=MAX_STROKE_MM,
            );
        }
        if let Some(radius) = self.drawing.corner_radius_mut() {
            kit::number_row(ui, "Corner radius", radius, "mm", 0.0..=50.0);
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
        if tool == Tool::Pin {
            kit::value_row(ui, "Next pin", &self.next_pin());
        }
        if tool.is_chain() {
            kit::value_row(ui, "Points", &pending.to_string());
        }
    }

    /// The terminal the Pin tool places next: the first without a pin, or
    /// a new one.
    fn next_pin(&self) -> String {
        let Some(set) = &self.draft else {
            return String::new();
        };
        set.component
            .terminals
            .iter()
            .find(|t| set.symbol.pin(t.id).is_none())
            .map_or_else(|| "A new terminal".to_owned(), |t| t.name.clone())
    }

    /// Act on the tool card's buttons: Done returns to Select, Cancel drops
    /// the chain, Finish ends it.
    pub(super) fn tool_card_response(&mut self, response: kit::ToolCardResponse) {
        if response.finish {
            self.finish_chain();
        }
        if response.cancel {
            self.drawing.cancel();
        }
        if response.done {
            self.arm_tool(Tool::Select);
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
        kit::section(ui, "symbol-unit", "Symbol", true, None, |ui| {
            if let Some(id) = &mut current {
                kit::row(ui, "Unit", |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        add = ui_kit::button(ui, "Add unit", Button::Small)
                            .on_hover_text("Add a unit, such as one gate of a quad part")
                            .clicked();
                        ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
                        changed = kit::dropdown(ui, "symbol-unit-choice", id, &options);
                    });
                });
            }
            if let Some(unit) = set.symbol.units.get_mut(index) {
                kit::text_row(ui, "Label", &mut unit.label)
                    .on_hover_text("The unit's letter, shown after the designator (U1A)");
            }
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

    /// The grid popover's rows: the symbol grid step (an undoable library
    /// setting), snapping and grid display.
    pub(super) fn grid_rows(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let grid = engine.drafts().grid;
        let mut step = grid.symbol_mm();
        let mut options: Vec<(f64, String)> = GRID_STEPS
            .iter()
            .map(|s| (*s, format!("{} mm", kit::number_text(*s))))
            .collect();
        if !GRID_STEPS.contains(&step) {
            options.push((step, format!("{} mm", kit::number_text(step))));
        }
        let mut operation = None;
        if kit::choice_row(ui, "Symbol grid", &mut step, &options)
            && let Ok(symbol_nm) = Nm::from_mm_f64_rounded(step)
        {
            operation = Some(Operation::Library(LibraryCommand::SetGrid {
                grid: Grid { symbol_nm, ..grid }.into(),
            }));
        }
        let conventions = engine.drafts().symbols;
        let mut resistor = conventions.resistor;
        let styles: Vec<(ResistorStyle, String)> = [ResistorStyle::Iec, ResistorStyle::Us]
            .into_iter()
            .map(|style| (style, style.label().to_owned()))
            .collect();
        if kit::choice_row(ui, "Resistor symbol", &mut resistor, &styles) {
            operation = Some(Operation::Library(LibraryCommand::SetSymbolConventions {
                conventions: SymbolConventions { resistor },
            }));
        }
        kit::check_row(ui, "Snap", &mut self.snap);
        kit::check_row(ui, "Show grid", &mut self.show_grid);
        kit::hint(
            ui,
            "Hold Cmd/Ctrl or Alt to place without snapping",
            "Snap finds ends, pins, midpoints, centres, curves, alignment and the grid",
        );
        operation
    }

    /// The grid button beside the canvas navigation bar, and its popover.
    /// Returns the grid step change, if any.
    pub(super) fn grid_popover(
        &mut self,
        ui: &mut Ui,
        canvas: egui::Rect,
        engine: &Engine,
    ) -> Option<Operation> {
        let t = solidcraft_theme::Tokens::get();
        // The navigation bar is one button wide, centred at the canvas
        // bottom (app/chrome.rs `navigation_bar`); this bar sits right of it.
        let size = egui::vec2(
            t.control_height + 2.0 * t.space_4,
            t.control_height + 2.0 * t.space_4,
        );
        let nav_half = (t.control_height + 2.0 * t.space_4) / 2.0;
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                canvas.center().x + nav_half + t.space_8,
                canvas.bottom() - t.space_16 - size.y,
            ),
            size,
        );
        if !canvas.contains_rect(rect) {
            return None;
        }
        ui.painter().rect(
            rect,
            t.radius_lg,
            t.toolbar,
            t.container_border(t.border),
            egui::StrokeKind::Inside,
        );
        let mut operation = None;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink(t.space_4))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let step = kit::number_text(engine.drafts().grid.symbol_mm());
        let button = crate::shell::icon_button(
            &mut child,
            "grid",
            &format!("Grid and snap · {step} mm"),
            true,
        );
        solidcraft_ui_egui::ui_kit::Container::new(
            ui.ctx(),
            solidcraft_ui_egui::ui_kit::Surface::Popup,
        )
        .menu(&button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            kit::form(ui, |ui| {
                ui.set_width(GRID_POPOVER_WIDTH);
                operation = self.grid_rows(ui, engine);
            });
        });
        operation
    }

    /// One selected shape (GRAPHIC) or text (TEXT): its exact values.
    pub(super) fn graphic_card(&mut self, ui: &mut Ui, engine: &Engine, id: GraphicId) {
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
            Item::Shape {
                shape: stored,
                width_nm,
                fill,
            } => {
                // Edited in millimetres; stored back rounded to nanometres.
                let mut mm = stored.to_mm();
                let mut width = width_nm.to_mm_f64();
                let shape = &mut mm;
                let width = &mut width;
                kit::section(ui, "symbol-shape", "Graphic", true, None, |ui| {
                    let radius = shape_rows(ui, shape);
                    if !shape.corners_fit() {
                        // The one invalid value sits in the radius field:
                        // marked there, with the fix beside it.
                        if let Some(radius) = &radius {
                            kit::field_error(ui, radius, "Too large for its edges", false);
                        }
                        kit::actions(ui, |ui| {
                            if ui_kit::button(ui, "Fit radii", Button::Normal)
                                .on_hover_text("Reduce each corner radius to what its edges allow")
                                .clicked()
                            {
                                fit_radii(shape);
                            }
                        });
                    }
                    kit::number_row(ui, "Stroke", width, "mm", 0.0..=MAX_STROKE_MM);
                    if shape.is_closed() {
                        kit::choice_row(ui, "Fill", fill, &fill_options());
                    }
                });
                if *shape != stored.to_mm()
                    && let Ok(s) = ShapeNm::from_mm_rounded(shape)
                {
                    *stored = s;
                }
                if let Ok(w) = Nm::from_mm_f64_rounded(*width) {
                    *width_nm = w;
                }
            }
            Item::Text(text) => {
                kit::section(ui, "symbol-text", "Text", true, None, |ui| {
                    let mut kind = match &text.content {
                        TextContent::Literal(_) => None,
                        other => Some(other.clone()),
                    };
                    let mut kinds: Vec<(Option<TextContent>, String)> = vec![(None, "Text".into())];
                    kinds.extend(field_options.into_iter().map(|(c, l)| (Some(c), l)));
                    if kit::choice_row(ui, "Shows", &mut kind, &kinds) {
                        text.content = kind.unwrap_or(TextContent::Literal("Text".into()));
                    }
                    if let TextContent::Literal(s) = &mut text.content {
                        let invalid = !printable_text(s);
                        let response = kit::multiline_row(ui, "Content", s, invalid)
                            .on_hover_text("Enter breaks the line; up to 16 lines");
                        if focus {
                            response.request_focus();
                        }
                    }
                    let mut at = text.at.to_mm_f64();
                    if kit::pair_row(ui, "Anchor", &mut at, "mm", COORD)
                        && let Ok(at) = PointNm::from_mm_f64_rounded(at)
                    {
                        text.at = at;
                    }
                    let mut height = text.height_nm.to_mm_f64();
                    if kit::number_row(ui, "Height", &mut height, "mm", TEXT_HEIGHT_MM)
                        && let Ok(height) = Nm::from_mm_f64_rounded(height)
                    {
                        text.height_nm = height;
                    }
                    kit::number_row(ui, "Line spacing", &mut text.line_spacing, "", LINE_SPACING);
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
                    // The upright rule turns text that would read
                    // downward or leftward and flips its alignment;
                    // say so where the stored value no longer shows.
                    let (_, shown) = upright(text.rotation.degrees(), text.justify);
                    if shown != text.justify {
                        let name = |h| match h {
                            HAlign::Left => "left",
                            HAlign::Center => "centre",
                            HAlign::Right => "right",
                        };
                        let vname = |v| match v {
                            VAlign::Top => "top",
                            VAlign::Middle => "middle",
                            VAlign::Bottom => "bottom",
                        };
                        kit::hint(
                            ui,
                            &format!(
                                "Shown {}-{} aligned so it reads upright",
                                vname(shown.v),
                                name(shown.h)
                            ),
                            "Text never reads downward or leftward; its alignment flips instead",
                        );
                    }
                    kit::check_row(ui, "Visible", &mut text.visible);
                });
            }
        }
    }

    /// A selected pin's drawing: PIN · Graphic.
    pub(super) fn pin_graphic_section(&mut self, ui: &mut Ui, terminal: TerminalId) {
        if let Some(pin) = self.draft.as_mut().and_then(|s| {
            s.symbol
                .units
                .iter_mut()
                .flat_map(|u| u.pins.iter_mut())
                .find(|p| p.terminal == terminal)
        }) {
            kit::section(ui, "symbol-pin", "Pin · Graphic", true, None, |ui| {
                let mut end = pin.position_mm();
                if kit::pair_row(ui, "Pin end", &mut end, "mm", COORD)
                    && let Ok(end) = PointNm::from_mm_f64_rounded(end)
                {
                    pin.position = end;
                }
                let directions = [
                    (PinDirection::Right, "Right (body to the right)".to_owned()),
                    (PinDirection::Left, "Left".into()),
                    (PinDirection::Up, "Up".into()),
                    (PinDirection::Down, "Down".into()),
                ];
                kit::choice_row(ui, "Direction", &mut pin.direction, &directions);
                let mut length = pin.length_mm();
                if kit::number_row(ui, "Length", &mut length, "mm", 0.0..=MAX_PIN_LENGTH_MM)
                    && let Ok(length) = Nm::from_mm_f64_rounded(length)
                {
                    pin.length_nm = length;
                }
                kit::check_row(ui, "Inverted (bubble)", &mut pin.decorations.inverted);
                kit::check_row(ui, "Clock (wedge)", &mut pin.decorations.clock);
                kit::check_row(ui, "Show name", &mut pin.name.visible);
                kit::check_row(ui, "Show number", &mut pin.number.visible);
            });
        }
    }

    /// Bring the selection into the Symbol view, keeping its scale: the
    /// view centres on it only when part of it is off the canvas, so a
    /// visible object never jumps. The fitted view is left alone.
    pub(super) fn frame_selection(&mut self) {
        let bounds = self
            .unit_index()
            .and_then(|i| self.draft.as_ref()?.symbol.units.get(i))
            .and_then(|unit| super::arrange::bounds(unit, &self.selection));
        let size = self.symbol_canvas_size;
        if let (Some(bounds), Some(view)) = (bounds, self.symbol_view.as_mut()) {
            let screen = |p: [f64; 2]| {
                view.pan + egui::vec2(p[0] as f32 * view.scale, -(p[1] as f32) * view.scale)
            };
            let visible = size.is_some_and(|size| {
                let half = size / 2.0 - egui::vec2(16.0, 16.0);
                [bounds.min, bounds.max].into_iter().all(|corner| {
                    let at = screen(corner);
                    at.x.abs() <= half.x && at.y.abs() <= half.y
                })
            });
            if !visible {
                let [x, y] = bounds.center();
                view.pan = egui::vec2(-(x as f32) * view.scale, y as f32 * view.scale);
            }
        }
    }
}

/// Rows for a shape's points and size. Returns the corner-radius row, if
/// the shape has one.
fn shape_rows(ui: &mut Ui, shape: &mut Shape) -> Option<egui::Response> {
    match shape {
        Shape::Line { a, b } => {
            kit::pair_row(ui, "Start", a, "mm", COORD);
            kit::pair_row(ui, "End", b, "mm", COORD);
            None
        }
        Shape::Rect { a, b, radius } => {
            kit::pair_row(ui, "Corner", a, "mm", COORD);
            kit::pair_row(ui, "Opposite corner", b, "mm", COORD);
            Some(row_response(ui, "radius", |ui| {
                kit::number_row(ui, "Corner radius", radius, "mm", RADIUS);
            }))
        }
        Shape::Circle { center, radius } => {
            kit::pair_row(ui, "Centre", center, "mm", COORD);
            kit::number_row(ui, "Radius", radius, "mm", RADIUS);
            None
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
            None
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
