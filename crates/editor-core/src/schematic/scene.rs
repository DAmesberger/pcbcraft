//! What a sheet draws besides symbols, resolved to plain shapes and text in
//! sheet millimetres (y up): label flags, power ports, sheet symbols with
//! their pins, notes, lines and rectangles, and the page frame with its
//! title block. The canvas and the PDF export both draw only from this, so
//! the screen and the print agree. Original geometry; the power port
//! shapes repeat the ground and supply drawings of
//! `library::symbol_style`.
use super::{PlacedUnit, junction_dots, labels, placed_unit, wire_ends, wire_route, wiring};
use crate::geometry::{Bounds, Point, Shape, Transform2};
use crate::ids::Id;
use crate::ids::SheetId;
use crate::library::Rotation;
use crate::library::graphics::{PIN_TEXT_HEIGHT_MM, SYMBOL_MODULE_MM, TEXT_LINE_SPACING};
use crate::library::{HAlign, Justify, TextMetric, VAlign};
use crate::model::{
    Label, LabelKind, PortShape, PowerStyle, Project, SheetItem, SheetSide, SheetSymbol,
};
pub use crate::schematic::PlacedText;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Label, power-port and sheet-pin text height, mm: the pin name and
/// number height, so a net name reads as one with the pins it labels and
/// its outline fits a pin pitch.
pub const LABEL_TEXT_MM: f64 = PIN_TEXT_HEIGHT_MM;
/// Half the height of a label's outline (a global label's flag, a local
/// label's tag), mm: 2 mm tall, so outlines on adjacent 2.5 mm pins keep a
/// gap, and the name's underscores clear the edge.
const FLAG_HALF_MM: f64 = 0.4 * SYMBOL_MODULE_MM;
/// A local label's tag: space either side of the name, and its corner
/// radius, mm.
const TAG_PAD_MM: f64 = 0.6;
const TAG_RADIUS_MM: f64 = 0.5;
/// The steps a label that places itself may stand off its pin, along a
/// leader, to clear what is drawn there: a symbol module each.
const LEADER_STEP_MM: f64 = SYMBOL_MODULE_MM;
const MAX_LEADER_STEPS: usize = 6;
/// How wide label text is measured, per cap height: net names are mostly
/// capitals and digits, which print at about 0.68 em in Helvetica against
/// its 0.718 em cap height, so the core metric's 0.6 is too narrow for
/// them. Flags are sized and labels kept apart by it.
pub const LABEL_METRIC: TextMetric = TextMetric { advance: 0.95 };
/// The gap a label that places itself keeps from other drawing, mm.
const LABEL_CLEARANCE_MM: f64 = 0.2;
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
    LABEL_METRIC.advance * height * text.chars().count() as f64
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

/// What a label or power port draws where a person put it: at its stored
/// position and rotation. A label that places itself is drawn by
/// [`sheet_labels`].
pub fn label(label: &Label) -> Drawing {
    label_at(label, label.position, label.rotation, 0.0, false)
}

/// What a label draws with its connection point at `at`, turned by
/// `rotation`, standing `leader` mm off that point along its facing on a
/// wire-width leader. With `upright_name`, a power port's name reads
/// along its stem rather than across, so ports pointing up or down from a
/// dense row of pins fit the pitch.
fn label_at(
    label: &Label,
    at: Point,
    rotation: Rotation,
    leader: f64,
    upright_name: bool,
) -> Drawing {
    let turn = Transform2 {
        translate: [0.0, 0.0],
        rotation_deg: rotation.degrees(),
        mirror: false,
    };
    // The way the drawing faces: along +x for flags and text, up for a
    // supply port, down for a ground.
    let facing = match label.kind {
        LabelKind::Power {
            style: PowerStyle::Ground,
        } => [0.0, -1.0],
        LabelKind::Power { .. } => [0.0, 1.0],
        _ => [1.0, 0.0],
    };
    let out = turn.apply(facing);
    let mut d = Drawing::default();
    let origin = if leader > 0.0 {
        let end = [at[0] + out[0] * leader, at[1] + out[1] * leader];
        d.line(at, end, WIRE_MM);
        end
    } else {
        at
    };
    let t = Transform2 {
        translate: origin,
        ..turn
    };
    let dir = {
        let o = t.apply([0.0, 0.0]);
        let x = t.apply([1.0, 0.0]);
        [x[0] - o[0], x[1] - o[1]]
    };
    let h = LABEL_TEXT_MM;
    match label.kind {
        LabelKind::Local => {
            // A tag with rounded corners, square to the wire, set apart
            // from a global label's pointed flag; the name runs along it.
            let half = FLAG_HALF_MM;
            let length = text_width(&label.name, h) + 2.0 * TAG_PAD_MM;
            let corners = [[0.0, half], [length, half], [length, -half], [0.0, -half]];
            d.strokes.push(Stroke {
                shape: Shape::Polygon {
                    points: corners.into_iter().map(|p| t.apply(p)).collect(),
                    radii: vec![TAG_RADIUS_MM; 4],
                },
                width: LINE_MM,
                filled: false,
            });
            let mut text = along(label.name.clone(), t.apply([length / 2.0, 0.0]), dir);
            text.justify.h = HAlign::Center;
            d.texts.push(text);
        }
        LabelKind::Global { shape } => {
            let half = FLAG_HALF_MM;
            let length = text_width(&label.name, h) + 2.0 * half + 1.0;
            let outline = flag(shape, length, half)
                .into_iter()
                .map(|p| t.apply(p))
                .collect();
            d.polygon(outline, false);
            // The name runs along the flag: across, or up a standing flag.
            let mut text = along(label.name.clone(), t.apply([length / 2.0, 0.0]), dir);
            text.justify.h = HAlign::Center;
            d.texts.push(text);
        }
        LabelKind::Hierarchical { shape } => {
            let half = h * 0.6;
            let length = 2.0 * half;
            let outline = flag(shape, length, half)
                .into_iter()
                .map(|p| t.apply(p))
                .collect();
            d.polygon(outline, false);
            d.texts
                .push(along(label.name.clone(), t.apply([length + 0.6, 0.0]), dir));
        }
        LabelKind::Power { style } => {
            let up = {
                let o = t.apply([0.0, 0.0]);
                let y = t.apply([0.0, 1.0]);
                [y[0] - o[0], y[1] - o[1]]
            };
            let p = |x: f64, y: f64| t.apply([x, y]);
            let (name_at, facing) = match style {
                PowerStyle::Ground => {
                    d.line(p(0.0, 0.0), p(0.0, -2.5), LINE_MM);
                    d.line(p(-1.5, -2.5), p(1.5, -2.5), LINE_MM);
                    d.line(p(-1.0, -3.1), p(1.0, -3.1), LINE_MM);
                    d.line(p(-0.5, -3.7), p(0.5, -3.7), LINE_MM);
                    (p(0.0, -4.3), [-up[0], -up[1]])
                }
                PowerStyle::Bar => {
                    d.line(p(0.0, 0.0), p(0.0, 2.5), LINE_MM);
                    d.line(p(-1.5, 2.5), p(1.5, 2.5), LINE_MM);
                    (p(0.0, 3.1), up)
                }
                PowerStyle::Arrow => {
                    d.line(p(0.0, 0.0), p(0.0, 1.5), LINE_MM);
                    d.polygon(vec![p(-1.0, 1.5), p(1.0, 1.5), p(0.0, 3.0)], true);
                    (p(0.0, 3.6), up)
                }
            };
            if upright_name {
                d.texts.push(along(label.name.clone(), name_at, facing));
            } else {
                d.text(label.name.clone(), name_at, h, away(facing));
            }
        }
    }
    d
}

/// Label text starting at `at` and reading along the axis `dir`: across
/// for a horizontal axis, bottom to top for a vertical one (the upright
/// rule turns text facing left or down).
fn along(text: String, at: Point, dir: Point) -> PlacedText {
    let rotation_deg = if dir[0].abs() >= dir[1].abs() {
        if dir[0] >= 0.0 { 0.0 } else { 180.0 }
    } else if dir[1] >= 0.0 {
        90.0
    } else {
        270.0
    };
    PlacedText {
        graphic: None,
        text,
        at,
        height: LABEL_TEXT_MM,
        rotation_deg,
        justify: justify(HAlign::Left, VAlign::Middle),
        mirrored: false,
        line_spacing: TEXT_LINE_SPACING,
    }
}

/// Boxes of everything a drawing inks, one per stroke and text.
fn ink_boxes(d: &Drawing, m: TextMetric) -> Vec<Bounds> {
    d.strokes
        .iter()
        .map(|s| s.shape.bounds(s.width))
        .chain(d.texts.iter().map(|t| t.bounds(m)))
        .collect()
}

/// Whether two boxes overlap by more than touching.
fn overlap(a: &Bounds, b: &Bounds) -> bool {
    a.min[0] < b.max[0] && b.min[0] < a.max[0] && a.min[1] < b.max[1] && b.min[1] < a.max[1]
}

/// What the symbols of a sheet ink, as boxes a label must stay clear of:
/// body shapes, texts and fields, pin names and numbers, and pin stubs
/// with the end each stub's pin connects at.
fn unit_obstacles(units: &[PlacedUnit], m: TextMetric) -> (Vec<Bounds>, Vec<(Point, Bounds)>) {
    let mut boxes = Vec::new();
    let mut stubs = Vec::new();
    for unit in units {
        boxes.extend(unit.shapes.iter().map(|s| s.shape.bounds(s.width)));
        boxes.extend(unit.texts.iter().chain(&unit.fields).map(|t| t.bounds(m)));
        for pin in &unit.pins {
            boxes.extend(pin.name.iter().chain(&pin.number).map(|t| t.bounds(m)));
            boxes.extend(pin.decorations.iter().map(|d| d.bounds(LINE_MM)));
            stubs.push((
                pin.end,
                Bounds::point(pin.end)
                    .including(pin.body_end)
                    .inflated(LINE_MM / 2.0),
            ));
        }
    }
    (boxes, stubs)
}

/// Every label and power port of a sheet with what it draws, in the
/// project's order: the one placement the canvas, the PDF and previews
/// share.
///
/// A label a person placed (or one not attached to a pin) is drawn where
/// it was put. A label that places itself ([`labels::attached_pin`]) sits
/// on its pin's end facing away from the symbol, and stands off along a
/// leader, a module at a time, until it clears every symbol's body, text
/// and pins and every label drawn before it. Labels are placed down each
/// row of pins in turn, so a dense row alternates between two depths.
pub fn sheet_labels(project: &Project, sheet: SheetId) -> Vec<(Id, Drawing)> {
    let units: Vec<PlacedUnit> = project
        .occurrences
        .iter()
        .filter(|o| o.sheet == sheet)
        .filter_map(|o| placed_unit(project, o.id).ok())
        .collect();
    labels_on(project, sheet, &units)
}

fn labels_on(project: &Project, sheet: SheetId, units: &[PlacedUnit]) -> Vec<(Id, Drawing)> {
    let m = LABEL_METRIC;
    let (mut taken, stubs) = unit_obstacles(units, m);
    let on_sheet: Vec<&Label> = project.labels.iter().filter(|l| l.sheet == sheet).collect();
    let mut drawn: Vec<Option<Drawing>> = vec![None; on_sheet.len()];
    // Where each self-placing label's pin ends and which way it faces.
    let mut auto: Vec<(usize, Point, Point)> = Vec::new();
    // How far past its pin's end each label starts: clear of a pin name
    // or number that runs beyond the end (a long number on a short pin).
    let mut start = vec![0.0; on_sheet.len()];
    for (i, l) in on_sheet.iter().enumerate() {
        let frame = labels::attached_pin(project, l).and_then(|e| {
            units.iter().find_map(|u| {
                let occurrence = project.occurrence(u.occurrence?)?;
                if occurrence.instance != e.instance {
                    return None;
                }
                let pin = u.pins.iter().find(|p| p.terminal == e.terminal)?;
                let d = labels::outward(u, e.terminal)?;
                let reach = |p: Point| (p[0] - pin.end[0]) * d[0] + (p[1] - pin.end[1]) * d[1];
                let past = pin
                    .name
                    .iter()
                    .chain(&pin.number)
                    .flat_map(|t| t.bounds(m).corners())
                    .map(reach)
                    .fold(f64::NEG_INFINITY, f64::max);
                Some((pin.end, d, past))
            })
        });
        match frame {
            Some((end, d, past)) => {
                if past > -LABEL_CLEARANCE_MM {
                    start[i] = past + 2.0 * LABEL_CLEARANCE_MM;
                }
                auto.push((i, end, d));
            }
            None => {
                let d = label(l);
                taken.extend(ink_boxes(&d, m));
                drawn[i] = Some(d);
            }
        }
    }
    // Row by row: by facing, then across the row, then along it.
    let key = |p: Point, d: Point| {
        let side = (d[0] * 2.0 + d[1]) as i64;
        let (across, along) = if d[0] != 0.0 {
            (p[0], -p[1])
        } else {
            (p[1], p[0])
        };
        (
            side,
            (across * 1000.0).round() as i64,
            (along * 1000.0).round() as i64,
        )
    };
    auto.sort_by_key(|(i, p, d)| (key(*p, *d), *i));
    // Each self-placing label's pin end, facing and name width.
    let rows: Vec<(Point, Point, f64)> = auto
        .iter()
        .filter_map(|(i, p, d)| Some((*p, *d, text_width(&on_sheet.get(*i)?.name, LABEL_TEXT_MM))))
        .collect();
    for (i, end, d) in auto {
        let Some(l) = on_sheet.get(i) else {
            continue;
        };
        let rotation = labels::auto_rotation(l.kind, d);
        let others = |b: &Bounds| {
            taken.iter().filter(|t| overlap(b, t)).count()
                + stubs
                    .iter()
                    .filter(|(at, s)| *at != end && overlap(b, s))
                    .count()
        };
        // A power port's name may also read along its stem, and does so
        // from the start when it would reach across a neighbouring label
        // on the same row (a port pointing up or down from a pin row).
        let crowded = rows.iter().any(|(p, e, width)| {
            *e == d && (p[0] - end[0]) * d[0] + (p[1] - end[1]) * d[1] == 0.0 && {
                let gap = ((p[0] - end[0]) * d[1]).abs() + ((p[1] - end[1]) * d[0]).abs();
                gap > 1e-9 && gap < (width + text_width(&l.name, LABEL_TEXT_MM)) / 2.0 + 0.5
            }
        });
        let names: &[bool] = match l.kind {
            LabelKind::Power { .. } if crowded && d[1] != 0.0 => &[true],
            LabelKind::Power { .. } => &[false, true],
            _ => &[false],
        };
        let mut best: Option<(usize, Drawing)> = None;
        'search: for step in 0..=MAX_LEADER_STEPS {
            for upright in names {
                let leader = start[i] + step as f64 * LEADER_STEP_MM;
                let candidate = label_at(l, end, rotation, leader, *upright);
                let hits: usize = ink_boxes(&candidate, m)
                    .iter()
                    .map(|b| others(&b.inflated(LABEL_CLEARANCE_MM)))
                    .sum();
                if best.as_ref().is_none_or(|(h, _)| hits < *h) {
                    best = Some((hits, candidate));
                }
                if hits == 0 {
                    break 'search;
                }
            }
        }
        // No room along the pin (another label or port faces it across a
        // short gap): step aside, a module at a time either way, on a
        // leader that runs a little beyond the pin's end and turns, so it
        // never runs along the row of pin ends. The turn comes at a fifth,
        // a half or four fifths of a module, so the leaders of neighbouring
        // labels that both step aside pass each other.
        if best.as_ref().is_some_and(|(h, _)| *h > 0) {
            let across = [-d[1], d[0]];
            // A ground steps down first and a supply up, so a turned port
            // still points the usual way where it can.
            let up = match l.kind {
                LabelKind::Power {
                    style: PowerStyle::Ground,
                } => -1.0,
                LabelKind::Power { .. } => 1.0,
                _ => 0.0,
            };
            let sides = if across[1] * up < 0.0 {
                [-1.0, 1.0]
            } else {
                [1.0, -1.0]
            };
            'aside: for step in 1..=MAX_LEADER_STEPS {
                for side in sides {
                    for turn in [0.5, 0.2, 0.8] {
                        let out = start[i] + turn * LEADER_STEP_MM;
                        let bend = [end[0] + d[0] * out, end[1] + d[1] * out];
                        let by = side * step as f64 * LEADER_STEP_MM;
                        let corner = [bend[0] + across[0] * by, bend[1] + across[1] * by];
                        for upright in names {
                            let mut candidate = match l.kind {
                                // A port turns with its leader: its stem
                                // runs aside and its symbol ends it.
                                LabelKind::Power { .. } => label_at(
                                    l,
                                    bend,
                                    labels::auto_rotation(
                                        l.kind,
                                        [across[0] * side, across[1] * side],
                                    ),
                                    (step - 1) as f64 * LEADER_STEP_MM,
                                    *upright,
                                ),
                                _ => {
                                    let mut flag = label_at(l, corner, rotation, 0.0, *upright);
                                    flag.line(bend, corner, WIRE_MM);
                                    flag
                                }
                            };
                            candidate.line(end, bend, WIRE_MM);
                            let hits: usize = ink_boxes(&candidate, m)
                                .iter()
                                .map(|b| others(&b.inflated(LABEL_CLEARANCE_MM)))
                                .sum();
                            if best.as_ref().is_none_or(|(h, _)| hits < *h) {
                                best = Some((hits, candidate));
                            }
                            if hits == 0 {
                                break 'aside;
                            }
                        }
                    }
                }
            }
        }
        if let Some((_, d)) = best {
            taken.extend(ink_boxes(&d, m));
            drawn[i] = Some(d);
        }
    }
    on_sheet
        .iter()
        .zip(drawn)
        .filter_map(|(l, d)| Some((l.id, d?)))
        .collect()
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
    let units: Vec<PlacedUnit> = project
        .occurrences
        .iter()
        .filter(|o| o.sheet == sheet)
        .filter_map(|o| placed_unit(project, o.id).ok())
        .collect();
    for (_, drawing) in labels_on(project, sheet, &units) {
        items.extend(drawing);
    }
    // A label sitting on its pin's end leaves its wire no length to draw.
    let wires = project
        .wires
        .iter()
        .filter(|w| wiring::wire_sheet(project, w) == Some(sheet))
        .filter_map(|w| wire_ends(project, w).filter(|(a, b)| a != b))
        .map(|(a, b)| wire_route(a, b))
        .collect();
    let dots = junction_dots(project)
        .into_iter()
        .filter(|(end, _)| wiring::end_sheet(project, *end) == Some(sheet))
        .map(|(_, p)| p)
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
