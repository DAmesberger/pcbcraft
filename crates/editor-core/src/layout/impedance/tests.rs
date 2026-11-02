//! The impedance formulas against independent reference values
//! (docs/high-speed-routing.md, "Reference values"):
//!
//! - Pozar, *Microwave Engineering*, Example 3.7 (microstrip): a 50 Ω
//!   line on d = 1.27 mm, εr = 2.20 has W/d = 3.081 and εe = 1.87 (his
//!   synthesis formulas, about 1 %).
//! - Pozar, Example 3.5 (stripline): a 50 Ω line between planes
//!   b = 3.2 mm apart, εr = 2.20, is W = 2.66 mm wide.
//! - Cohn's exact conformal-mapping stripline (K(k)/K(k')), evaluated
//!   here by the arithmetic-geometric mean, for the zero-thickness case.
//! - Hand evaluation of the IPC-2141 coupling equations at chosen points.
use super::formulas::*;
use super::*;
use crate::model::{CopperFoil, Stackup};
use crate::rules::{NetClassSpec, RulesCommand, signal::SignalRules};

fn near(got: f64, want: f64, rel: f64) -> bool {
    (got - want).abs() <= rel * want.abs()
}

#[test]
fn microstrip_matches_pozar_example_3_7() {
    let (z0, eff) = microstrip(3.081 * 1.27, 1.27, 0.0, 2.20);
    assert!(near(z0, 50.0, 0.01), "{z0}");
    assert!(near(eff, 1.87, 0.01), "{eff}");
}

#[test]
fn an_air_microstrip_has_unit_permittivity() {
    let (z0, eff) = microstrip(1.0, 1.0, 0.0, 1.0);
    assert!((eff - 1.0).abs() < 1e-12);
    // Hand evaluation of Hammerstad–Jensen at u = 1: 59.96 Ω · ln(6 + √5).
    assert!(near(z0, 126.42, 0.002), "{z0}");
}

#[test]
fn copper_thickness_lowers_the_impedance() {
    let thin = microstrip(0.3, 0.2, 0.0, 4.3).0;
    let thick = microstrip(0.3, 0.2, 0.035, 4.3).0;
    assert!(thick < thin && thick > 0.85 * thin, "{thin} {thick}");
    let thin = stripline_centred(0.2, 0.6, 0.0, 4.3);
    let thick = stripline_centred(0.2, 0.6, 0.035, 4.3);
    assert!(thick < thin && thick > 0.85 * thin, "{thin} {thick}");
}

#[test]
fn stripline_matches_cohn_and_pozar_example_3_5() {
    let exact = cohn_stripline(2.66, 3.2, 2.20);
    assert!(near(exact, 50.0, 0.005), "{exact}");
    // Wheeler's zero-thickness form within 1 % of the exact one over the
    // widths boards use.
    for k in 1..=30 {
        let ratio = f64::from(k) * 0.1;
        let w = ratio * 1.0;
        let wheeler = stripline_centred(w, 1.0, 0.0, 4.0);
        let cohn = cohn_stripline(w, 1.0, 4.0);
        assert!(near(wheeler, cohn, 0.01), "w/b {ratio}: {wheeler} {cohn}");
    }
    // An offset stripline with equal heights is the centred one.
    let centred = stripline_centred(0.2, 2.0 * 0.3 + 0.035, 0.035, 4.2);
    assert!(near(stripline(0.2, 0.3, 0.3, 0.035, 4.2), centred, 1e-12));
}

#[test]
fn coupling_equations_match_hand_evaluation() {
    // 2·50·(1 − 0.48·e^−0.96) = 81.62 Ω.
    assert!(near(diff_microstrip(50.0, 0.2, 0.2), 81.62, 0.0005));
    // 2·50·(1 − 0.347·e^−1.45) = 91.86 Ω.
    assert!(near(diff_stripline(50.0, 0.2, 0.4), 91.86, 0.0005));
    // Far apart, the halves are two independent lines.
    assert!(near(diff_microstrip(50.0, 100.0, 0.2), 100.0, 1e-9));
    assert!(near(diff_stripline(50.0, 100.0, 0.4), 100.0, 1e-9));
}

#[test]
fn a_deeply_buried_microstrip_sees_the_full_permittivity() {
    let (air, _) = microstrip(0.2, 0.2, 0.035, 1.0);
    let (z, eff) = embedded_microstrip(0.2, 0.2, 0.035, 50.0, 4.0);
    assert!(near(eff, 4.0, 1e-9));
    assert!(near(z, air / 2.0, 1e-9));
    // A thin cover lies between the surface microstrip and full burial.
    let (surface, _) = microstrip(0.2, 0.2, 0.035, 4.0);
    let (thin, _) = embedded_microstrip(0.2, 0.2, 0.035, 0.05, 4.0);
    assert!(thin <= surface && thin >= z, "{surface} {thin} {z}");
}

#[test]
fn solvers_invert_the_formulas() -> Result<()> {
    let s = Structure::Microstrip { h: 0.2, er: 4.3 };
    let w = s.width_for(50.0, 0.035).ok_or(Error::Invalid("w".into()))?;
    assert!(near(s.z0(w, 0.035), 50.0, 1e-6));
    let w = s
        .width_for_diff(90.0, 0.2, 0.035)
        .ok_or(Error::Invalid("w".into()))?;
    assert!(near(s.zdiff(w, 0.2, 0.035), 90.0, 1e-6));
    let g = s
        .gap_for_diff(90.0, 0.15, 0.035)
        .ok_or(Error::Invalid("g".into()))?;
    assert!(near(s.zdiff(0.15, g, 0.035), 90.0, 1e-6));
    assert_eq!(s.width_for(1000.0, 0.035), None);
    Ok(())
}

#[test]
fn layers_find_their_reference_planes() -> Result<()> {
    let two = Stackup::default();
    let top = structure(&two, CopperLayer::Top)?;
    assert_eq!(top.structure, Structure::Microstrip { h: 1.6, er: 4.6 });
    assert!(top.assumed_references && top.dk_estimated);
    let four = Stackup::preset(4)?;
    let top = structure(&four, CopperLayer::Top)?;
    let Structure::Microstrip { h, er } = top.structure else {
        return Err(Error::Invalid("microstrip".into()));
    };
    assert!((h - 0.2).abs() < 1e-9 && (er - 4.3).abs() < 1e-9);
    let inner = structure(&four, CopperLayer::Inner(1))?;
    let Structure::Stripline { h1, h2, .. } = inner.structure else {
        return Err(Error::Invalid("stripline".into()));
    };
    assert!(
        (h1 - 0.2).abs() < 1e-9 && (h2 - 1.165).abs() < 1e-9,
        "{h1} {h2}"
    );
    // Only Inner 2 a plane: Inner 1 is embedded microstrip under the top
    // prepreg and the top foil.
    let mut marked = four.clone();
    if let Some(foil) = marked.copper.get_mut(2) {
        *foil = CopperFoil {
            plane: true,
            ..*foil
        };
    }
    let inner = structure(&marked, CopperLayer::Inner(1))?;
    let Structure::EmbeddedMicrostrip { h, cover, .. } = inner.structure else {
        return Err(Error::Invalid("embedded".into()));
    };
    assert!(
        (h - 1.165).abs() < 1e-9 && (cover - 0.235).abs() < 1e-9,
        "{h} {cover}"
    );
    assert_eq!(inner.references, vec![CopperLayer::Inner(2)]);
    // The top layer has no plane under it once planes are marked and
    // Inner 1 is not one: the next plane down is Inner 2.
    let top = structure(&marked, CopperLayer::Top)?;
    assert!(matches!(top.structure, Structure::Microstrip { h, .. } if (h - 1.4).abs() < 1e-9));
    Ok(())
}

fn usb_class(project: &mut Project, signal: SignalRules) -> Result<NetClassId> {
    crate::rules::apply(
        &mut project.rules,
        RulesCommand::AddNetClass {
            class: NetClassSpec {
                name: "USB".into(),
                signal,
                ..Default::default()
            },
        },
    )?
    .ok_or(Error::Invalid("class".into()))
}

/// The acceptance case: a 90 Ω USB 2.0 pair on the outer layer of the
/// 4-layer preset (0.2 mm 2116 prepreg over a plane).
#[test]
fn a_90_ohm_usb_pair_derives_its_width_and_gap() -> Result<()> {
    let mut project = Project {
        stackup: Stackup::preset(4)?,
        ..Default::default()
    };
    let class = usb_class(
        &mut project,
        SignalRules {
            diff_impedance: Some(90.0),
            ..Default::default()
        },
    )?;
    let derived = derive(&project, Some(class), CopperLayer::Top);
    assert!(derived.problems.is_empty(), "{:?}", derived.problems);
    let (w, g, z) = (
        derived.pair_width.unwrap_or_default(),
        derived.pair_gap.unwrap_or_default(),
        derived.zdiff.unwrap_or_default(),
    );
    assert!(near(z, 90.0, 1e-4), "{z}");
    // The tightest gap the default profile allows sets the width.
    assert!((g - 0.2).abs() < 1e-9, "{g}");
    assert!((0.1..0.4).contains(&w), "{w}");
    Ok(())
}

#[test]
fn an_unreachable_target_names_the_limit() -> Result<()> {
    let mut project = Project::default();
    let class = usb_class(
        &mut project,
        SignalRules {
            impedance: Some(20.0),
            ..Default::default()
        },
    )?;
    let derived = derive(&project, Some(class), CopperLayer::Top);
    assert_eq!(derived.width, Some(5.0));
    let [problem] = derived.problems.as_slice() else {
        return Err(Error::Invalid(format!("{:?}", derived.problems)));
    };
    assert!(problem.contains("wider than routing allows"), "{problem}");
    assert!(crate::issues::style::naming(problem).is_empty());
    Ok(())
}

#[test]
fn the_calculator_refuses_bad_dimensions() {
    let project = Project::default();
    for width in [0.0, -1.0, f64::NAN, 1e9] {
        assert!(
            calculate(
                &project,
                &ImpedanceRequest::Layer {
                    layer: CopperLayer::Top,
                    width,
                    gap: None
                }
            )
            .is_err()
        );
    }
    assert!(
        calculate(
            &project,
            &ImpedanceRequest::Layer {
                layer: CopperLayer::Inner(1),
                width: 0.2,
                gap: None
            }
        )
        .is_err()
    );
}
