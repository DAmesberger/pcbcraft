//! Selection-wide edits of a symbol unit: move, rotate, mirror, duplicate,
//! delete and box selection. Each works on the working copy and keeps
//! identity: graphics keep their IDs, pins their terminals; duplicates get
//! fresh IDs from the symbol's `next_local`. Pure functions, so the canvas
//! and the tests share them.
use super::Selected;
use super::snap::segment_touches_box;
use pcbcraft_core::geometry::{Bounds, Transform2};
use pcbcraft_core::library::graphics::GRID_TOLERANCE_MM;
use pcbcraft_core::library::{
    Graphic, HAlign, Item, PinDirection, Rotation, SymbolDefinition, TextMetric, Unit, VAlign,
};
use pcbcraft_core::{GraphicId, Point};
use std::collections::BTreeSet;

pub type Selection = BTreeSet<Selected>;

/// The extent of the selected objects, `None` when nothing is selected.
pub fn bounds(unit: &Unit, selection: &Selection) -> Option<Bounds> {
    let m = TextMetric::default();
    let graphics = unit
        .graphics
        .iter()
        .filter(|g| selection.contains(&Selected::Graphic(g.id)))
        .map(|g| g.item.bounds(m));
    let pins = unit
        .pins
        .iter()
        .filter(|p| selection.contains(&Selected::Pin(p.terminal)))
        .map(|p| Bounds::point(p.position).including(p.body_end()));
    graphics.chain(pins).reduce(Bounds::union)
}

/// Move the selection by `delta`.
pub fn translate(unit: &mut Unit, selection: &Selection, delta: Point) {
    for g in &mut unit.graphics {
        if selection.contains(&Selected::Graphic(g.id)) {
            g.item = g.item.translated(delta);
        }
    }
    for p in &mut unit.pins {
        if selection.contains(&Selected::Pin(p.terminal)) {
            p.position = [p.position[0] + delta[0], p.position[1] + delta[1]];
        }
    }
}

fn rotate_direction(d: PinDirection) -> PinDirection {
    match d {
        PinDirection::Right => PinDirection::Up,
        PinDirection::Up => PinDirection::Left,
        PinDirection::Left => PinDirection::Down,
        PinDirection::Down => PinDirection::Right,
    }
}

fn next_rotation(r: Rotation) -> Rotation {
    match r {
        Rotation::R0 => Rotation::R90,
        Rotation::R90 => Rotation::R180,
        Rotation::R180 => Rotation::R270,
        Rotation::R270 => Rotation::R0,
    }
}

/// Round to the grid so a pivot keeps grid points on the grid.
fn snapped(p: Point, grid: f64) -> Point {
    super::snap::to_grid(p, grid)
}

/// The pivot rotation and mirroring turn about: the selection's centre on
/// the grid.
pub fn pivot(unit: &Unit, selection: &Selection, grid: f64) -> Option<Point> {
    bounds(unit, selection).map(|b| snapped(b.center(), grid))
}

/// Turn the selection 90° counter-clockwise about `about`. Pins turn with
/// it and keep their terminals; text turns a quarter.
pub fn rotate(unit: &mut Unit, selection: &Selection, about: Point) {
    let t = Transform2::translation([-about[0], -about[1]]).then(&Transform2 {
        translate: about,
        rotation_deg: 90.0,
        mirror: false,
    });
    for g in &mut unit.graphics {
        if !selection.contains(&Selected::Graphic(g.id)) {
            continue;
        }
        g.item = match &g.item {
            Item::Shape { shape, width, fill } => Item::Shape {
                shape: shape.transformed(&t),
                width: *width,
                fill: *fill,
            },
            Item::Text(text) => {
                let mut text = text.clone();
                text.at = t.apply(text.at);
                text.rotation = next_rotation(text.rotation);
                Item::Text(text)
            }
        };
    }
    for p in &mut unit.pins {
        if selection.contains(&Selected::Pin(p.terminal)) {
            p.position = t.apply(p.position);
            p.direction = rotate_direction(p.direction);
        }
    }
}

/// Mirror the selection left to right about the vertical line through
/// `about`. Text stays readable: its anchor mirrors and its justification
/// swaps sides.
pub fn mirror(unit: &mut Unit, selection: &Selection, about: Point) {
    // Reflect across x (y → −y), then turn 180°: x → −x.
    let t = Transform2::translation([-about[0], -about[1]]).then(&Transform2 {
        translate: about,
        rotation_deg: 180.0,
        mirror: true,
    });
    for g in &mut unit.graphics {
        if !selection.contains(&Selected::Graphic(g.id)) {
            continue;
        }
        g.item = match &g.item {
            Item::Shape { shape, width, fill } => Item::Shape {
                shape: shape.transformed(&t),
                width: *width,
                fill: *fill,
            },
            Item::Text(text) => {
                let mut text = text.clone();
                text.at = t.apply(text.at);
                if matches!(text.rotation, Rotation::R0 | Rotation::R180) {
                    text.justify.h = match text.justify.h {
                        HAlign::Left => HAlign::Right,
                        HAlign::Center => HAlign::Center,
                        HAlign::Right => HAlign::Left,
                    };
                } else {
                    text.justify.v = match text.justify.v {
                        VAlign::Top => VAlign::Bottom,
                        VAlign::Middle => VAlign::Middle,
                        VAlign::Bottom => VAlign::Top,
                    };
                }
                Item::Text(text)
            }
        };
    }
    for p in &mut unit.pins {
        if selection.contains(&Selected::Pin(p.terminal)) {
            p.position = t.apply(p.position);
            p.direction = match p.direction {
                PinDirection::Right => PinDirection::Left,
                PinDirection::Left => PinDirection::Right,
                other => other,
            };
        }
    }
}

/// Copy the selected graphics, offset by `offset`, with new IDs; pins are
/// not copied (a terminal has one pin). Returns the new selection and how
/// many pins were skipped, or `None` when IDs ran out (nothing changes).
pub fn duplicate(
    symbol: &mut SymbolDefinition,
    unit: usize,
    selection: &Selection,
    offset: Point,
) -> Option<(Selection, usize)> {
    let source = symbol.units.get(unit)?;
    let copies: Vec<Item> = source
        .graphics
        .iter()
        .filter(|g| selection.contains(&Selected::Graphic(g.id)))
        .map(|g| g.item.translated(offset))
        .collect();
    let skipped = selection
        .iter()
        .filter(|s| matches!(s, Selected::Pin(_)))
        .count();
    let first = symbol.next_local;
    let next = first.checked_add(u32::try_from(copies.len()).ok()?)?;
    let target = symbol.units.get_mut(unit)?;
    let mut selected = Selection::new();
    for (i, item) in copies.into_iter().enumerate() {
        let id = GraphicId(first.checked_add(u32::try_from(i).ok()?)?);
        target.graphics.push(Graphic { id, item });
        selected.insert(Selected::Graphic(id));
    }
    symbol.next_local = next;
    Some((selected, skipped))
}

/// Remove the selected graphics and pins. Terminals stay: a pin-less
/// terminal can be placed again with the Pin tool.
pub fn delete(unit: &mut Unit, selection: &Selection) {
    unit.graphics
        .retain(|g| !selection.contains(&Selected::Graphic(g.id)));
    unit.pins
        .retain(|p| !selection.contains(&Selected::Pin(p.terminal)));
}

/// Objects a box from `a` to `b` selects. A box dragged to the right is a
/// window and takes what lies wholly inside; to the left it is a crossing
/// and also takes what it touches.
pub fn boxed(unit: &Unit, a: Point, b: Point) -> Selection {
    let window = b[0] >= a[0];
    let area = Bounds::point(a).including(b);
    let m = TextMetric::default();
    let inside = |x: &Bounds| {
        area.inflated(GRID_TOLERANCE_MM).contains(x.min)
            && area.inflated(GRID_TOLERANCE_MM).contains(x.max)
    };
    let mut out = Selection::new();
    for g in &unit.graphics {
        let extent = g.item.bounds(m);
        let hit = if window {
            inside(&extent)
        } else {
            match &g.item {
                Item::Shape { shape, .. } => {
                    let mut ring = shape.flatten(0.01);
                    if shape.is_closed()
                        && let Some(first) = ring.first().copied()
                    {
                        ring.push(first);
                    }
                    ring.len() == 1 && area.contains(ring[0])
                        || ring
                            .windows(2)
                            .any(|w| segment_touches_box(w[0], w[1], area.min, area.max))
                        || (shape.is_closed() && shape.distance(area.center(), true) == 0.0)
                }
                Item::Text(_) => extent.intersects(&area),
            }
        };
        if hit {
            out.insert(Selected::Graphic(g.id));
        }
    }
    for p in &unit.pins {
        let stub = Bounds::point(p.position).including(p.body_end());
        let hit = if window {
            inside(&stub)
        } else {
            segment_touches_box(p.position, p.body_end(), area.min, area.max)
        };
        if hit {
            out.insert(Selected::Pin(p.terminal));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::geometry::Shape;
    use pcbcraft_core::library::graphics::{self, Fill};
    use pcbcraft_core::library::{Justify, Pin, TextContent};
    use pcbcraft_core::{TerminalId, UnitId};

    fn unit() -> Unit {
        Unit {
            id: UnitId(1),
            label: "A".into(),
            graphics: vec![
                Graphic {
                    id: GraphicId(2),
                    item: Item::Shape {
                        shape: Shape::rect([0.0, 0.0], [5.0, 2.5]),
                        width: 0.25,
                        fill: Fill::None,
                    },
                },
                graphics::text(
                    GraphicId(3),
                    TextContent::Literal("IN".into()),
                    [0.0, 5.0],
                    Justify {
                        h: HAlign::Left,
                        v: VAlign::Middle,
                    },
                ),
            ],
            pins: vec![Pin::new(
                TerminalId(1),
                [-2.5, 0.0],
                PinDirection::Right,
                2.5,
            )],
        }
    }

    fn all() -> Selection {
        [
            Selected::Graphic(GraphicId(2)),
            Selected::Graphic(GraphicId(3)),
            Selected::Pin(TerminalId(1)),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn four_quarter_turns_and_two_mirrors_restore_the_unit() {
        let original = unit();
        let mut u = original.clone();
        let about = [2.5, 2.5];
        rotate(&mut u, &all(), about);
        assert_eq!(u.pins[0].direction, PinDirection::Up);
        assert_eq!(u.pins[0].position, [5.0, -2.5]);
        assert!(matches!(
            &u.graphics[0].item,
            Item::Shape {
                shape: Shape::Rect { .. },
                ..
            }
        ));
        for _ in 0..3 {
            rotate(&mut u, &all(), about);
        }
        assert_eq!(u, original);
        mirror(&mut u, &all(), about);
        assert_eq!(u.pins[0].direction, PinDirection::Left);
        assert_eq!(u.pins[0].position, [7.5, 0.0]);
        assert!(
            matches!(&u.graphics[1].item, Item::Text(t) if t.justify.h == HAlign::Right && t.at == [5.0, 5.0])
        );
        mirror(&mut u, &all(), about);
        assert_eq!(u, original);
    }

    #[test]
    fn duplicates_get_fresh_ids_and_skip_pins() -> Result<(), String> {
        let mut symbol = SymbolDefinition {
            next_local: 4,
            units: vec![unit()],
            ..Default::default()
        };
        let (selected, skipped) =
            duplicate(&mut symbol, 0, &all(), [2.5, -2.5]).ok_or("duplicated")?;
        assert_eq!(skipped, 1);
        assert_eq!(symbol.next_local, 6);
        assert_eq!(
            selected,
            [
                Selected::Graphic(GraphicId(4)),
                Selected::Graphic(GraphicId(5))
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(symbol.units[0].graphics.len(), 4);
        assert_eq!(symbol.units[0].pins.len(), 1);
        delete(&mut symbol.units[0], &selected);
        assert_eq!(symbol.units[0], unit());
        Ok(())
    }

    #[test]
    fn window_boxes_take_what_is_inside_and_crossing_boxes_what_they_touch() {
        let u = unit();
        // Rightward over the rectangle only.
        let window = boxed(&u, [-0.5, -0.5], [5.5, 3.0]);
        assert_eq!(
            window,
            [Selected::Graphic(GraphicId(2))].into_iter().collect()
        );
        // Leftward over the rectangle's left edge and the pin stub.
        let crossing = boxed(&u, [1.0, -0.5], [-1.0, 0.5]);
        assert_eq!(
            crossing,
            [
                Selected::Graphic(GraphicId(2)),
                Selected::Pin(TerminalId(1))
            ]
            .into_iter()
            .collect()
        );
    }
}
