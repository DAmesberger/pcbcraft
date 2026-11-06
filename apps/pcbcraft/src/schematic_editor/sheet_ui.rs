//! The active sheet and what the canvas draws and picks on it besides
//! symbols and wires: the page frame and title block, labels and power
//! ports, sheet symbols with their pins, and notes, lines and rectangles,
//! all as core resolves them (`schematic::scene`). Also the label, power,
//! sheet-symbol and drawing tools' clicks, and the gestures on those
//! objects. Every edit is one typed `SchematicCommand`
//! (docs/schematic-capture.md).
use super::{SchematicEditor, Tool};
use crate::symbol_view::{self, View};
use egui::{Color32, Painter, Stroke};
use pcbcraft_core::geometry::Bounds;
use pcbcraft_core::library::{Rotation, TextMetric};
use pcbcraft_core::schematic::scene::{self, Drawing};
use pcbcraft_core::schematic::{self, SchematicCommand, WireTarget};
use pcbcraft_core::{
    Engine, Id, LabelKind, Operation, Point, PortShape, PowerStyle, Project, SheetId, SheetItem,
    WireEnd,
};
use solidcraft_ui_egui::theme::Tokens;

/// A selectable sheet object other than a symbol occurrence or a wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Item {
    Label(Id),
    SheetSymbol(Id),
    SheetPin(Id),
    Drawing(Id),
}

/// Which label the label tools put down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum LabelTool {
    Local,
    Global,
    Hierarchical,
    Power,
}

impl LabelTool {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Local => "Net label",
            Self::Global => "Global label",
            Self::Hierarchical => "Hierarchical label",
            Self::Power => "Power port",
        }
    }
}

/// Which furniture the drawing tools add.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum DrawTool {
    Note,
    Line,
    Rect,
}

/// What the tools put down next: the tool cards edit it.
#[derive(Clone, Debug)]
pub(super) struct Drafts {
    pub label: String,
    pub global: String,
    pub global_shape: PortShape,
    pub hierarchical: String,
    pub hierarchical_shape: PortShape,
    pub power: String,
    pub power_style: PowerStyle,
    /// The name of the sheet a new sheet symbol creates; empty for "Sheet n".
    pub sheet_name: String,
    pub note: String,
    pub note_height: f64,
}

impl Default for Drafts {
    fn default() -> Self {
        Self {
            label: "NET".into(),
            global: "SIGNAL".into(),
            global_shape: PortShape::Bidirectional,
            hierarchical: "PORT".into(),
            hierarchical_shape: PortShape::Bidirectional,
            power: "GND".into(),
            power_style: PowerStyle::Ground,
            sheet_name: String::new(),
            note: "Note".into(),
            note_height: 2.5,
        }
    }
}

impl Drafts {
    /// The label a label tool puts down: name and kind.
    pub(super) fn label(&self, tool: LabelTool) -> (String, LabelKind) {
        match tool {
            LabelTool::Local => (self.label.trim().to_owned(), LabelKind::Local),
            LabelTool::Global => (
                self.global.trim().to_owned(),
                LabelKind::Global {
                    shape: self.global_shape,
                },
            ),
            LabelTool::Hierarchical => (
                self.hierarchical.trim().to_owned(),
                LabelKind::Hierarchical {
                    shape: self.hierarchical_shape,
                },
            ),
            LabelTool::Power => (
                self.power.trim().to_owned(),
                LabelKind::Power {
                    style: self.power_style,
                },
            ),
        }
    }
}

/// The default size of a clicked sheet symbol, mm.
pub(super) const SHEET_SYMBOL_MM: Point = [30.0, 20.0];

/// What the active sheet draws besides symbols and wires, resolved once per
/// revision and sheet.
#[derive(Clone, Debug, Default)]
pub(super) struct SceneCache {
    pub revision: u64,
    pub sheet: SheetId,
    pub frame: Drawing,
    /// The page, x ∈ [0, w], y ∈ [−h, 0].
    pub page: Option<Bounds>,
    pub items: Vec<(Item, Drawing)>,
}

/// Where an item's drag anchor sits: a label's point, a symbol's top-left
/// corner, a note's anchor, a line's start or a rectangle's corner.
pub(super) fn anchor(project: &Project, item: Item) -> Option<Point> {
    match item {
        Item::Label(id) => project
            .label(id)
            .map(|l| schematic::labels::anchor(project, l)),
        Item::SheetSymbol(id) => project.sheet_symbol(id).map(|s| s.position),
        Item::SheetPin(id) => project.sheet_pin(id).map(|(s, p)| s.pin_position(p)),
        Item::Drawing(id) => project.drawing(id).map(|d| match &d.item {
            SheetItem::Text { at, .. } => *at,
            SheetItem::Line { from, .. } => *from,
            SheetItem::Rect { min, .. } => *min,
        }),
    }
}

/// The sheet an item lies on.
pub(super) fn item_sheet(project: &Project, item: Item) -> Option<SheetId> {
    match item {
        Item::Label(id) => project.label(id).map(|l| l.sheet),
        Item::SheetSymbol(id) => project.sheet_symbol(id).map(|s| s.sheet),
        Item::SheetPin(id) => project.sheet_pin(id).map(|(s, _)| s.sheet),
        Item::Drawing(id) => project.drawing(id).map(|d| d.sheet),
    }
}

/// The one command that moves `item` so its anchor lands on `to`; `None`
/// for a sheet pin (its offset is edited in its card).
pub(super) fn moved(project: &Project, item: Item, to: Point) -> Option<Operation> {
    let command = match item {
        Item::Label(label) => SchematicCommand::MoveLabel {
            label,
            position: to,
        },
        Item::SheetSymbol(symbol) => SchematicCommand::MoveSheetSymbol {
            symbol,
            position: to,
        },
        Item::SheetPin(_) => return None,
        Item::Drawing(drawing) => {
            let item = &project.drawing(drawing)?.item;
            let from = anchor(project, Item::Drawing(drawing))?;
            let d = [to[0] - from[0], to[1] - from[1]];
            let shift = |p: Point| [p[0] + d[0], p[1] + d[1]];
            let item = match item.clone() {
                SheetItem::Text {
                    at,
                    text,
                    height,
                    rotation,
                } => SheetItem::Text {
                    at: shift(at),
                    text,
                    height,
                    rotation,
                },
                SheetItem::Line { from, to } => SheetItem::Line {
                    from: shift(from),
                    to: shift(to),
                },
                SheetItem::Rect { min, max } => SheetItem::Rect {
                    min: shift(min),
                    max: shift(max),
                },
            };
            SchematicCommand::EditDrawing { drawing, item }
        }
    };
    Some(Operation::Schematic(command))
}

/// The one command that removes `item`.
pub(super) fn removed(item: Item) -> Operation {
    Operation::Schematic(match item {
        Item::Label(label) => SchematicCommand::RemoveLabel { label },
        Item::SheetSymbol(symbol) => SchematicCommand::RemoveSheetSymbol { symbol },
        Item::SheetPin(pin) => SchematicCommand::RemoveSheetPin { pin },
        Item::Drawing(drawing) => SchematicCommand::RemoveDrawing { drawing },
    })
}

/// Paint one resolved drawing.
pub(super) fn paint_drawing(painter: &Painter, view: View, drawing: &Drawing, ink: Color32) {
    for stroke in &drawing.strokes {
        let width = (stroke.width as f32 * view.scale).max(1.0);
        symbol_view::paint_shape(
            painter,
            view,
            &stroke.shape,
            stroke.filled.then_some(ink),
            Some(Stroke::new(width, ink)),
        );
    }
    for text in &drawing.texts {
        symbol_view::paint_text(painter, view, text, ink);
    }
}

impl SchematicEditor {
    /// The sheet the canvas shows.
    pub fn active_sheet(&self) -> SheetId {
        self.sheet
    }

    /// Show `sheet` on the canvas (the Browser, a jump, Open sheet). The
    /// selection is released and the page is fitted.
    pub fn show_sheet(&mut self, sheet: SheetId, project: &Project) {
        if project.sheet(sheet).is_none() {
            return;
        }
        if self.sheet != sheet {
            self.select(None, None, project);
            self.wire = None;
            self.draw_from = None;
            self.fit_requested = true;
        }
        self.sheet = sheet;
    }

    /// Another document replaced the last one (opened, recovered or new):
    /// start on its first page with nothing selected and the page framed.
    /// The frame waits for the next canvas frame, whenever Schematic shows.
    pub fn document_opened(&mut self, project: &Project) {
        self.select(None, None, project);
        self.wire = None;
        self.draw_from = None;
        self.frame_request = None;
        self.sheet = project.sheets.first();
        self.fit_requested = true;
    }

    /// Switch to `sheet` without fitting (a located object frames itself).
    pub(super) fn switch_sheet(&mut self, sheet: Option<SheetId>, project: &Project) {
        if let Some(sheet) = sheet.filter(|s| project.sheet(*s).is_some()) {
            if self.sheet != sheet {
                self.wire = None;
                self.draw_from = None;
            }
            self.sheet = sheet;
        }
    }

    /// Resolve what the active sheet draws besides symbols and wires, once
    /// per revision and sheet. A deleted active sheet falls back to the
    /// first page.
    pub(super) fn refresh_scene(&mut self, engine: &Engine) {
        let project = engine.project();
        if project.sheet(self.sheet).is_none() {
            self.sheet = project.sheets.first();
        }
        if self
            .scene
            .as_ref()
            .is_some_and(|s| s.revision == engine.revision() && s.sheet == self.sheet)
        {
            return;
        }
        let sheet = self.sheet;
        let mut items = Vec::new();
        for d in project.drawings.iter().filter(|d| d.sheet == sheet) {
            items.push((Item::Drawing(d.id), scene::item(&d.item)));
        }
        for s in project.sheet_symbols.iter().filter(|s| s.sheet == sheet) {
            items.push((Item::SheetSymbol(s.id), scene::sheet_symbol(project, s)));
        }
        for (id, drawing) in scene::sheet_labels(project, sheet) {
            items.push((Item::Label(id), drawing));
        }
        let page = project.sheet(sheet).map(|s| {
            let [w, h] = s.size_mm();
            let [x, y] = s.top_left();
            Bounds::point([x, y]).including([x + w, y - h])
        });
        self.scene = Some(SceneCache {
            revision: engine.revision(),
            sheet,
            frame: scene::frame(project, sheet),
            page,
            items,
        });
    }

    /// The selected item, if it still exists.
    pub(super) fn selected_item(&self, project: &Project) -> Option<Item> {
        self.item.filter(|i| anchor(project, *i).is_some())
    }

    /// Select one sheet object, releasing any instance or wire.
    pub(super) fn select_item(&mut self, item: Option<Item>, project: &Project) {
        self.select(None, None, project);
        self.item = item;
        self.refused = None;
    }

    /// The topmost item drawn within `tol` mm of `p` on the active sheet:
    /// sheet pins first, then labels, then symbols and furniture.
    pub(super) fn item_at(&self, project: &Project, p: Point, tol: f64) -> Option<Item> {
        let scene = self.scene.as_ref()?;
        let m = TextMetric::default();
        for symbol in project
            .sheet_symbols
            .iter()
            .filter(|s| s.sheet == self.sheet)
        {
            for pin in &symbol.pins {
                let at = symbol.pin_position(pin);
                if (at[0] - p[0]).hypot(at[1] - p[1]) <= tol.max(1.0) {
                    return Some(Item::SheetPin(pin.id));
                }
            }
        }
        let hit = |want: fn(&Item) -> bool| {
            scene
                .items
                .iter()
                .rev()
                .filter(|(i, _)| want(i))
                .find(|(_, d)| d.hit(p, tol, m))
                .map(|(i, _)| *i)
        };
        hit(|i| matches!(i, Item::Label(_)))
            .or_else(|| hit(|i| !matches!(i, Item::Label(_))))
            .or_else(|| {
                // Inside a sheet symbol's box counts as on it.
                project
                    .sheet_symbols
                    .iter()
                    .filter(|s| s.sheet == self.sheet)
                    .find(|s| {
                        let [x, y] = s.position;
                        p[0] >= x && p[0] <= x + s.size[0] && p[1] <= y && p[1] >= y - s.size[1]
                    })
                    .map(|s| Item::SheetSymbol(s.id))
            })
    }

    /// The wire target a label drawn under `p` stands for, with its sheet
    /// position. A label placing itself on a pin stands for that pin, so a
    /// new wire leaves it on its pin (a second wire on the label itself
    /// would end its self-placing).
    pub(super) fn label_target_at(
        &self,
        project: &Project,
        p: Point,
        tol: f64,
    ) -> Option<(WireTarget, Point)> {
        let scene = self.scene.as_ref()?;
        let m = TextMetric::default();
        let id = scene
            .items
            .iter()
            .rev()
            .find_map(|(item, drawing)| match item {
                Item::Label(id) if drawing.hit(p, tol, m) => Some(*id),
                _ => None,
            })?;
        let label = project.label(id)?;
        let target = match schematic::labels::attached_pin(project, label) {
            Some(pin) => WireTarget::Terminal(pin),
            None => WireTarget::from(WireEnd::label(id)),
        };
        Some((target, super::target_position(project, target)?))
    }

    /// The labels and sheet pins of the active sheet as wire targets, with
    /// their sheet positions.
    pub(super) fn point_targets(&self, project: &Project) -> Vec<(WireTarget, Point)> {
        let labels = project
            .labels
            .iter()
            .filter(|l| l.sheet == self.sheet)
            .map(|l| {
                (
                    WireTarget::from(WireEnd::label(l.id)),
                    schematic::labels::anchor(project, l),
                )
            });
        let pins = project
            .sheet_symbols
            .iter()
            .filter(|s| s.sheet == self.sheet)
            .flat_map(|s| {
                s.pins.iter().map(move |p| {
                    (
                        WireTarget::from(WireEnd::sheet_pin(p.id)),
                        s.pin_position(p),
                    )
                })
            });
        labels.chain(pins).collect()
    }

    /// Paint the page frame, then labels, sheet symbols and furniture. The
    /// selected item, and labels and pins on `net`, take the accent; a
    /// dragged item is drawn at its preview.
    pub(super) fn paint_scene(&self, painter: &Painter, view: View, project: &Project) {
        let t = Tokens::get();
        let Some(scene) = self.scene.as_ref() else {
            return;
        };
        paint_drawing(painter, view, &scene.frame, t.text_dim.gamma_multiply(0.7));
        let net = self.highlighted_net();
        for (item, drawing) in &scene.items {
            let on_net = net.is_some_and(|n| match item {
                Item::Label(id) => self.point_nets.get(&WireEnd::label(*id)) == Some(&n),
                _ => false,
            });
            let selected = self.item == Some(*item)
                || self.group.contains_item(*item)
                || matches!((self.item, item), (Some(Item::SheetPin(pin)), Item::SheetSymbol(s))
                    if project.sheet_pin(pin).is_some_and(|(sym, _)| sym.id == *s));
            let ink = if selected || on_net { t.accent } else { t.text };
            let delta = self
                .item_preview
                .filter(|(i, _)| i == item)
                .and_then(|(i, to)| {
                    let from = anchor(project, i)?;
                    Some([to[0] - from[0], to[1] - from[1]])
                })
                .or_else(|| self.group_item_delta(*item));
            let view = match delta {
                Some(d) => View {
                    origin: view.screen(d),
                    scale: view.scale,
                },
                None => view,
            };
            paint_drawing(painter, view, drawing, ink);
        }
    }

    /// The net the canvas highlights: the one being wired from, the
    /// selected wire's, or the selected label's or sheet pin's.
    pub(super) fn highlighted_net(&self) -> Option<Id> {
        match self.item {
            Some(Item::Label(id)) => self.point_nets.get(&WireEnd::label(id)).copied(),
            Some(Item::SheetPin(id)) => self.point_nets.get(&WireEnd::sheet_pin(id)).copied(),
            _ => self
                .selected_wire
                .and_then(|id| self.wire_nets.get(&id).copied()),
        }
    }

    /// A click of a placing tool at sheet point `at` (snapped), landing on
    /// `target` when the pointer is over a pin, junction, label, sheet pin
    /// or wire: the one command it issues.
    pub(super) fn tool_click(
        &mut self,
        at: Point,
        target: Option<(WireTarget, Point)>,
    ) -> Option<Operation> {
        let sheet = Some(self.sheet);
        match self.tool {
            Tool::Label(tool) => {
                let (name, kind) = self.drafts.label(tool);
                // The tool card marks an invalid name; nothing is placed.
                if !pcbcraft_core::nets::name_valid(&name) {
                    return None;
                }
                let (position, attach) = match target {
                    Some((target, p)) => (p, Some(target)),
                    None => (at, None),
                };
                Some(Operation::Schematic(SchematicCommand::PlaceLabel {
                    sheet,
                    name,
                    kind,
                    position,
                    rotation: Rotation::R0,
                    attach,
                }))
            }
            Tool::SheetSymbol => {
                let name = self.drafts.sheet_name.trim();
                Some(Operation::Schematic(SchematicCommand::PlaceSheetSymbol {
                    sheet,
                    child: None,
                    name: (!name.is_empty()).then(|| name.to_owned()),
                    position: at,
                    size: SHEET_SYMBOL_MM,
                }))
            }
            Tool::Draw(DrawTool::Note) => {
                Some(Operation::Schematic(SchematicCommand::AddDrawing {
                    sheet,
                    item: SheetItem::Text {
                        at,
                        text: self.drafts.note.clone(),
                        height: self.drafts.note_height,
                        rotation: Rotation::R0,
                    },
                }))
            }
            Tool::Draw(kind @ (DrawTool::Line | DrawTool::Rect)) => match self.draw_from.take() {
                None => {
                    self.draw_from = Some(at);
                    None
                }
                Some(from) if from == at => {
                    self.draw_from = Some(from);
                    None
                }
                Some(from) => Some(Operation::Schematic(SchematicCommand::AddDrawing {
                    sheet,
                    item: if kind == DrawTool::Line {
                        SheetItem::Line { from, to: at }
                    } else {
                        SheetItem::Rect {
                            min: [from[0].min(at[0]), from[1].min(at[1])],
                            max: [from[0].max(at[0]), from[1].max(at[1])],
                        }
                    },
                })),
            },
            _ => None,
        }
    }

    /// A sheet symbol dragged out as a rectangle from `a` to `b`.
    pub(super) fn sheet_symbol_rect(&self, a: Point, b: Point) -> Option<Operation> {
        let size = [(a[0] - b[0]).abs(), (a[1] - b[1]).abs()];
        if size[0] < 5.0 || size[1] < 5.0 {
            return None;
        }
        let name = self.drafts.sheet_name.trim();
        Some(Operation::Schematic(SchematicCommand::PlaceSheetSymbol {
            sheet: Some(self.sheet),
            child: None,
            name: (!name.is_empty()).then(|| name.to_owned()),
            position: [a[0].min(b[0]), a[1].max(b[1])],
            size,
        }))
    }

    /// Turn the selected label a quarter counter-clockwise from the way it
    /// is drawn (a port on a pin faces by itself until it is first turned).
    pub(super) fn rotated_item(&self, project: &Project) -> Option<Operation> {
        let Some(Item::Label(id)) = self.item else {
            return None;
        };
        let rotation = pcbcraft_core::schematic::labels::current_rotation(project, id).ok()?;
        Some(Operation::Schematic(SchematicCommand::EditLabel {
            label: id,
            name: None,
            kind: None,
            rotation: Some(rotation.turned(1)),
        }))
    }

    /// The page and every symbol on the active sheet, for Fit.
    pub(super) fn sheet_extent(&self, project: &Project) -> Option<Bounds> {
        let m = TextMetric::default();
        let symbols = project
            .occurrences
            .iter()
            .filter(|o| o.sheet == self.sheet)
            .filter_map(|o| self.placed.get(&o.id)?.bounds(m));
        let items = self
            .scene
            .iter()
            .flat_map(|s| s.items.iter().filter_map(|(_, d)| d.bounds(m)));
        self.scene
            .as_ref()
            .and_then(|s| s.page)
            .into_iter()
            .chain(symbols)
            .chain(items)
            .reduce(Bounds::union)
    }

    /// Jump across sheets to the next (`step` 1) or previous (−1) place of
    /// the selected label's net or name: switch sheet, select it there and
    /// frame it.
    pub(super) fn jump(&mut self, engine: &Engine, step: isize) {
        let project = engine.project();
        let places = self.label_places(engine);
        if places.len() < 2 {
            return;
        }
        let here = self.item;
        let index = places.iter().position(|(i, _, _)| Some(*i) == here);
        let len = places.len() as isize;
        let next = index.map_or(0, |i| (i as isize + step).rem_euclid(len)) as usize;
        let Some((item, sheet, at)) = places.get(next).copied() else {
            return;
        };
        self.switch_sheet(Some(sheet), project);
        self.select_item(Some(item), project);
        self.frame_request = Some(Bounds::point(at).inflated(15.0));
    }

    /// Every label and sheet pin of the selected label's net across sheets
    /// in page order (`schematic.net_places`); for a label on no net, the
    /// labels that share its name and scope.
    pub(super) fn label_places(&self, engine: &Engine) -> Vec<(Item, SheetId, Point)> {
        let project = engine.project();
        let Some(Item::Label(id)) = self.item else {
            return Vec::new();
        };
        let net = self.point_nets.get(&WireEnd::label(id)).copied();
        if let Some(net) = net
            && let Ok(pcbcraft_core::QueryResult::NetPlaces(places)) =
                engine.query(pcbcraft_core::Query::NetPlaces { net })
        {
            return places
                .places
                .iter()
                .filter_map(|p| match p.kind {
                    schematic::NetPlaceKind::Label { label, .. } => {
                        Some((Item::Label(label), p.sheet, p.position))
                    }
                    schematic::NetPlaceKind::SheetPin { pin, .. } => {
                        Some((Item::SheetPin(pin), p.sheet, p.position))
                    }
                    schematic::NetPlaceKind::Terminal { .. } => None,
                })
                .collect();
        }
        let Some(label) = project.label(id) else {
            return Vec::new();
        };
        let mut places: Vec<(Item, SheetId, Point)> = project
            .labels
            .iter()
            .filter(|l| {
                l.name.eq_ignore_ascii_case(&label.name)
                    && (l.kind.is_global() == label.kind.is_global())
                    && (label.kind.is_global() || l.sheet == label.sheet)
            })
            .map(|l| {
                (
                    Item::Label(l.id),
                    l.sheet,
                    schematic::labels::anchor(project, l),
                )
            })
            .collect();
        places.sort_by_key(|(_, s, _)| project.sheets.number(*s).unwrap_or(usize::MAX));
        places
    }
}
