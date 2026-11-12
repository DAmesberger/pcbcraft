//! Pad hole checks on the demo board (R1 at (10, 10), R2 at (25, 10))
//! with its resistor pads made through-hole, under the shipped JLCPCB
//! example profile. Every expected ring and gap is derived by hand from
//! the pad and hole sizes.
use super::super::tests::fixture;
use crate::diagnostics::{Error, Finding, Result};
use crate::geometry::Point;
use crate::ids::Id;
use crate::layout::{PlacedPad, checks, placed_pads};
use crate::library::{Hole, HoleShape, PadKind, PadShape};
use crate::model::{CopperLayer, Project, Track, Via, ViaSpan};
use crate::rules::{CopperObject, ProjectRules, RuleKey, RuleOverride, RuleValue, fixtures};

/// The demo board on the JLCPCB example, every resistor pad of `kind`
/// with land `shape` and `hole`.
fn tht(kind: PadKind, shape: PadShape, hole: Hole) -> Result<Project> {
    let mut project = fixture()?;
    project.rules = ProjectRules {
        profile: Some(fixtures::jlcpcb_example()?),
        ..ProjectRules::default()
    };
    for footprint in &mut project.library.footprints {
        for pad in &mut footprint.definition.pads {
            pad.kind = kind;
            pad.shape = shape.clone();
            pad.hole = Some(hole);
        }
    }
    Ok(project)
}

fn round(diameter: f64, plated: bool) -> Hole {
    Hole {
        shape: HoleShape::Round { diameter },
        plated,
        offset: [0.0, 0.0],
    }
}

fn found(project: &Project, code: &str) -> Vec<Finding> {
    checks(project)
        .into_iter()
        .filter(|f| f.code == code)
        .collect()
}

/// R1's pad numbered `number`.
fn r1_pad(project: &Project, number: &str) -> Result<PlacedPad> {
    placed_pads(project)
        .into_iter()
        .find(|p| {
            p.pad.number == number
                && project
                    .instance(p.instance)
                    .is_some_and(|i| i.designator == "R1")
        })
        .ok_or_else(|| Error::NotFound(format!("R1 pad {number}")))
}

fn add_track(project: &mut Project, net: Id, start: Point, end: Point) -> Result<Id> {
    let id = project.allocate()?;
    project.tracks.push(Track {
        id,
        net,
        start,
        end,
        width: 0.2,
        layer: CopperLayer::Top,
        locked: false,
    });
    Ok(id)
}

fn add_via(project: &mut Project, net: Id, position: Point) -> Result<Id> {
    let id = project.allocate()?;
    project.vias.push(Via {
        id,
        net,
        position,
        drill: 0.3,
        diameter: 0.6,
        span: ViaSpan::Through,
        tented: true,
        locked: false,
    });
    Ok(id)
}

/// The gap a message states ("… 0.3 mm …" after `before`), parsed.
fn gap_in(message: &str, before: &str) -> Result<f64> {
    message
        .split_once(before)
        .and_then(|(_, rest)| rest.split_once(" mm"))
        .and_then(|(gap, _)| gap.parse().ok())
        .ok_or_else(|| Error::Invalid(format!("no gap in {message}")))
}

/// A plated pad hole keeps the profile's PTH annular ring on its pad:
/// round holes centred and offset, and a slot in an oblong land. Unplated
/// holes have no ring rule.
#[test]
fn a_plated_pad_hole_keeps_the_pth_annular_ring() -> Result<()> {
    let circle = |diameter| PadShape::Circle { diameter };
    // 1.0 mm land, 0.6 mm hole: 0.2 mm of copper, under 0.25 mm.
    let project = tht(PadKind::PlatedThrough, circle(1.0), round(0.6, true))?;
    let findings = found(&project, "board.pth_annular_ring");
    assert_eq!(findings.len(), 4, "every resistor pad: {findings:#?}");
    assert!(
        findings.iter().any(|f| f.message
            == "R1 pad 1: annular ring 0.2 mm; needs at least 0.25 mm \
                (JLCPCB 2-layer 1 oz: min PTH annular ring)"),
        "{findings:#?}"
    );
    // 1.1 mm land: exactly 0.25 mm passes.
    let project = tht(PadKind::PlatedThrough, circle(1.1), round(0.6, true))?;
    assert!(found(&project, "board.pth_annular_ring").is_empty());
    // The same hole 0.1 mm off centre leaves 0.15 mm on one side.
    let mut offset = round(0.6, true);
    offset.offset = [0.1, 0.0];
    let project = tht(PadKind::PlatedThrough, circle(1.1), offset)?;
    let findings = found(&project, "board.pth_annular_ring");
    assert_eq!(findings.len(), 4, "{findings:#?}");
    assert!(findings[0].message.contains(": annular ring 0.15 mm;"));
    // A 0.6 × 1.6 mm slot along a 1.0 × 2.0 mm oblong land: 0.2 mm all
    // round; in a 1.1 × 2.1 mm land, 0.25 mm.
    let slot = Hole {
        shape: HoleShape::Slot {
            width: 0.6,
            length: 1.6,
            angle_deg: 0.0,
        },
        plated: true,
        offset: [0.0, 0.0],
    };
    let oblong = |x, y| PadShape::Oblong { size: [x, y] };
    let project = tht(PadKind::PlatedThrough, oblong(2.0, 1.0), slot)?;
    let findings = found(&project, "board.pth_annular_ring");
    assert_eq!(findings.len(), 4, "{findings:#?}");
    assert!(findings[0].message.contains(": annular ring 0.2 mm;"));
    let project = tht(PadKind::PlatedThrough, oblong(2.1, 1.1), slot)?;
    assert!(found(&project, "board.pth_annular_ring").is_empty());
    // Turned across the land, the slot breaks out of it: no ring at all.
    let mut across = slot;
    across.shape = HoleShape::Slot {
        width: 0.6,
        length: 1.6,
        angle_deg: 90.0,
    };
    let project = tht(PadKind::PlatedThrough, oblong(2.1, 1.1), across)?;
    let findings = found(&project, "board.pth_annular_ring");
    assert!(findings[0].message.contains(": annular ring 0 mm;"));
    // An unplated hole keeps no ring.
    let project = tht(PadKind::Mechanical, circle(1.0), round(0.6, false))?;
    assert!(found(&project, "board.pth_annular_ring").is_empty());
    Ok(())
}

/// A pad hole is measured against other nets' copper on every layer by
/// its kind's rule (JLCPCB: 0.35 mm for a plated hole, 0.2 mm for an
/// unplated one), even where the pad's own copper keeps the copper rule.
/// Copper the copper graph joins to the pad is exempt; copper of the same
/// net that does not reach the pad is not.
#[test]
fn a_pad_hole_keeps_its_clearance_to_other_copper() -> Result<()> {
    // 0.8 mm land, 0.6 mm plated hole. A 0.2 mm track whose centre runs
    // 0.7 mm above the hole centre: 0.2 mm from the land (over the 0.1 mm
    // track to pad rule), 0.3 mm from the hole (under 0.35 mm).
    let mut project = tht(
        PadKind::PlatedThrough,
        PadShape::Circle { diameter: 0.8 },
        round(0.6, true),
    )?;
    let pad = r1_pad(&project, "1")?;
    let [x, y] = pad.center();
    let own = pad
        .net
        .ok_or_else(|| Error::NotFound("R1 pad 1's net".into()))?;
    let other = project
        .nets
        .iter()
        .map(|n| n.id)
        .find(|n| *n != own)
        .ok_or_else(|| Error::NotFound("another net".into()))?;
    let near = add_track(&mut project, other, [x - 0.5, y + 0.7], [x + 0.5, y + 0.7])?;
    assert!(found(&project, "board.clearance").is_empty());
    let findings = found(&project, "board.pad_hole_clearance");
    assert_eq!(findings.len(), 1, "{findings:#?}");
    let message = &findings[0].message;
    let name = project.net_name(other).to_owned();
    assert!(message.starts_with("R1 pad 1: hole is "), "{message}");
    assert!(message.contains(&format!(" mm from a track on {name}; ")));
    assert!(
        message.ends_with("needs 0.35 mm (JLCPCB 2-layer 1 oz: track to plated hole clearance)"),
        "{message}"
    );
    let gap = gap_in(message, "hole is ")?;
    assert!(
        (gap - 0.3).abs() <= crate::library::PAD_ARC_TOLERANCE_MM,
        "{message}"
    );
    // A track of the pad's own net routed into the pad crosses the hole
    // but is joined to the pad: exempt.
    project.tracks.retain(|t| t.id != near);
    let joined = add_track(&mut project, own, [x, y], [x - 2.0, y])?;
    let findings = found(&project, "board.pad_hole_clearance");
    assert!(findings.is_empty(), "{findings:#?}");
    // The same net 0.2 mm off the land joins nothing there: measured.
    project.tracks.retain(|t| t.id != joined);
    add_track(&mut project, own, [x - 0.5, y + 0.7], [x + 0.5, y + 0.7])?;
    assert_eq!(found(&project, "board.pad_hole_clearance").len(), 1);

    // An unplated 0.6 mm hole keeps 0.2 mm ("NPTH to Track"): the track
    // 0.3 mm away passes, one 0.15 mm away does not.
    let mut project = tht(
        PadKind::NonPlatedHole,
        PadShape::Circle { diameter: 0.6 },
        round(0.6, false),
    )?;
    add_track(&mut project, other, [x - 0.5, y + 0.7], [x + 0.5, y + 0.7])?;
    let findings = found(&project, "board.pad_hole_clearance");
    assert!(findings.is_empty(), "{findings:#?}");
    add_track(
        &mut project,
        other,
        [x - 0.5, y - 0.55],
        [x + 0.5, y - 0.55],
    )?;
    let findings = found(&project, "board.pad_hole_clearance");
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert!(
        findings[0]
            .message
            .ends_with("needs 0.2 mm (JLCPCB 2-layer 1 oz: track to unplated hole clearance)"),
        "{}",
        findings[0].message
    );
    Ok(())
}

/// A pad hole keeps the hole-kind spacing to other holes: a via drill
/// 0.3 mm from a plated pad hole is under JLCPCB's 0.45 mm pad hole
/// spacing, and so is the same via by an unplated pad hole (the example
/// reads the pad hole row as covering unplated pad holes); an override
/// of the via-to-unplated pair moves that verdict.
#[test]
fn a_pad_hole_keeps_its_spacing_to_other_holes() -> Result<()> {
    let mut project = tht(
        PadKind::PlatedThrough,
        PadShape::Circle { diameter: 1.2 },
        round(0.6, true),
    )?;
    let pad = r1_pad(&project, "1")?;
    let [x, y] = pad.center();
    let net = pad
        .net
        .ok_or_else(|| Error::NotFound("R1 pad 1's net".into()))?;
    // Hole edges 0.3 mm apart: 0.3 + 0.3 + 0.15 between centres.
    add_via(&mut project, net, [x, y - 0.75])?;
    let findings = found(&project, "board.pad_hole_spacing");
    assert_eq!(findings.len(), 1, "{findings:#?}");
    let message = &findings[0].message;
    let name = project.net_name(net).to_owned();
    assert!(
        message.starts_with(&format!("R1 pad 1 and a via on {name}: holes are ")),
        "{message}"
    );
    assert!(
        message.ends_with("needs 0.45 mm (JLCPCB 2-layer 1 oz: via hole to plated hole clearance)"),
        "{message}"
    );
    let gap = gap_in(message, "holes are ")?;
    assert!(
        (gap - 0.3).abs() <= 2.0 * crate::library::PAD_ARC_TOLERANCE_MM,
        "{message}"
    );

    let mut project = tht(
        PadKind::NonPlatedHole,
        PadShape::Circle { diameter: 1.2 },
        round(0.6, false),
    )?;
    add_via(&mut project, net, [x, y - 0.75])?;
    let findings = found(&project, "board.pad_hole_spacing");
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert!(
        findings[0]
            .message
            .ends_with("needs 0.45 mm (JLCPCB 2-layer 1 oz: via hole to unplated hole clearance)"),
        "{}",
        findings[0].message
    );
    project.rules.overrides.push(RuleOverride {
        key: RuleKey::Clearance {
            a: CopperObject::ViaHole,
            b: CopperObject::NpthHole,
        },
        value: RuleValue::Length(0.25),
    });
    assert!(found(&project, "board.pad_hole_spacing").is_empty());
    Ok(())
}

/// An unplated hole in a pad with copper keeps the profile's NPTH pad
/// annular ring (JLCPCB: 0.45 mm, so 0.2 mm can be cleared for the
/// sealing film): a 1.4 mm land round a 0.6 mm hole leaves 0.4 mm; a
/// 1.5 mm land passes. A bare unplated hole has no copper and no ring.
#[test]
fn an_unplated_hole_in_copper_keeps_the_npth_pad_annular_ring() -> Result<()> {
    let circle = |diameter| PadShape::Circle { diameter };
    let project = tht(PadKind::Mechanical, circle(1.4), round(0.6, false))?;
    let findings = found(&project, "board.npth_annular_ring");
    assert_eq!(findings.len(), 4, "every resistor pad: {findings:#?}");
    assert!(
        findings.iter().any(|f| f.message
            == "R1 pad 1: annular ring 0.4 mm round the unplated hole; needs at least 0.45 mm \
                (JLCPCB 2-layer 1 oz: min NPTH pad annular ring)"),
        "{findings:#?}"
    );
    assert!(found(&project, "board.pth_annular_ring").is_empty());
    let project = tht(PadKind::Mechanical, circle(1.5), round(0.6, false))?;
    assert!(found(&project, "board.npth_annular_ring").is_empty());
    let project = tht(PadKind::NonPlatedHole, circle(0.6), round(0.6, false))?;
    assert!(found(&project, "board.npth_annular_ring").is_empty());
    Ok(())
}
