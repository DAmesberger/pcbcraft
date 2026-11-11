//! Slice B2a: generated tabbed gull-wing models (SOT-23, SOT-223, DPAK). A
//! moulded body on its standoff, gull-wing lead rows on −y and +y with their
//! own counts and spacing (lead step × pitch), and an optional tab: a wide
//! gull-wing tab lead (SOT-223) or a flat flange on the seating plane under
//! the body (DPAK). The body is cut around the leads, so leads and moulding
//! stay disjoint. Leads are numbered in the core's lead order: the −y row
//! in +x, the +y row in −x, then the tab.
use super::profile::bent_strip;
use super::{Build, check_count, check_positive, require, top_edges, upright_edges};
use crate::Result;
use crate::native::{Expr, Pattern, Plane, ProfileBody};
use pcbcraft_core::library::BodyRole;
use pcbcraft_core::library::package_form::TabbedForm;

/// The builder revision (`PackageModel::Generated { family_rev }`).
pub(super) const FAMILY_REV: Option<u16> = Some(1);

/// How far a lead root reaches into the moulding (at most a third of the
/// half body), and the shoulder's share of the bend room (as gull-wing).
const ROOT_MM: f64 = 0.25;
const SHOULDER_SHARE: f64 = 0.3;
/// Rounds of the moulding's upright corners and top edges (est.: transfer
/// moulded bodies leave the mould with 0.1–0.3 mm radii), capped below.
const CORNER_MM: f64 = 0.2;
const TOP_EDGE_MM: f64 = 0.12;

pub(super) fn check(f: &TabbedForm) -> Result<()> {
    check_positive(&[
        ("pitch", f.pitch()),
        ("body length", f.body_length()),
        ("body width", f.body_width()),
        ("body height", f.body_height()),
        ("span", f.span()),
        ("lead width", f.lead_width()),
        ("lead thickness", f.lead_thickness()),
        ("foot length", f.foot_length()),
    ])?;
    check_count("leads on the −y side", f.leads_per_side[0])?;
    if f.leads_per_side[1] > 0 {
        check_count("leads on the +y side", f.leads_per_side[1])?;
    }
    require(
        f.leads_per_side[1] > 0 || f.tab.is_some(),
        "the +y side needs a lead or a tab",
    )?;
    if let Some(tab) = f.tab {
        check_positive(&[("tab width", tab.width()), ("tab length", tab.length())])?;
    }
    require(
        f.bend_room() > f.lead_thickness(),
        "the span must leave room outside the body for the lead to bend down to its foot",
    )?;
    Ok(())
}

/// The bent side profile of a lead whose tip is `half_span` from the
/// centre and whose foot is `foot` long, in (distance across y, z).
fn profile(f: &TabbedForm, half_span: f64, foot: f64) -> Result<crate::pad_outline::Contour> {
    let t = f.lead_thickness();
    let exit = f.lead_exit();
    let half_body = f.body_width() / 2.0;
    let root = ROOT_MM.min(half_body / 3.0);
    let foot_in = half_span - foot;
    let shoulder = half_body + SHOULDER_SHARE * (foot_in - half_body);
    bent_strip(
        &[
            [half_body - root, exit],
            [shoulder, exit],
            [foot_in, t / 2.0],
            [half_span, t / 2.0],
        ],
        t,
        t,
    )
}

pub(super) fn build(b: &mut Build<'_>, f: &TabbedForm) -> Result<()> {
    let t = f.lead_thickness();
    let [n, m] = f.leads_per_side;
    let pitch = b.length("pitch", f.pitch())?;
    let bl = b.length("body_length", f.body_length())?;
    let bw = b.length("body_width", f.body_width())?;
    let bh = b.length("body_height", f.body_height())?;
    let standoff = b.length("standoff", f.standoff())?;
    b.length("span", f.span())?;
    let lw = b.length("lead_width", f.lead_width())?;
    b.length("lead_thickness", t)?;
    b.length("foot_length", f.foot_length())?;

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

    let lead_material = b.material(BodyRole::Lead);
    let shape = profile(f, f.span() / 2.0, f.foot_length())?;
    let mut ordered_rows = Vec::new();
    for (row, part, count, plus) in [(0usize, "row_s", n, false), (1, "row_n", m, true)] {
        if count == 0 {
            continue;
        }
        let count_param = b.count(&format!("{part}_leads"), count)?;
        let step = b.count(&format!("{part}_step"), f.lead_step[row])?;
        let contour = if plus {
            shape.clone()
        } else {
            shape.mapped(|[s, z]| [-s, z], true)
        };
        let name = b.name(&format!("{part} lead"));
        let offset = format!("- ({count_param} - 1) / 2 * {step} * {pitch} - {lw} / 2");
        b.native.plane_body(
            Plane::Yz,
            ProfileBody {
                name: &name,
                offset: offset.into(),
                contours: std::slice::from_ref(&contour),
                height: Expr::from(lw.clone()),
            },
        )?;
        b.push(name.clone(), BodyRole::Lead, lead_material);
        let names = if count > 1 {
            let all = b.native.pattern_bodies(
                &b.name(&format!("{part} pattern")),
                &[&name],
                Pattern {
                    dir1: [1.0, 0.0, 0.0],
                    count1: count_param.clone().into(),
                    spacing1: format!("{step} * {pitch}").into(),
                    second: None,
                },
            )?;
            b.push_copies(&all, BodyRole::Lead, lead_material);
            all
        } else {
            vec![name]
        };
        ordered_rows.push((names, 0, !plus));
    }
    let mut leads = b.order(&ordered_rows)?;

    if let Some(tab) = f.tab {
        let tw = b.length("tab_width", tab.width())?;
        let tl = b.length("tab_length", tab.length())?;
        let name = b.name("tab");
        match tab.flange() {
            None => {
                let contour = profile(f, f.span() / 2.0, tab.length())?;
                b.native.plane_body(
                    Plane::Yz,
                    ProfileBody {
                        name: &name,
                        offset: format!("- {tw} / 2").into(),
                        contours: std::slice::from_ref(&contour),
                        height: Expr::from(tw.clone()),
                    },
                )?;
                b.push(name.clone(), BodyRole::Lead, lead_material);
            }
            Some(protrusion) => {
                let p = b.length("flange_protrusion", protrusion)?;
                let thickness = b.p("lead_thickness");
                b.solid(
                    &name,
                    BodyRole::Lead,
                    lead_material,
                    [
                        format!("- {tw} / 2"),
                        format!("{bw} / 2 + {p} - {tl}"),
                        "0 mm".into(),
                    ],
                    [tw.clone(), tl.clone(), thickness],
                )?;
            }
        }
        leads.push(name);
    }
    let tools: Vec<&str> = leads.iter().map(String::as_str).collect();
    b.native.cut_keeping(&b.name("lead roots"), &body, &tools)?;
    b.number(&leads);

    // Rounded upright corners clear of the end leads and the tab, then
    // rounded top edges clear of the lead exit.
    let row_length = |count: u32, step: u32| {
        f64::from(count.saturating_sub(1)) * f64::from(step) * f.pitch() + f.lead_width()
    };
    let widest = row_length(n, f.lead_step[0])
        .max(if m > 0 {
            row_length(m, f.lead_step[1])
        } else {
            0.0
        })
        .max(f.tab.map_or(0.0, |tab| tab.width()));
    let short = f.body_length().min(f.body_width());
    let corner = CORNER_MM
        .min(0.06 * short)
        .min((f.body_length() - widest) / 4.0);
    let top = f.standoff() + f.body_height();
    let edge = TOP_EDGE_MM
        .min(0.1 * f.body_height())
        .min((top - f.lead_exit() - t / 2.0) / 2.0)
        .min(corner.max(0.0));
    let lo = [-f.body_length() / 2.0, -f.body_width() / 2.0, f.standoff()];
    let hi = [f.body_length() / 2.0, f.body_width() / 2.0, top];
    // Pick the upright edges above the flange and lead roots.
    let mut uprights = upright_edges(lo, hi);
    for p in &mut uprights {
        p[2] = (f.lead_exit() + t / 2.0 + top) / 2.0;
    }
    b.round("corner_round", &body, &uprights, corner)?;
    b.round("edge_round", &body, &top_edges(lo, hi), edge)?;
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
    use pcbcraft_core::library::package_form::TabbedForm;
    use pcbcraft_core::library::package_form::tabbed::Tab;

    fn package(form: TabbedForm) -> PackageDefinition {
        let form = PackageForm::Tabbed(Box::new(form));
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

    /// Original fixtures (round numbers in the outlines' range).
    fn sot23_5() -> TabbedForm {
        TabbedForm {
            leads_per_side: [3, 2],
            pitch_nm: nm(0.95),
            body_length_nm: nm(2.9),
            body_width_nm: nm(1.6),
            body_height_nm: nm(1.1),
            standoff_nm: nm(0.05),
            span_nm: nm(2.8),
            lead_width_nm: nm(0.4),
            lead_thickness_nm: nm(0.12),
            foot_length_nm: nm(0.45),
            tab: None,
            lead_step: [1, 2],
            lands: Default::default(),
        }
    }

    fn sot223() -> TabbedForm {
        TabbedForm {
            leads_per_side: [3, 0],
            pitch_nm: nm(2.3),
            body_length_nm: nm(6.5),
            body_width_nm: nm(3.5),
            body_height_nm: nm(1.6),
            standoff_nm: nm(0.05),
            span_nm: nm(7.0),
            lead_width_nm: nm(0.7),
            lead_thickness_nm: nm(0.25),
            foot_length_nm: nm(0.9),
            tab: Some(Tab {
                width_nm: nm(3.0),
                length_nm: nm(0.9),
                flange_nm: None,
            }),
            lead_step: [1, 1],
            lands: Default::default(),
        }
    }

    fn dpak() -> TabbedForm {
        TabbedForm {
            leads_per_side: [2, 0],
            pitch_nm: nm(2.3),
            body_length_nm: nm(6.5),
            body_width_nm: nm(6.0),
            body_height_nm: nm(2.3),
            standoff_nm: nm(0.0),
            span_nm: nm(11.0),
            lead_width_nm: nm(0.8),
            lead_thickness_nm: nm(0.5),
            foot_length_nm: nm(1.5),
            tab: Some(Tab {
                width_nm: nm(5.3),
                length_nm: nm(5.5),
                flange_nm: Some(nm(1.0)),
            }),
            lead_step: [2, 1],
            lands: Default::default(),
        }
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

    /// Lead bodies in terminal order stand over the core's lead points,
    /// reach the seating plane, and every body is disjoint from the others.
    fn leads_stand_on_their_points(form: TabbedForm) -> Result<ModelDefinition> {
        let pkg = package(form);
        let points = pkg.form.lead_points();
        let mut native = Native::new("models")?;
        let def = build_definition(&mut native, "pkgt", &pkg)?;
        let mut leads: Vec<&GeneratedBody> =
            def.bodies.iter().filter(|b| b.terminal.is_some()).collect();
        leads.sort_by_key(|b| b.terminal);
        assert_eq!(leads.len(), points.len());
        for (lead, point) in leads.iter().zip(&points) {
            let (lo, hi) = bounds(&native, &lead.name)?;
            for axis in 0..2 {
                assert!(
                    lo[axis] - 1e-6 <= point[axis] && point[axis] <= hi[axis] + 1e-6,
                    "{}: lead point {point:?} outside {lo:?}..{hi:?}",
                    lead.name
                );
            }
            assert!(lo[2].abs() <= 1e-6, "{} on the seating plane", lead.name);
        }
        super::super::tests::disjoint_within(&mut native, &def, 1e-6)?;
        Ok(def)
    }

    #[test]
    fn sot23_5_rows_have_their_own_spacing() -> Result<()> {
        let def = leads_stand_on_their_points(sot23_5())?;
        assert_eq!(
            def.bodies
                .iter()
                .filter(|b| b.role == BodyRole::Lead)
                .count(),
            5
        );
        assert_eq!(
            def.bodies
                .iter()
                .find(|b| b.role == BodyRole::Body)
                .map(|b| b.material),
            Some(pcbcraft_core::materials::MaterialId::EpoxyMould)
        );
        Ok(())
    }

    #[test]
    fn sot223_tab_lead_is_the_last_terminal() -> Result<()> {
        let def = leads_stand_on_their_points(sot223())?;
        let tab = def
            .bodies
            .iter()
            .find(|b| b.name == "pkgt tab")
            .and_then(|b| b.terminal);
        assert_eq!(tab, Some(4));
        Ok(())
    }

    #[test]
    fn dpak_flange_lies_flat_under_the_body() -> Result<()> {
        let pkg = package(dpak());
        let mut native = Native::new("models")?;
        build_definition(&mut native, "pkgt", &pkg)?;
        let (lo, hi) = bounds(&native, "pkgt tab")?;
        assert!(lo[2].abs() < 1e-6 && (hi[2] - 0.5).abs() < 1e-6);
        assert!((hi[1] - 4.0).abs() < 1e-6 && (lo[1] + 1.5).abs() < 1e-6);
        leads_stand_on_their_points(dpak())?;
        Ok(())
    }

    #[test]
    fn the_pitch_parameter_moves_the_rows() -> Result<()> {
        let pkg = package(sot23_5());
        let mut native = Native::new("models")?;
        let def = build_definition(&mut native, "pkgt", &pkg)?;
        let last = def
            .bodies
            .iter()
            .find(|b| b.terminal == Some(4))
            .map(|b| b.name.clone())
            .ok_or_else(|| Error::Verification("no lead 4".into()))?;
        native.change_parameters(serde_json::json!({ "pkgt_pitch": "1.0 mm" }))?;
        assert_eq!(native.evaluation_error(), None);
        let (lo, hi) = bounds(&native, &last)?;
        // Lead 4 (+y, at +x) sits two pitches right of the centre line / 2.
        assert!(((lo[0] + hi[0]) / 2.0 - 1.0).abs() < 1e-6, "{lo:?} {hi:?}");
        Ok(())
    }

    #[test]
    fn inconsistent_forms_are_refused() -> Result<()> {
        let mut f = sot223();
        f.leads_per_side = [3, 1];
        let mut native = Native::new("models")?;
        let refused = build_definition(&mut native, "pkgt", &package(f));
        assert!(matches!(&refused, Err(e) if e.to_string().contains("model.form")));
        Ok(())
    }
}
