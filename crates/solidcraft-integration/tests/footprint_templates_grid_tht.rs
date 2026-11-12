//! B2c/B2d evidence: the grid array and through-hole footprint templates
//! built with the real `SketchEvaluator`, from packages whose typed form
//! drives the template defaults. The forms are original synthetic
//! packages with round numbers (from no library, datasheet or EDA tool).

use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::ids::PackageTerminalId;
use pcbcraft_core::library::footprint_edit::{self, AssignRule};
use pcbcraft_core::library::footprint_source::{Expr, FootprintEdit, ParamName, ParamUnit};
use pcbcraft_core::library::package_form::grid::GridForm;
use pcbcraft_core::library::package_form::tht::{ThtForm, ThtKind};
use pcbcraft_core::library::templates::footprint_family::grid::GridLand;
use pcbcraft_core::library::templates::footprint_family::tht::ThtLayout;
use pcbcraft_core::library::templates::footprint_family::{self, FootprintTemplate, TemplateKind};
use pcbcraft_core::library::{
    FootprintDefinition, HoleShape, MaskRule, PackageDefinition, PackageForm, PackageTerminal, Pad,
    PadKind, PadShape,
};
use pcbcraft_solidcraft::footprint::SketchEvaluator;

/// A test length in mm, as stored.
fn nm(mm: f64) -> pcbcraft_core::units::Nm {
    pcbcraft_core::units::Nm::from_mm_f64_rounded(mm).unwrap_or_default()
}

/// A test point or size in mm, as stored.
#[allow(dead_code)]
fn pt(p: [f64; 2]) -> pcbcraft_core::units::PointNm {
    pcbcraft_core::units::PointNm::from_mm_f64_rounded(p).unwrap_or_default()
}

const EV: SketchEvaluator = SketchEvaluator;
const TOL: f64 = 1e-6;

/// A package of `form` whose terminals are designated `names` in lead
/// order.
fn package(form: PackageForm, names: &[String]) -> PackageDefinition {
    let envelope_nm = form.envelope().unwrap_or([1.0; 3]).map(nm);
    let count = u32::try_from(names.len()).unwrap_or(0);
    PackageDefinition {
        envelope_nm,
        terminals: names
            .iter()
            .zip(1..)
            .map(|(n, k)| PackageTerminal {
                id: PackageTerminalId(k + 100),
                designation: n.clone(),
            })
            .collect(),
        next_local: count + 101,
        form,
        ..PackageDefinition::default()
    }
}

fn numbered(n: usize) -> Vec<String> {
    (1..=n).map(|k| k.to_string()).collect()
}

fn empty() -> FootprintDefinition {
    FootprintDefinition {
        next_local: 1,
        ..FootprintDefinition::default()
    }
}

fn build(pkg: &PackageDefinition, template: &FootprintTemplate) -> Result<FootprintDefinition> {
    Ok(footprint_family::build(&empty(), pkg, template, &EV)?.0)
}

fn pad<'a>(def: &'a FootprintDefinition, number: &str) -> Result<&'a Pad> {
    def.pads
        .iter()
        .find(|p| p.number == number)
        .ok_or_else(|| Error::NotFound(format!("no pad {number}")))
}

fn at(def: &FootprintDefinition, number: &str, x: f64, y: f64) -> Result<()> {
    let p = pad(def, number)?.position();
    if (p[0] - x).abs() > TOL || (p[1] - y).abs() > TOL {
        return Err(Error::Invalid(format!(
            "pad {number} at {p:?}, expected [{x}, {y}]"
        )));
    }
    Ok(())
}

fn lint(def: &FootprintDefinition) -> Vec<String> {
    def.lint(pcbcraft_core::DefinitionId(1))
        .into_iter()
        .map(|f| f.code)
        .collect()
}

/// Every terminal assigned by designation passes the publish rules, and
/// each terminal's pad lies where the form puts its lead.
fn publishable_at_the_leads(def: &FootprintDefinition, pkg: &PackageDefinition) -> Result<()> {
    let (def, _) = footprint_edit::assign_terminals(def, pkg, &AssignRule::ByPackageDesignation)?;
    footprint_edit::check_fresh(&def, pkg, &EV)?;
    let leads = pkg.form.lead_points();
    assert_eq!(leads.len(), pkg.terminals.len());
    for (terminal, lead) in pkg.terminals.iter().zip(&leads) {
        let p = def
            .pads
            .iter()
            .find(|p| p.terminal == Some(terminal.id))
            .ok_or_else(|| Error::NotFound(format!("pad of {}", terminal.designation)))?;
        assert!(
            (p.position()[0] - lead[0]).abs() < TOL && (p.position()[1] - lead[1]).abs() < TOL,
            "{}: pad {:?}, lead {lead:?}",
            terminal.designation,
            p.position
        );
    }
    Ok(())
}

fn set(
    def: &FootprintDefinition,
    pkg: &PackageDefinition,
    name: &str,
    expr: &str,
    unit: ParamUnit,
) -> Result<FootprintDefinition> {
    let (def, _) = footprint_edit::edit(
        def,
        pkg,
        &[FootprintEdit::SetParameter {
            name: ParamName(name.into()),
            expr: Expr(expr.into()),
            unit,
        }],
        &EV,
    )?;
    Ok(def)
}

fn bga(rows: u32, cols: u32, depopulated: Vec<[u32; 2]>) -> GridForm {
    let pitch = 0.8;
    let size = |n: u32| f64::from(n) * pitch + 0.6;
    GridForm {
        depopulated,
        ..GridForm::ball_grid(
            rows,
            cols,
            nm(pitch),
            [nm(size(cols)), nm(size(rows)), nm(0.9)],
            [nm(0.4), nm(0.3)],
        )
    }
}

fn grid_package(form: GridForm) -> PackageDefinition {
    let names = form.ball_names();
    package(PackageForm::Grid(Box::new(form)), &names)
}

#[test]
fn a_ball_grid_is_lettered_from_a1_with_nsmd_lands_at_the_balls() -> Result<()> {
    let pkg = grid_package(bga(4, 5, Vec::new()));
    let template = FootprintTemplate::defaults(TemplateKind::Grid, &pkg);
    let FootprintTemplate::Grid(t) = &template else {
        return Err(Error::Invalid(format!("{template:?}")));
    };
    assert_eq!((t.rows.0.as_str(), t.cols.0.as_str()), ("pkg_ME", "pkg_MD"));
    assert_eq!(t.land_kind, GridLand::Nsmd);
    let def = build(&pkg, &template)?;
    assert_eq!(def.pads.len(), 20);
    at(&def, "A1", -1.6, 1.2)?;
    at(&def, "A5", 1.6, 1.2)?;
    at(&def, "B1", -1.6, 0.4)?;
    at(&def, "D5", 1.6, -1.2)?;
    for p in &def.pads {
        assert_eq!(p.kind, PadKind::Smd);
        // 0.8 × the 0.4 mm ball, the mask 0.05 mm clear all round.
        assert!(
            matches!(p.shape, PadShape::Circle { diameter_nm } if (diameter_nm.to_mm_f64() - 0.32).abs() < TOL),
            "{:?}",
            p.shape
        );
        assert!(
            matches!(p.mask, MaskRule::Expansion(v) if (v.to_mm_f64() - 0.05).abs() < TOL),
            "{}: {:?}",
            p.number,
            p.mask
        );
    }
    assert_eq!(
        def.polarity.as_ref().map(|p| p.pad),
        Some(pad(&def, "A1")?.id)
    );
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");
    publishable_at_the_leads(&def, &pkg)?;
    // The matrix is a parameter: one more column re-solves the grid
    // around the origin.
    let wider = set(&def, &pkg, "cols", "6", ParamUnit::Count)?;
    assert_eq!(wider.pads.len(), 24);
    at(&wider, "A1", -2.0, 1.2)?;
    at(&wider, "D6", 2.0, -1.2)?;
    Ok(())
}

#[test]
fn depopulated_positions_leave_gaps_and_keep_every_other_name() -> Result<()> {
    // A 10 × 10 grid with a 2 × 2 centre void and an empty A2.
    let form = bga(10, 10, vec![[4, 4], [4, 5], [5, 4], [5, 5], [0, 1]]);
    let pkg = grid_package(form);
    let FootprintTemplate::Grid(mut t) = FootprintTemplate::defaults(TemplateKind::Grid, &pkg)
    else {
        return Err(Error::Invalid("grid defaults".into()));
    };
    assert_eq!(t.depopulated.len(), 5);
    assert_eq!(t.skip_letters, "IOQSXZ");
    t.land_kind = GridLand::Smd;
    let def = build(&pkg, &FootprintTemplate::Grid(t))?;
    assert_eq!(def.pads.len(), 95);
    for gone in ["A2", "E5", "E6", "F5", "F6"] {
        assert!(pad(&def, gone).is_err(), "{gone} is depopulated");
    }
    // Row I is skipped: the ninth row is J, and A3 keeps its name.
    at(&def, "A3", -2.0, 3.6)?;
    at(&def, "J1", -3.6, -2.8)?;
    assert!(pad(&def, "I1").is_err());
    // Solder-mask defined: the opening 0.05 mm inside the copper.
    assert!(matches!(pad(&def, "K10")?.mask, MaskRule::Expansion(v) if (v.to_mm_f64() + 0.05).abs() < TOL));
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");
    publishable_at_the_leads(&def, &pkg)?;
    Ok(())
}

#[test]
fn a_land_grid_uses_the_land_diameter_and_a_depopulated_a1_is_refused() -> Result<()> {
    let mut form = bga(3, 3, Vec::new());
    form.contact = pcbcraft_core::library::package_form::grid::GridContact::Land;
    form.contact_height_nm = nm(0.0);
    let pkg = grid_package(form.clone());
    let template = FootprintTemplate::defaults(TemplateKind::Grid, &pkg);
    let def = build(&pkg, &template)?;
    assert!(
        matches!(pad(&def, "B2")?.shape, PadShape::Circle { diameter_nm } if (diameter_nm.to_mm_f64() - 0.4).abs() < TOL)
    );
    publishable_at_the_leads(&def, &pkg)?;

    form.depopulated = vec![[0, 0]];
    let pkg = grid_package(form);
    let refused = build(&pkg, &FootprintTemplate::defaults(TemplateKind::Grid, &pkg));
    assert!(
        matches!(&refused, Err(Error::Invalid(m)) if m.contains("A1")),
        "{refused:?}"
    );
    let FootprintTemplate::Grid(mut bad) = template else {
        return Err(Error::Invalid("grid".into()));
    };
    bad.skip_letters = "ZA".into();
    assert!(build(&pkg, &FootprintTemplate::Grid(bad)).is_err());
    Ok(())
}

fn tht_package(form: ThtForm) -> PackageDefinition {
    let n = usize::try_from(PackageForm::Tht(Box::new(form)).lead_count()).unwrap_or(0);
    package(PackageForm::Tht(Box::new(form)), &numbered(n))
}

fn dip8() -> ThtForm {
    ThtForm::new(
        ThtKind::Dip {
            pins: 8,
            row_pitch_nm: nm(7.5),
            body_length_nm: nm(9.5),
            body_width_nm: nm(6.5),
            body_height_nm: nm(3.5),
        },
        nm(2.5),
        nm(0.5),
        nm(3.0),
    )
}

#[test]
fn a_dip_is_plated_counter_clockwise_from_the_bottom_left_with_pin_one_marked() -> Result<()> {
    let pkg = tht_package(dip8());
    let template = FootprintTemplate::defaults(TemplateKind::Tht, &pkg);
    let FootprintTemplate::Tht(t) = &template else {
        return Err(Error::Invalid(format!("{template:?}")));
    };
    assert_eq!(t.layout, ThtLayout::DualRow);
    assert_eq!(t.per_row.0, "pkg_n / 2");
    let def = build(&pkg, &template)?;
    assert_eq!(def.pads.len(), 8);
    at(&def, "1", -3.75, -3.75)?;
    at(&def, "4", 3.75, -3.75)?;
    at(&def, "5", 3.75, 3.75)?;
    at(&def, "8", -3.75, 3.75)?;
    for p in &def.pads {
        assert_eq!(p.kind, PadKind::PlatedThrough);
        let hole = p
            .hole
            .as_ref()
            .ok_or_else(|| Error::NotFound("hole".into()))?;
        assert!(hole.plated);
        // The drill clears the 0.5 × 0.25 lead's diagonal by 0.3 mm,
        // rounded up to 0.05 mm.
        assert!(
            matches!(hole.shape, HoleShape::Round { diameter_nm } if (diameter_nm.to_mm_f64() - 0.9).abs() < TOL),
            "{:?}",
            hole.shape
        );
    }
    assert!(matches!(pad(&def, "1")?.shape, PadShape::RoundRect { .. }));
    assert!(
        matches!(pad(&def, "2")?.shape, PadShape::Circle { diameter_nm } if (diameter_nm.to_mm_f64() - 1.5).abs() < TOL)
    );
    assert!(def.polarity.is_some());
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");
    publishable_at_the_leads(&def, &pkg)?;
    Ok(())
}

#[test]
fn two_lead_and_to_rows_follow_the_form() -> Result<()> {
    // Axial: two pads a lead pitch apart, unmarked unless asked.
    let axial = ThtForm::new(
        ThtKind::Axial {
            body_diameter_nm: nm(2.5),
            body_length_nm: nm(6.0),
        },
        nm(10.0),
        nm(0.6),
        nm(3.0),
    );
    let pkg = tht_package(axial);
    let template = FootprintTemplate::defaults(TemplateKind::Tht, &pkg);
    let def = build(&pkg, &template)?;
    assert_eq!(def.pads.len(), 2);
    at(&def, "1", -5.0, 0.0)?;
    at(&def, "2", 5.0, 0.0)?;
    assert!(def.polarity.is_none());
    assert!(pad(&def, "1")?.hole.as_ref().is_some_and(|h| h.plated));
    publishable_at_the_leads(&def, &pkg)?;
    // A polarised radial part asks for its pin-1 mark.
    let radial = ThtForm::new(
        ThtKind::Radial {
            body_diameter_nm: nm(5.0),
            body_height_nm: nm(11.0),
        },
        nm(2.0),
        nm(0.5),
        nm(3.0),
    );
    let pkg = tht_package(radial);
    let FootprintTemplate::Tht(mut t) = FootprintTemplate::defaults(TemplateKind::Tht, &pkg) else {
        return Err(Error::Invalid("tht".into()));
    };
    t.pin1 = true;
    let def = build(&pkg, &FootprintTemplate::Tht(t))?;
    at(&def, "1", -1.0, 0.0)?;
    // The land shrinks within the 2 mm pitch.
    assert!(
        matches!(pad(&def, "2")?.shape, PadShape::Circle { diameter_nm } if diameter_nm.to_mm_f64() <= 1.75 + TOL)
    );
    assert!(def.polarity.is_some());
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");
    publishable_at_the_leads(&def, &pkg)?;
    // A TO row of three, from −x.
    let to = ThtForm::new(
        ThtKind::To {
            pins: 3,
            body_length_nm: nm(10.0),
            body_width_nm: nm(4.5),
            body_height_nm: nm(9.0),
        },
        nm(2.5),
        nm(0.8),
        nm(3.0),
    );
    let pkg = tht_package(to);
    let def = build(&pkg, &FootprintTemplate::defaults(TemplateKind::Tht, &pkg))?;
    at(&def, "1", -2.5, 0.0)?;
    at(&def, "3", 2.5, 0.0)?;
    assert!(matches!(pad(&def, "1")?.shape, PadShape::RoundRect { .. }));
    let found = lint(&def);
    assert!(found.is_empty(), "{found:?}");
    publishable_at_the_leads(&def, &pkg)?;
    Ok(())
}

#[test]
fn later_templates_build_from_bare_packages_with_literals() -> Result<()> {
    for (kind, n) in [(TemplateKind::Grid, 16), (TemplateKind::Tht, 3)] {
        let pkg = package(PackageForm::Generic, &numbered(n));
        let template = FootprintTemplate::defaults(kind, &pkg);
        let json = serde_json::to_string(&template).unwrap_or_default();
        assert!(!json.contains("pkg_"), "{kind:?}: {json}");
        let def = build(&pkg, &template)?;
        assert_eq!(def.pads.len(), n, "{kind:?}");
        let found = lint(&def);
        assert!(found.is_empty(), "{kind:?}: {found:?}");
    }
    Ok(())
}
