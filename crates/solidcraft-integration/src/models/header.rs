//! Straight pin headers: a plastic (PBT or LCP) base on the seating plane
//! and square gold-plated pins through it, standing above the base and
//! reaching below the seating plane into the board. The base is cut around
//! the pins, so pins and plastic stay disjoint.
use super::{Build, check_count, check_positive, require};
use crate::Result;
use crate::native::{BoxBody, Expr, Pattern};
use pcbcraft_core::library::{BodyRole, HeaderForm};

pub(super) fn check(h: &HeaderForm) -> Result<()> {
    check_positive(&[
        ("pitch", h.pitch),
        ("row pitch", h.row_pitch),
        ("body height", h.body_height),
        ("pin side", h.pin_side),
        ("pin length above the body", h.pin_above),
    ])?;
    require(
        h.pin_below.is_finite() && h.pin_below >= 0.0,
        "the pin length below the seating plane must be zero or more",
    )?;
    check_count("rows", h.rows)?;
    check_count("pins per row", h.pins_per_row)?;
    check_count("pins", h.rows.saturating_mul(h.pins_per_row))?;
    require(
        h.pin_side < h.pitch && h.pin_side < h.row_pitch,
        "the pins must be thinner than the pitch",
    )?;
    Ok(())
}

pub(super) fn build(b: &mut Build<'_>, h: &HeaderForm) -> Result<()> {
    let rows = b.count("rows", h.rows)?;
    let n = b.count("pins", h.pins_per_row)?;
    let pitch = b.length("pitch", h.pitch)?;
    let row_pitch = b.length("row_pitch", h.row_pitch)?;
    let bh = b.length("body_height", h.body_height)?;
    let side = b.length("pin_side", h.pin_side)?;
    let above = b.length("pin_above", h.pin_above)?;
    let below = b.length("pin_below", h.pin_below)?;

    let body = b.name("body");
    let material = b.material(BodyRole::Body);
    b.solid(
        &body,
        BodyRole::Body,
        material,
        [
            format!("- {n} * {pitch} / 2"),
            format!("- {rows} * {row_pitch} / 2"),
            "0 mm".into(),
        ],
        [
            format!("{n} * {pitch}"),
            format!("{rows} * {row_pitch}"),
            bh.clone(),
        ],
    )?;
    let pin = b.name("pin");
    let material = b.material(BodyRole::Lead);
    b.native.solid_box(BoxBody {
        name: &pin,
        corner: [
            format!("- ({n} - 1) / 2 * {pitch} - {side} / 2"),
            format!("- ({rows} - 1) / 2 * {row_pitch} - {side} / 2"),
            format!("- {below}"),
        ]
        .map(Expr::from),
        size: [
            side.clone(),
            side.clone(),
            format!("{below} + {bh} + {above}"),
        ]
        .map(Expr::from),
    })?;
    b.push(pin.clone(), BodyRole::Lead, material);
    let pins = if h.rows * h.pins_per_row > 1 {
        let (first, second) = match (h.pins_per_row > 1, h.rows > 1) {
            (true, true) => (
                ([1.0, 0.0, 0.0], n.clone(), pitch.clone()),
                Some(([0.0, 1.0, 0.0], rows.clone(), row_pitch.clone())),
            ),
            (true, false) => (([1.0, 0.0, 0.0], n.clone(), pitch.clone()), None),
            _ => (([0.0, 1.0, 0.0], rows.clone(), row_pitch.clone()), None),
        };
        let all = b.native.pattern_bodies(
            &b.name("pin pattern"),
            &[&pin],
            Pattern {
                dir1: first.0,
                count1: first.1.into(),
                spacing1: first.2.into(),
                second: second.map(|(d, c, s)| (d, c.into(), s.into())),
            },
        )?;
        b.push_copies(&all, BodyRole::Lead, material);
        all
    } else {
        vec![pin]
    };
    // Across the rows first: (column, row) in x then y order.
    let mut keyed = pins
        .iter()
        .map(|p| {
            let c = b.centre(p)?;
            Ok(((c[0] / h.pitch).round() as i64, c[1], p.clone()))
        })
        .collect::<Result<Vec<_>>>()?;
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let ordered: Vec<String> = keyed.into_iter().map(|(_, _, p)| p).collect();
    let tools: Vec<&str> = ordered.iter().map(String::as_str).collect();
    b.native.cut_keeping(&b.name("pin holes"), &body, &tools)?;
    b.number(&ordered);
    Ok(())
}
