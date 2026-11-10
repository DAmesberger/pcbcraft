//! Properties cards of labels, power ports, sheet symbols, sheet pins and
//! drawings, the cards of their tools, and the Sheet card (page, title
//! block, pages and annotation). Fields commit live, one typed command per
//! edit (docs/sidebar-ia.md); the canvas hint is never repeated here.
use super::sheet_ui::{DrawTool, Item, LabelTool};
use super::{Live, SchematicEditor, focus_left, live};
use crate::kit;
use crate::refusal::{self, Origin};
use crate::shell::Header;
use pcbcraft_core::library::Rotation;
use pcbcraft_core::schematic::{AnnotateMode, AnnotateOrder, Numbering, SchematicCommand};
use pcbcraft_core::units::{Nm, PointNm};
use pcbcraft_core::{
    Engine, LabelKind, Operation, Paper, PaperOrientation, PortShape, PowerStyle, Project,
    ProjectProperties, SheetItem, SheetSide,
};
use solidcraft_ui_egui::ui_kit::{self, Button};

fn op(command: SchematicCommand) -> Option<Operation> {
    Some(Operation::Schematic(command))
}

/// The card header of a selected item.
pub(super) fn header(project: &Project, item: Item) -> Header {
    match item {
        Item::Label(id) => match project.label(id) {
            Some(l) => Header::new(
                "point",
                l.name.clone(),
                match l.kind {
                    LabelKind::Local => "Net label",
                    LabelKind::Global { .. } => "Global label",
                    LabelKind::Hierarchical { .. } => "Hierarchical label",
                    LabelKind::Power { .. } => "Power port",
                },
            ),
            None => Header::new("point", "Label", "Label"),
        },
        Item::SheetSymbol(id) => {
            let name = project
                .sheet_symbol(id)
                .and_then(|s| project.sheet(s.child))
                .map_or_else(|| "Sheet".to_owned(), |s| s.name.clone());
            Header::new("folder", name, "Sheet symbol")
        }
        Item::SheetPin(id) => {
            let name = project
                .sheet_pin(id)
                .map_or_else(|| "Pin".to_owned(), |(_, p)| p.name.clone());
            Header::new("point", name, "Sheet pin")
        }
        Item::Drawing(id) => match project.drawing(id).map(|d| &d.item) {
            Some(SheetItem::Line { .. }) => Header::new("line", "Line", "Drawing"),
            Some(SheetItem::Rect { .. }) => Header::new("rect", "Rectangle", "Drawing"),
            _ => Header::new("list", "Note", "Drawing"),
        },
    }
}

const KINDS: [(&str, u8); 4] = [
    ("Net label", 0),
    ("Global label", 1),
    ("Hierarchical label", 2),
    ("Power port", 3),
];

fn kind_index(kind: LabelKind) -> u8 {
    match kind {
        LabelKind::Local => 0,
        LabelKind::Global { .. } => 1,
        LabelKind::Hierarchical { .. } => 2,
        LabelKind::Power { .. } => 3,
    }
}

/// The kind of index `i`, keeping `from`'s shape where both have one.
fn kind_of(i: u8, from: LabelKind) -> LabelKind {
    let shape = match from {
        LabelKind::Global { shape } | LabelKind::Hierarchical { shape } => shape,
        _ => PortShape::Bidirectional,
    };
    match i {
        1 => LabelKind::Global { shape },
        2 => LabelKind::Hierarchical { shape },
        3 => LabelKind::Power {
            style: PowerStyle::Bar,
        },
        _ => LabelKind::Local,
    }
}

fn shapes() -> Vec<(PortShape, String)> {
    PortShape::ALL
        .iter()
        .map(|s| (*s, s.label().to_owned()))
        .collect()
}

fn styles() -> Vec<(PowerStyle, String)> {
    PowerStyle::ALL
        .iter()
        .map(|s| (*s, s.label().to_owned()))
        .collect()
}

fn rotations() -> Vec<(Rotation, String)> {
    [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270]
        .into_iter()
        .map(|r| (r, format!("{}°", r.degrees())))
        .collect()
}

fn sides() -> Vec<(SheetSide, String)> {
    [
        (SheetSide::Left, "Left"),
        (SheetSide::Right, "Right"),
        (SheetSide::Top, "Top"),
        (SheetSide::Bottom, "Bottom"),
    ]
    .into_iter()
    .map(|(s, l)| (s, l.to_owned()))
    .collect()
}

/// Common supply and ground names with the style each draws in.
const POWER_PRESETS: [(&str, PowerStyle); 6] = [
    ("GND", PowerStyle::Ground),
    ("+3V3", PowerStyle::Bar),
    ("+5V", PowerStyle::Bar),
    ("VBUS", PowerStyle::Arrow),
    ("+1V8", PowerStyle::Bar),
    ("AGND", PowerStyle::Ground),
];

/// One title-block field of the project properties.
type Field = fn(&mut ProjectProperties) -> &mut String;

fn name_problem(text: &str) -> Option<&'static str> {
    (!pcbcraft_core::nets::name_valid(text.trim()))
        .then_some("Enter 1–64 characters without spaces")
}

impl SchematicEditor {
    /// A live text field over `current`, buffered under `key`: returns the
    /// trimmed text once it commits (Enter or focus loss), never while
    /// `problem` rejects it.
    fn live_text(
        &mut self,
        ui: &mut egui::Ui,
        key: String,
        label: &str,
        current: &str,
        multiline: bool,
        problem: fn(&str) -> Option<&'static str>,
    ) -> Option<String> {
        // The menu's Edit gives this field the keyboard once.
        let focus = self.focus_field.as_deref() == Some(key.as_str());
        let index = match self.item_text.iter().position(|(k, _, _)| *k == key) {
            Some(i) => i,
            None => {
                self.item_text
                    .push((key, current.to_owned(), current.to_owned()));
                self.item_text.len() - 1
            }
        };
        let entry = &mut self.item_text[index];
        if entry.1 != current {
            entry.1 = current.to_owned();
            entry.2 = current.to_owned();
        }
        let invalid = problem(&entry.2);
        let response = if multiline {
            kit::multiline_row(ui, label, &mut entry.2, invalid.is_some())
        } else {
            kit::text_row(ui, label, &mut entry.2)
        };
        if focus {
            self.focus_field = None;
            response.request_focus();
        }
        let changed = entry.2.trim() != current;
        match live(ui, &response, &mut self.refused, changed, invalid) {
            Live::Commit => {
                refusal::mark(ui.ctx(), Origin::Field(response.id));
                Some(entry.2.trim().to_owned())
            }
            Live::Revert => {
                entry.2 = current.to_owned();
                None
            }
            Live::Idle => None,
        }
    }

    /// A number pair (or a single number with `single`) edited in a
    /// buffer under `key` and returned once focus leaves the row.
    fn live_pair(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        label: &str,
        current: [f64; 2],
        single: bool,
        range: std::ops::RangeInclusive<f64>,
    ) -> Option<[f64; 2]> {
        if self.pair_buf.as_ref().is_none_or(|(k, _)| k != key) {
            self.pair_buf = Some((key.to_owned(), current));
        }
        let mut value = self.pair_buf.as_ref().map_or(current, |(_, v)| *v);
        let rect = ui
            .scope(|ui| {
                if single {
                    kit::number_row(ui, label, &mut value[0], "mm", range);
                } else {
                    kit::pair_row(ui, label, &mut value, "mm", range);
                }
            })
            .response
            .rect;
        self.pair_buf = Some((key.to_owned(), value));
        match focus_left(ui, ui.scope_id().with(("pair", key)), rect) {
            Some(false) if value != current => Some(value),
            Some(_) => {
                self.pair_buf = Some((key.to_owned(), current));
                None
            }
            None => None,
        }
    }

    /// The card of a selected label, sheet symbol, sheet pin or drawing.
    pub(super) fn item_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        item: Item,
    ) -> Option<Operation> {
        match item {
            Item::Label(id) => self.label_section(ui, engine, id),
            Item::SheetSymbol(id) => self.symbol_section(ui, engine, id),
            Item::SheetPin(id) => self.pin_section(ui, engine, id),
            Item::Drawing(id) => self.drawing_section(ui, engine, id),
        }
    }

    fn label_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        id: pcbcraft_core::Id,
    ) -> Option<Operation> {
        let project = engine.project();
        let label = project.label(id)?.clone();
        let edit = |name: Option<String>, kind: Option<LabelKind>, rotation: Option<Rotation>| {
            op(SchematicCommand::EditLabel {
                label: id,
                name,
                kind,
                rotation,
            })
        };
        let mut output = None;
        kit::section(ui, "label", "Label", true, None, |ui| {
            if let Some(name) = self.live_text(
                ui,
                format!("label/{id}/name"),
                "Name",
                &label.name,
                false,
                name_problem,
            ) {
                output = edit(Some(name), None, None);
            }
            let options: Vec<(u8, String)> =
                KINDS.iter().map(|(l, i)| (*i, (*l).to_owned())).collect();
            let mut kind = kind_index(label.kind);
            if kit::choice_row(ui, "Type", &mut kind, &options) {
                output = edit(None, Some(kind_of(kind, label.kind)), None);
            }
            match label.kind {
                LabelKind::Global { shape } | LabelKind::Hierarchical { shape } => {
                    let mut shape = shape;
                    if kit::choice_row(ui, "Shape", &mut shape, &shapes()) {
                        let kind = match label.kind {
                            LabelKind::Global { .. } => LabelKind::Global { shape },
                            _ => LabelKind::Hierarchical { shape },
                        };
                        output = edit(None, Some(kind), None);
                    }
                }
                LabelKind::Power { style } => {
                    let mut style = style;
                    if kit::choice_row(ui, "Style", &mut style, &styles()) {
                        output = edit(None, Some(LabelKind::Power { style }), None);
                    }
                }
                LabelKind::Local => {}
            }
            let mut rotation = label.rotation;
            if kit::choice_row(ui, "Rotation", &mut rotation, &rotations()) {
                output = edit(None, None, Some(rotation));
            }
            let net = self
                .point_nets
                .get(&pcbcraft_core::WireEnd::label(id))
                .and_then(|n| project.net(*n))
                .map_or_else(|| "Not connected".to_owned(), |n| n.name.clone());
            kit::value_row(ui, "Net", &net);
        });
        let places = self.label_places(engine);
        let mut step = 0;
        kit::section(ui, "label-places", "Other places", true, None, |ui| {
            let others = places.len().saturating_sub(1);
            let text = match others {
                0 => "None".to_owned(),
                1 => "1 on this net".to_owned(),
                n => format!("{n} on this net"),
            };
            kit::value_row(ui, "Places", &text);
            if others > 0 {
                kit::actions(ui, |ui| {
                    if ui_kit::button(ui, "Next", Button::Normal)
                        .on_hover_text("Show the next label or sheet pin of this net")
                        .clicked()
                    {
                        step = 1;
                    }
                    if ui_kit::button(ui, "Previous", Button::Normal)
                        .on_hover_text("Show the previous label or sheet pin of this net")
                        .clicked()
                    {
                        step = -1;
                    }
                });
            }
        });
        if step != 0 {
            self.jump(engine, step);
        }
        output
    }

    fn symbol_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        id: pcbcraft_core::Id,
    ) -> Option<Operation> {
        let project = engine.project();
        let symbol = project.sheet_symbol(id)?.clone();
        let child = project.sheet(symbol.child)?.clone();
        let mut output = None;
        let mut open = false;
        let mut select = None;
        kit::section(ui, "sheet-symbol", "Sheet symbol", true, None, |ui| {
            if let Some(name) = self.live_text(
                ui,
                format!("symbol/{id}/sheet"),
                "Sheet",
                &child.name,
                false,
                |t| t.trim().is_empty().then_some("Enter a sheet name"),
            ) {
                output = op(SchematicCommand::RenameSheet {
                    sheet: child.id,
                    name,
                });
            }
            let page = project.sheets.number(child.id).unwrap_or(0);
            kit::value_row(ui, "Page", &page.to_string());
            if let Some(size) = self.live_pair(
                ui,
                &format!("symbol/{id}/size"),
                "Size",
                symbol.size_mm(),
                false,
                5.0..=2000.0,
            ) {
                output = super::sheet_nm(size)
                    .and_then(|size| op(SchematicCommand::ResizeSheetSymbol { symbol: id, size }));
            }
            open = kit::summary_row(ui, "", Some("Open sheet"));
        });
        kit::section(
            ui,
            "sheet-pins",
            "Pins",
            true,
            Some(symbol.pins.len()),
            |ui| {
                kit::list_frame(ui, symbol.pins.len(), kit::LIST_MAX_ROWS, |ui, index| {
                    let Some(pin) = symbol.pins.get(index) else {
                        return;
                    };
                    let side = format!("{:?}", pin.side);
                    let row = kit::Row {
                        icon: "point",
                        label: &pin.name,
                        secondary: &side,
                        tip: "Select this pin",
                        ..Default::default()
                    };
                    if kit::tree_row(ui, ui.scope_id().with(("pin", pin.id)), &row).clicked {
                        select = Some(pin.id);
                    }
                });
                if kit::summary_row(ui, "", Some("Add pin")) {
                    let name = (1..)
                        .map(|n| format!("PIN{n}"))
                        .find(|n| !symbol.pins.iter().any(|p| p.name.eq_ignore_ascii_case(n)))
                        .unwrap_or_else(|| "PIN".into());
                    let offset = symbol
                        .pins
                        .iter()
                        .filter(|p| p.side == SheetSide::Left)
                        .map(|p| p.offset_mm())
                        .fold(0.0, f64::max)
                        + 5.0;
                    output = Nm::from_mm_f64_rounded(offset).ok().and_then(|offset| {
                        op(SchematicCommand::AddSheetPin {
                            symbol: id,
                            name,
                            shape: PortShape::Bidirectional,
                            side: SheetSide::Left,
                            offset,
                        })
                    });
                }
                if kit::summary_row(ui, "", Some("Sync pins")) {
                    output = op(SchematicCommand::SyncSheetPins { symbol: id });
                }
            },
        );
        if open {
            self.show_sheet(child.id, project);
        }
        if let Some(pin) = select {
            self.select_item(Some(Item::SheetPin(pin)), project);
        }
        output
    }

    fn pin_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        id: pcbcraft_core::Id,
    ) -> Option<Operation> {
        let project = engine.project();
        let (symbol, pin) = project.sheet_pin(id)?;
        let (symbol, pin) = (symbol.clone(), pin.clone());
        let edit = |name, shape, side, offset: Option<f64>| {
            let offset = match offset {
                Some(mm) => Some(Nm::from_mm_f64_rounded(mm).ok()?),
                None => None,
            };
            op(SchematicCommand::EditSheetPin {
                pin: id,
                name,
                shape,
                side,
                offset,
            })
        };
        let mut output = None;
        let mut back = false;
        kit::section(ui, "sheet-pin", "Sheet pin", true, None, |ui| {
            if let Some(name) = self.live_text(
                ui,
                format!("pin/{id}/name"),
                "Name",
                &pin.name,
                false,
                name_problem,
            ) {
                output = edit(Some(name), None, None, None);
            }
            let mut shape = pin.shape;
            if kit::choice_row(ui, "Shape", &mut shape, &shapes()) {
                output = edit(None, Some(shape), None, None);
            }
            let mut side = pin.side;
            if kit::choice_row(ui, "Side", &mut side, &sides()) {
                output = edit(
                    None,
                    None,
                    Some(side),
                    Some(0.0_f64.max(pin.offset_mm().min(5.0))),
                );
            }
            if let Some([offset, _]) = self.live_pair(
                ui,
                &format!("pin/{id}/offset"),
                "Offset",
                [pin.offset_mm(), 0.0],
                true,
                0.0..=2000.0,
            ) {
                output = edit(None, None, None, Some(offset));
            }
            let child = project
                .sheet(symbol.child)
                .map_or_else(String::new, |s| s.name.clone());
            kit::value_row(ui, "Sheet symbol", &child);
            back = kit::summary_row(ui, "", Some("Select sheet symbol"));
        });
        if back {
            self.select_item(Some(Item::SheetSymbol(symbol.id)), project);
        }
        output
    }

    fn drawing_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        id: pcbcraft_core::Id,
    ) -> Option<Operation> {
        let project = engine.project();
        let item = project.drawing(id)?.item.clone();
        let mut output = None;
        kit::section(ui, "drawing", "Drawing", true, None, |ui| match &item {
            SheetItem::Text {
                at,
                text,
                height_nm,
                rotation,
            } => {
                if let Some(new) =
                    self.live_text(ui, format!("drawing/{id}/text"), "Text", text, true, |t| {
                        t.trim().is_empty().then_some("Enter the note's text")
                    })
                {
                    output = op(SchematicCommand::EditDrawing {
                        drawing: id,
                        item: SheetItem::Text {
                            at: *at,
                            text: new,
                            height_nm: *height_nm,
                            rotation: *rotation,
                        },
                    });
                }
                if let Some([h, _]) = self.live_pair(
                    ui,
                    &format!("drawing/{id}/height"),
                    "Height",
                    [height_nm.to_mm_f64(), 0.0],
                    true,
                    0.5..=50.0,
                ) && let Ok(height_nm) = Nm::from_mm_f64_rounded(h)
                {
                    output = op(SchematicCommand::EditDrawing {
                        drawing: id,
                        item: SheetItem::Text {
                            at: *at,
                            text: text.clone(),
                            height_nm,
                            rotation: *rotation,
                        },
                    });
                }
            }
            SheetItem::Line { from, to } => {
                let point = |p: &PointNm| {
                    let p = p.to_mm_f64();
                    format!("{}, {} mm", kit::number_text(p[0]), kit::number_text(p[1]))
                };
                kit::value_row(ui, "From", &point(from));
                kit::value_row(ui, "To", &point(to));
            }
            SheetItem::Rect { min, max } => {
                let size = format!(
                    "{} × {} mm",
                    kit::number_text(max.x_nm.to_mm_f64() - min.x_nm.to_mm_f64()),
                    kit::number_text(max.y_nm.to_mm_f64() - min.y_nm.to_mm_f64())
                );
                kit::value_row(ui, "Size", &size);
            }
        });
        output
    }

    /// The label and power-port tools' card: what the next click places.
    pub(super) fn label_tool_section(&mut self, ui: &mut egui::Ui, tool: LabelTool) {
        kit::section(ui, "label-tool", tool.title(), true, None, |ui| {
            let drafts = &mut self.drafts;
            let name = match tool {
                LabelTool::Local => &mut drafts.label,
                LabelTool::Global => &mut drafts.global,
                LabelTool::Hierarchical => &mut drafts.hierarchical,
                LabelTool::Power => &mut drafts.power,
            };
            let response = kit::text_row(ui, "Name", name);
            if let Some(problem) = name_problem(name) {
                kit::field_error(ui, &response, problem, true);
            }
            match tool {
                LabelTool::Global => {
                    kit::choice_row(ui, "Shape", &mut drafts.global_shape, &shapes());
                }
                LabelTool::Hierarchical => {
                    kit::choice_row(ui, "Shape", &mut drafts.hierarchical_shape, &shapes());
                }
                LabelTool::Power => {
                    kit::choice_row(ui, "Style", &mut drafts.power_style, &styles());
                    let presets: Vec<(usize, String)> = POWER_PRESETS
                        .iter()
                        .enumerate()
                        .map(|(i, (n, _))| (i, (*n).to_owned()))
                        .collect();
                    let mut chosen = POWER_PRESETS
                        .iter()
                        .position(|(n, _)| *n == drafts.power.trim())
                        .unwrap_or(usize::MAX);
                    if kit::choice_row(ui, "Common", &mut chosen, &presets)
                        && let Some((n, style)) = POWER_PRESETS.get(chosen)
                    {
                        drafts.power = (*n).to_owned();
                        drafts.power_style = *style;
                    }
                }
                LabelTool::Local => {}
            }
            kit::hint(
                ui,
                "Same names join",
                match tool {
                    LabelTool::Local => "Labels of one name join on this sheet.",
                    LabelTool::Hierarchical => {
                        "Joins the sheet pin of this name on the parent sheet's symbol."
                    }
                    _ => "Joins every global label and power port of this name.",
                },
            );
        });
    }

    /// The sheet-symbol tool's card: the name of the sheet it creates.
    pub(super) fn sheet_tool_section(&mut self, ui: &mut egui::Ui) {
        kit::section(ui, "sheet-tool", "Sheet symbol", true, None, |ui| {
            kit::text_row(ui, "New sheet", &mut self.drafts.sheet_name);
            kit::hint(
                ui,
                "A new child sheet",
                "The symbol stands for a new sheet; its pins join hierarchical labels there.",
            );
        });
    }

    /// The drawing tools' card.
    pub(super) fn draw_tool_section(&mut self, ui: &mut egui::Ui, tool: DrawTool) {
        kit::section(ui, "draw-tool", "Drawing", true, None, |ui| match tool {
            DrawTool::Note => {
                let empty = self.drafts.note.trim().is_empty();
                let response = kit::multiline_row(ui, "Text", &mut self.drafts.note, empty);
                if empty {
                    kit::field_error(ui, &response, "Enter the note's text", true);
                }
                kit::number_row(ui, "Height", &mut self.drafts.note_height, "mm", 0.5..=50.0);
            }
            DrawTool::Line | DrawTool::Rect => {
                kit::value_row(
                    ui,
                    "Start",
                    if self.draw_from.is_some() {
                        "Set"
                    } else {
                        "Not set"
                    },
                );
                kit::hint(
                    ui,
                    "Never electrical",
                    "Lines and rectangles never join nets.",
                );
            }
        });
    }

    /// Nothing selected: the active sheet's page, the pages, the title
    /// block and annotation.
    pub(super) fn sheet_section(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
    ) -> Option<Operation> {
        let project = engine.project();
        let sheet = project.sheet(self.sheet)?.clone();
        let count = project.sheets.list.len();
        let page = project.sheets.number(sheet.id).unwrap_or(1);
        let mut output = None;
        kit::section(ui, "sheet-page", "Page", true, None, |ui| {
            if let Some(name) = self.live_text(
                ui,
                format!("sheet/{}/name", sheet.id),
                "Name",
                &sheet.name,
                false,
                |t| t.trim().is_empty().then_some("Enter a sheet name"),
            ) {
                output = op(SchematicCommand::RenameSheet {
                    sheet: sheet.id,
                    name,
                });
            }
            kit::value_row(ui, "Page", &format!("{page} of {count}"));
            let papers: Vec<(Paper, String)> = Paper::ALL
                .iter()
                .map(|p| (*p, p.label().to_owned()))
                .collect();
            let mut paper = sheet.paper;
            let mut orientation = sheet.orientation;
            let a = kit::choice_row(ui, "Paper", &mut paper, &papers);
            let b = kit::choice_row(
                ui,
                "Orientation",
                &mut orientation,
                &[
                    (PaperOrientation::Landscape, "Landscape".to_owned()),
                    (PaperOrientation::Portrait, "Portrait".to_owned()),
                ],
            );
            if a || b {
                output = op(SchematicCommand::SetPage {
                    sheet: sheet.id,
                    paper,
                    orientation,
                });
            }
        });
        let on_sheet = |o: &&pcbcraft_core::SymbolOccurrence| o.sheet == sheet.id;
        let symbols = project.occurrences.iter().filter(on_sheet).count();
        let labels = project
            .labels
            .iter()
            .filter(|l| l.sheet == sheet.id)
            .count();
        let wires = self
            .wire_sheets
            .values()
            .filter(|s| **s == sheet.id)
            .count();
        let mut resolve = false;
        kit::section(ui, "sheet", "On this sheet", true, None, |ui| {
            kit::value_row(ui, "Symbols", &symbols.to_string());
            kit::value_row(ui, "Wires", &wires.to_string());
            kit::value_row(ui, "Labels", &labels.to_string());
            let unresolved = super::sheet_card::unresolved_parts(project);
            if unresolved == 0 {
                kit::value_row(ui, "Parts", "All resolved");
            } else {
                kit::value_row(ui, "Parts", &format!("{unresolved} unresolved"));
                resolve = kit::summary_row(ui, "", Some("Resolve parts…"));
            }
        });
        if resolve {
            self.palette.close();
            self.resolve.open();
        }
        kit::section(ui, "sheet-pages", "Pages", true, Some(count), |ui| {
            if kit::summary_row(ui, "", Some("Add sheet")) {
                output = op(SchematicCommand::AddSheet {
                    name: None,
                    paper: None,
                    orientation: None,
                });
            }
            if page > 1 && kit::summary_row(ui, "", Some("Move up")) {
                output = op(SchematicCommand::ReorderSheet {
                    sheet: sheet.id,
                    index: page - 2,
                });
            }
            if page < count && kit::summary_row(ui, "", Some("Move down")) {
                output = op(SchematicCommand::ReorderSheet {
                    sheet: sheet.id,
                    index: page,
                });
            }
            if count > 1
                && kit::summary_row(ui, "Removes what is drawn on it", Some("Delete sheet"))
            {
                output = op(SchematicCommand::DeleteSheet { sheet: sheet.id });
            }
        });
        kit::section(ui, "design", "Design", true, None, |ui| {
            let connected = self.nets.iter().filter(|n| n.terminals.len() >= 2).count();
            ui.scope(|ui| kit::value_row(ui, "Connected nets", &connected.to_string()))
                .response
                .on_hover_text("Nets of the whole design that join two or more terminals");
        });
        let props = project.properties.clone();
        kit::section(ui, "title-block", "Title block", true, None, |ui| {
            if let Some(name) = self.live_text(
                ui,
                "project/name".to_owned(),
                "Project",
                &project.name,
                false,
                |t| {
                    if t.trim().is_empty() {
                        Some("Enter a project name")
                    } else {
                        (t.trim().len() > 256).then_some("At most 256 characters")
                    }
                },
            ) {
                output = op(SchematicCommand::RenameProject { name });
            }
            let fields: [(&str, &str, Field); 5] = [
                ("title", "Title", |p| &mut p.title),
                ("revision", "Revision", |p| &mut p.revision),
                ("date", "Date", |p| &mut p.date),
                ("author", "Author", |p| &mut p.author),
                ("company", "Company", |p| &mut p.company),
            ];
            for (key, label, field) in fields {
                let mut next = props.clone();
                let current = field(&mut next).clone();
                if let Some(text) =
                    self.live_text(ui, format!("project/{key}"), label, &current, false, |t| {
                        (t.len() > 256).then_some("At most 256 characters")
                    })
                {
                    *field(&mut next) = text;
                    output = op(SchematicCommand::SetProperties { properties: next });
                }
            }
        });
        let mut annotate = false;
        kit::section(ui, "annotate", "Annotate", false, None, |ui| {
            let (order, mode, numbering) = &mut self.annotate;
            kit::choice_row(
                ui,
                "Order",
                order,
                &[
                    (AnnotateOrder::Rows, "Rows, then columns".to_owned()),
                    (AnnotateOrder::Columns, "Columns, then rows".to_owned()),
                ],
            );
            kit::choice_row(
                ui,
                "Designators",
                mode,
                &[
                    (AnnotateMode::Unannotated, "Only unnumbered".to_owned()),
                    (AnnotateMode::All, "Renumber all".to_owned()),
                ],
            );
            kit::choice_row(
                ui,
                "Numbers",
                numbering,
                &[
                    (Numbering::Sequential, "1, 2, 3 …".to_owned()),
                    (Numbering::SheetHundreds, "By page: 101, 201 …".to_owned()),
                ],
            );
            annotate = kit::summary_row(ui, "", Some("Annotate"));
        });
        if annotate {
            output = self.annotation(engine);
        }
        output
    }

    /// `schematic.annotate` with the card's options, previewed on a copy
    /// so a design with nothing to number gets a note instead of an empty
    /// undo step; the applied count is a toast on the next frame.
    pub(super) fn annotation(&mut self, engine: &Engine) -> Option<Operation> {
        let (order, mode, numbering) = self.annotate;
        let mut copy = engine.project().clone();
        let count =
            pcbcraft_core::schematic::annotate::annotate(&mut copy, order, mode, numbering, &[])
                .map(|changes| changes.len())
                .unwrap_or(0);
        if count == 0 {
            self.notice = Some("Every designator is already numbered".into());
            return None;
        }
        self.annotate_watch = Some((engine.revision(), count, None));
        op(SchematicCommand::Annotate {
            order,
            mode,
            numbering,
            sheets: Vec::new(),
        })
    }

    /// The toast of an applied annotation or a pending notice.
    pub(super) fn say_notices(&mut self, ctx: &egui::Context, engine: &Engine) {
        let frame = ctx.cumulative_frame_nr();
        if let Some((revision, count, seen)) = self.annotate_watch {
            // Decided on the frame after the command was sent: applied
            // (the revision moved) or refused (it did not).
            let Some(seen) = seen else {
                self.annotate_watch = Some((revision, count, Some(frame)));
                return;
            };
            if frame == seen {
                return;
            }
            self.annotate_watch = None;
            if engine.revision() == revision {
                return;
            }
            let text = if count == 1 {
                "Annotated 1 designator".to_owned()
            } else {
                format!("Annotated {count} designators")
            };
            kit::toast::post(
                ctx,
                kit::severity::Level::Ok,
                text,
                vec![kit::toast::ToastAction::Undo],
            );
        }
        if let Some(text) = self.notice.take() {
            kit::toast::post(ctx, kit::severity::Level::Info, text, Vec::new());
        }
    }
}
