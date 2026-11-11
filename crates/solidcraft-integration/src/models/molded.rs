//! Slice B2b: generated molded two-terminal models. A rounded moulded body
//! with flat leads under its ends (SOD-523, SOD-123F), leads bent down from
//! mid-height (SOD-123, SOD-323), or J-leads folded down the end faces and
//! under the body (SMA/SMB/SMC, tantalum); or a round glass body between two
//! cylindrical end caps (MELF). A polarised form gets a band at terminal 1
//! (−x) in the core's band area: a thin `Marking` body inlaid in the top
//! (a ring round a MELF). The body is cut around leads, caps and band, so
//! all bodies stay disjoint.
use super::profile::bent_strip;
use super::{Build, check_positive, mm, require, top_edges};
use crate::Result;
use crate::native::{Expr, Plane, ProfileBody};
use crate::pad_outline::Contour;
use pcbcraft_core::library::BodyRole;
use pcbcraft_core::library::package_form::MoldedForm;
use pcbcraft_core::library::package_form::molded::MoldedLead;
use pcbcraft_core::materials::MaterialId;

/// The builder revision (`PackageModel::Generated { family_rev }`).
pub(super) const FAMILY_REV: Option<u16> = Some(1);

/// How far a lead root reaches into the moulding (at most a sixth of the
/// body length), and a bent lead's shoulder share (as gull-wing).
const ROOT_MM: f64 = 0.25;
const SHOULDER_SHARE: f64 = 0.3;
/// Rounds of the moulding's upright corners and top edges (est.: moulded
/// diode and tantalum bodies show 0.1–0.3 mm radii), capped below.
const CORNER_MM: f64 = 0.2;
const TOP_EDGE_MM: f64 = 0.15;
/// Round of a MELF cap's outer edge (est.).
const CAP_EDGE_MM: f64 = 0.08;
/// Depth of the inlaid band (est.: laser or ink layers are tens of µm).
const BAND_DEPTH_MM: f64 = 0.03;

pub(super) fn check(f: &MoldedForm) -> Result<()> {
    check_positive(&[
        ("body length", f.body_length()),
        ("body width", f.body_width()),
        ("body height", f.body_height()),
        ("overall length", f.span()),
        ("terminal length", f.terminal_length()),
        ("terminal width", f.terminal_width()),
    ])?;
    require(
        2.0 * f.terminal_length() < f.span(),
        "the two terminals must not meet",
    )?;
    if f.lead != MoldedLead::Melf {
        check_positive(&[("lead thickness", f.lead_thickness())])?;
    }
    Ok(())
}

pub(super) fn build(b: &mut Build<'_>, f: &MoldedForm) -> Result<()> {
    match f.lead {
        MoldedLead::Melf => melf(b, f),
        _ => moulded(b, f),
    }
}

/// A material override for `role`, else `fallback` (a family-specific
/// default the core's role defaults do not name).
fn material_or(b: &Build<'_>, role: BodyRole, fallback: MaterialId) -> MaterialId {
    b.materials
        .roles
        .iter()
        .find(|r| r.role == role)
        .map_or(fallback, |r| r.material)
}

fn moulded(b: &mut Build<'_>, f: &MoldedForm) -> Result<()> {
    let t = f.lead_thickness();
    let bl = b.length("body_length", f.body_length())?;
    let bw = b.length("body_width", f.body_width())?;
    let bh = b.length("body_height", f.body_height())?;
    let span = b.length("span", f.span())?;
    let tl = b.length("terminal_length", f.terminal_length())?;
    let tw = b.length("terminal_width", f.terminal_width())?;
    let th = b.length("lead_thickness", t)?;
    let bottom = f.body_bottom();

    let body = b.name("body");
    let material = b.material(BodyRole::Body);
    b.solid(
        &body,
        BodyRole::Body,
        material,
        [format!("- {bl} / 2"), format!("- {bw} / 2"), mm(bottom)],
        [bl.clone(), bw.clone(), format!("{bh} - {}", mm(bottom))],
    )?;

    let half_body = f.body_length() / 2.0;
    let half_span = f.span() / 2.0;
    let root = ROOT_MM.min(f.body_length() / 6.0);
    let exit = f.lead_exit();
    let lead_material = b.material(BodyRole::Lead);
    let mut leads = Vec::new();
    for (k, plus) in [(1, false), (2, true)] {
        let name = b.name(&format!("terminal {k}"));
        match f.lead {
            MoldedLead::Flat => {
                // A flat strip on the seating plane from under the body to
                // the tip; the contact may reach further under the body.
                let inner_by_contact = f.terminal_length() >= half_span - half_body + root;
                let length = if inner_by_contact {
                    tl.clone()
                } else {
                    format!("{span} / 2 - {bl} / 2 + {}", mm(root))
                };
                let x = if plus {
                    format!("{span} / 2 - ({length})")
                } else {
                    format!("- {span} / 2")
                };
                b.solid(
                    &name,
                    BodyRole::Lead,
                    lead_material,
                    [x, format!("- {tw} / 2"), "0 mm".into()],
                    [format!("({length})"), tw.clone(), th.clone()],
                )?;
            }
            MoldedLead::Bent | MoldedLead::JLead => {
                let points = if f.lead == MoldedLead::Bent {
                    let foot_in = half_span - f.terminal_length();
                    let shoulder = half_body + SHOULDER_SHARE * (foot_in - half_body);
                    vec![
                        [half_body - root, exit],
                        [shoulder, exit],
                        [foot_in, t / 2.0],
                        [half_span, t / 2.0],
                    ]
                } else {
                    let outer = half_span - t / 2.0;
                    vec![
                        [half_body - root, exit],
                        [outer, exit],
                        [outer, t / 2.0],
                        [half_span - f.terminal_length(), t / 2.0],
                    ]
                };
                let profile = bent_strip(&points, t, t)?;
                // The XZ plane maps (u, v) to (u, offset, −v).
                let contour = if plus {
                    profile.mapped(|[s, z]| [s, -z], true)
                } else {
                    profile.mapped(|[s, z]| [-s, -z], false)
                };
                b.native.plane_body(
                    Plane::Xz,
                    ProfileBody {
                        name: &name,
                        offset: format!("- {tw} / 2").into(),
                        contours: std::slice::from_ref(&contour),
                        height: Expr::from(tw.clone()),
                    },
                )?;
                b.push(name.clone(), BodyRole::Lead, lead_material);
            }
            MoldedLead::Melf => {}
        }
        leads.push(name);
    }
    let tools: Vec<&str> = leads.iter().map(String::as_str).collect();
    b.native.cut_keeping(&b.name("lead roots"), &body, &tools)?;
    b.number(&leads);

    // Rounds: upright corners only where the terminals leave the corner
    // free, picked above the leads; then the top edges.
    let top = f.body_height();
    let lead_top = match f.lead {
        MoldedLead::Flat => t,
        _ => exit + t / 2.0,
    };
    let band_inset =
        pcbcraft_core::library::package_form::molded::BAND_INSET_SHARE * f.body_length();
    let small = f.body_length().min(f.body_width());
    let cap = (0.08 * small).min(band_inset * 0.8);
    let corner_room = (f.body_width() - f.terminal_width()) / 2.0;
    let corner = if corner_room > 0.0 {
        CORNER_MM.min(cap).min(corner_room / 2.0)
    } else {
        0.0
    };
    let edge = TOP_EDGE_MM
        .min(0.12 * f.body_height())
        .min(cap)
        .min((top - lead_top) / 2.0);
    let edge = if corner > 0.0 { edge.min(corner) } else { edge };
    let lo = [-half_body, -f.body_width() / 2.0, bottom];
    let hi = [half_body, f.body_width() / 2.0, top];
    let z = (lead_top + top) / 2.0;
    let uprights = [
        [lo[0], lo[1], z],
        [hi[0], lo[1], z],
        [hi[0], hi[1], z],
        [lo[0], hi[1], z],
    ];
    b.round("corner_round", &body, &uprights, corner)?;
    b.round("edge_round", &body, &top_edges(lo, hi), edge)?;

    if let Some(area) = f.band_area() {
        let name = b.name("band");
        let material = b.material(BodyRole::Marking);
        let [w, across] = area.size();
        b.solid(
            &name,
            BodyRole::Marking,
            material,
            [
                mm(area.origin()[0] - w / 2.0),
                mm(-across / 2.0),
                format!("{bh} - {}", mm(BAND_DEPTH_MM)),
            ],
            [mm(w), mm(across), mm(BAND_DEPTH_MM)],
        )?;
        b.native
            .cut_keeping(&b.name("band inlay"), &body, &[&name])?;
    }
    Ok(())
}

fn melf(b: &mut Build<'_>, f: &MoldedForm) -> Result<()> {
    let bl = b.length("body_length", f.body_length())?;
    b.length("body_diameter", f.body_width())?;
    let span = b.length("span", f.span())?;
    let tl = b.length("terminal_length", f.terminal_length())?;
    b.length("cap_diameter", f.terminal_width())?;
    let rc = f.terminal_width() / 2.0;
    let rb = f.body_width() / 2.0;
    // The axis lies a cap radius above the seating plane.
    let circle = |radius: f64| Contour::Circle {
        center: [0.0, rc],
        radius,
    };

    let body = b.name("body");
    let glass = material_or(b, BodyRole::Body, MaterialId::BorosilicateGlass);
    b.native.plane_body(
        Plane::Yz,
        ProfileBody {
            name: &body,
            offset: format!("- {bl} / 2").into(),
            contours: &[circle(rb)],
            height: Expr::from(bl.clone()),
        },
    )?;
    b.push(body.clone(), BodyRole::Body, glass);

    let cap_material = b.material(BodyRole::Termination);
    let mut caps = Vec::new();
    for (k, offset) in [
        (1, format!("- {span} / 2")),
        (2, format!("{span} / 2 - {tl}")),
    ] {
        let name = b.name(&format!("cap {k}"));
        b.native.plane_body(
            Plane::Yz,
            ProfileBody {
                name: &name,
                offset: offset.into(),
                contours: &[circle(rc)],
                height: Expr::from(tl.clone()),
            },
        )?;
        b.push(name.clone(), BodyRole::Termination, cap_material);
        caps.push(name);
    }
    if f.body_length() > f.span() - 2.0 * f.terminal_length() + 1e-9 {
        let tools: Vec<&str> = caps.iter().map(String::as_str).collect();
        b.native.cut_keeping(&b.name("cap seats"), &body, &tools)?;
    }
    b.number(&caps);

    let edge = CAP_EDGE_MM
        .min(0.15 * f.terminal_length())
        .min(0.1 * rc)
        .min((rc - rb).max(0.0) + 0.5 * f.terminal_length());
    if edge > 0.0 {
        let r = b.length("cap_round", edge)?;
        for (name, x) in caps.iter().zip([-f.span() / 2.0, f.span() / 2.0]) {
            let feature = format!("{name} round");
            b.native
                .fillet(&feature, name, &[[x, 0.0, 2.0 * rc]], r.clone().into())?;
        }
    }

    if let Some(area) = f.band_area() {
        let name = b.name("band");
        let ink = material_or(b, BodyRole::Marking, MaterialId::EpoxyMould);
        let w = area.size()[0];
        let depth = BAND_DEPTH_MM.min(rb / 4.0);
        b.native.plane_body(
            Plane::Yz,
            ProfileBody {
                name: &name,
                offset: mm(area.origin()[0] - w / 2.0).into(),
                contours: &[circle(rb), circle(rb - depth)],
                height: mm(w).into(),
            },
        )?;
        b.push(name.clone(), BodyRole::Marking, ink);
        b.native
            .cut_keeping(&b.name("band inlay"), &body, &[&name])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::*;
    #[allow(unused_imports)]
    use crate::models::tests::{nm, pt};
    use crate::native::Native;
    use pcbcraft_core::PackageTerminalId;
    use pcbcraft_core::library::PackageTerminal;
    use pcbcraft_core::library::package_form::MoldedForm;
    use pcbcraft_core::library::package_form::molded::MoldedLead;
    use pcbcraft_core::materials::MaterialId;

    fn package(form: MoldedForm) -> PackageDefinition {
        let form = PackageForm::Molded(Box::new(form));
        PackageDefinition {
            next_local: 3,
            terminals: (1..=2)
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

    /// Original fixtures (round numbers in the outlines' range).
    fn form(lead: MoldedLead) -> MoldedForm {
        match lead {
            MoldedLead::Flat => MoldedForm {
                lead,
                body_length_nm: nm(1.2),
                body_width_nm: nm(0.8),
                body_height_nm: nm(0.6),
                span_nm: nm(1.6),
                terminal_length_nm: nm(0.3),
                terminal_width_nm: nm(0.3),
                polarised: true,
            },
            MoldedLead::Bent => MoldedForm {
                lead,
                body_length_nm: nm(2.7),
                body_width_nm: nm(1.6),
                body_height_nm: nm(1.1),
                span_nm: nm(3.7),
                terminal_length_nm: nm(0.3),
                terminal_width_nm: nm(0.6),
                polarised: true,
            },
            MoldedLead::JLead => MoldedForm {
                lead,
                body_length_nm: nm(4.3),
                body_width_nm: nm(2.6),
                body_height_nm: nm(2.2),
                span_nm: nm(5.2),
                terminal_length_nm: nm(1.1),
                terminal_width_nm: nm(1.4),
                polarised: true,
            },
            MoldedLead::Melf => MoldedForm {
                lead,
                body_length_nm: nm(3.0),
                body_width_nm: nm(1.4),
                body_height_nm: nm(1.4),
                span_nm: nm(3.5),
                terminal_length_nm: nm(0.4),
                terminal_width_nm: nm(1.5),
                polarised: true,
            },
        }
    }

    const ALL: [MoldedLead; 4] = [
        MoldedLead::Flat,
        MoldedLead::Bent,
        MoldedLead::JLead,
        MoldedLead::Melf,
    ];

    fn measure(native: &Native, name: &str) -> Result<([f64; 3], [f64; 3], f64)> {
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
            m.volume,
        ))
    }

    #[test]
    fn every_lead_kind_builds_disjoint_bodies_on_its_lead_points() -> Result<()> {
        for lead in ALL {
            let pkg = package(form(lead));
            let points = pkg.form.lead_points();
            let mut native = Native::new("models")?;
            let def = build_definition(&mut native, "pkgt", &pkg)?;
            let mut terminals: Vec<&GeneratedBody> =
                def.bodies.iter().filter(|b| b.terminal.is_some()).collect();
            terminals.sort_by_key(|b| b.terminal);
            assert_eq!(terminals.len(), 2, "{lead:?}");
            for (body, point) in terminals.iter().zip(&points) {
                let (lo, hi, _) = measure(&native, &body.name)?;
                assert!(
                    lo[0] - 1e-6 <= point[0] && point[0] <= hi[0] + 1e-6,
                    "{lead:?} {}: {point:?} outside {lo:?}..{hi:?}",
                    body.name
                );
                assert!(
                    lo[2].abs() < 1e-6,
                    "{lead:?} {} on the seating plane",
                    body.name
                );
            }
            // Within the envelope, and disjoint.
            let env = pkg.form.envelope().unwrap_or_default();
            for body in &def.bodies {
                let (lo, hi, _) = measure(&native, &body.name)?;
                for axis in 0..2 {
                    assert!(
                        hi[axis] - lo[axis] <= env[axis] + 1e-6,
                        "{lead:?} {}",
                        body.name
                    );
                }
                assert!(hi[2] <= env[2] + 1e-6, "{lead:?} {} height", body.name);
            }
            // Curved (MELF) faces are measured through their tessellation,
            // so the two volumes agree to about 1e-5 relative there (the
            // smallest body, a 0.06 mm³ band ring, is still 100 × the
            // tolerance).
            let tolerance = if lead == MoldedLead::Melf { 1e-4 } else { 1e-6 };
            super::super::tests::disjoint_within(&mut native, &def, tolerance)
                .map_err(|e| Error::Verification(format!("{lead:?}: {e}")))?;
        }
        Ok(())
    }

    #[test]
    fn the_band_marks_terminal_one_in_the_core_s_band_area() -> Result<()> {
        for lead in ALL {
            let f = form(lead);
            let area = f
                .band_area()
                .ok_or_else(|| Error::Verification("no band".into()))?;
            let mut native = Native::new("models")?;
            let def = build_definition(&mut native, "pkgt", &package(f))?;
            let band = def
                .bodies
                .iter()
                .find(|b| b.role == BodyRole::Marking)
                .ok_or_else(|| Error::Verification(format!("{lead:?}: no band body")))?;
            let (lo, hi, _) = measure(&native, &band.name)?;
            assert!((lo[0] - (area.origin()[0] - area.size()[0] / 2.0)).abs() < 1e-6);
            assert!((hi[0] - (area.origin()[0] + area.size()[0] / 2.0)).abs() < 1e-6);
            assert!(
                (hi[2] - area.origin()[2]).abs() < 1e-6,
                "{lead:?} band on top"
            );
            assert!(hi[0] < 0.0, "{lead:?}: the band is at terminal 1 (−x)");
            let mut plain = form(lead);
            plain.polarised = false;
            let mut native = Native::new("models")?;
            let def = build_definition(&mut native, "pkgt", &package(plain))?;
            assert!(def.bodies.iter().all(|b| b.role != BodyRole::Marking));
        }
        Ok(())
    }

    #[test]
    fn a_melf_is_glass_between_tin_caps_and_the_moulded_bodies_are_epoxy() -> Result<()> {
        let mut native = Native::new("models")?;
        let def = build_definition(&mut native, "pkgt", &package(form(MoldedLead::Melf)))?;
        let material = |role| {
            def.bodies
                .iter()
                .find(|b| b.role == role)
                .map(|b| b.material)
        };
        assert_eq!(
            material(BodyRole::Body),
            Some(MaterialId::BorosilicateGlass)
        );
        assert_eq!(material(BodyRole::Termination), Some(MaterialId::TinMatte));
        let mut native = Native::new("models")?;
        let def = build_definition(&mut native, "pkgt", &package(form(MoldedLead::JLead)))?;
        assert!(
            def.bodies
                .iter()
                .any(|b| b.role == BodyRole::Body && b.material == MaterialId::EpoxyMould)
        );
        Ok(())
    }

    #[test]
    fn the_body_is_rounded_by_a_named_radius() -> Result<()> {
        let pkg = package(form(MoldedLead::JLead));
        let mut native = Native::new("models")?;
        build_definition(&mut native, "pkgt", &pkg)?;
        let (_, _, before) = measure(&native, "pkgt body")?;
        native.change_parameters(serde_json::json!({ "pkgt_edge_round": "0.17 mm" }))?;
        assert_eq!(native.evaluation_error(), None);
        let (_, _, after) = measure(&native, "pkgt body")?;
        assert!(after < before, "{before} → {after}");
        Ok(())
    }

    #[test]
    fn inconsistent_forms_are_refused() -> Result<()> {
        let mut f = form(MoldedLead::Melf);
        f.body_height_nm = nm(1.0);
        let mut native = Native::new("models")?;
        let refused = build_definition(&mut native, "pkgt", &package(f));
        assert!(matches!(&refused, Err(e) if e.to_string().contains("model.form")));
        Ok(())
    }
}
