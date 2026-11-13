//! Pad arrays (design §2.5 step 8): rigid copies of a solved seed made
//! outside the solver, then skips and numbering. Quarter and half turns
//! use an exact table, so copies carry no cos(90°) residue. Identity is the
//! generation key `(array, i, j)`, never position or number.

use crate::geom::Vec2;
use sparksmith_core::library::footprint_source::{
    ArrayLayout, NumberingScheme, PadArray, Pin1Position,
};

use super::params::Values;
use super::{Outcome, problem};

const ARRAY: &str = "footprint.array";

/// One generated instance: its key indices, centre, extra rotation
/// (degrees) and display number.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Instance {
    pub i: u16,
    pub j: u16,
    pub centre: Vec2,
    pub turn_deg: f64,
    pub number: String,
}

/// Turn `v` counter-clockwise by a multiple of 90° exactly.
fn quarter(v: Vec2, turns: u8) -> Vec2 {
    match turns % 4 {
        0 => v,
        1 => Vec2::new(-v.y, v.x),
        2 => Vec2::new(-v.x, -v.y),
        _ => Vec2::new(v.y, -v.x),
    }
}

fn add(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(a.x + b.x, a.y + b.y)
}

fn scale(v: Vec2, k: f64) -> Vec2 {
    Vec2::new(v.x * k, v.y * k)
}

/// Row letters (the bijective base over the letters kept): A, B, … AA, AB.
fn row_name(letters: &[char], mut i: usize) -> String {
    let n = letters.len().max(1);
    let mut out = Vec::new();
    loop {
        out.push(letters.get(i % n).copied().unwrap_or('?'));
        if i < n {
            break;
        }
        i = i / n - 1;
    }
    out.iter().rev().collect()
}

struct Generated {
    i: u16,
    j: u16,
    centre: Vec2,
    turn: f64,
}

/// Expand `array` from a seed centred at `seed`. `direction` is the unit
/// vector of the layout's direction line (linear and dual-row layouts).
pub(crate) fn expand(
    array: &PadArray,
    seed: Vec2,
    direction: Option<Vec2>,
    values: &Values,
    max: usize,
) -> Outcome<Vec<Instance>> {
    let need_direction = || {
        direction.ok_or_else(|| {
            problem(
                ARRAY,
                format!("array {} needs a direction line", array.id.0),
            )
        })
    };
    let mut generated: Vec<Generated> = Vec::new();
    let mut grid = None;
    let check = |n: usize| {
        if n > max {
            Err(problem(
                "footprint.too_large",
                format!("array {} makes {n} pads (at most {max})", array.id.0),
            ))
        } else {
            Ok(())
        }
    };
    // (rows, columns) of the key space, for numbering.
    let shape: (u16, u16);
    match &array.layout {
        ArrayLayout::Linear { count, pitch, .. } => {
            let (n, p, d) = (
                values.count(&count.0)?,
                values.length(&pitch.0)?,
                need_direction()?,
            );
            check(usize::from(n))?;
            for j in 0..n {
                generated.push(Generated {
                    i: 0,
                    j,
                    centre: add(seed, scale(d, f64::from(j) * p)),
                    turn: 0.0,
                });
            }
            shape = (1, n);
        }
        ArrayLayout::DualRow {
            per_row,
            pitch,
            row_span,
            ..
        } => {
            let (n, p, span, d) = (
                values.count(&per_row.0)?,
                values.length(&pitch.0)?,
                values.length(&row_span.0)?,
                need_direction()?,
            );
            check(usize::from(n) * 2)?;
            // Row 1 faces row 0 across the array centre, turned 180°.
            let normal = quarter(d, 1);
            let centre = add(
                add(seed, scale(normal, span * 0.5)),
                scale(d, f64::from(n.saturating_sub(1)) * p * 0.5),
            );
            for row in 0..2u16 {
                for j in 0..n {
                    let at = add(seed, scale(d, f64::from(j) * p));
                    let centre_rel = at - centre;
                    let (pos, turn) = if row == 0 {
                        (at, 0.0)
                    } else {
                        (add(centre, quarter(centre_rel, 2)), 180.0)
                    };
                    generated.push(Generated {
                        i: row,
                        j,
                        centre: pos,
                        turn,
                    });
                }
            }
            shape = (2, n);
        }
        ArrayLayout::Quad {
            nx,
            ny,
            pitch,
            span_x,
            span_y,
            pin1,
        } => {
            let (nx, ny, p, sx, sy) = (
                values.count(&nx.0)?,
                values.count(&ny.0)?,
                values.length(&pitch.0)?,
                values.length(&span_x.0)?,
                values.length(&span_y.0)?,
            );
            check(2 * (usize::from(nx) + usize::from(ny)))?;
            // Absolute sides counter-clockwise: left (down), bottom
            // (right), right (up), top (left). Offsets from the centre.
            let side_count = |s: u8| if s.is_multiple_of(2) { ny } else { nx };
            let offset = |s: u8, k: u16| {
                let n = side_count(s);
                let t = (f64::from(n.saturating_sub(1)) * 0.5 - f64::from(k)) * p;
                match s % 4 {
                    0 => Vec2::new(-sx * 0.5, t),
                    1 => Vec2::new(-t, -sy * 0.5),
                    2 => Vec2::new(sx * 0.5, -t),
                    _ => Vec2::new(t, sy * 0.5),
                }
            };
            let first: u8 = match pin1 {
                Pin1Position::Left => 0,
                Pin1Position::Bottom => 1,
                Pin1Position::Right => 2,
                Pin1Position::Top => 3,
            };
            let centre = seed - offset(first, 0);
            for order in 0..4u8 {
                let side = (first + order) % 4;
                for k in 0..side_count(side) {
                    generated.push(Generated {
                        i: u16::from(order),
                        j: k,
                        centre: add(centre, offset(side, k)),
                        turn: 90.0 * f64::from(order),
                    });
                }
            }
            shape = (4, nx.max(ny));
        }
        ArrayLayout::Grid {
            rows,
            cols,
            pitch_x,
            pitch_y,
            stagger,
        } => {
            let (r, c, px, py) = (
                values.count(&rows.0)?,
                values.count(&cols.0)?,
                values.length(&pitch_x.0)?,
                values.length(&pitch_y.0)?,
            );
            check(usize::from(r) * usize::from(c))?;
            for i in 0..r {
                let shift = if *stagger && i % 2 == 1 {
                    px * 0.5
                } else {
                    0.0
                };
                for j in 0..c {
                    generated.push(Generated {
                        i,
                        j,
                        centre: add(
                            seed,
                            Vec2::new(f64::from(j) * px + shift, -f64::from(i) * py),
                        ),
                        turn: 0.0,
                    });
                }
            }
            grid = Some((r, c));
            shape = (r, c);
        }
    }
    let skip = &array.skip;
    if grid.is_none() && (skip.centre_void.is_some() || skip.perimeter_depth.is_some()) {
        return Err(problem(
            ARRAY,
            format!(
                "array {}: a centre void or perimeter depth needs a grid",
                array.id.0
            ),
        ));
    }
    let skipped = |i: u16, j: u16| {
        if skip.explicit.contains(&[i, j]) {
            return true;
        }
        let Some((rows, cols)) = grid else {
            return false;
        };
        if let Some([h, w]) = skip.centre_void {
            let r0 = rows.saturating_sub(h) / 2;
            let c0 = cols.saturating_sub(w) / 2;
            if (r0..r0.saturating_add(h)).contains(&i) && (c0..c0.saturating_add(w)).contains(&j) {
                return true;
            }
        }
        if let Some(d) = skip.perimeter_depth {
            let ring = i
                .min(rows.saturating_sub(1).saturating_sub(i))
                .min(j)
                .min(cols.saturating_sub(1).saturating_sub(j));
            if ring >= d {
                return true;
            }
        }
        false
    };
    let numbers = numbering(array, &generated, shape)?;
    let mut out = Vec::with_capacity(generated.len());
    for (g, number) in generated.into_iter().zip(numbers) {
        if skipped(g.i, g.j) {
            if g.i == 0 && g.j == 0 {
                return Err(problem(
                    ARRAY,
                    format!("array {} skips its own seed (instance 0, 0)", array.id.0),
                ));
            }
            continue;
        }
        out.push(Instance {
            i: g.i,
            j: g.j,
            centre: g.centre,
            turn_deg: g.turn,
            number,
        });
    }
    Ok(out)
}

/// Display numbers of every generated instance (skipped ones included, so
/// a missing pin keeps its gap). Numbers are never identity.
fn numbering(
    array: &PadArray,
    generated: &[Generated],
    (_, cols): (u16, u16),
) -> Outcome<Vec<String>> {
    if let NumberingScheme::GridAlpha { skip_letters } = &array.numbering {
        let skip: Vec<char> = skip_letters
            .chars()
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let letters: Vec<char> = ('A'..='Z').filter(|c| !skip.contains(c)).collect();
        if letters.is_empty() {
            return Err(problem(ARRAY, "grid numbering skips every letter"));
        }
        return Ok(generated
            .iter()
            .map(|g| {
                format!(
                    "{}{}",
                    row_name(&letters, usize::from(g.i)),
                    u32::from(g.j) + 1
                )
            })
            .collect());
    }
    let first: u32 = if array.first_number.is_empty() {
        1
    } else {
        array.first_number.parse().map_err(|_| {
            problem(
                ARRAY,
                format!(
                    "array {}: first number `{}` is not a whole number",
                    array.id.0, array.first_number
                ),
            )
        })?
    };
    let dual = matches!(array.layout, ArrayLayout::DualRow { .. });
    let grid = matches!(array.layout, ArrayLayout::Grid { .. });
    let n = u32::from(cols);
    let index = |seq: usize, g: &Generated| -> u32 {
        let (i, j) = (u32::from(g.i), u32::from(g.j));
        let seq = u32::try_from(seq).unwrap_or(u32::MAX);
        match &array.numbering {
            // Two rows numbered alternately across (headers).
            NumberingScheme::Zigzag if dual => {
                if i == 0 {
                    2 * j
                } else {
                    2 * (n - 1 - j) + 1
                }
            }
            // Serpentine rows.
            NumberingScheme::Zigzag if grid => i * n + if i % 2 == 0 { j } else { n - 1 - j },
            // Each row in the same direction as row 0.
            NumberingScheme::PerRow if dual => {
                if i == 0 {
                    j
                } else {
                    n + (n - 1 - j)
                }
            }
            _ => seq,
        }
    };
    generated
        .iter()
        .enumerate()
        .map(|(seq, g)| {
            first
                .checked_add(index(seq, g))
                .map(|v| v.to_string())
                .ok_or_else(|| problem(ARRAY, "pad numbers overflow"))
        })
        .collect()
}
