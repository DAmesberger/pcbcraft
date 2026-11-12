//! The polygon rendering and the exact comparison on hand-written files
//! whose areas and differences are derived by hand.
use super::*;
use crate::compare::{compare, worst_width_nm};
use crate::drills::{DrillHit, Feature, pair};
use crate::gerber;
use std::f64::consts::PI;

const HEAD: &str = "%FSLAX46Y46*%\n%MOMM*%\n";

fn image(body: &str) -> Fallible<Shapes> {
    render(&gerber::parse(&format!("{HEAD}{body}M02*\n"))?, |_| true)
}

/// Relative closeness of two areas.
fn near(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs().max(1e-12)
}

#[test]
fn flashes_strokes_and_regions_have_their_exact_areas() -> Fallible<()> {
    // A 1 x 2 rectangle: exact.
    let rect = image("%ADD10R,1.000000X2.000000*%\nD10*\nX0Y0D03*\n")?;
    assert_eq!(area_mm2(&rect), 2.0);
    // A 1 mm disc: chords within 0.25 µm, so the area is within the ring
    // of that width.
    let disc = image("%ADD10C,1.000000*%\nD10*\nX0Y0D03*\n")?;
    let exact = PI * 0.25;
    assert!(area_mm2(&disc) <= exact && exact - area_mm2(&disc) < PI * 1.0 * 2.5e-4);
    // A stroke of width 0.2 and length 3: a 3 x 0.2 bar and a disc.
    let stroke = image("%ADD10C,0.200000*%\nD10*\nX0Y0D02*\nG01*\nX3000000Y0D01*\n")?;
    // The caps' chords cut at most 0.25 µm over their 0.63 mm of arc.
    let exact = 0.6 + PI * 0.01;
    assert!(area_mm2(&stroke) <= exact && exact - area_mm2(&stroke) < 0.63 * 2.5e-4);
    // A region closed by a half-circle arc: a half disc of radius 1, its
    // chords within 0.25 µm over π mm of arc.
    let region = image(
        "G75*\nG36*\nX-1000000Y0D02*\nG01*\nX1000000Y0D01*\nG03*\nX-1000000Y0I-1000000J0D01*\nG37*\n",
    )?;
    assert!(near(area_mm2(&region), PI / 2.0, PI * 2.5e-4 / (PI / 2.0)));
    // Clear polarity removes: a 2 x 2 square less a 1 x 1 square.
    let cleared = image(
        "%ADD10R,2.000000X2.000000*%\n%ADD11R,1.000000X1.000000*%\nD10*\nX0Y0D03*\n%LPC*%\nD11*\nX0Y0D03*\n",
    )?;
    assert_eq!(area_mm2(&cleared), 3.0);
    Ok(())
}

#[test]
fn macros_unite_their_primitives_and_erase_with_exposure_off() -> Fallible<()> {
    // Two overlapping 1 x 1 centre lines (1 x 1.5 together) less a
    // 0.5 x 0.5 square cut with exposure off.
    let body = "%AMTWO*\n21,1,1,1,0,0,0*\n21,1,1,1,0,0.5,0*\n21,0,0.5,0.5,0,0.5,0*%\n\
                %ADD10TWO*%\nD10*\nX0Y0D03*\n";
    assert_eq!(area_mm2(&image(body)?), 1.5 - 0.25);
    // An outline primitive turned 90°: the same area.
    let body = "%AMTRI*\n4,1,3,0,0,1,0,0,1,0,0,90*%\n%ADD10TRI*%\nD10*\nX0Y0D03*\n";
    assert_eq!(area_mm2(&image(body)?), 0.5);
    Ok(())
}

#[test]
fn a_file_against_itself_differs_nowhere() -> Fallible<()> {
    let a = image(
        "%ADD10C,0.600000*%\n%ADD11O,1.000000X0.500000*%\nD10*\nX0Y0D03*\nD11*\nX2000000Y0D03*\n",
    )?;
    let d = compare(&a, &a, 1000)?;
    assert_eq!(d.difference_mm2, 0.0);
    assert_eq!(d.worst_width_um, 0.0);
    assert!(d.passes() && d.pieces.is_empty());
    Ok(())
}

#[test]
fn a_half_micron_shift_is_measured_as_half_a_micron() -> Fallible<()> {
    let a = image(
        "%ADD10R,1.000000X2.000000*%\n%ADD11C,0.800000*%\nD10*\nX0Y0D03*\nD11*\nX3000000Y1000000D03*\n",
    )?;
    let b = translated(&a, [0.0005, 0.0])?;
    let d = compare(&a, &b, 1000)?;
    assert!(
        (d.worst_width_um - 0.5).abs() <= 0.1,
        "measured {} µm",
        d.worst_width_um
    );
    assert!(d.passes());
    // Against a tolerance of 0.25 µm it fails, with its pieces listed.
    let strict = compare(&a, &b, 250)?;
    assert!(!strict.passes() && !strict.pieces.is_empty());
    Ok(())
}

#[test]
fn the_worst_width_is_the_widest_piece() -> Fallible<()> {
    // A 1 x 1 square against one 6 µm larger on each side: a ring 6 µm
    // wide, whose widest disc sits in a corner, touching both outer sides
    // and the inner corner: diameter 2 · 6√2 / (1 + √2).
    let a = image("%ADD10R,1.000000X1.000000*%\nD10*\nX0Y0D03*\n")?;
    let b = image("%ADD10R,1.012000X1.012000*%\nD10*\nX0Y0D03*\n")?;
    let d = compare(&a, &b, 1000)?;
    let corner = 2.0 * 6.0 * 2f64.sqrt() / (1.0 + 2f64.sqrt());
    assert!(
        (d.worst_width_um - corner).abs() <= 0.01,
        "{}",
        d.worst_width_um
    );
    assert!(!d.passes());
    let piece = d.pieces.first().ok_or("no piece listed")?;
    assert!(piece.min[0] < -0.5 && piece.max[0] > 0.5);
    // A missing pad: the piece is the pad itself.
    let c = image("%ADD10R,1.000000X1.000000*%\nD10*\nX0Y0D03*\nX5000000Y0D03*\n")?;
    let d = compare(&a, &c, 1000)?;
    assert_eq!(d.difference_mm2, 1.0);
    assert!((d.worst_width_um - 1000.0).abs() <= 0.01);
    assert_eq!(d.pieces.len(), 1);
    assert_eq!(worst_width_nm(&Vec::new())?, 0);
    Ok(())
}

#[test]
fn drill_hits_pair_one_to_one_within_the_tolerance() {
    let round = |x: f64, d: f64| DrillHit {
        feature: Feature::Round { at: [x, 1.0] },
        diameter: d,
    };
    let slot = |x: f64| DrillHit {
        feature: Feature::Slot {
            from: [x, 0.0],
            to: [x + 1.0, 0.0],
        },
        diameter: 0.6,
    };
    let a = [round(0.0, 0.3), round(1.0, 0.3), slot(5.0)];
    // B: the first hit 0.4 µm off, the slot drawn the other way round.
    let b = [
        DrillHit {
            feature: Feature::Slot {
                from: [6.0, 0.0],
                to: [5.0, 0.0],
            },
            diameter: 0.6,
        },
        round(1.0, 0.3),
        round(0.0004, 0.3),
    ];
    let d = pair(&a, &b, 0.001);
    assert!(d.passes(), "{d:?}");
    assert_eq!(d.matched, 3);
    assert!((d.worst_position_um - 0.4).abs() < 1e-6);
    // A wrong diameter and a missing hit are left unmatched.
    let c = [round(0.0, 0.35), round(1.0, 0.3)];
    let d = pair(&a, &c, 0.001);
    assert!(!d.passes());
    assert_eq!((d.unmatched_a.len(), d.unmatched_b.len()), (2, 1));
}
