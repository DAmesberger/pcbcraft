//! B2a/B2b evidence: the tabbed (SOT, DPAK) and molded two-terminal (SOD,
//! SMA, MELF) footprint templates built with the real `SketchEvaluator`,
//! driven by the package forms' `pkg_*` values. Fixtures are original
//! round-number forms, from no library, datasheet or EDA tool.

use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::ids::PackageTerminalId;
use pcbcraft_core::library::footprint_edit;
use pcbcraft_core::library::footprint_source::{Expr, FootprintEdit, ParamName, ParamUnit};
use pcbcraft_core::library::package_form::molded::MoldedLead;
use pcbcraft_core::library::package_form::tabbed::Tab;
use pcbcraft_core::library::package_form::{MoldedForm, TabbedForm};
use pcbcraft_core::library::templates::footprint_family::{self, FootprintTemplate, TemplateKind};
use pcbcraft_core::library::{
    FootprintDefinition, PackageDefinition, PackageForm, PackageTerminal, Pad,
};
use pcbcraft_solidcraft::footprint::SketchEvaluator;

const EV: SketchEvaluator = SketchEvaluator;
const TOL: f64 = 1e-6;

fn package(form: PackageForm) -> PackageDefinition {
    let n = u32::try_from(form.lead_count()).unwrap_or(0);
    PackageDefinition {
        envelope: form.envelope().unwrap_or([1.0; 3]),
        terminals: (1..=n)
            .map(|k| PackageTerminal {
                id: PackageTerminalId(k + 100),
                designation: k.to_string(),
            })
            .collect(),
        next_local: n + 101,
        form,
        ..PackageDefinition::default()
    }
}

fn empty() -> FootprintDefinition {
    FootprintDefinition {
        next_local: 1,
        ..FootprintDefinition::default()
    }
}

fn built(pkg: &PackageDefinition, kind: TemplateKind) -> Result<FootprintDefinition> {
    let template = FootprintTemplate::defaults(kind, pkg);
    let (def, _) = footprint_family::build(&empty(), pkg, &template, &EV)?;
    Ok(def)
}

fn pad<'a>(def: &'a FootprintDefinition, number: &str) -> Result<&'a Pad> {
    def.pads
        .iter()
        .find(|p| p.number == number)
        .ok_or_else(|| Error::NotFound(format!("no pad {number}")))
}

/// Pad `number`'s centre and size.
fn check(def: &FootprintDefinition, number: &str, at: [f64; 2], size: [f64; 2]) -> Result<()> {
    let p = pad(def, number)?;
    let b = p.bounds();
    let got = [b.max[0] - b.min[0], b.max[1] - b.min[1]];
    let ok = (0..2).all(|i| (p.position[i] - at[i]).abs() < TOL && (got[i] - size[i]).abs() < TOL);
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "pad {number} at {:?} size {got:?}, expected {at:?} size {size:?}",
            p.position
        )))
    }
}

fn lint(def: &FootprintDefinition) -> Vec<String> {
    def.lint(pcbcraft_core::DefinitionId(1))
        .into_iter()
        .map(|f| f.code)
        .collect()
}

fn sot23_3() -> TabbedForm {
    TabbedForm {
        leads_per_side: [2, 1],
        pitch: 0.95,
        body_length: 2.9,
        body_width: 1.3,
        body_height: 1.0,
        standoff: 0.05,
        span: 2.4,
        lead_width: 0.4,
        lead_thickness: 0.12,
        foot_length: 0.4,
        tab: None,
        lead_step: [2, 1],
        lands: Default::default(),
    }
}

fn sot223() -> TabbedForm {
    TabbedForm {
        leads_per_side: [3, 0],
        pitch: 2.3,
        body_length: 6.5,
        body_width: 3.5,
        body_height: 1.6,
        standoff: 0.05,
        span: 7.0,
        lead_width: 0.7,
        lead_thickness: 0.25,
        foot_length: 0.9,
        tab: Some(Tab {
            width: 3.0,
            length: 0.9,
            flange: None,
        }),
        lead_step: [1, 1],
        lands: Default::default(),
    }
}

fn dpak() -> TabbedForm {
    TabbedForm {
        leads_per_side: [2, 0],
        pitch: 2.3,
        body_length: 6.5,
        body_width: 6.0,
        body_height: 2.3,
        standoff: 0.0,
        span: 11.0,
        lead_width: 0.8,
        lead_thickness: 0.5,
        foot_length: 1.5,
        tab: Some(Tab {
            width: 5.3,
            length: 5.5,
            flange: Some(1.0),
        }),
        lead_step: [2, 1],
        lands: Default::default(),
    }
}

fn tabbed(f: TabbedForm) -> PackageDefinition {
    package(PackageForm::Tabbed(Box::new(f)))
}

#[test]
fn sot23_3_lands_follow_the_lead_grid() -> Result<()> {
    let def = built(&tabbed(sot23_3()), TemplateKind::Tabbed)?;
    assert_eq!(def.pads.len(), 3);
    // Row span 2.4 − 0.4 + 0.2 = 2.2; lands 0.6 × 1.0.
    check(&def, "1", [-0.95, -1.1], [0.6, 1.0])?;
    check(&def, "2", [0.95, -1.1], [0.6, 1.0])?;
    check(&def, "3", [0.0, 1.1], [0.6, 1.0])?;
    assert_eq!(
        def.polarity.as_ref().map(|p| p.pad),
        Some(pad(&def, "1")?.id)
    );
    assert!(lint(&def).is_empty(), "{:?}", lint(&def));
    Ok(())
}

#[test]
fn sot23_5_lands_skip_the_middle_of_the_top_row_and_follow_the_pitch() -> Result<()> {
    let mut f = sot23_3();
    f.leads_per_side = [3, 2];
    f.lead_step = [1, 2];
    f.body_width = 1.6;
    f.span = 2.8;
    let pkg = tabbed(f);
    let def = built(&pkg, TemplateKind::Tabbed)?;
    assert_eq!(def.pads.len(), 5);
    let y = (2.8 - 0.4 + 0.2) / 2.0;
    check(&def, "2", [0.0, -y], [0.6, 1.0])?;
    check(&def, "4", [0.95, y], [0.6, 1.0])?;
    check(&def, "5", [-0.95, y], [0.6, 1.0])?;
    // Editing the pitch re-solves every land.
    let (def, _) = footprint_edit::edit(
        &def,
        &pkg,
        &[FootprintEdit::SetParameter {
            name: ParamName("pitch".into()),
            expr: Expr("1.0".into()),
            unit: ParamUnit::Mm,
        }],
        &EV,
    )?;
    check(&def, "3", [1.0, -y], [0.6, 1.0])?;
    check(&def, "5", [-1.0, y], [0.6, 1.0])?;
    Ok(())
}

/// The datasheet's lands go through the same path as the estimates: the
/// template's defaults name the form's values, and the pads solve to them.
#[test]
fn a_datasheet_land_pattern_drives_the_template() -> Result<()> {
    let mut f = sot23_3();
    f.lands.size = Some([0.6, 1.3]);
    f.lands.span = Some(2.1);
    let pkg = tabbed(f);
    let def = built(&pkg, TemplateKind::Tabbed)?;
    check(&def, "1", [-0.95, -1.05], [0.6, 1.3])?;
    check(&def, "2", [0.95, -1.05], [0.6, 1.3])?;
    check(&def, "3", [0.0, 1.05], [0.6, 1.3])?;

    let mut s = sot223();
    s.lands.tab_size = Some([3.5, 2.0]);
    let def = built(&tabbed(s), TemplateKind::Tabbed)?;
    // The rows keep their estimates; the tab land takes the datasheet's.
    check(&def, "1", [-2.3, -(7.0 - 0.9 + 0.2) / 2.0], [0.9, 1.5])?;
    check(&def, "4", [0.0, 3.15], [3.5, 2.0])?;
    Ok(())
}

#[test]
fn sot223_and_dpak_tab_lands_come_last() -> Result<()> {
    let def = built(&tabbed(sot223()), TemplateKind::Tabbed)?;
    assert_eq!(def.pads.len(), 4);
    let y = (7.0 - 0.9 + 0.2) / 2.0;
    check(&def, "1", [-2.3, -y], [0.9, 1.5])?;
    check(&def, "3", [2.3, -y], [0.9, 1.5])?;
    // Tab contact centre 3.5 − 0.45; the land is shifted 0.1 out.
    check(&def, "4", [0.0, 3.15], [3.2, 1.5])?;
    assert!(lint(&def).is_empty(), "{:?}", lint(&def));

    let def = built(&tabbed(dpak()), TemplateKind::Tabbed)?;
    assert_eq!(def.pads.len(), 3);
    let y = (11.0 - 1.5 + 0.2) / 2.0;
    check(&def, "1", [-2.3, -y], [1.0, 2.1])?;
    check(&def, "2", [2.3, -y], [1.0, 2.1])?;
    check(&def, "3", [0.0, 1.25], [5.5, 5.9])?;
    assert!(lint(&def).is_empty(), "{:?}", lint(&def));
    Ok(())
}

fn sma() -> MoldedForm {
    MoldedForm {
        lead: MoldedLead::JLead,
        body_length: 4.3,
        body_width: 2.6,
        body_height: 2.2,
        span: 5.2,
        terminal_length: 1.1,
        terminal_width: 1.4,
        polarised: true,
    }
}

#[test]
fn molded_lands_straddle_the_terminals_with_pin_one_marked() -> Result<()> {
    let pkg = package(PackageForm::Molded(Box::new(sma())));
    let def = built(&pkg, TemplateKind::Molded)?;
    assert_eq!(def.pads.len(), 2);
    // Gap 5.2 − 2.2 − 0.4 = 2.6, lands 1.7 × 1.6, centres ±2.15.
    check(&def, "1", [-2.15, 0.0], [1.7, 1.6])?;
    check(&def, "2", [2.15, 0.0], [1.7, 1.6])?;
    assert_eq!(
        def.polarity.as_ref().map(|p| p.pad),
        Some(pad(&def, "1")?.id)
    );
    assert!(lint(&def).is_empty(), "{:?}", lint(&def));
    let mut plain = sma();
    plain.polarised = false;
    let def = built(
        &package(PackageForm::Molded(Box::new(plain))),
        TemplateKind::Molded,
    )?;
    assert_eq!(def.polarity, None);
    Ok(())
}

#[test]
fn literal_defaults_build_without_a_family_form() -> Result<()> {
    for (kind, n) in [(TemplateKind::Tabbed, 3), (TemplateKind::Molded, 2)] {
        let pkg = PackageDefinition {
            envelope: [3.0, 2.5, 1.1],
            terminals: (1..=n)
                .map(|k| PackageTerminal {
                    id: PackageTerminalId(k + 100),
                    designation: k.to_string(),
                })
                .collect(),
            next_local: n + 101,
            ..PackageDefinition::default()
        };
        let def = built(&pkg, kind)?;
        assert_eq!(def.pads.len(), n as usize, "{kind:?}");
        assert!(lint(&def).is_empty(), "{kind:?}: {:?}", lint(&def));
    }
    Ok(())
}

#[test]
fn a_bad_count_changes_nothing() {
    let pkg = tabbed(sot23_3());
    let FootprintTemplate::Tabbed(mut t) = FootprintTemplate::defaults(TemplateKind::Tabbed, &pkg)
    else {
        return;
    };
    t.leads_per_side = [0, 1];
    let refused = footprint_family::build(&empty(), &pkg, &FootprintTemplate::Tabbed(t), &EV);
    assert!(
        matches!(&refused, Err(Error::Invalid(m)) if m.starts_with("footprint.template_invalid")),
        "{refused:?}"
    );
}
