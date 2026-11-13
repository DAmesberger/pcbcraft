//! Symbol drawing in nm, on Sparksmith's symbol grid.
//!
//! The conventions match the host's symbol style: a 2.5 mm module, pins
//! ending on the grid, the designator above the body and the component
//! name below it.
use crate::units::div_round;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use sparksmith_generator_abi::v1::{
    Fill, Graphic, HAlign, Point, Primitive, SymbolPin, TextContent, Unit, VAlign,
};

/// The symbol grid module.
pub const MODULE_NM: i64 = 2_500_000;
/// Body stroke.
pub const STROKE_NM: i64 = 250_000;
/// Capacitor plate stroke.
pub const PLATE_STROKE_NM: i64 = 350_000;
/// Text height of the designator and name.
pub const TEXT_HEIGHT_NM: i64 = 1_250_000;
/// The two connection ends of a horizontal two-terminal symbol, ±x.
pub const TWO_TERMINAL_PIN_X_NM: i64 = 2 * MODULE_NM;
/// Arrow head length and half width.
pub const ARROW_NM: [i64; 2] = [600_000, 300_000];
/// Gap between the body and its labels.
pub const LABEL_GAP_NM: i64 = 500_000;

/// Pin directions as `rot_mdeg`: the way a pin reaches from its connection
/// point towards the body.
pub const RIGHT: i64 = 0;
pub const UP: i64 = 90_000;
pub const LEFT: i64 = 180_000;
pub const DOWN: i64 = 270_000;

/// One symbol unit being drawn.
#[derive(Default)]
pub struct Drawing {
    pub pins: Vec<SymbolPin>,
    pub graphics: Vec<Graphic>,
    /// Show pin numbers (off for two-terminal passives).
    pub numbered: bool,
}

impl Drawing {
    pub fn shape(&mut self, shape: Primitive, stroke_nm: i64, fill: Fill) {
        self.graphics.push(Graphic {
            shape,
            stroke_nm,
            fill,
        });
    }

    /// An unfilled shape in the body stroke.
    pub fn open(&mut self, shape: Primitive) {
        self.shape(shape, STROKE_NM, Fill::None);
    }

    pub fn line(&mut self, a: Point, b: Point) {
        self.open(Primitive::Line { a, b });
    }

    /// A capacitor plate.
    pub fn plate(&mut self, a: Point, b: Point) {
        self.shape(Primitive::Line { a, b }, PLATE_STROKE_NM, Fill::None);
    }

    pub fn pin(&mut self, terminal: &str, at: Point, rot_mdeg: i64, length_nm: i64) {
        self.pins.push(SymbolPin {
            terminal: terminal.into(),
            at,
            length_nm,
            rot_mdeg,
            show_name: false,
            show_number: self.numbered,
        });
    }

    /// Terminal `a` at x = −5 mm and `b` at x = +5 mm on y = 0, their stubs
    /// reaching the body at −`left` and +`right`.
    pub fn two_pins(&mut self, a: &str, b: &str, left: i64, right: i64) {
        let x = TWO_TERMINAL_PIN_X_NM;
        self.pin(a, [-x, 0], RIGHT, x - left);
        self.pin(b, [x, 0], LEFT, x - right);
    }

    /// A solid arrow head with its tip at `tip`, pointing away from `from`.
    pub fn arrow_head(&mut self, from: Point, tip: Point) {
        let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
        let Some(length) = dx
            .checked_mul(dx)
            .and_then(|xx| dy.checked_mul(dy).and_then(|yy| xx.checked_add(yy)))
            .map(i64::isqrt)
        else {
            return;
        };
        if length == 0 {
            return;
        }
        // Unit vector scaled by a length: (d * l) / |d|, rounded.
        let scale = |d: i64, l: i64| d.checked_mul(l).and_then(|v| div_round(v, length));
        let [l, w] = ARROW_NM;
        let (Some(bx), Some(by), Some(wx), Some(wy)) =
            (scale(dx, l), scale(dy, l), scale(dx, w), scale(dy, w))
        else {
            return;
        };
        let back = [tip[0] - bx, tip[1] - by];
        self.shape(
            Primitive::Polygon {
                points: vec![
                    tip,
                    [back[0] - wy, back[1] + wx],
                    [back[0] + wy, back[1] - wx],
                ],
            },
            STROKE_NM,
            Fill::Solid,
        );
    }

    /// A line from `from` to `tip` with an arrow head at `tip`.
    pub fn arrow(&mut self, from: Point, tip: Point) {
        self.line(from, tip);
        self.arrow_head(from, tip);
    }

    /// The designator above `top` and the component name below `bottom`.
    pub fn labels(&mut self, top: i64, bottom: i64) {
        self.text(
            TextContent::Designator,
            [0, top + LABEL_GAP_NM],
            VAlign::Bottom,
        );
        self.text(
            TextContent::ComponentName,
            [0, bottom - LABEL_GAP_NM],
            VAlign::Top,
        );
    }

    fn text(&mut self, content: TextContent, at: Point, v_align: VAlign) {
        self.shape(
            Primitive::Text {
                at,
                height_nm: TEXT_HEIGHT_NM,
                content,
                h_align: HAlign::Center,
                v_align,
            },
            0,
            Fill::None,
        );
    }

    /// The finished unit.
    pub fn unit(self, key: &str) -> Unit {
        Unit {
            key: String::from(key),
            pins: self.pins,
            graphics: self.graphics,
        }
    }
}

/// `p` rotated by 45° about the origin, rounded to the nm.
pub fn rotate_45(p: Point) -> Option<Point> {
    // 1/√2 to 1e-12, as a ratio of integers.
    const NUM: i64 = 707_106_781_187;
    const DEN: i64 = 1_000_000_000_000;
    let r = |v: i64| v.checked_mul(NUM).and_then(|x| div_round(x, DEN));
    Some([r(p[0].checked_sub(p[1])?)?, r(p[0].checked_add(p[1])?)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_head_is_a_triangle_at_the_tip() {
        let mut d = Drawing::default();
        d.arrow_head([0, 0], [1_000_000, 0]);
        assert_eq!(
            d.graphics.first().map(|g| g.shape.clone()),
            Some(Primitive::Polygon {
                points: vec![[1_000_000, 0], [400_000, 300_000], [400_000, -300_000]]
            })
        );
    }

    #[test]
    fn rotates_by_45_degrees() {
        assert_eq!(rotate_45([1_000_000, 0]), Some([707_107, 707_107]));
        assert_eq!(rotate_45([0, 1_000_000]), Some([-707_107, 707_107]));
    }
}
