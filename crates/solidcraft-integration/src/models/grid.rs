//! Slice B2c: grid arrays (BGA and WLCSP on balls, LGA on lands). A body
//! (substrate and moulding as one block) stands on the balls; each ball is
//! a round column of the ball's diameter and height (est.: rounding every
//! ball's ends multiplied the build time about sixfold on a 364-ball grid,
//! measured in this crate's tests, for a shape seen only up close); LGA
//! lands are thin discs set into the body's underside,
//! the body cut around them so lands and body stay disjoint. Balls follow
//! the form's lead order (row-major from A1 at −x, +y) and leave the
//! depopulated positions empty.
//!
//! A full matrix is one seed ball and a two-way pattern driven by the
//! `rows` and `cols` parameters; a depopulated one is a seed and a pattern
//! for each run of balls along a row.
use super::{Build, check_count, check_positive, mm, require, top_edges};
use crate::Result;
use crate::native::{Expr, Pattern, ProfileBody};
use crate::pad_outline::Contour;
use sparksmith_core::library::BodyRole;
use sparksmith_core::library::package_form::GridForm;
use sparksmith_core::library::package_form::grid::GridContact;
use sparksmith_core::materials::MaterialId;

/// The builder revision (`PackageModel::Generated { family_rev }`).
pub(super) const FAMILY_REV: Option<u16> = Some(1);

/// LGA land thickness (est.: plated lands stand 20–40 µm), at most a
/// quarter of the body height.
const LAND_MM: f64 = 0.03;
/// Round of the body's top edges (est.: sawn substrates stay sharp).
const TOP_EDGE_MM: f64 = 0.05;
/// Pin-1 mark depth (est.).
const DIMPLE_DEPTH_MM: f64 = 0.03;

pub(super) fn check(form: &GridForm) -> Result<()> {
    check_positive(&[
        ("pitch along a row", form.pitch()[0]),
        ("pitch between rows", form.pitch()[1]),
        ("body length", form.body_length()),
        ("body width", form.body_width()),
        ("body height", form.body_height()),
        ("contact diameter", form.contact_diameter()),
    ])?;
    check_count("rows", form.rows)?;
    check_count("columns", form.cols)?;
    let leads = u32::try_from(form.populated().len()).unwrap_or(u32::MAX);
    check_count("contacts", leads)?;
    match form.contact {
        GridContact::Ball => check_positive(&[("ball height", form.contact_height())]),
        GridContact::Land => require(form.contact_height() == 0.0, "lands have no ball height"),
    }
}

/// `role`'s overridden material, else `fallback` (the family's own
/// default where the shared table has none: solder balls, gold lands).
fn material_or(b: &Build<'_>, role: BodyRole, fallback: MaterialId) -> MaterialId {
    b.materials
        .roles
        .iter()
        .find(|r| r.role == role)
        .map_or(fallback, |r| r.material)
}

pub(super) fn build(b: &mut Build<'_>, form: &GridForm) -> Result<()> {
    let balls = form.contact == GridContact::Ball;
    let rows = b.count("rows", form.rows)?;
    let cols = b.count("cols", form.cols)?;
    let pitch = b.length("pitch", form.pitch()[0])?;
    let row_pitch = b.length("row_pitch", form.pitch()[1])?;
    let bl = b.length("body_length", form.body_length())?;
    let bw = b.length("body_width", form.body_width())?;
    let bh = b.length("body_height", form.body_height())?;
    b.length("contact_diameter", form.contact_diameter())?;
    let land = LAND_MM.min(form.body_height() / 4.0);
    let (contact_h, base) = if balls {
        let h = b.length("ball_height", form.contact_height())?;
        (h.clone(), h)
    } else {
        (b.length("land_thickness", land)?, "0 mm".to_owned())
    };

    let body = b.name("body");
    let material = b.material(BodyRole::Body);
    b.solid(
        &body,
        BodyRole::Body,
        material,
        [format!("- {bl} / 2"), format!("- {bw} / 2"), base.clone()],
        [bl.clone(), bw.clone(), bh.clone()],
    )?;

    let (role, material) = if balls {
        (
            BodyRole::Lead,
            material_or(b, BodyRole::Lead, MaterialId::SolderSac),
        )
    } else {
        (
            BodyRole::Termination,
            material_or(b, BodyRole::Termination, MaterialId::Gold),
        )
    };
    let radius = form.contact_diameter() / 2.0;
    let full = form.depopulated.is_empty();
    // (row, first column, run length) of each seed and its pattern.
    let runs: Vec<(u32, u32, u32)> = if full {
        vec![(0, 0, form.cols)]
    } else {
        let populated = form.populated();
        let mut runs: Vec<(u32, u32, u32)> = Vec::new();
        for [r, c] in populated {
            match runs.last_mut() {
                Some((row, first, n)) if *row == r && *first + *n == c => *n += 1,
                _ => runs.push((r, c, 1)),
            }
        }
        runs
    };
    let mut contacts = Vec::new();
    for (k, (row, first, n)) in runs.iter().enumerate() {
        let name = b.name(&format!("contact {}", k + 1));
        let [x, y] = form.centre(*row, *first);
        b.native.profile_body(ProfileBody {
            name: &name,
            offset: Expr::from("0 mm"),
            contours: &[Contour::Circle {
                center: [x, y],
                radius,
            }],
            height: Expr::from(contact_h.clone()),
        })?;
        b.push(name.clone(), role, material);
        let along = if full { cols.clone() } else { n.to_string() };
        let second = (full && form.rows > 1).then(|| {
            (
                [0.0, -1.0, 0.0],
                rows.clone().into(),
                row_pitch.clone().into(),
            )
        });
        if *n > 1 || second.is_some() {
            let all = b.native.pattern_bodies(
                &b.name(&format!("contact {} pattern", k + 1)),
                &[&name],
                Pattern {
                    dir1: [1.0, 0.0, 0.0],
                    count1: along.into(),
                    spacing1: pitch.clone().into(),
                    second,
                },
            )?;
            b.push_copies(&all, role, material);
            contacts.extend(all);
        } else {
            contacts.push(name);
        }
    }
    // Row-major from A1: rows from +y down, columns from −x.
    let mut keyed = contacts
        .iter()
        .map(|n| {
            let c = b.centre(n)?;
            Ok((
                (-c[1] / form.pitch()[1]).round() as i64,
                (c[0] / form.pitch()[0]).round() as i64,
                n.clone(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    keyed.sort_by_key(|a| (a.0, a.1));
    let ordered: Vec<String> = keyed.into_iter().map(|(_, _, n)| n).collect();
    if !balls {
        let tools: Vec<&str> = ordered.iter().map(String::as_str).collect();
        b.native
            .cut_keeping(&b.name("land pockets"), &body, &tools)?;
    }
    b.number(&ordered);

    let (hl, hw) = (form.body_length() / 2.0, form.body_width() / 2.0);
    let z0 = if balls { form.contact_height() } else { 0.0 };
    let top = z0 + form.body_height();
    let edge = TOP_EDGE_MM.min(0.1 * form.body_height());
    b.round(
        "edge_round",
        &body,
        &top_edges([-hl, -hw, z0], [hl, hw, top]),
        edge,
    )?;
    // A pin-1 dimple in the top at the A1 corner (−x, +y).
    let short = form.body_length().min(form.body_width());
    let mark = (0.06 * short).clamp(0.1, 0.4);
    if 6.0 * mark < short && DIMPLE_DEPTH_MM < form.body_height() / 4.0 {
        let at = 2.0 * mark;
        b.native.cylinder_cut(
            &b.name("pin 1 mark"),
            [
                format!("- {bl} / 2 + {}", mm(at)).into(),
                format!("{bw} / 2 - {}", mm(at)).into(),
                format!("{base} + {bh} - {}", mm(DIMPLE_DEPTH_MM)).into(),
            ],
            mm(mark).into(),
            mm(2.0 * DIMPLE_DEPTH_MM).into(),
            &[&body],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::Error;
    #[allow(unused_imports)]
    use crate::models::tests::{nm, pt};
    use sparksmith_core::PackageTerminalId;
    use sparksmith_core::library::PackageTerminal;
    use sparksmith_core::library::package_form::GridForm;
    use sparksmith_core::library::package_form::grid::GridContact;

    fn package(form: GridForm) -> PackageDefinition {
        let names = form.ball_names();
        let form = PackageForm::Grid(Box::new(form));
        PackageDefinition {
            next_local: u32::try_from(names.len()).unwrap_or(0) + 1,
            terminals: names
                .iter()
                .zip(1..)
                .map(|(n, k)| PackageTerminal {
                    id: PackageTerminalId(k),
                    designation: n.clone(),
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

    fn built(form: GridForm) -> Result<(Native, ModelDefinition)> {
        let mut native = Native::new("models")?;
        let definition = build_definition(&mut native, "pkgt", &package(form))?;
        Ok((native, definition))
    }

    fn bounds(native: &Native, name: &str) -> Result<([f64; 3], [f64; 3])> {
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

    /// Each numbered contact's centre at its lead point, and its terminal
    /// the package terminal of that position.
    fn contacts_at_the_leads(
        native: &Native,
        def: &ModelDefinition,
        form: &GridForm,
    ) -> Result<()> {
        let pkg = package(form.clone());
        let leads = pkg.form.lead_points();
        let contacts: Vec<&GeneratedBody> = def
            .bodies
            .iter()
            .filter(|b| matches!(b.role, BodyRole::Lead | BodyRole::Termination))
            .collect();
        assert_eq!(contacts.len(), leads.len());
        for (terminal, lead) in pkg.terminals.iter().zip(&leads) {
            let body = contacts
                .iter()
                .find(|b| b.terminal == Some(terminal.id.0))
                .ok_or_else(|| {
                    Error::Verification(format!("no body for {}", terminal.designation))
                })?;
            let (lo, hi) = bounds(native, &body.name)?;
            near((lo[0] + hi[0]) / 2.0, lead[0], &terminal.designation);
            near((lo[1] + hi[1]) / 2.0, lead[1], &terminal.designation);
            near(lo[2], 0.0, "on the seating plane");
        }
        Ok(())
    }

    #[test]
    fn a_ball_grid_stands_on_solder_balls_numbered_from_a1() -> Result<()> {
        let _context = crate::looks::test_context().map(|c| c.enter());
        let form = GridForm::ball_grid(
            3,
            4,
            nm(0.8),
            [nm(4.0), nm(3.0), nm(0.7)],
            [nm(0.4), nm(0.3)],
        );
        let (native, def) = built(form.clone())?;
        let body = def
            .bodies
            .iter()
            .find(|b| b.role == BodyRole::Body)
            .ok_or_else(|| Error::Verification("no body".into()))?;
        let (lo, hi) = bounds(&native, &body.name)?;
        near(lo[2], 0.3, "body on the balls");
        near(hi[2], 1.0, "seated height");
        near(hi[0] - lo[0], 4.0, "body length");
        let balls: Vec<_> = def
            .bodies
            .iter()
            .filter(|b| b.role == BodyRole::Lead)
            .collect();
        assert_eq!(balls.len(), 12);
        assert!(balls.iter().all(|b| b.material == MaterialId::SolderSac));
        let (blo, bhi) = bounds(&native, &balls[0].name)?;
        near(bhi[0] - blo[0], 0.4, "ball diameter");
        near(bhi[2] - blo[2], 0.3, "ball height");
        contacts_at_the_leads(&native, &def, &form)?;
        Ok(())
    }

    #[test]
    fn depopulated_positions_have_no_ball() -> Result<()> {
        let _context = crate::looks::test_context().map(|c| c.enter());
        let mut form = GridForm::ball_grid(
            5,
            5,
            nm(0.5),
            [nm(3.0), nm(3.0), nm(0.5)],
            [nm(0.3), nm(0.2)],
        );
        form.depopulated = vec![[2, 2], [0, 1], [4, 4]];
        let (native, def) = built(form.clone())?;
        let balls = def
            .bodies
            .iter()
            .filter(|b| b.role == BodyRole::Lead)
            .count();
        assert_eq!(balls, 22);
        contacts_at_the_leads(&native, &def, &form)?;
        Ok(())
    }

    #[test]
    fn a_land_grid_sets_gold_lands_into_the_body() -> Result<()> {
        let _context = crate::looks::test_context().map(|c| c.enter());
        let mut form = GridForm::ball_grid(
            2,
            3,
            nm(1.0),
            [nm(3.5), nm(2.5), nm(0.8)],
            [nm(0.6), nm(0.0)],
        );
        form.contact = GridContact::Land;
        let (native, def) = built(form.clone())?;
        let lands: Vec<_> = def
            .bodies
            .iter()
            .filter(|b| b.role == BodyRole::Termination)
            .collect();
        assert_eq!(lands.len(), 6);
        assert!(lands.iter().all(|b| b.material == MaterialId::Gold));
        let body = def
            .bodies
            .iter()
            .find(|b| b.role == BodyRole::Body)
            .ok_or_else(|| Error::Verification("no body".into()))?;
        let (lo, hi) = bounds(&native, &body.name)?;
        near(lo[2], 0.0, "body on the seating plane");
        near(hi[2], 0.8, "body height");
        contacts_at_the_leads(&native, &def, &form)?;
        Ok(())
    }

    #[test]
    fn a_large_grid_with_a_centre_void_builds_every_ball() -> Result<()> {
        let _context = crate::looks::test_context().map(|c| c.enter());
        // 20 × 20 at 0.8 mm with a 6 × 6 centre void: 364 balls, two
        // runs on each of the void's rows.
        let mut form = GridForm::ball_grid(
            20,
            20,
            nm(0.8),
            [nm(17.0), nm(17.0), nm(1.2)],
            [nm(0.45), nm(0.35)],
        );
        form.depopulated = (7..13).flat_map(|r| (7..13).map(move |c| [r, c])).collect();
        let (native, def) = built(form.clone())?;
        let balls = def
            .bodies
            .iter()
            .filter(|b| b.role == BodyRole::Lead)
            .count();
        assert_eq!(balls, 364);
        contacts_at_the_leads(&native, &def, &form)?;
        Ok(())
    }
}
