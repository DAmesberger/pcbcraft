//! What a sheet draws besides symbols, resolved to plain shapes and text in
//! sheet millimetres (y up): label flags, power ports, sheet symbols with
//! their pins, notes, lines and rectangles, and the page frame with its
//! title block. The canvas and the PDF export both draw only from this, so
//! the screen and the print agree. Original geometry; the power port
//! shapes repeat the ground and supply drawings of
//! `library::symbol_style`.
use super::{PlacedUnit, junction_dots, placed_unit, wire_ends, wire_route, wiring};
use crate::geometry::{Bounds, Point, Shape, Transform2};
use crate::ids::SheetId;
use crate::library::graphics::TEXT_LINE_SPACING;
use crate::library::{HAlign, Justify, TextMetric, VAlign};
use crate::model::{
    Label, LabelKind, PortShape, PowerStyle, Project, SheetItem, SheetSide, SheetSymbol,
};
pub use crate::schematic::PlacedText;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Label and sheet-pin text height, mm.
pub const LABEL_TEXT_MM: f64 = 1.5;
/// Stroke widths, mm.
pub const LINE_MM: f64 = 0.2;
pub const WIRE_MM: f64 = 0.25;
pub const FRAME_MM: f64 = 0.35;
/// Page frame margins: the zone band lies between them, mm.
pub const FRAME_OUTER_MM: f64 = 5.0;
pub const FRAME_INNER_MM: f64 = 10.0;
/// Title block size, mm.
pub const TITLE_BLOCK_MM: Point = [110.0, 32.0];
/// Pitch of the frame's zone references, mm (nominal).
pub const ZONE_MM: f64 = 50.0;

/// One stroked (or filled) shape.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Stroke {
    pub shape: Shape,
    pub width: f64,
    pub filled: bool,
}

/// Shapes and text of one object.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Drawing {
    pub strokes: Vec<Stroke>,
    pub texts: Vec<PlacedText>,
}

impl Drawing {
    fn line(&mut self, a: Point, b: Point, width: f64) {
        self.strokes.push(Stroke {
            shape: Shape::Line { a, b },
            width,
            filled: false,
        });
    }
    fn polygon(&mut self, points: Vec<Point>, filled: bool) {
        self.strokes.push(Stroke {
            shape: Shape::polygon(points),
            width: LINE_MM,
            filled,
        });
    }
    fn rect(&mut self, a: Point, b: Point, width: f64) {
        self.strokes.push(Stroke {
            shape: Shape::rect(a, b),
            width,
            filled: false,
        });
    }
    fn text(&mut self, text: impl Into<String>, at: Point, height: f64, justify: Justify) {
        self.texts.push(PlacedText {
            graphic: None,
            text: text.into(),
            at,
            height,
            rotation_deg: 0.0,
            justify,
            mirrored: false,
            line_spacing: TEXT_LINE_SPACING,
        });
    }
    fn extend(&mut self, other: Drawing) {
        self.strokes.extend(other.strokes);
        self.texts.extend(other.texts);
    }
    /// Everything drawn, under the core text metric; `None` when empty.
    pub fn bounds(&self, m: TextMetric) -> Option<Bounds> {
        self.strokes
            .iter()
            .map(|s| s.shape.bounds(s.width))
            .chain(self.texts.iter().map(|t| t.bounds(m)))
            .reduce(Bounds::union)
    }
    /// Whether `p` lies within `tol` of anything drawn.
    pub fn hit(&self, p: Point, tol: f64, m: TextMetric) -> bool {
        self.texts
            .iter()
            .any(|t| t.bounds(m).inflated(tol).contains(p))
            || self.strokes.iter().any(|s| {
                s.shape.coarse_bounds(s.width).inflated(tol).contains(p)
                    && s.shape.distance(p, s.filled) <= tol + s.width / 2.0
            })
    }
}

fn justify(h: HAlign, v: VAlign) -> Justify {
    Justify { h, v }
}

/// Text anchored at the end of direction `d` stays horizontal and reads
/// away from the point it labels.
fn away(d: Point) -> Justify {
    if d[0].abs() >= d[1].abs() {
        justify(
            if d[0] >= 0.0 {
                HAlign::Left
            } else {
                HAlign::Right
            },
            VAlign::Middle,
        )
    } else {
        justify(
            HAlign::Center,
            if d[1] >= 0.0 {
                VAlign::Bottom
            } else {
                VAlign::Top
            },
        )
    }
}

fn text_width(text: &str, height: f64) -> f64 {
    TextMetric::default().advance * height * text.chars().count() as f64
}

/// A flag outline along +x from the origin, `length` long and `2 * half`
/// high: the point faces the origin for an input, away for an output,
/// both ways for bidirectional, none for passive.
fn flag(shape: PortShape, length: f64, half: f64) -> Vec<Point> {
    match shape {
        PortShape::Passive => vec![[0.0, half], [length, half], [length, -half], [0.0, -half]],
        PortShape::Input => vec![
            [0.0, 0.0],
            [half, half],
            [length, half],
            [length, -half],
            [half, -half],
        ],
        PortShape::Output => vec![
            [0.0, half],
            [length - half, half],
            [length, 0.0],
            [length - half, -half],
            [0.0, -half],
        ],
        PortShape::Bidirectional => vec![
            [0.0, 0.0],
            [half, half],
            [length - half, half],
            [length, 0.0],
            [length - half, -half],
            [half, -half],
        ],
    }
}

fn frame_of(label: &Label) -> Transform2 {
    Transform2 {
        translate: label.position,
        rotation_deg: label.rotation.degrees(),
        mirror: false,
    }
}

/// What a label or power port draws.
pub fn label(label: &Label) -> Drawing {
    let t = frame_of(label);
    let dir = {
        let o = t.apply([0.0, 0.0]);
        let x = t.apply([1.0, 0.0]);
        [x[0] - o[0], x[1] - o[1]]
    };
    let h = LABEL_TEXT_MM;
    let mut d = Drawing::default();
    match label.kind {
        LabelKind::Local => {
            // Text sits just above (or beside) the wire, reading away.
            let horizontal = dir[0].abs() >= dir[1].abs();
            let at = if horizontal {
                [label.position[0], label.position[1] + 0.4]
            } else {
                [label.position[0] - 0.4, label.position[1]]
            };
            let mut j = away(dir);
            j.v = VAlign::Bottom;
            if !horizontal {
                j.h = if dir[1] >= 0.0 {
                    HAlign::Left
                } else {
                    HAlign::Right
                };
            }
            d.texts.push(PlacedText {
                graphic: None,
                text: label.name.clone(),
                at,
                height: h,
                rotation_deg: if horizontal { 0.0 } else { 90.0 },
                justify: j,
                mirrored: false,
                line_spacing: TEXT_LINE_SPACING,
            });
        }
        LabelKind::Global { shape } => {
            let half = h;
            let length = text_width(&label.name, h) + 2.0 * half + 1.0;
            let outline = flag(shape, length, half)
                .into_iter()
                .map(|p| t.apply(p))
                .collect();
            d.polygon(outline, false);
            d.text(
                label.name.clone(),
                t.apply([length / 2.0, 0.0]),
                h,
                justify(HAlign::Center, VAlign::Middle),
            );
        }
        LabelKind::Hierarchical { shape } => {
            let half = h * 0.6;
            let length = 2.0 * half;
            let outline = flag(shape, length, half)
                .into_iter()
                .map(|p| t.apply(p))
                .collect();
            d.polygon(outline, false);
            d.text(
                label.name.clone(),
                t.apply([length + 0.6, 0.0]),
                h,
                away(dir),
            );
        }
        LabelKind::Power { style } => {
            let up = {
                let o = t.apply([0.0, 0.0]);
                let y = t.apply([0.0, 1.0]);
                [y[0] - o[0], y[1] - o[1]]
            };
            let p = |x: f64, y: f64| t.apply([x, y]);
            match style {
                PowerStyle::Ground => {
                    d.line(p(0.0, 0.0), p(0.0, -2.5), LINE_MM);
                    d.line(p(-1.5, -2.5), p(1.5, -2.5), LINE_MM);
                    d.line(p(-1.0, -3.1), p(1.0, -3.1), LINE_MM);
                    d.line(p(-0.5, -3.7), p(0.5, -3.7), LINE_MM);
                    d.text(label.name.clone(), p(0.0, -4.3), h, away([-up[0], -up[1]]));
                }
                PowerStyle::Bar => {
                    d.line(p(0.0, 0.0), p(0.0, 2.5), LINE_MM);
                    d.line(p(-1.5, 2.5), p(1.5, 2.5), LINE_MM);
                    d.text(label.name.clone(), p(0.0, 3.1), h, away(up));
                }
                PowerStyle::Arrow => {
                    d.line(p(0.0, 0.0), p(0.0, 1.5), LINE_MM);
                    d.polygon(vec![p(-1.0, 1.5), p(1.0, 1.5), p(0.0, 3.0)], true);
                    d.text(label.name.clone(), p(0.0, 3.6), h, away(up));
                }
            }
        }
    }
    d
}

/// The direction from a side of a sheet symbol into its box.
fn inward(side: SheetSide) -> f64 {
    match side {
        SheetSide::Left => 0.0,
        SheetSide::Bottom => 90.0,
        SheetSide::Right => 180.0,
        SheetSide::Top => 270.0,
    }
}

/// What a sheet symbol draws: its box, the child sheet's name above it and
/// page below it, and each pin's port shape and name inside the edge.
pub fn sheet_symbol(project: &Project, symbol: &SheetSymbol) -> Drawing {
    let mut d = Drawing::default();
    let [x, y] = symbol.position;
    let [w, h] = symbol.size;
    d.rect([x, y], [x + w, y - h], FRAME_MM);
    let child = project.sheet(symbol.child);
    d.text(
        child.map_or_else(|| "missing sheet".to_owned(), |s| s.name.clone()),
        [x, y + 0.8],
        LABEL_TEXT_MM * 1.2,
        justify(HAlign::Left, VAlign::Bottom),
    );
    if let Some(page) = project.sheets.number(symbol.child) {
        d.text(
            format!("Page {page}"),
            [x, y - h - 0.8],
            LABEL_TEXT_MM,
            justify(HAlign::Left, VAlign::Top),
        );
    }
    for pin in &symbol.pins {
        let t = Transform2 {
            translate: symbol.pin_position(pin),
            rotation_deg: inward(pin.side),
            mirror: false,
        };
        let half = LABEL_TEXT_MM * 0.6;
        let outline = flag(pin.shape, 2.0 * half, half)
            .into_iter()
            .map(|p| t.apply(p))
            .collect();
        d.polygon(outline, false);
        let o = t.apply([0.0, 0.0]);
        let i = t.apply([1.0, 0.0]);
        d.text(
            pin.name.clone(),
            t.apply([2.0 * half + 0.6, 0.0]),
            LABEL_TEXT_MM,
            away([i[0] - o[0], i[1] - o[1]]),
        );
    }
    d
}

/// What a note, line or rectangle draws.
pub fn item(item: &SheetItem) -> Drawing {
    let mut d = Drawing::default();
    match item {
        SheetItem::Text {
            at,
            text,
            height,
            rotation,
        } => d.texts.push(PlacedText {
            graphic: None,
            text: text.clone(),
            at: *at,
            height: *height,
            rotation_deg: rotation.degrees(),
            justify: justify(HAlign::Left, VAlign::Bottom),
            mirrored: false,
            line_spacing: TEXT_LINE_SPACING,
        }),
        SheetItem::Line { from, to } => d.line(*from, *to, LINE_MM),
        SheetItem::Rect { min, max } => d.rect(*min, *max, LINE_MM),
    }
    d
}

/// Zone letters down the frame: A, B, … Z, then AA, AB, ….
fn zone_letter(i: usize) -> String {
    let letters = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut s = String::new();
    let mut n = i;
    loop {
        s.insert(0, char::from(letters[n % 26]));
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    s
}

/// The page frame of a sheet: a border with zone references and the title
/// block in the bottom-right corner, filled from the project properties and
/// the sheet's page ("Sheet 2 of 5"). An empty title shows the project
/// name.
pub fn frame(project: &Project, sheet: SheetId) -> Drawing {
    let mut d = Drawing::default();
    let Some(page) = project.sheet(sheet) else {
        return d;
    };
    let [w, h] = page.size_mm();
    let (o, i) = (FRAME_OUTER_MM, FRAME_INNER_MM);
    d.rect([o, -o], [w - o, -(h - o)], LINE_MM);
    d.rect([i, -i], [w - i, -(h - i)], FRAME_MM);
    // Zones: equal divisions near the nominal pitch, numbered across and
    // lettered down, in the band between the two borders.
    let small = 1.8;
    let columns = ((w - 2.0 * i) / ZONE_MM).round().max(1.0) as usize;
    let rows = ((h - 2.0 * i) / ZONE_MM).round().max(1.0) as usize;
    let cw = (w - 2.0 * i) / columns as f64;
    let rh = (h - 2.0 * i) / rows as f64;
    for c in 0..columns {
        let x0 = i + cw * c as f64;
        if c > 0 {
            d.line([x0, -o], [x0, -i], LINE_MM);
            d.line([x0, -(h - i)], [x0, -(h - o)], LINE_MM);
        }
        let mid = x0 + cw / 2.0;
        let label = (c + 1).to_string();
        let centre = justify(HAlign::Center, VAlign::Middle);
        d.text(label.clone(), [mid, -(o + i) / 2.0], small, centre);
        d.text(label, [mid, -(h - (o + i) / 2.0)], small, centre);
    }
    for r in 0..rows {
        let y0 = -(i + rh * r as f64);
        if r > 0 {
            d.line([o, y0], [i, y0], LINE_MM);
            d.line([w - i, y0], [w - o, y0], LINE_MM);
        }
        let mid = y0 - rh / 2.0;
        let label = zone_letter(r);
        let centre = justify(HAlign::Center, VAlign::Middle);
        d.text(label.clone(), [(o + i) / 2.0, mid], small, centre);
        d.text(label, [w - (o + i) / 2.0, mid], small, centre);
    }
    // Title block: five rows, two columns below the title.
    let [bw, bh] = TITLE_BLOCK_MM;
    let (x1, y0) = (w - i, -(h - i));
    let x0 = x1 - bw;
    let y1 = y0 + bh;
    d.rect([x0, y0], [x1, y1], FRAME_MM);
    let props = &project.properties;
    let title = if props.title.trim().is_empty() {
        project.name.clone()
    } else {
        props.title.clone()
    };
    let row = (bh - 12.0) / 4.0;
    let title_bottom = y1 - 12.0;
    d.line([x0, title_bottom], [x1, title_bottom], LINE_MM);
    let caption = 1.2;
    let value = 2.0;
    let top_left = justify(HAlign::Left, VAlign::Top);
    let base_left = justify(HAlign::Left, VAlign::Bottom);
    d.text("Title", [x0 + 1.0, y1 - 0.8], caption, top_left);
    d.text(title, [x0 + 2.0, title_bottom + 2.0], 4.0, base_left);
    let mid = x0 + bw / 2.0;
    let count = project.sheets.list.len();
    let number = project.sheets.number(sheet).unwrap_or(0);
    let cells: [[(&str, String); 2]; 4] = [
        [
            ("Company", props.company.clone()),
            ("Author", props.author.clone()),
        ],
        [
            ("Date", props.date.clone()),
            ("Revision", props.revision.clone()),
        ],
        [
            ("Sheet", format!("{number} of {count}")),
            ("Size", page.paper.label().to_owned()),
        ],
        [
            ("Sheet name", page.name.clone()),
            ("Project", project.name.clone()),
        ],
    ];
    for (k, pair) in cells.into_iter().enumerate() {
        let top = title_bottom - row * k as f64;
        let bottom = top - row;
        if k > 0 {
            d.line([x0, top], [x1, top], LINE_MM);
        }
        d.line([mid, top], [mid, bottom], LINE_MM);
        for (c, (name, text)) in pair.into_iter().enumerate() {
            let x = if c == 0 { x0 } else { mid };
            d.text(name, [x + 1.0, top - 0.5], caption, top_left);
            d.text(text, [x + 1.5, bottom + 0.8], value, base_left);
        }
    }
    // Drawn with the page's top-left corner at the origin, then moved to
    // where the page lies on the sheet.
    let t = Transform2 {
        translate: page.top_left(),
        rotation_deg: 0.0,
        mirror: false,
    };
    for stroke in &mut d.strokes {
        stroke.shape = stroke.shape.transformed(&t);
    }
    for text in &mut d.texts {
        text.at = t.apply(text.at);
    }
    d
}

/// Everything one sheet draws, for printing and previews.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SheetScene {
    pub sheet: SheetId,
    /// Page width and height, mm.
    pub size: Point,
    /// The page's top-left corner on the sheet; the page spans
    /// x ∈ [x, x + w], y ∈ [y − h, y].
    pub top_left: Point,
    pub frame: Drawing,
    /// Labels, power ports, sheet symbols and furniture.
    pub items: Drawing,
    /// Each wire's drawn route.
    pub wires: Vec<[Point; 3]>,
    /// Junction dots.
    pub dots: Vec<Point>,
    /// No-connect crosses on decided pins.
    pub no_connects: Vec<Point>,
    pub units: Vec<PlacedUnit>,
}

/// Resolve one sheet's drawing.
pub fn sheet_scene(project: &Project, sheet: SheetId) -> SheetScene {
    let mut items = Drawing::default();
    for drawing in project.drawings.iter().filter(|d| d.sheet == sheet) {
        items.extend(item(&drawing.item));
    }
    for symbol in project.sheet_symbols.iter().filter(|s| s.sheet == sheet) {
        items.extend(sheet_symbol(project, symbol));
    }
    for l in project.labels.iter().filter(|l| l.sheet == sheet) {
        items.extend(label(l));
    }
    let wires = project
        .wires
        .iter()
        .filter(|w| wiring::wire_sheet(project, w) == Some(sheet))
        .filter_map(|w| wire_ends(project, w).map(|(a, b)| wire_route(a, b)))
        .collect();
    let dots = junction_dots(project)
        .into_iter()
        .filter(|(end, _)| wiring::end_sheet(project, *end) == Some(sheet))
        .map(|(_, p)| p)
        .collect();
    let units: Vec<PlacedUnit> = project
        .occurrences
        .iter()
        .filter(|o| o.sheet == sheet)
        .filter_map(|o| placed_unit(project, o.id).ok())
        .collect();
    let mut no_connects = Vec::new();
    for o in project.occurrences.iter().filter(|o| o.sheet == sheet) {
        let Some(instance) = project.instance(o.instance) else {
            continue;
        };
        if let Some(unit) = units.iter().find(|u| u.occurrence == Some(o.id)) {
            no_connects.extend(
                unit.pins
                    .iter()
                    .filter(|p| instance.no_connect.contains(&p.terminal))
                    .map(|p| p.end),
            );
        }
    }
    SheetScene {
        sheet,
        size: project.sheet(sheet).map_or([0.0, 0.0], |s| s.size_mm()),
        top_left: project
            .sheet(sheet)
            .map_or(crate::model::sheet::PAGE_TOP_LEFT, |s| s.top_left()),
        frame: frame(project, sheet),
        items,
        wires,
        dots,
        no_connects,
        units,
    }
}
