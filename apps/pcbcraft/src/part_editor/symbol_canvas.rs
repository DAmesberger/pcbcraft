//! The Symbol view's canvas: drawing tools, snapping, selection, moving
//! and arranging on the working copy. Painting goes through
//! [`crate::symbol_view`] from what core resolves, so what is drawn is
//! what is picked. A finished gesture changes the working copy once; the
//! editor's `commit` sends it as one `library.edit` when the pointer is up,
//! so each tool action is one undo step and an unfinished shape never
//! reaches the document.
use super::arrange::{self, Selection};
use super::drawing::{Click, Placed, Tool};
use super::snap::{self, SNAP_PX, Snap, Targets};
use super::{PartEditor, Selected};
use crate::kit::severity::Level;
use crate::symbol_view;
use egui::{Align2, Pos2, Rect, Response, Stroke, Ui, pos2, vec2};
use pcbcraft_core::geometry::Bounds;
use pcbcraft_core::library::graphics::{
    self, Fill, HAlign, Justify, SYMBOL_STROKE_MM, TextContent, VAlign,
};
use pcbcraft_core::library::{Graphic, Item, Pin, PinDirection, SymbolHit, TextMetric, Unit};
use pcbcraft_core::schematic;
use pcbcraft_core::units::{Nm, PointNm};
use pcbcraft_core::{Engine, GraphicId, Point, TerminalId, UnitId};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::ui_kit::type_face;
use std::sync::Arc;

/// The unit being drawn as core resolves it, and its snap targets, for one
/// working copy at one engine revision.
pub(super) struct DrawnCache {
    revision: u64,
    unit: UnitId,
    set: super::working_set::WorkingSet,
    drawn: Arc<schematic::PlacedUnit>,
    /// Snap targets for the objects being moved and the pending points.
    targets: Option<(Selection, Vec<Point>, Arc<Targets>)>,
}

/// A graphic refused because the symbol ran out of local IDs.
const NO_MORE_GRAPHICS: &str = "No more graphics can be added to this symbol";

/// Pick tolerance around drawn symbol content, in pixels.
const PICK_PX: f32 = 6.0;
/// Grid dots closer than this many pixels are thinned out.
const GRID_MIN_PX: f32 = 9.0;

/// Pan and zoom of the Symbol view. Fitted once, then stable while
/// drawing so the view never jumps under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ViewState {
    /// Screen offset of the millimetre origin from the canvas centre.
    pub pan: egui::Vec2,
    /// Pixels per millimetre.
    pub scale: f32,
}

/// A drag in progress.
#[derive(Clone, Debug)]
pub(super) enum Gesture {
    /// Moving the selection: where the drag started and the unit before it.
    Move { start: Point, before: Box<Unit> },
    /// A selection box from `start` to `end`.
    Boxing { start: Point, end: Point },
}

/// A selection-wide arrange action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Arrange {
    Rotate,
    Mirror,
    Duplicate,
    Delete,
}

impl Arrange {
    pub const ALL: [Arrange; 4] = [
        Arrange::Rotate,
        Arrange::Mirror,
        Arrange::Duplicate,
        Arrange::Delete,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Rotate => "arrange-rotate",
            Self::Mirror => "arrange-mirror",
            Self::Duplicate => "arrange-duplicate",
            Self::Delete => "arrange-delete",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Rotate => "Rotate",
            Self::Mirror => "Mirror",
            Self::Duplicate => "Duplicate",
            Self::Delete => "Delete",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Rotate => "spin",
            Self::Mirror => "mirror",
            Self::Duplicate => "copy",
            Self::Delete => "delete",
        }
    }
    pub fn tip(self) -> &'static str {
        match self {
            Self::Rotate => "Turn the selection 90° counter-clockwise",
            Self::Mirror => "Mirror the selection left to right",
            Self::Duplicate => "Copy the selected graphics; pins are not copied",
            Self::Delete => "Remove the selected graphics and pins; terminals stay",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }
}

fn selected_of(hit: SymbolHit) -> Option<Selected> {
    match hit {
        SymbolHit::Pin(t) => Some(Selected::Pin(t)),
        SymbolHit::Graphic(g) => Some(Selected::Graphic(g)),
        SymbolHit::Field => None,
    }
}

/// Allocate one local ID from `next`.
fn allocate(next: &mut u32) -> Option<u32> {
    let id = *next;
    *next = next.checked_add(1)?;
    Some(id)
}

impl PartEditor {
    /// The index of the unit being drawn in the working copy.
    pub(super) fn unit_index(&self) -> Option<usize> {
        let set = self.draft.as_ref()?;
        if set.symbol.units.is_empty() {
            return None;
        }
        Some(
            self.unit
                .and_then(|id| set.symbol.units.iter().position(|u| u.id == id))
                .unwrap_or(0),
        )
    }

    fn current_unit(&self) -> Option<&Unit> {
        let index = self.unit_index()?;
        self.draft.as_ref()?.symbol.units.get(index)
    }

    fn current_unit_mut(&mut self) -> Option<&mut Unit> {
        let index = self.unit_index()?;
        self.draft.as_mut()?.symbol.units.get_mut(index)
    }

    /// Select exactly `selection`; a single pin also becomes the terminal
    /// the inspector shows.
    pub(super) fn select(&mut self, selection: Selection) {
        if let [Selected::Pin(t)] = selection.iter().copied().collect::<Vec<_>>()[..] {
            self.selected = Some(t);
        }
        self.selection = selection;
    }

    /// Keep the selection to objects that still exist (after undo, delete
    /// or a reload).
    pub(super) fn prune_selection(&mut self) {
        let Some(unit) = self.current_unit() else {
            self.selection.clear();
            return;
        };
        let keep: Selection = self
            .selection
            .iter()
            .copied()
            .filter(|s| match s {
                Selected::Pin(t) => unit.pin(*t).is_some(),
                Selected::Graphic(g) => unit.graphic(*g).is_some(),
            })
            .collect();
        self.selection = keep;
    }

    /// Run an arrange action on the selection.
    pub(super) fn arrange(&mut self, action: Arrange, grid: f64) {
        let selection = self.selection.clone();
        // The ribbon disables Arrange without a selection; a key does
        // nothing then.
        if selection.is_empty() {
            return;
        }
        let Some(index) = self.unit_index() else {
            return;
        };
        let Some(set) = self.draft.as_mut() else {
            return;
        };
        match action {
            Arrange::Rotate | Arrange::Mirror => {
                let Some(unit) = set.symbol.units.get_mut(index) else {
                    return;
                };
                let Some(about) = arrange::pivot(unit, &selection, grid) else {
                    return;
                };
                if action == Arrange::Rotate {
                    arrange::rotate(unit, &selection, about);
                } else {
                    arrange::mirror(unit, &selection, about);
                }
            }
            Arrange::Duplicate => {
                match arrange::duplicate(&mut set.symbol, index, &selection, [grid, -grid]) {
                    Some((copies, skipped)) => {
                        self.selection = copies;
                        if skipped > 0 {
                            let pins = if skipped == 1 { "pin" } else { "pins" };
                            self.tell(
                                Level::Warning,
                                format!("{skipped} {pins} not duplicated: a terminal has one pin"),
                            );
                        }
                    }
                    None => self.tell(Level::Error, NO_MORE_GRAPHICS),
                }
            }
            Arrange::Delete => {
                if let Some(unit) = set.symbol.units.get_mut(index) {
                    arrange::delete(unit, &selection);
                }
                self.selection.clear();
            }
        }
    }

    /// Why a gesture beyond the ±2 m a drawing stores adds nothing.
    const BEYOND_RANGE: &str = "That is beyond the ±2 m a drawing holds";

    /// Add what a finished tool gesture produced to the current unit.
    fn place(&mut self, placed: Placed, engine: &Engine) {
        let Some(index) = self.unit_index() else {
            self.tell(Level::Error, "Add a terminal or load a template first");
            return;
        };
        let width = self.stroke_width;
        let field = self.field_content(engine);
        let item = match placed {
            Placed::Shape(shape) => {
                if shape.stored_points() >= graphics::MAX_SHAPE_POINTS {
                    self.tell(
                        Level::Info,
                        format!(
                            "Finished at {} points, the most one shape holds",
                            graphics::MAX_SHAPE_POINTS
                        ),
                    );
                }
                let fill = if shape.is_closed() {
                    self.fill
                } else {
                    Fill::None
                };
                let (Ok(shape), Ok(width_nm)) = (
                    graphics::ShapeNm::from_mm_rounded(&shape),
                    Nm::from_mm_f64_rounded(width),
                ) else {
                    self.tell(Level::Error, Self::BEYOND_RANGE);
                    return;
                };
                Item::Shape {
                    shape,
                    width_nm,
                    fill,
                }
            }
            Placed::Text(at) => {
                let Ok(text) = graphics::text(
                    GraphicId(0),
                    TextContent::Literal("Text".into()),
                    at,
                    Justify {
                        h: HAlign::Left,
                        v: VAlign::Middle,
                    },
                ) else {
                    self.tell(Level::Error, Self::BEYOND_RANGE);
                    return;
                };
                text.item
            }
            Placed::Field(at) => {
                let Ok(text) = graphics::text(
                    GraphicId(0),
                    field,
                    at,
                    Justify {
                        h: HAlign::Left,
                        v: VAlign::Middle,
                    },
                ) else {
                    self.tell(Level::Error, Self::BEYOND_RANGE);
                    return;
                };
                text.item
            }
            Placed::Pin(at) => {
                self.place_pin(index, at);
                return;
            }
        };
        let text = matches!(item, Item::Text(_));
        let Some(set) = self.draft.as_mut() else {
            return;
        };
        let Some(id) = allocate(&mut set.symbol.next_local) else {
            self.tell(Level::Error, NO_MORE_GRAPHICS);
            return;
        };
        let Some(unit) = set.symbol.units.get_mut(index) else {
            return;
        };
        unit.graphics.push(Graphic {
            id: GraphicId(id),
            item,
        });
        self.selection = [Selected::Graphic(GraphicId(id))].into_iter().collect();
        self.focus_text = text;
    }

    /// Place the next unplaced terminal's pin at `at`, or a new terminal's
    /// when every terminal has a pin. Its direction points into the body.
    fn place_pin(&mut self, index: usize, at: Point) {
        let unplaced = self.draft.as_ref().and_then(|set| {
            set.component
                .terminals
                .iter()
                .map(|t| t.id)
                .find(|t| set.symbol.pin(*t).is_none())
        });
        let terminal = match unplaced {
            Some(t) => t,
            None => match self.new_terminal() {
                Some(t) => t,
                None => return,
            },
        };
        let Some(unit) = self
            .draft
            .as_mut()
            .and_then(|set| set.symbol.units.get_mut(index))
        else {
            return;
        };
        // The drawn outline without its stroke, so pins on the grid end on
        // the grid.
        let body = unit
            .graphics
            .iter()
            .filter_map(|g| match &g.item {
                Item::Shape { shape, .. } => Some(shape.to_mm().bounds(0.0)),
                Item::Text(_) => None,
            })
            .reduce(Bounds::union);
        let pin = match body {
            Some(body) if !body.contains(at) => graphics::pin_toward(terminal, at, body),
            _ => PointNm::from_mm_f64_rounded(at)
                .map(|at| Pin::new(terminal, at, PinDirection::Right, graphics::SYMBOL_MODULE)),
        };
        let Ok(pin) = pin else {
            self.tell(Level::Error, Self::BEYOND_RANGE);
            return;
        };
        unit.pins.push(pin);
        self.selected = Some(terminal);
        self.selection = [Selected::Pin(terminal)].into_iter().collect();
    }

    /// The Field tool's content: the chosen one, else the first of the
    /// designator, the component name and the component's fields that the
    /// unit does not show yet.
    pub(super) fn field_content(&self, engine: &Engine) -> TextContent {
        if let Some(content) = &self.field_choice {
            return content.clone();
        }
        let shown: Vec<TextContent> = self
            .current_unit()
            .map(|u| {
                u.graphics
                    .iter()
                    .filter_map(|g| match &g.item {
                        Item::Text(t) => Some(t.content.clone()),
                        Item::Shape { .. } => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.field_options(engine)
            .into_iter()
            .map(|(c, _)| c)
            .find(|c| !shown.contains(c))
            .unwrap_or(TextContent::Designator)
    }

    /// What a field placeholder can show, with labels.
    pub(super) fn field_options(&self, engine: &Engine) -> Vec<(TextContent, String)> {
        let mut out = vec![
            (TextContent::Designator, "Designator".to_owned()),
            (TextContent::ComponentName, "Component name".to_owned()),
            (TextContent::UnitLabel, "Unit label".to_owned()),
        ];
        if let Some(set) = &self.draft {
            for selection in &set.component.fields {
                let name = engine
                    .project()
                    .library
                    .field(selection.field)
                    .map_or_else(|| selection.field.id.to_string(), |f| f.meta.name.clone());
                out.push((TextContent::Field(selection.field.id), name));
            }
        }
        out
    }

    /// The unit being drawn, as core resolves it; resolved again only when
    /// the working copy, the unit or the engine revision changed.
    fn drawn(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        engine: &Engine,
    ) -> Option<Arc<schematic::PlacedUnit>> {
        let set = self.draft.as_ref()?;
        let Some(unit) = self.current_unit() else {
            ui.put(
                rect,
                egui::Label::new("Add a terminal or load a template to start drawing"),
            );
            return None;
        };
        let unit = unit.id;
        if let Some(cache) = &self.drawn_cache
            && cache.revision == engine.revision()
            && cache.unit == unit
            && cache.set == *set
        {
            return Some(Arc::clone(&cache.drawn));
        }
        let drawn = Arc::new(
            schematic::definition_unit(
                &engine.project().library,
                schematic::DefinitionUnit {
                    symbol: &set.symbol,
                    unit,
                    component: &set.component,
                    component_name: &set.metas.component.name,
                    physical: Some((&set.binding, &set.package)),
                },
            )
            .ok()?,
        );
        self.drawn_cache = Some(DrawnCache {
            revision: engine.revision(),
            unit,
            set: set.clone(),
            drawn: Arc::clone(&drawn),
            targets: None,
        });
        Some(drawn)
    }

    /// Snap targets of the unit being drawn, skipping `moving`; built again
    /// only when the drawing, the moved objects or the pending points
    /// changed.
    fn targets(&mut self, moving: &Selection) -> Arc<Targets> {
        let pending = self.drawing.pending().to_vec();
        if let Some(cache) = &self.drawn_cache
            && let Some((m, p, targets)) = &cache.targets
            && m == moving
            && *p == pending
        {
            return Arc::clone(targets);
        }
        let targets = Arc::new(
            self.current_unit()
                .map(|u| Targets::of(u, |s| moving.contains(s), &pending))
                .unwrap_or_default(),
        );
        if let Some(cache) = &mut self.drawn_cache {
            cache.targets = Some((moving.clone(), pending, Arc::clone(&targets)));
        }
        targets
    }

    /// Keyboard: tools, arrange, finish, cancel and popping points. Plain
    /// letters only act while no text field has the keyboard.
    fn symbol_keys(&mut self, ui: &Ui, grid: f64) {
        use crate::keys::{pressed, pressed_tool};
        // A disabled canvas (the Details tab's preview) takes no keys.
        if !ui.is_enabled() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let ctx = ui.ctx().clone();
        // Undo pops a pending point before the app's undo sees it.
        if self.drawing.busy() && pressed(&ctx, "edit.undo") {
            self.drawing.undo_point();
        }
        for action in Arrange::ALL {
            if pressed_tool(&ctx, "symbol", action.id()) {
                self.arrange(action, grid);
            }
        }
        if pressed(&ctx, "symbol.repeat") && self.drawing.repeat() {
            self.selection.clear();
        }
        for tool in Tool::ALL {
            if pressed_tool(&ctx, "symbol", tool.id()) {
                self.arm_tool(tool);
            }
        }
        if pressed(&ctx, "symbol.finish") {
            self.finish_chain();
        }
        if pressed(&ctx, "symbol.cancel") {
            self.cancel_tool();
        }
    }

    /// Finish a polyline or polygon from its pending points.
    pub(super) fn finish_chain(&mut self) {
        if let Some(placed) = self.drawing.finish() {
            self.pending_place = Some(placed);
        }
    }

    /// Esc: end the chain, then the tool, then the selection.
    pub(super) fn cancel_tool(&mut self) {
        if !self.drawing.cancel() {
            self.selection.clear();
            self.terminal_card = None;
        }
        self.gesture = None;
    }

    /// The Symbol view: paint, snap, draw, select and move.
    pub(super) fn symbol_canvas(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        response: &Response,
        engine: &Engine,
    ) {
        let t = Tokens::get();
        let grid = engine.drafts().grid.symbol_mm();
        let m = TextMetric::default();
        self.symbol_canvas_size = Some(rect.size());
        self.prune_selection();
        if let Some(placed) = self.pending_place.take() {
            self.place(placed, engine);
        }
        let painter = ui.painter().with_clip_rect(rect);
        let Some(drawn) = self.drawn(ui, rect, engine) else {
            return;
        };
        // Fit once (about the origin), then keep the view stable.
        let view_state = *self.symbol_view.get_or_insert_with(|| {
            let (x, y) = drawn.bounds(m).map_or((2.5, 2.5), |b| {
                (
                    b.min[0].abs().max(b.max[0].abs()),
                    b.min[1].abs().max(b.max[1].abs()),
                )
            });
            let scale = (rect.width() as f64 / (2.0 * x + 6.0))
                .min(rect.height() as f64 / (2.0 * y + 6.0))
                .clamp(0.5, 80.0) as f32;
            ViewState {
                pan: egui::Vec2::ZERO,
                scale,
            }
        });
        let view = symbol_view::View {
            origin: rect.center() + view_state.pan,
            scale: view_state.scale,
        };
        let pointer = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p));
        // Navigation: scroll zooms about the pointer; a middle drag, or a
        // drag with Space held (Solidcraft's momentary pan), pans.
        // Space counts only when pressed on the canvas, never while typing:
        // a space typed into a field must not leave the canvas panning.
        let typing = ui.ctx().egui_wants_keyboard_input();
        ui.input(|i| {
            for event in &i.events {
                if let egui::Event::Key {
                    key: egui::Key::Space,
                    pressed,
                    repeat: false,
                    ..
                } = event
                {
                    self.space_held = *pressed && !typing;
                }
            }
        });
        if typing || !ui.input(|i| i.key_down(egui::Key::Space)) {
            self.space_held = false;
        }
        let space = self.space_held;
        // The wheel and a pinch zoom about the pointer; two fingers pan.
        let wheel = self.trackpad.read(ui, false);
        if let Some(p) = pointer.filter(|_| response.contains_pointer())
            && wheel.moves()
        {
            let scale = (view_state.scale * wheel.scale_factor()).clamp(0.5, 200.0);
            let world = view.world(p);
            let origin = p - vec2(world[0] as f32 * scale, -(world[1] as f32) * scale);
            self.symbol_view = Some(ViewState {
                pan: origin - rect.center() + wheel.pan,
                scale,
            });
        }
        let panning = response.dragged_by(egui::PointerButton::Middle)
            || (space && response.dragged_by(egui::PointerButton::Primary));
        if panning && let Some(v) = &mut self.symbol_view {
            v.pan += response.drag_delta();
        }
        self.symbol_keys(ui, grid);
        let suppress = ui.input(|i| i.modifiers.command || i.modifiers.alt);
        let tolerance = f64::from(SNAP_PX / view.scale);
        let tool = self.drawing.tool();
        let moving: Selection = match &self.gesture {
            Some(Gesture::Move { .. }) => self.selection.clone(),
            _ => Selection::new(),
        };
        let targets = self.targets(&moving);
        let snap_at = |p: Pos2| {
            snap::snap(
                view.world(p),
                tolerance,
                grid,
                &targets,
                self.snap && !suppress,
            )
        };
        let cursor: Option<Snap> = pointer.filter(|_| tool != Tool::Select).map(snap_at);
        if let Some(c) = &cursor {
            self.drawing.hover(c.at);
        }
        // Tool clicks.
        if tool != Tool::Select && !panning && !space {
            if response.secondary_clicked() {
                self.drawing.cancel();
            } else if response.clicked()
                && let Some(c) = response.interact_pointer_pos().map(snap_at)
            {
                let before = self.drawing.pending().len();
                let placed = self.drawing.click(Click {
                    at: c.at,
                    tolerance_mm: tolerance,
                });
                let repeated = tool.is_chain() && self.drawing.pending().len() == before;
                if let Some(placed) = placed {
                    self.place(placed, engine);
                }
                // A double click finishes a chain only where it was: egui
                // counts quick clicks anywhere as double.
                if response.double_clicked() && repeated {
                    self.finish_chain();
                }
            }
        }
        // Selection, moving and box selection. A click on the canvas
        // leaves a TERMINAL card for what it selects.
        if tool == Tool::Select && !panning && !space {
            if response.clicked() || response.drag_started() {
                self.terminal_card = None;
            }
            self.select_gestures(ui, response, &drawn, view, grid, suppress);
        }
        if let Some(placed) = self.pending_place.take() {
            self.place(placed, engine);
        }
        self.paint_symbol(&painter, rect, view, grid, &drawn, cursor.as_ref());
        // A cursor hint for the armed tool, cut short with … before the
        // view controls centred at the bottom (Fit), never under them.
        if tool != Tool::Select {
            let hint = format!("{} · {}", tool.label(), self.tool_hint(tool));
            let left = rect.left() + t.space_16;
            let reserve = t.control_height / 2.0 + t.space_4 + t.space_16;
            let width = (rect.center().x - reserve - left).max(0.0);
            let mut job = egui::text::LayoutJob::simple_singleline(
                hint,
                egui::FontId::proportional(t.font_label),
                t.text_dim,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(width);
            let galley = painter.layout_job(job);
            painter.galley(
                pos2(left, rect.bottom() - t.space_16 - galley.size().y),
                galley,
                t.text_dim,
            );
        }
    }

    /// The canvas hint for the armed tool.
    fn tool_hint(&self, tool: Tool) -> &'static str {
        if tool.is_chain() {
            return "click points; Enter or double-click finishes; Esc ends";
        }
        if tool == Tool::Pin
            && let Some(set) = &self.draft
            && set
                .component
                .terminals
                .iter()
                .all(|t| set.symbol.pin(t.id).is_some())
            && set.package.chip_kind().is_some()
            && set.package.terminals.len() >= 2
        {
            return "a new pin adds a terminal, which a chip body does not allow \
                    (change Body on the 3D tab)";
        }
        "click to place; Esc ends the tool"
    }

    fn select_gestures(
        &mut self,
        ui: &Ui,
        response: &Response,
        drawn: &schematic::PlacedUnit,
        view: symbol_view::View,
        grid: f64,
        suppress: bool,
    ) {
        let m = symbol_view::metric(ui.painter());
        let tolerance = f64::from(PICK_PX / view.scale);
        let hit = |p: Pos2| drawn.hit(view.world(p), tolerance, m).and_then(selected_of);
        let shift = ui.input(|i| i.modifiers.shift);
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            match hit(p) {
                Some(s) if shift => {
                    if !self.selection.remove(&s) {
                        self.selection.insert(s);
                    }
                }
                Some(s) => self.select([s].into_iter().collect()),
                None if !shift => self.selection.clear(),
                None => {}
            }
        }
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
        {
            let start = view.world(p);
            match hit(p) {
                Some(s) => {
                    if !self.selection.contains(&s) {
                        if shift {
                            self.selection.insert(s);
                        } else {
                            self.select([s].into_iter().collect());
                        }
                    }
                    if let Some(before) = self.current_unit().cloned() {
                        self.gesture = Some(Gesture::Move {
                            start,
                            before: Box::new(before),
                        });
                    }
                }
                None => self.gesture = Some(Gesture::Boxing { start, end: start }),
            }
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            let now = view.world(p);
            let selection = self.selection.clone();
            match &mut self.gesture {
                Some(Gesture::Boxing { end, .. }) => *end = now,
                Some(Gesture::Move { start, before }) => {
                    let raw = [now[0] - start[0], now[1] - start[1]];
                    // Moves keep grid points on the grid unless snapping is
                    // off or suppressed.
                    let delta = if self.snap && !suppress {
                        snap::to_grid(raw, grid)
                    } else {
                        raw
                    };
                    let mut unit = (**before).clone();
                    arrange::translate(&mut unit, &selection, delta);
                    if let Some(current) = self.current_unit_mut() {
                        *current = unit;
                    }
                }
                None => {}
            }
        }
        if response.drag_stopped() {
            if let Some(Gesture::Boxing { start, end }) = self.gesture.take() {
                let boxed = arrange::boxed(drawn, start, end, m);
                if shift {
                    self.selection.extend(boxed);
                } else {
                    self.select(boxed);
                }
            }
            self.gesture = None;
        }
    }

    fn paint_symbol(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        view: symbol_view::View,
        grid: f64,
        drawn: &schematic::PlacedUnit,
        cursor: Option<&Snap>,
    ) {
        let t = Tokens::get();
        // Grid dots, thinned to a legible pitch.
        if self.show_grid && grid > 0.0 {
            let mut step = grid as f32 * view.scale;
            let mut every = 1.0_f64;
            while step > 0.0 && step < GRID_MIN_PX {
                step *= 2.0;
                every *= 2.0;
            }
            let pitch = grid * every;
            let lo = view.world(rect.left_bottom());
            let hi = view.world(rect.right_top());
            let (x0, x1) = (
                (lo[0] / pitch).floor() as i64,
                (hi[0] / pitch).ceil() as i64,
            );
            let (y0, y1) = (
                (lo[1] / pitch).floor() as i64,
                (hi[1] / pitch).ceil() as i64,
            );
            if (x1 - x0).saturating_mul(y1 - y0) < 40_000 {
                let dot = t.text_dim.gamma_multiply(0.45);
                for i in x0..=x1 {
                    for j in y0..=y1 {
                        let p = view.screen([i as f64 * pitch, j as f64 * pitch]);
                        painter.circle_filled(p, 1.0, dot);
                    }
                }
            }
        }
        let origin = view.screen([0.0, 0.0]);
        for d in [vec2(10.0, 0.0), vec2(0.0, 10.0)] {
            painter.line_segment([origin - d, origin + d], Stroke::new(1.0, t.text_dim));
        }
        painter.text(
            rect.left_top() + vec2(t.space_16, t.space_16),
            Align2::LEFT_TOP,
            "Symbol · electrical terminals",
            type_face::semibold(t.font_panel),
            t.text,
        );
        symbol_view::paint(
            painter,
            view,
            drawn,
            symbol_view::Ink {
                stroke: t.text,
                body: t.panel,
                text: t.text,
                pin: t.accent,
                pin_text: t.text_dim,
            },
        );
        // The selection in the selection colour, wider than any stroke;
        // every connection end gets a dot.
        let stub = (SYMBOL_STROKE_MM as f32 * view.scale).max(1.0);
        let selected = t.body_selected;
        for shape in &drawn.shapes {
            if self.selection.contains(&Selected::Graphic(shape.graphic)) {
                let width = (shape.width as f32 * view.scale).max(1.0) + 3.0;
                symbol_view::paint_shape(
                    painter,
                    view,
                    &shape.shape,
                    None,
                    Some(Stroke::new(width, selected)),
                );
                for v in shape.shape.vertices() {
                    painter.rect_filled(
                        Rect::from_center_size(view.screen(v), vec2(6.0, 6.0)),
                        1.0,
                        selected,
                    );
                }
            }
        }
        let m = symbol_view::metric(painter);
        for text in &drawn.texts {
            if let Some(g) = text.graphic
                && self.selection.contains(&Selected::Graphic(g))
            {
                let b = text.bounds(m);
                painter.rect_stroke(
                    Rect::from_two_pos(view.screen(b.min), view.screen(b.max)).expand(2.0),
                    2.0,
                    Stroke::new(1.5, selected),
                    egui::StrokeKind::Outside,
                );
            }
        }
        for pin in &drawn.pins {
            // The terminal list's choice shows only while selecting: an
            // armed tool edits nothing that is highlighted.
            let chosen = self.selection.contains(&Selected::Pin(pin.terminal))
                || (self.selection.is_empty()
                    && self.drawing.tool() == Tool::Select
                    && self.selected == Some(pin.terminal));
            let ink = if chosen {
                painter.line_segment(
                    [view.screen(pin.end), view.screen(pin.body_end)],
                    Stroke::new(stub + 4.0, selected),
                );
                selected
            } else {
                t.accent
            };
            painter.circle_filled(view.screen(pin.end), stub / 2.0 + 4.0, ink);
        }
        // The shape being drawn, its points, the snap and its guides.
        if let Some(c) = cursor {
            for [a, b] in &c.guides {
                painter.add(egui::Shape::dashed_line(
                    &[view.screen(*a), view.screen(*b)],
                    Stroke::new(1.0, t.accent_soft),
                    4.0,
                    3.0,
                ));
            }
            if let Some(shape) = self.drawing.preview(c.at) {
                symbol_view::paint_shape(
                    painter,
                    view,
                    &shape,
                    None,
                    Some(Stroke::new(
                        (self.stroke_width as f32 * view.scale).max(1.5),
                        t.preview_sketch,
                    )),
                );
            }
            for p in self.drawing.pending() {
                painter.circle_filled(view.screen(*p), 3.0, t.preview_sketch);
            }
            let at = view.screen(c.at);
            let marker = match c.kind {
                snap::Kind::Free => t.text_dim,
                snap::Kind::Grid => t.text_dim,
                _ => t.accent,
            };
            painter.circle_stroke(at, 4.0, Stroke::new(1.5, marker));
            let label = c.kind.label();
            if !label.is_empty() {
                painter.text(
                    at + vec2(7.0, -7.0),
                    Align2::LEFT_BOTTOM,
                    label,
                    egui::FontId::proportional(t.font_caption),
                    t.accent,
                );
            }
        }
        if let Some(Gesture::Boxing { start, end }) = &self.gesture {
            let r = Rect::from_two_pos(view.screen(*start), view.screen(*end));
            let window = end[0] >= start[0];
            let ink = if window { t.box_window } else { t.box_crossing };
            painter.rect_filled(r, 0.0, ink.gamma_multiply(0.15));
            if window {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, ink), egui::StrokeKind::Inside);
            } else {
                let corners = [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                    r.left_top(),
                ];
                painter.add(egui::Shape::dashed_line(
                    &corners,
                    Stroke::new(1.0, ink),
                    5.0,
                    3.0,
                ));
            }
        }
    }

    /// Add a unit drawn like the first unit's frame: designator text only.
    pub(super) fn add_unit(&mut self) -> Option<UnitId> {
        let set = self.draft.as_mut()?;
        if set.symbol.units.len() >= pcbcraft_core::library::MAX_UNITS {
            let most = pcbcraft_core::library::MAX_UNITS;
            self.tell(
                Level::Error,
                format!("No unit added: a symbol holds at most {most}"),
            );
            return None;
        }
        let id = allocate(&mut set.symbol.next_local)?;
        let designator = allocate(&mut set.symbol.next_local)?;
        let used: Vec<String> = set.symbol.units.iter().map(|u| u.label.clone()).collect();
        let label = ('A'..='Z')
            .map(String::from)
            .find(|l| !used.contains(l))
            .unwrap_or_else(|| format!("U{id}"));
        set.symbol.units.push(Unit {
            id: UnitId(id),
            label,
            graphics: vec![
                graphics::text(
                    GraphicId(designator),
                    TextContent::Designator,
                    [0.0, graphics::SYMBOL_MODULE_MM],
                    Justify {
                        h: HAlign::Center,
                        v: VAlign::Bottom,
                    },
                )
                .ok()?,
            ],
            pins: vec![],
        });
        self.unit = Some(UnitId(id));
        self.selection.clear();
        self.symbol_view = None;
        Some(UnitId(id))
    }

    /// The pin selected alone, if any.
    pub(super) fn selected_pin(&self) -> Option<TerminalId> {
        match self.selection.iter().copied().collect::<Vec<_>>()[..] {
            [Selected::Pin(t)] => Some(t),
            _ => None,
        }
    }

    /// The graphic selected alone, if any.
    pub(super) fn selected_graphic(&self) -> Option<GraphicId> {
        match self.selection.iter().copied().collect::<Vec<_>>()[..] {
            [Selected::Graphic(g)] => Some(g),
            _ => None,
        }
    }
}
