//! Slice B2d: through-hole packages. Every lead reaches `lead_below` under
//! the seating plane (the board's top face) into its drill, and its root
//! a little way into the body, which is cut around it so leads and body
//! stay disjoint.
//!
//! - **Axial**: a round body lying on its standoff along x; each lead
//!   leaves an end on the body's axis, runs out and bends down at
//!   `lead_pitch` (one bent strip of the lead's diameter square, its bend
//!   the form's centreline radius).
//! - **Radial**: a round body standing up; straight round leads, or formed
//!   ones that leave the body at `body_pitch` and jog out to `lead_pitch`
//!   in the standoff.
//! - **DIP**: a moulded block on its standoff; strip leads leave its long
//!   sides, run out to the row and bend down.
//! - **TO**: a moulded block on its standoff; square leads straight down,
//!   or formed as for radial parts.
//!
//! Lead order is the form's: lead 1 at −x; a DIP counter-clockwise from
//! the −x end of the −y row.
use super::profile::bent_strip;
use super::{Build, check_count, check_positive, require, top_edges, upright_edges};
use crate::Result;
use crate::native::{BoxBody, Expr, Pattern, Plane, ProfileBody};
use crate::pad_outline::Contour;
use pcbcraft_core::library::BodyRole;
use pcbcraft_core::library::package_form::ThtForm;
use pcbcraft_core::library::package_form::tht::{
    DIP_LEAD_EXIT_SHARE, DIP_LEAD_THICKNESS_MM, ThtKind,
};

/// The builder revision (`PackageModel::Generated { family_rev }`).
pub(super) const FAMILY_REV: Option<u16> = Some(1);

/// How far a lead root reaches into the body (est.), at most a quarter of
/// the body along the lead.
const ROOT_MM: f64 = 0.5;
/// Round of an axial body's ends and of a radial body's top (est.).
const END_ROUND_MM: f64 = 0.3;
/// Rounds of a moulded block's upright corners and top edges (est.).
const CORNER_MM: f64 = 0.3;
const TOP_EDGE_MM: f64 = 0.2;

pub(super) fn check(f: &ThtForm) -> Result<()> {
    check_positive(&[
        ("lead pitch", f.lead_pitch),
        ("lead diameter", f.lead_diameter),
        ("lead length below the board", f.lead_below),
        ("bend radius", f.bend_radius()),
        ("lead pitch at the body", f.pitch_at_body()),
    ])?;
    let s = f.standoff();
    require(
        s.is_finite() && s >= 0.0,
        "the standoff must be zero or more",
    )?;
    match f.kind {
        ThtKind::Axial {
            body_diameter,
            body_length,
        } => check_positive(&[
            ("body diameter", body_diameter),
            ("body length", body_length),
        ]),
        ThtKind::Radial {
            body_diameter,
            body_height,
        } => check_positive(&[
            ("body diameter", body_diameter),
            ("body height", body_height),
        ]),
        ThtKind::Dip {
            pins,
            row_pitch,
            body_length,
            body_width,
            body_height,
        } => {
            check_count("pins per row", pins / 2)?;
            check_positive(&[
                ("row pitch", row_pitch),
                ("body length", body_length),
                ("body width", body_width),
                ("body height", body_height),
            ])
        }
        ThtKind::To {
            pins,
            body_length,
            body_width,
            body_height,
        } => {
            check_count("pins", pins)?;
            check_positive(&[
                ("body length", body_length),
                ("body width", body_width),
                ("body height", body_height),
            ])
        }
    }
}

/// The named parameters every construction shares.
struct Leads {
    pitch: String,
    d: String,
    below: String,
    standoff: String,
}

pub(super) fn build(b: &mut Build<'_>, f: &ThtForm) -> Result<()> {
    let leads = Leads {
        pitch: b.length("lead_pitch", f.lead_pitch)?,
        d: b.length("lead_diameter", f.lead_diameter)?,
        below: b.length("lead_below", f.lead_below)?,
        standoff: b.length("standoff", f.standoff())?,
    };
    match f.kind {
        ThtKind::Axial {
            body_diameter,
            body_length,
        } => axial(b, f, body_diameter, body_length),
        ThtKind::Radial {
            body_diameter,
            body_height,
        } => radial(b, f, &leads, body_diameter, body_height),
        ThtKind::Dip {
            pins,
            row_pitch,
            body_length,
            body_width,
            body_height,
        } => dip(
            b,
            f,
            &leads,
            pins / 2,
            row_pitch,
            [body_length, body_width, body_height],
        ),
        ThtKind::To {
            pins,
            body_length,
            body_width,
            body_height,
        } => to(b, f, &leads, pins, [body_length, body_width, body_height]),
    }
}

/// A lead of the given centreline in the x–z plane (x, z), square in
/// section (`d` across y, centred), as one bent strip.
fn xz_lead(b: &mut Build<'_>, name: &str, points: &[[f64; 2]], f: &ThtForm) -> Result<()> {
    let d = f.lead_diameter;
    let strip = bent_strip(points, d, f.bend_radius())?;
    // The XZ plane draws (x, z) at sketch (x, −z): a reflection.
    let strip = strip.mapped(|[x, z]| [x, -z], true);
    b.native.plane_body(
        Plane::Xz,
        ProfileBody {
            name,
            offset: Expr::from(super::mm(-d / 2.0)),
            contours: std::slice::from_ref(&strip),
            height: Expr::from(super::mm(d)),
        },
    )?;
    let material = b.material(BodyRole::Lead);
    b.push(name.to_owned(), BodyRole::Lead, material);
    Ok(())
}

/// Formed (or straight) leads of a row: lead `i` leaves the body at its
/// `body_pitch` position at `top`, jogs within the standoff and enters
/// the board at its `lead_pitch` position.
fn row_leads(b: &mut Build<'_>, f: &ThtForm, n: u32, top: f64) -> Result<Vec<String>> {
    let s = f.standoff();
    let mut out = Vec::new();
    for i in 0..n {
        let xb = ThtForm::along(i, n, f.pitch_at_body());
        let xp = ThtForm::along(i, n, f.lead_pitch);
        let points = if (xb - xp).abs() < 1e-9 {
            vec![[xp, top], [xp, -f.lead_below]]
        } else {
            vec![
                [xb, top],
                [xb, 0.75 * s],
                [xp, 0.25 * s],
                [xp, -f.lead_below],
            ]
        };
        let name = b.name(&format!("lead {}", i + 1));
        xz_lead(b, &name, &points, f)?;
        out.push(name);
    }
    Ok(out)
}

/// Cut `body` around the leads in `ordered` and number them.
fn finish(b: &mut Build<'_>, body: &str, ordered: &[String]) -> Result<()> {
    let tools: Vec<&str> = ordered.iter().map(String::as_str).collect();
    b.native.cut_keeping(&b.name("lead roots"), body, &tools)?;
    b.number(ordered);
    Ok(())
}

fn axial(b: &mut Build<'_>, f: &ThtForm, dia: f64, length: f64) -> Result<()> {
    b.length("body_diameter", dia)?;
    let bl = b.length("body_length", length)?;
    let s = f.standoff();
    let zc = s + dia / 2.0;
    let body = b.name("body");
    let material = b.material(BodyRole::Body);
    b.native.plane_body(
        Plane::Yz,
        ProfileBody {
            name: &body,
            offset: Expr::from(format!("- {bl} / 2")),
            contours: &[Contour::Circle {
                center: [0.0, zc],
                radius: dia / 2.0,
            }],
            height: Expr::from(bl.clone()),
        },
    )?;
    b.push(body.clone(), BodyRole::Body, material);
    let root = ROOT_MM.min(length / 4.0);
    let half = f.lead_pitch / 2.0;
    let mut ordered = Vec::new();
    for (k, sign) in [(1, -1.0), (2, 1.0)] {
        let name = b.name(&format!("lead {k}"));
        let points = [
            [sign * (length / 2.0 - root), zc],
            [sign * half, zc],
            [sign * half, -f.lead_below],
        ];
        xz_lead(b, &name, &points, f)?;
        ordered.push(name);
    }
    finish(b, &body, &ordered)?;
    let r = END_ROUND_MM.min(0.2 * dia).min(length / 6.0);
    let top = s + dia;
    b.round(
        "end_round",
        &body,
        &[[-length / 2.0, 0.0, top], [length / 2.0, 0.0, top]],
        r,
    )
}

fn radial(b: &mut Build<'_>, f: &ThtForm, p: &Leads, dia: f64, height: f64) -> Result<()> {
    b.length("body_diameter", dia)?;
    let bh = b.length("body_height", height)?;
    let s = f.standoff();
    let body = b.name("body");
    let material = b.material(BodyRole::Body);
    b.native.profile_body(ProfileBody {
        name: &body,
        offset: Expr::from(p.standoff.clone()),
        contours: &[Contour::Circle {
            center: [0.0, 0.0],
            radius: dia / 2.0,
        }],
        height: Expr::from(bh),
    })?;
    b.push(body.clone(), BodyRole::Body, material);
    let root = ROOT_MM.min(height / 4.0);
    let ordered = if f.formed() {
        row_leads(b, f, 2, s + root)?
    } else {
        let material = b.material(BodyRole::Lead);
        let mut out = Vec::new();
        for (k, sign) in [(1, -1.0), (2, 1.0)] {
            let name = b.name(&format!("lead {k}"));
            b.native.profile_body(ProfileBody {
                name: &name,
                offset: Expr::from(format!("- {}", p.below)),
                contours: &[Contour::Circle {
                    center: [sign * f.lead_pitch / 2.0, 0.0],
                    radius: f.lead_diameter / 2.0,
                }],
                height: Expr::from(format!(
                    "{} + {} + {}",
                    p.below,
                    p.standoff,
                    super::mm(root)
                )),
            })?;
            b.push(name.clone(), BodyRole::Lead, material);
            out.push(name);
        }
        out
    };
    finish(b, &body, &ordered)?;
    let r = END_ROUND_MM.min(0.1 * dia).min(height / 6.0);
    b.round("top_round", &body, &[[dia / 2.0, 0.0, s + height]], r)
}

/// A moulded block `[length, width, height]` on the standoff.
fn block(b: &mut Build<'_>, p: &Leads, size: [f64; 3]) -> Result<String> {
    let bl = b.length("body_length", size[0])?;
    let bw = b.length("body_width", size[1])?;
    let bh = b.length("body_height", size[2])?;
    let body = b.name("body");
    let material = b.material(BodyRole::Body);
    b.solid(
        &body,
        BodyRole::Body,
        material,
        [
            format!("- {bl} / 2"),
            format!("- {bw} / 2"),
            p.standoff.clone(),
        ],
        [bl, bw, bh],
    )?;
    Ok(body)
}

/// Round the block's upright corners and top edges, kept within `clear`
/// of the outermost leads.
fn round_block(
    b: &mut Build<'_>,
    body: &str,
    f: &ThtForm,
    size: [f64; 3],
    clear: f64,
) -> Result<()> {
    let s = f.standoff();
    let lo = [-size[0] / 2.0, -size[1] / 2.0, s];
    let hi = [size[0] / 2.0, size[1] / 2.0, s + size[2]];
    let short = size[0].min(size[1]);
    let corner = CORNER_MM.min(0.08 * short).min(clear / 2.0);
    b.round("corner_round", body, &upright_edges(lo, hi), corner)?;
    let edge = TOP_EDGE_MM.min(0.1 * size[2]).min(corner);
    b.round("edge_round", body, &top_edges(lo, hi), edge)
}

fn dip(
    b: &mut Build<'_>,
    f: &ThtForm,
    p: &Leads,
    n: u32,
    row_pitch: f64,
    size: [f64; 3],
) -> Result<()> {
    let row_length = f64::from(n.saturating_sub(1)) * f.lead_pitch + f.lead_diameter;
    let clear = (size[0] - row_length) / 2.0;
    let body = block(b, p, size)?;
    let n_param = b.count("per_row", n)?;
    b.length("row_pitch", row_pitch)?;
    let s = f.standoff();
    let t = DIP_LEAD_THICKNESS_MM;
    let exit = s + DIP_LEAD_EXIT_SHARE * size[2];
    let half_body = size[1] / 2.0;
    let root = ROOT_MM.min(half_body / 3.0);
    let half_row = row_pitch / 2.0;
    let material = b.material(BodyRole::Lead);
    let mut rows = Vec::new();
    for (part, plus) in [("row_s", false), ("row_n", true)] {
        let strip = bent_strip(
            &[
                [half_body - root, exit],
                [half_row, exit],
                [half_row, -f.lead_below],
            ],
            t,
            f.bend_radius(),
        )?;
        let strip = if plus {
            strip
        } else {
            strip.mapped(|[y, z]| [-y, z], true)
        };
        let name = b.name(&format!("{part} lead"));
        b.native.plane_body(
            Plane::Yz,
            ProfileBody {
                name: &name,
                offset: Expr::from(format!("- ({n_param} - 1) / 2 * {} - {} / 2", p.pitch, p.d)),
                contours: std::slice::from_ref(&strip),
                height: Expr::from(p.d.clone()),
            },
        )?;
        b.push(name.clone(), BodyRole::Lead, material);
        let names = if n > 1 {
            let all = b.native.pattern_bodies(
                &b.name(&format!("{part} pattern")),
                &[&name],
                Pattern {
                    dir1: [1.0, 0.0, 0.0],
                    count1: n_param.clone().into(),
                    spacing1: p.pitch.clone().into(),
                    second: None,
                },
            )?;
            b.push_copies(&all, BodyRole::Lead, material);
            all
        } else {
            vec![name]
        };
        rows.push((names, 0, !plus));
    }
    let ordered = b.order(&rows)?;
    finish(b, &body, &ordered)?;
    round_block(b, &body, f, size, clear)
}

fn to(b: &mut Build<'_>, f: &ThtForm, p: &Leads, n: u32, size: [f64; 3]) -> Result<()> {
    let row_length = f64::from(n.saturating_sub(1)) * f.pitch_at_body() + f.lead_diameter;
    let clear = ((size[0] - row_length) / 2.0).min((size[1] - f.lead_diameter) / 2.0);
    let body = block(b, p, size)?;
    let n_param = b.count("pins", n)?;
    let s = f.standoff();
    let root = ROOT_MM.min(size[2] / 4.0);
    let leads = if f.formed() {
        row_leads(b, f, n, s + root)?
    } else {
        let name = b.name("lead 1");
        let material = b.material(BodyRole::Lead);
        b.native.solid_box(BoxBody {
            name: &name,
            corner: [
                format!("- ({n_param} - 1) / 2 * {} - {} / 2", p.pitch, p.d),
                format!("- {} / 2", p.d),
                format!("- {}", p.below),
            ]
            .map(Expr::from),
            size: [
                p.d.clone(),
                p.d.clone(),
                format!("{} + {} + {}", p.below, p.standoff, super::mm(root)),
            ]
            .map(Expr::from),
        })?;
        b.push(name.clone(), BodyRole::Lead, material);
        if n > 1 {
            let all = b.native.pattern_bodies(
                &b.name("lead pattern"),
                &[&name],
                Pattern {
                    dir1: [1.0, 0.0, 0.0],
                    count1: n_param.clone().into(),
                    spacing1: p.pitch.clone().into(),
                    second: None,
                },
            )?;
            b.push_copies(&all, BodyRole::Lead, material);
            all
        } else {
            vec![name]
        }
    };
    let ordered = b.order(&[(leads, 0, true)])?;
    finish(b, &body, &ordered)?;
    round_block(b, &body, f, size, clear.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::Error;
    use pcbcraft_core::PackageTerminalId;
    use pcbcraft_core::library::PackageTerminal;
    use pcbcraft_core::library::package_form::ThtForm;
    use pcbcraft_core::library::package_form::tht::ThtKind;

    // Original synthetic forms with round numbers, from no library.
    fn package(form: ThtForm) -> PackageDefinition {
        let form = PackageForm::Tht(Box::new(form));
        let n = u32::try_from(form.lead_count()).unwrap_or(0);
        PackageDefinition {
            next_local: n + 1,
            terminals: (1..=n)
                .map(|i| PackageTerminal {
                    id: PackageTerminalId(i),
                    designation: i.to_string(),
                })
                .collect(),
            model: PackageModel::Generated {
                family_rev: family_rev(&form).unwrap_or(0),
                materials: GeneratedMaterials::default(),
            },
            form,
            ..PackageDefinition::default()
        }
    }

    /// A body's box: (min, max).
    type Box3 = ([f64; 3], [f64; 3]);

    fn bounds(native: &Native, name: &str) -> Result<Box3> {
        let state = native.session().model.state();
        let body = state
            .body(name)
            .ok_or_else(|| Error::Verification(format!("no body {name}")))?;
        let m = body
            .measure()
            .map_err(|e| Error::Verification(e.to_string()))?;
        Ok((
            [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
            [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
        ))
    }

    fn near(a: f64, b: f64, what: &str) {
        assert!((a - b).abs() < 1e-6, "{what}: {a} != {b}");
    }

    /// The built model, every lead numbered to its terminal in order and
    /// reaching `lead_below` under the board; returns each lead's box in
    /// lead order, then the body's.
    fn built(form: ThtForm) -> Result<(Vec<Box3>, Box3)> {
        let mut native = Native::new("models")?;
        let pkg = package(form);
        let def = build_definition(&mut native, "pkgt", &pkg)?;
        let mut leads = Vec::new();
        for t in &pkg.terminals {
            let lead = def
                .bodies
                .iter()
                .find(|b| b.terminal == Some(t.id.0))
                .ok_or_else(|| Error::Verification(format!("no lead {}", t.designation)))?;
            assert_eq!(lead.role, BodyRole::Lead);
            let b = bounds(&native, &lead.name)?;
            near(b.0[2], -form.lead_below, "lead into the board");
            leads.push(b);
        }
        let body = def
            .bodies
            .iter()
            .find(|b| b.role == BodyRole::Body)
            .ok_or_else(|| Error::Verification("no body".into()))?;
        Ok((leads, bounds(&native, &body.name)?))
    }

    #[test]
    fn axial_leads_bend_down_at_the_lead_pitch() -> Result<()> {
        let form = ThtForm::new(
            ThtKind::Axial {
                body_diameter: 2.5,
                body_length: 6.0,
            },
            10.0,
            0.6,
            3.0,
        );
        let (leads, body) = built(form)?;
        near(body.0[0], -3.0, "body from −x");
        near(body.1[2], 2.5, "body resting on the board");
        // Lead 1 at −x: its outer face at −(pitch + d) / 2.
        near(leads[0].0[0], -5.3, "lead 1 outer face");
        near(leads[1].1[0], 5.3, "lead 2 outer face");
        // Each lead runs on the body's axis into the body's end.
        near(leads[1].1[2], 1.25 + 0.3, "lead top");
        assert!(leads[1].0[0] < 3.0, "the lead root is inside the body");
        Ok(())
    }

    #[test]
    fn radial_leads_go_straight_down_or_formed_out() -> Result<()> {
        let mut form = ThtForm::new(
            ThtKind::Radial {
                body_diameter: 5.0,
                body_height: 11.0,
            },
            2.0,
            0.5,
            3.0,
        );
        let (leads, body) = built(form)?;
        near(body.1[2], 11.0, "body height");
        near((leads[0].0[0] + leads[0].1[0]) / 2.0, -1.0, "lead 1 at −x");
        near((leads[1].0[0] + leads[1].1[0]) / 2.0, 1.0, "lead 2 at +x");
        // Formed out from 2 mm at the body to 5 mm on the board.
        form.lead_pitch = 5.0;
        form.body_pitch = Some(2.0);
        form.standoff = Some(1.5);
        let (leads, body) = built(form)?;
        near(body.0[2], 1.5, "standoff");
        near(leads[1].1[0], 2.75, "formed lead's foot outer face");
        near(leads[1].0[0], 0.75, "formed lead's root inner face");
        Ok(())
    }

    #[test]
    fn dip_strips_run_counter_clockwise_and_bend_down_at_the_rows() -> Result<()> {
        let form = ThtForm::new(
            ThtKind::Dip {
                pins: 8,
                row_pitch: 7.5,
                body_length: 9.5,
                body_width: 6.5,
                body_height: 3.5,
            },
            2.5,
            0.5,
            3.0,
        );
        let (leads, body) = built(form)?;
        near(body.0[2], 0.5, "standoff");
        let centre = |b: &([f64; 3], [f64; 3])| (b.0[0] + b.1[0]) / 2.0;
        near(centre(&leads[0]), -3.75, "pin 1 at −x");
        near(centre(&leads[3]), 3.75, "pin 4 at +x");
        near(centre(&leads[4]), 3.75, "pin 5 above pin 4");
        near(centre(&leads[7]), -3.75, "pin 8 above pin 1");
        // −y row for 1–4, +y row for 5–8, each bending down at its row.
        near(leads[0].0[1], -3.75 - 0.125, "pin 1 outer face");
        near(leads[7].1[1], 3.75 + 0.125, "pin 8 outer face");
        Ok(())
    }

    #[test]
    fn to_rows_are_straight_or_formed() -> Result<()> {
        let mut form = ThtForm::new(
            ThtKind::To {
                pins: 3,
                body_length: 4.5,
                body_width: 3.5,
                body_height: 4.5,
            },
            1.25,
            0.45,
            3.0,
        );
        let (leads, body) = built(form)?;
        near(body.0[2], 1.0, "estimated standoff");
        let centre = |b: &([f64; 3], [f64; 3])| (b.0[0] + b.1[0]) / 2.0;
        for (lead, x) in leads.iter().zip([-1.25, 0.0, 1.25]) {
            near(centre(lead), x, "straight lead");
        }
        // Formed: 1.25 mm at the body out to 2.5 mm on the board.
        form.lead_pitch = 2.5;
        form.body_pitch = Some(1.25);
        let (leads, _) = built(form)?;
        near(leads[0].0[0], -2.5 - 0.225, "outer lead's foot");
        near(centre(&leads[1]), 0.0, "middle lead straight");
        Ok(())
    }
}
