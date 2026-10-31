//! Gull-wing packages (SOIC, SOP, SSOP on two sides; QFP on four): a
//! moulded body on its standoff, and formed leads that leave the body at
//! mid-height, bend down and end in a flat foot on the seating plane. The
//! body is cut around the lead roots, so leads and moulding stay disjoint.
use super::profile::bent_strip;
use super::{Build, check_count, check_positive, mm, require};
use crate::Result;
use crate::native::{Expr, Pattern, Plane, ProfileBody};
use pcbcraft_core::library::{BodyRole, GullWingForm, LeadSides};

/// How far a lead root reaches into the moulding (at most a third of the
/// half body).
const ROOT_MM: f64 = 0.25;
/// Share of the room between body and foot the straight shoulder takes.
const SHOULDER_SHARE: f64 = 0.3;
/// Lead exit height as a share of the body height above the standoff.
const EXIT_SHARE: f64 = 0.5;
/// Pin-1 dimple depth (est.).
const DIMPLE_DEPTH_MM: f64 = 0.05;

/// One row of leads.
struct Row {
    /// Leads in the row and the named parameter holding the count.
    count: u32,
    count_param: String,
    /// Body half-extent and tip half-span across the row.
    half_body: f64,
    half_span: f64,
    /// The sketch plane, and (s, z) → sketch (u, v) with its orientation.
    plane: Plane,
    map: fn([f64; 2]) -> [f64; 2],
    mirror: bool,
    /// The axis the row runs along (0 = x, 1 = y) and its lead order.
    axis: usize,
    ascending: bool,
}

pub(super) fn check(g: &GullWingForm) -> Result<()> {
    check_positive(&[
        ("pitch", g.pitch),
        ("body length", g.body_length),
        ("body width", g.body_width),
        ("body height", g.body_height),
        ("span", g.span[0]),
        ("lead width", g.lead_width),
        ("lead thickness", g.lead_thickness),
        ("foot length", g.foot_length),
    ])?;
    require(
        g.standoff.is_finite() && g.standoff >= 0.0,
        "the standoff must be zero or more",
    )?;
    let four = g.sides == LeadSides::Four;
    check_count("leads per long side", g.leads_per_side[0])?;
    if four {
        check_count("leads per short side", g.leads_per_side[1])?;
        check_positive(&[("span across x", g.span[1])])?;
    }
    require(
        g.lead_width < g.pitch,
        "the lead width must be below the pitch",
    )?;
    let row_length = |n: u32| f64::from(n.saturating_sub(1)) * g.pitch + g.lead_width;
    require(
        row_length(g.leads_per_side[0]) <= g.body_length,
        "the long-side leads must fit along the body length",
    )?;
    if four {
        require(
            row_length(g.leads_per_side[1]) <= g.body_width,
            "the short-side leads must fit along the body width",
        )?;
    }
    let t = g.lead_thickness;
    let exit = g.standoff + EXIT_SHARE * g.body_height;
    require(
        exit - t / 2.0 > t && exit + t / 2.0 < g.standoff + g.body_height,
        "the lead thickness must fit inside the body height above the seating plane",
    )?;
    let mut across = vec![(g.body_width, g.span[0])];
    if four {
        across.push((g.body_length, g.span[1]));
    }
    for (body, span) in across {
        require(
            span / 2.0 - g.foot_length - body / 2.0 > t,
            "the span must leave room outside the body for the lead to bend down to its foot",
        )?;
    }
    Ok(())
}

pub(super) fn build(b: &mut Build<'_>, g: &GullWingForm) -> Result<()> {
    let four = g.sides == LeadSides::Four;
    let t = g.lead_thickness;
    let exit = g.standoff + EXIT_SHARE * g.body_height;

    let n = b.count("leads", g.leads_per_side[0])?;
    let m = if four {
        Some(b.count("leads_short", g.leads_per_side[1])?)
    } else {
        None
    };
    let pitch = b.length("pitch", g.pitch)?;
    let bl = b.length("body_length", g.body_length)?;
    let bw = b.length("body_width", g.body_width)?;
    let bh = b.length("body_height", g.body_height)?;
    let standoff = b.length("standoff", g.standoff)?;
    b.length("span", g.span[0])?;
    if four {
        b.length("span_x", g.span[1])?;
    }
    let lw = b.length("lead_width", g.lead_width)?;
    b.length("lead_thickness", g.lead_thickness)?;
    b.length("foot_length", g.foot_length)?;

    let body = b.name("body");
    let material = b.material(BodyRole::Body);
    b.solid(
        &body,
        BodyRole::Body,
        material,
        [
            format!("- {bl} / 2"),
            format!("- {bw} / 2"),
            standoff.clone(),
        ],
        [bl.clone(), bw.clone(), bh.clone()],
    )?;

    let y_row = |count_param: &str, plus: bool| Row {
        count: g.leads_per_side[0],
        count_param: count_param.to_owned(),
        half_body: g.body_width / 2.0,
        half_span: g.span[0] / 2.0,
        plane: Plane::Yz,
        map: if plus {
            |[s, z]| [s, z]
        } else {
            |[s, z]| [-s, z]
        },
        mirror: !plus,
        axis: 0,
        ascending: !plus,
    };
    let mut rows = vec![("row_s", y_row(&n, false))];
    if let Some(m) = &m {
        let x_row = |plus: bool| Row {
            count: g.leads_per_side[1],
            count_param: m.clone(),
            half_body: g.body_length / 2.0,
            half_span: g.span[1] / 2.0,
            plane: Plane::Xz,
            map: if plus {
                |[s, z]| [s, -z]
            } else {
                |[s, z]| [-s, -z]
            },
            mirror: plus,
            axis: 1,
            ascending: plus,
        };
        rows.push(("row_e", x_row(true)));
        rows.push(("row_n", y_row(&n, true)));
        rows.push(("row_w", x_row(false)));
    } else {
        rows.push(("row_n", y_row(&n, true)));
    }

    let lead_material = b.material(BodyRole::Lead);
    let mut ordered_rows = Vec::new();
    for (part, row) in &rows {
        let root = ROOT_MM.min(row.half_body / 3.0);
        let foot = row.half_span - g.foot_length;
        let room = foot - row.half_body;
        let shoulder = row.half_body + SHOULDER_SHARE * room;
        let profile = bent_strip(
            &[
                [row.half_body - root, exit],
                [shoulder, exit],
                [foot, t / 2.0],
                [row.half_span, t / 2.0],
            ],
            t,
            t,
        )?;
        let profile = profile.mapped(|p| (row.map)(p), row.mirror);
        let name = b.name(&format!("{part} lead"));
        // The first lead of the row, centred `(count - 1) / 2` pitches
        // before the row's middle.
        let offset = format!("- ({} - 1) / 2 * {pitch} - {lw} / 2", row.count_param);
        b.native.plane_body(
            row.plane,
            ProfileBody {
                name: &name,
                offset: offset.into(),
                contours: std::slice::from_ref(&profile),
                height: Expr::from(lw.clone()),
            },
        )?;
        b.push(name.clone(), BodyRole::Lead, lead_material);
        let names = if row.count > 1 {
            let mut dir = [0.0; 3];
            dir[row.axis] = 1.0;
            let all = b.native.pattern_bodies(
                &b.name(&format!("{part} pattern")),
                &[&name],
                Pattern {
                    dir1: dir,
                    count1: row.count_param.clone().into(),
                    spacing1: pitch.clone().into(),
                    second: None,
                },
            )?;
            b.push_copies(&all, BodyRole::Lead, lead_material);
            all
        } else {
            vec![name]
        };
        ordered_rows.push((names, row.axis, row.ascending));
    }
    let leads = b.order(&ordered_rows)?;
    let tools: Vec<&str> = leads.iter().map(String::as_str).collect();
    b.native.cut_keeping(&b.name("lead roots"), &body, &tools)?;
    b.number(&leads);

    // A pin-1 dimple in the top, at the corner of lead 1.
    let short = g.body_length.min(g.body_width);
    let radius = (0.12 * short).clamp(0.15, 0.5);
    if 5.0 * radius < short && DIMPLE_DEPTH_MM < g.body_height / 4.0 {
        let at = 2.0 * radius;
        b.native.cylinder_cut(
            &b.name("pin 1 mark"),
            [
                format!("- {bl} / 2 + {}", mm(at)).into(),
                format!("- {bw} / 2 + {}", mm(at)).into(),
                format!("{standoff} + {bh} - {}", mm(DIMPLE_DEPTH_MM)).into(),
            ],
            mm(radius).into(),
            mm(2.0 * DIMPLE_DEPTH_MM).into(),
            &[&body],
        )?;
    }
    Ok(())
}
