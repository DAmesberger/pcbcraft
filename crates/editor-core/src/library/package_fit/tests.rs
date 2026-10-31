//! Original fixtures: a SOIC-8-like gull-wing form on a hand-placed land
//! pattern whose rows run along y (pin 1 top left, counter-clockwise), the
//! model's rows along x, so the fit must find a quarter turn.
use super::*;
use crate::ids::{DefinitionId, PadId};
use crate::library::footprint::Pad;
use crate::library::package_form::{ChipForm, ChipKind, GullWingForm, LeadSides};
use crate::library::{PackageTerminal, footprint::FootprintDefinition};

fn soic_form() -> PackageForm {
    PackageForm::GullWing(Box::new(GullWingForm {
        sides: LeadSides::Two,
        leads_per_side: [4, 0],
        pitch: 1.27,
        body_length: 4.9,
        body_width: 3.9,
        body_height: 1.5,
        standoff: 0.1,
        span: [6.0, 0.0],
        lead_width: 0.4,
        lead_thickness: 0.2,
        foot_length: 0.6,
    }))
}

fn package(form: PackageForm, terminals: u32) -> PackageDefinition {
    PackageDefinition {
        terminals: (1..=terminals)
            .map(|i| PackageTerminal {
                id: PackageTerminalId(i),
                designation: i.to_string(),
            })
            .collect(),
        form,
        ..PackageDefinition::default()
    }
}

/// Pads 1–4 down the −x column, 5–8 up the +x column, at x = ±2.7.
fn soic_footprint(clockwise: bool) -> FootprintDefinition {
    let y = |i: u32| 1.905 - 1.27 * f64::from(i);
    let pads = (0..8)
        .map(|k| {
            let position = if k < 4 {
                [-2.7, y(k)]
            } else {
                [2.7, -y(k - 4)]
            };
            let position = if clockwise {
                [position[0], -position[1]]
            } else {
                position
            };
            Pad::smd_rect(
                PadId(k + 1),
                PackageTerminalId(k + 1),
                (k + 1).to_string(),
                position,
                [1.55, 0.6],
            )
        })
        .collect();
    FootprintDefinition {
        pads,
        ..FootprintDefinition::default()
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn soic_pairs(fp: &FootprintDefinition) -> Result<Vec<LeadPair>> {
    form_pairs(&package(soic_form(), 8), fp)
}

#[test]
fn lead_points_count_counter_clockwise_from_pin_one() {
    let leads = soic_form().lead_points();
    assert_eq!(leads.len(), 8);
    assert_eq!(leads[0], [-1.905, -2.7, 0.0]);
    assert_eq!(leads[3], [1.905, -2.7, 0.0]);
    assert_eq!(leads[4], [1.905, 2.7, 0.0]);
    assert_eq!(leads[7], [-1.905, 2.7, 0.0]);
    let chip = PackageForm::Chip(Box::new(ChipForm {
        kind: ChipKind::Resistor,
        length: 1.6,
        width: 0.8,
        height: 0.45,
        termination: 0.3,
    }));
    assert_eq!(
        chip.lead_points(),
        vec![[-0.65, 0.0, 0.0], [0.65, 0.0, 0.0]]
    );
    assert!(PackageForm::Generic.lead_points().is_empty());
}

#[test]
fn a_soic_fits_its_pads_with_an_exact_quarter_turn_and_is_seated() -> Result<()> {
    let fp = soic_footprint(false);
    let start = ModelPlacement {
        translation: [0.4, -0.3, 0.7],
        rotation_deg: [0.0, 0.0, 12.0],
    };
    for rotation in [FitRotation::Free, FitRotation::Quarter] {
        let fit = fit_leads(&start, &soic_pairs(&fp)?, rotation)?;
        assert_eq!(
            fit.placement.rotation_deg,
            [0.0, 0.0, -90.0],
            "{rotation:?}"
        );
        assert!(fit.placement.translation.iter().all(|v| v.abs() < 1e-9));
        assert!(fit.rms_mm < 1e-9, "rms {}", fit.rms_mm);
        assert_eq!(fit.residuals.len(), 8);
        assert!(!fit.mirrored_better);
    }
    Ok(())
}

#[test]
fn a_turned_land_pattern_fits_freely_but_not_by_quarter_turns() -> Result<()> {
    let mut fp = soic_footprint(false);
    for pad in &mut fp.pads {
        pad.position = rotate(pad.position, 30.0);
        pad.position = [pad.position[0] + 1.0, pad.position[1] - 2.0];
    }
    let pairs = soic_pairs(&fp)?;
    let free = fit_leads(&ModelPlacement::default(), &pairs, FitRotation::Free)?;
    assert!(close(free.placement.rotation_deg[2], -60.0));
    assert!(free.rms_mm < 1e-9);
    let quarter = fit_leads(&ModelPlacement::default(), &pairs, FitRotation::Quarter)?;
    assert!(quarter.placement.quarter_turns());
    assert!(quarter.rms_mm > 0.5, "rms {}", quarter.rms_mm);
    Ok(())
}

#[test]
fn a_clockwise_numbered_land_pattern_reports_a_better_mirrored_fit() -> Result<()> {
    let fit = fit_leads(
        &ModelPlacement::default(),
        &soic_pairs(&soic_footprint(true))?,
        FitRotation::Free,
    )?;
    assert!(fit.rms_mm > 0.5);
    assert!(fit.mirrored_better);
    Ok(())
}

#[test]
fn a_flipped_model_keeps_its_tilt_and_is_lifted_onto_the_plane() -> Result<()> {
    let fp = soic_footprint(false);
    let flipped = ModelPlacement {
        translation: [0.0, 0.0, -3.0],
        rotation_deg: [180.0, 0.0, 0.0],
    };
    let fit = fit_leads(&flipped, &soic_pairs(&fp)?, FitRotation::Free)?;
    // Upside down the model is mirrored, so it cannot fit exactly, but its
    // tilt is kept and the leads sit on the seating plane.
    let frame = fit.placement.transform();
    assert!(close(frame.apply_vector([0.0, 0.0, 1.0])[2], -1.0));
    let lowest = soic_form()
        .lead_points()
        .into_iter()
        .map(|p| frame.apply(p)[2])
        .fold(f64::INFINITY, f64::min);
    assert!(close(lowest, 0.0));
    Ok(())
}

#[test]
fn malformed_pairs_are_refused() {
    let pair = LeadPair {
        terminal: PackageTerminalId(1),
        pad: PadId(1),
        lead: [0.0; 3],
        pad_centre: [0.0; 2],
    };
    let none = ModelPlacement::default();
    assert!(fit_leads(&none, &[], FitRotation::Free).is_err());
    assert!(fit_leads(&none, &[pair, pair], FitRotation::Free).is_err());
    let bad = LeadPair {
        lead: [f64::NAN, 0.0, 0.0],
        ..pair
    };
    assert!(fit_leads(&none, &[bad], FitRotation::Free).is_err());
    let single = fit_leads(
        &none,
        &[LeadPair {
            pad_centre: [1.0, 2.0],
            ..pair
        }],
        FitRotation::Free,
    );
    assert_eq!(
        single.map(|f| f.placement.translation).ok(),
        Some([1.0, 2.0, 0.0])
    );
}

#[test]
fn intrinsic_angles_round_trip_and_quarter_turns_stay_exact() {
    for angles in [
        [10.0, 20.0, 30.0],
        [-75.0, 40.0, 170.0],
        [90.0, 0.0, -90.0],
        [180.0, 0.0, 90.0],
        [0.0, -90.0, 0.0],
    ] {
        let r = Transform3::intrinsic_xyz(angles).rotation;
        let back = Transform3::intrinsic_xyz(intrinsic_xyz_of(&r)).rotation;
        for (a, b) in r.iter().flatten().zip(back.iter().flatten()) {
            assert!((a - b).abs() < 1e-9, "{angles:?}");
        }
    }
    let r = Transform3::intrinsic_xyz([90.0, 0.0, -90.0]).rotation;
    assert_eq!(intrinsic_xyz_of(&r), [90.0, 0.0, -90.0]);
}

#[test]
fn quick_turns_keep_the_body_centre_and_undo_each_other() -> Result<()> {
    let mut p = package(soic_form(), 8);
    p.placement.translation = [1.0, 2.0, 0.0];
    let centre = body_centre(&p, &p.placement);
    for axis in [Axis::X, Axis::Y, Axis::Z] {
        let turned = apply_quick(
            &p,
            None,
            QuickAction::Rotate {
                axis,
                quarter_turns: 1,
            },
        )?;
        assert!(turned.quarter_turns());
        let c = body_centre(&p, &turned);
        assert!(c.iter().zip(centre).all(|(a, b)| close(*a, b)));
        let mut q = p.clone();
        q.placement = turned;
        let back = apply_quick(
            &q,
            None,
            QuickAction::Rotate {
                axis,
                quarter_turns: -1,
            },
        )?;
        assert_eq!(back.rotation_deg, [0.0; 3]);
        assert!(
            back.translation
                .iter()
                .zip(p.placement.translation)
                .all(|(a, b)| close(*a, b))
        );
    }
    let flipped = apply_quick(&p, None, QuickAction::Flip)?;
    assert_eq!(flipped.rotation_deg, [180.0, 0.0, 0.0]);
    assert!(
        apply_quick(
            &p,
            None,
            QuickAction::Rotate {
                axis: Axis::Z,
                quarter_turns: 0
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn seat_and_stand_up_put_the_leads_on_the_plane() -> Result<()> {
    let mut p = package(soic_form(), 8);
    p.placement.translation = [0.0, 0.0, 0.8];
    assert!(close(
        apply_quick(&p, None, QuickAction::Seat)?.translation[2],
        0.0
    ));
    p.placement = ModelPlacement::default();
    let stood = apply_quick(&p, None, QuickAction::StandUp)?;
    // The source's +y now points up.
    let up = stood.transform().apply_vector([0.0, 1.0, 0.0]);
    assert_eq!(up, [0.0, 0.0, 1.0]);
    let lowest = p
        .form
        .lead_points()
        .into_iter()
        .map(|q| stood.transform().apply(q)[2])
        .fold(f64::INFINITY, f64::min);
    assert!(close(lowest, 0.0));
    Ok(())
}

#[test]
fn pad_relative_actions_need_the_footprint_and_match_the_fit() -> Result<()> {
    let fp = soic_footprint(false);
    let mut p = package(soic_form(), 8);
    p.placement.translation = [0.5, 0.5, 0.0];
    assert!(apply_quick(&p, None, QuickAction::CentreOnPads).is_err());
    assert!(
        package_model::edit_package(&p, &[PackageEdit::ApplyQuick(QuickAction::Pin1ToPad1)])
            .is_err()
    );
    let centred = apply_quick(&p, Some(&fp), QuickAction::CentreOnPads)?;
    assert!(centred.translation.iter().all(|v| v.abs() < 1e-9));
    let pin1 = apply_quick(&p, Some(&fp), QuickAction::Pin1ToPad1)?;
    assert_eq!(pin1.rotation_deg, [0.0, 0.0, -90.0]);
    let fit = fit_leads(&p.placement, &form_pairs(&p, &fp)?, FitRotation::Quarter)?;
    assert!(
        pin1.translation
            .iter()
            .zip(fit.placement.translation)
            .all(|(a, b)| close(*a, b))
    );
    let edited = edit_package_in(
        &p,
        Some(&fp),
        &[
            PackageEdit::ApplyQuick(QuickAction::Pin1ToPad1),
            PackageEdit::ApplyQuick(QuickAction::Seat),
        ],
    )?;
    assert_eq!(edited.placement.rotation_deg, [0.0, 0.0, -90.0]);
    Ok(())
}

#[test]
fn findings_report_leads_off_lands_unseated_leads_and_a_sunken_body() -> Result<()> {
    let fp = soic_footprint(false);
    let footprint_id = DefinitionId(7);
    let mut p = package(soic_form(), 8);
    p.placement = fit_leads(&p.placement, &form_pairs(&p, &fp)?, FitRotation::Free)?.placement;
    assert!(placement_findings(&p, &fp, footprint_id).is_empty());
    // Across the 0.6 mm land: 0.1 mm stays within 25 %, 0.2 mm does not.
    p.placement.translation[1] = 0.1;
    assert!(placement_findings(&p, &fp, footprint_id).is_empty());
    p.placement.translation[1] = 0.2;
    let codes: Vec<String> = placement_findings(&p, &fp, footprint_id)
        .into_iter()
        .map(|f| f.code)
        .collect();
    assert_eq!(codes, ["package.lead_off_land"]);
    p.placement.translation = [0.0, 0.0, -0.5];
    let findings = placement_findings(&p, &fp, footprint_id);
    let codes: Vec<&str> = findings.iter().map(|f| f.code.as_str()).collect();
    assert_eq!(codes, ["package.body_below_board", "package.lead_unseated"]);
    assert!(findings.iter().all(|f| f.blocks.is_none()));
    Ok(())
}

/// A drafted SOIC-8 working set in an engine: the package carries the
/// typed form, the footprint the hand-placed land pattern.
fn soic_engine() -> Result<(crate::Engine, crate::tests::support::Fixture)> {
    use crate::library::Definition;
    use crate::tests::support::{self, Spec};
    let mut engine = crate::Engine::default();
    let terminals: Vec<(u32, String)> = (1..=8).map(|i| (i, i.to_string())).collect();
    let fixture = support::draft(
        &mut engine,
        &Spec {
            name: "SOIC",
            terminals: (1..=8).map(|i| (i, "P", support::passive())).collect(),
            units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
            package: terminals.iter().map(|(i, d)| (*i, d.as_str())).collect(),
            mapping: (1..=8).map(|i| (i, i)).collect(),
            physical: true,
        },
    )?;
    support::rewrite(&mut engine, fixture.package, |d| {
        if let Definition::Package(p) = d {
            p.form = soic_form();
        }
    })?;
    support::rewrite(&mut engine, fixture.footprint, |d| {
        if let Definition::Footprint(f) = d {
            f.pads = soic_footprint(false).pads;
        }
    })?;
    Ok((engine, fixture))
}

fn draft_placement(engine: &crate::Engine, id: DefinitionId) -> Option<ModelPlacement> {
    match &engine.drafts().get(id)?.definition {
        crate::library::Definition::Package(p) => Some(p.placement),
        _ => None,
    }
}

#[test]
fn fit_leads_and_quick_actions_are_one_undo_step_each_through_the_engine() -> Result<()> {
    use crate::tests::support::{Snapshot, apply};
    use crate::{LibraryCommand, Operation, Outcome};
    let (mut engine, f) = soic_engine()?;
    // Preview only: the reported fit, nothing stored.
    let report = apply(
        &mut engine,
        Operation::Library(LibraryCommand::FitLeads {
            draft: f.package,
            pairs: vec![],
            rotation: FitRotation::Free,
            confirm: false,
        }),
    )?;
    let Outcome::LeadsFitted { fit, applied } = report else {
        return Err(Error::Invalid("expected a fit".into()));
    };
    assert!(!applied && fit.rms_mm < 1e-9);
    assert_eq!(
        draft_placement(&engine, f.package),
        Some(ModelPlacement::default())
    );
    // Confirmed: stored as one undoable step.
    apply(
        &mut engine,
        Operation::Library(LibraryCommand::FitLeads {
            draft: f.package,
            pairs: vec![],
            rotation: FitRotation::Quarter,
            confirm: true,
        }),
    )?;
    assert_eq!(draft_placement(&engine, f.package), Some(fit.placement));
    engine.undo()?;
    assert_eq!(
        draft_placement(&engine, f.package),
        Some(ModelPlacement::default())
    );
    // A quick action relative to pads finds the footprint draft itself.
    apply(
        &mut engine,
        Operation::Library(LibraryCommand::EditPackage {
            draft: f.package,
            edits: vec![
                PackageEdit::ApplyQuick(QuickAction::Pin1ToPad1),
                PackageEdit::ApplyQuick(QuickAction::Seat),
            ],
        }),
    )?;
    assert_eq!(
        draft_placement(&engine, f.package).map(|p| p.rotation_deg),
        Some([0.0, 0.0, -90.0])
    );
    // A failing edit list changes nothing, not even the first edit.
    let kept = Snapshot::of(&engine);
    let failed = apply(
        &mut engine,
        Operation::Library(LibraryCommand::EditPackage {
            draft: f.package,
            edits: vec![
                PackageEdit::ApplyQuick(QuickAction::Flip),
                PackageEdit::ApplyQuick(QuickAction::Rotate {
                    axis: Axis::Z,
                    quarter_turns: 9,
                }),
            ],
        }),
    );
    assert!(failed.is_err());
    kept.assert_kept(&engine, "refused quick action");
    Ok(())
}

#[test]
fn placement_findings_reach_draft_checks_as_advisories() -> Result<()> {
    use crate::tests::support::rewrite;
    use crate::{CheckTarget, Query, QueryResult};
    let (mut engine, f) = soic_engine()?;
    rewrite(&mut engine, f.package, |d| {
        if let crate::library::Definition::Package(p) = d {
            p.placement.translation = [0.0, 0.0, -0.5];
        }
    })?;
    let Ok(QueryResult::Drafts(reports)) = engine.query(Query::CheckDrafts {}) else {
        return Err(Error::Invalid("expected draft reports".into()));
    };
    let findings: Vec<_> = reports
        .into_iter()
        .filter(|r| r.target == CheckTarget::Draft(f.footprint))
        .flat_map(|r| r.findings)
        .filter(|x| x.code.starts_with("package."))
        .collect();
    let codes: Vec<&str> = findings.iter().map(|x| x.code.as_str()).collect();
    // Unturned, the leads miss the lands too.
    assert!(codes.contains(&"package.body_below_board"));
    assert!(codes.contains(&"package.lead_unseated"));
    assert!(codes.contains(&"package.lead_off_land"));
    assert!(findings.iter().all(|x| x.blocks.is_none()));
    Ok(())
}
