//! No-lead packages (DFN/SON on two sides, QFN on four): a moulded body
//! whose underside holds the leadframe's side terminals, flush with the
//! body's bottom and its side faces, and an optional exposed pad. The body
//! is cut around them, so terminals and moulding stay disjoint.
use super::{Build, check_count, check_positive, mm, require};
use crate::Result;
use crate::native::{BoxBody, Expr, Pattern};
use pcbcraft_core::library::{BodyRole, LeadSides, NoLeadForm};

/// Leadframe thickness (est.: 0.2 mm is the common QFN frame), at most a
/// third of the body height.
const FRAME_MM: f64 = 0.2;
/// Pin-1 dimple depth (est.).
const DIMPLE_DEPTH_MM: f64 = 0.03;

pub(super) fn check(f: &NoLeadForm) -> Result<()> {
    check_positive(&[
        ("pitch", f.pitch),
        ("body length", f.body_length),
        ("body width", f.body_width),
        ("body height", f.body_height),
        ("terminal length", f.terminal_length),
        ("terminal width", f.terminal_width),
    ])?;
    let four = f.sides == LeadSides::Four;
    check_count("terminals per long side", f.terminals_per_side[0])?;
    if four {
        check_count("terminals per short side", f.terminals_per_side[1])?;
    }
    require(
        f.terminal_width < f.pitch,
        "the terminal width must be below the pitch",
    )?;
    let row_length = |n: u32| f64::from(n.saturating_sub(1)) * f.pitch + f.terminal_width;
    require(
        row_length(f.terminals_per_side[0]) <= f.body_length,
        "the long-side terminals must fit along the body length",
    )?;
    require(
        2.0 * f.terminal_length < f.body_width,
        "the terminals of opposite sides must not meet",
    )?;
    if four {
        require(
            row_length(f.terminals_per_side[1]) <= f.body_width
                && 2.0 * f.terminal_length < f.body_length,
            "the short-side terminals must fit along the body width",
        )?;
    }
    if let Some([el, ew]) = f.exposed_pad {
        check_positive(&[("exposed pad length", el), ("exposed pad width", ew)])?;
        let x_room = if four {
            f.body_length - 2.0 * f.terminal_length
        } else {
            f.body_length
        };
        require(
            el < x_room && ew < f.body_width - 2.0 * f.terminal_length,
            "the exposed pad must stay clear of the terminals",
        )?;
    }
    Ok(())
}

pub(super) fn build(b: &mut Build<'_>, f: &NoLeadForm) -> Result<()> {
    let four = f.sides == LeadSides::Four;
    let frame = FRAME_MM.min(f.body_height / 3.0);

    let n = b.count("terminals", f.terminals_per_side[0])?;
    let m = if four {
        Some(b.count("terminals_short", f.terminals_per_side[1])?)
    } else {
        None
    };
    let pitch = b.length("pitch", f.pitch)?;
    let bl = b.length("body_length", f.body_length)?;
    let bw = b.length("body_width", f.body_width)?;
    let bh = b.length("body_height", f.body_height)?;
    let tl = b.length("terminal_length", f.terminal_length)?;
    let tw = b.length("terminal_width", f.terminal_width)?;
    let fr = b.length("frame", frame)?;

    let body = b.name("body");
    let material = b.material(BodyRole::Body);
    b.solid(
        &body,
        BodyRole::Body,
        material,
        [format!("- {bl} / 2"), format!("- {bw} / 2"), "0 mm".into()],
        [bl.clone(), bw.clone(), bh.clone()],
    )?;

    // (part, corner, size, pattern axis, count parameter and count, order).
    let first = |count: &str| format!("- ({count} - 1) / 2 * {pitch} - {tw} / 2");
    let mut rows = vec![(
        "row_s",
        [first(&n), format!("- {bw} / 2"), "0 mm".into()],
        [tw.clone(), tl.clone(), fr.clone()],
        0,
        n.clone(),
        f.terminals_per_side[0],
        true,
    )];
    let north = (
        "row_n",
        [first(&n), format!("{bw} / 2 - {tl}"), "0 mm".into()],
        [tw.clone(), tl.clone(), fr.clone()],
        0,
        n.clone(),
        f.terminals_per_side[0],
        false,
    );
    if let Some(m) = &m {
        let x_size = [tl.clone(), tw.clone(), fr.clone()];
        let count = f.terminals_per_side[1];
        rows.push((
            "row_e",
            [format!("{bl} / 2 - {tl}"), first(m), "0 mm".into()],
            x_size.clone(),
            1,
            m.clone(),
            count,
            true,
        ));
        rows.push(north);
        rows.push((
            "row_w",
            [format!("- {bl} / 2"), first(m), "0 mm".into()],
            x_size,
            1,
            m.clone(),
            count,
            false,
        ));
    } else {
        rows.push(north);
    }
    let material = b.material(BodyRole::Termination);
    let mut ordered = Vec::new();
    for (part, corner, size, axis, count_param, count, ascending) in rows {
        let name = b.name(&format!("{part} terminal"));
        b.native.solid_box(BoxBody {
            name: &name,
            corner: corner.map(Expr::from),
            size: size.map(Expr::from),
        })?;
        b.push(name.clone(), BodyRole::Termination, material);
        let names = if count > 1 {
            let mut dir = [0.0; 3];
            dir[axis] = 1.0;
            let all = b.native.pattern_bodies(
                &b.name(&format!("{part} pattern")),
                &[&name],
                Pattern {
                    dir1: dir,
                    count1: count_param.into(),
                    spacing1: pitch.clone().into(),
                    second: None,
                },
            )?;
            b.push_copies(&all, BodyRole::Termination, material);
            all
        } else {
            vec![name]
        };
        ordered.push((names, axis, ascending));
    }
    let mut numbered = b.order(&ordered)?;
    if let Some([el, ew]) = f.exposed_pad {
        let el = b.length("pad_length", el)?;
        let ew = b.length("pad_width", ew)?;
        let name = b.name("exposed pad");
        let material = b.material(BodyRole::ExposedPad);
        b.solid(
            &name,
            BodyRole::ExposedPad,
            material,
            [format!("- {el} / 2"), format!("- {ew} / 2"), "0 mm".into()],
            [el, ew, fr.clone()],
        )?;
        numbered.push(name);
    }
    let tools: Vec<&str> = numbered.iter().map(String::as_str).collect();
    b.native
        .cut_keeping(&b.name("frame pockets"), &body, &tools)?;
    b.number(&numbered);

    let short = f.body_length.min(f.body_width);
    let radius = (0.08 * short).clamp(0.1, 0.4);
    if 6.0 * radius < short && DIMPLE_DEPTH_MM < f.body_height / 4.0 {
        let at = 2.0 * radius;
        b.native.cylinder_cut(
            &b.name("pin 1 mark"),
            [
                format!("- {bl} / 2 + {}", mm(at)).into(),
                format!("- {bw} / 2 + {}", mm(at)).into(),
                format!("{bh} - {}", mm(DIMPLE_DEPTH_MM)).into(),
            ],
            mm(radius).into(),
            mm(2.0 * DIMPLE_DEPTH_MM).into(),
            &[&body],
        )?;
    }
    Ok(())
}
