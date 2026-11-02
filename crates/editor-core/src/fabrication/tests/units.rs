//! The reader and rasteriser on hand-written files whose areas are derived
//! by hand. Pixel-centre sampling misjudges only pixels the boundary
//! crosses, so an area is within (perimeter × pixel) of the exact value,
//! and in practice far closer; the tolerances below are half that bound.
use super::Fallible;
use super::excellon_reader::{self, Hit};
use super::expected;
use super::raster::{self, Bitmap};
use super::reader::{self, Kind, Template};
use crate::Engine;
use crate::model::{CopperLayer, Layer};
use std::f64::consts::PI;

const HEAD: &str = "%FSLAX46Y46*%\n%MOMM*%\n";

/// The dark area of a Gerber file over a window, at `pixel`.
fn area(source: &str, min: [f64; 2], max: [f64; 2], pixel: f64) -> Fallible<f64> {
    let file = reader::parse(source)?;
    let mut bitmap = Bitmap::new(min, max, pixel)?;
    raster::render_gerber(&file, &mut bitmap)?;
    Ok(bitmap.area_mm2())
}

fn close(got: f64, want: f64, perimeter: f64, pixel: f64) {
    let tol = 0.5 * perimeter * pixel;
    assert!(
        (got - want).abs() <= tol,
        "area {got} mm², expected {want} ± {tol}"
    );
}

#[test]
fn a_circle_flash_has_its_disc_area() -> Fallible<()> {
    let src = format!("{HEAD}%ADD10C,1*%\nD10*\nX5000000Y5000000D03*\nM02*\n");
    let a = area(&src, [4.0, 4.0], [6.0, 6.0], 0.002)?;
    close(a, PI / 4.0, PI, 0.002);
    Ok(())
}

#[test]
fn a_draw_with_a_round_aperture_is_a_stadium() -> Fallible<()> {
    // 10 mm × 0.2 mm plus two half discs of radius 0.1.
    let src = format!("{HEAD}%ADD10C,0.2*%\nD10*\nG01*\nX0Y0D02*\nX10000000Y0D01*\nM02*\n");
    let a = area(&src, [-1.0, -1.0], [11.0, 1.0], 0.002)?;
    close(a, 2.0 + PI * 0.01, 20.0 + 0.2 * PI, 0.002);
    // Diagonal: the same area.
    let src = format!("{HEAD}%ADD10C,0.2*%\nD10*\nG01*\nX0Y0D02*\nX6000000Y8000000D01*\nM02*\n");
    let a = area(&src, [-1.0, -1.0], [7.0, 9.0], 0.002)?;
    close(a, 2.0 + PI * 0.01, 20.0 + 0.2 * PI, 0.002);
    Ok(())
}

#[test]
fn standard_apertures_with_holes_flash_their_shapes() -> Fallible<()> {
    let win = ([-3.0, -3.0], [3.0, 3.0]);
    // Rectangle 2 × 1 with a 0.5 mm hole.
    let src = format!("{HEAD}%ADD10R,2X1X0.5*%\nD10*\nX0Y0D03*\nM02*\n");
    close(
        area(&src, win.0, win.1, 0.002)?,
        2.0 - PI * 0.0625,
        6.0 + 0.5 * PI,
        0.002,
    );
    // Obround 3 × 1: a 2 × 1 middle and a 1 mm disc.
    let src = format!("{HEAD}%ADD10O,3X1*%\nD10*\nX0Y0D03*\nM02*\n");
    close(
        area(&src, win.0, win.1, 0.002)?,
        2.0 + PI * 0.25,
        4.0 + PI,
        0.002,
    );
    // A square as a regular 4-gon of 2 mm circumscribed diameter: 2 r².
    let src = format!("{HEAD}%ADD10P,2X4*%\nD10*\nX0Y0D03*\nM02*\n");
    close(
        area(&src, win.0, win.1, 0.002)?,
        2.0,
        4.0 * 2f64.sqrt(),
        0.002,
    );
    Ok(())
}

#[test]
fn a_cut_in_region_keeps_its_hole() -> Fallible<()> {
    // A 10 × 10 square with a 4 × 4 hole joined by a cut-in: 100 − 16.
    let src = format!(
        "{HEAD}G36*\nX0Y0D02*\nG01*\nX10000000Y0D01*\nX10000000Y3000000D01*\nX7000000Y3000000D01*\n\
         X7000000Y7000000D01*\nX3000000Y7000000D01*\nX3000000Y3000000D01*\nX7000000Y3000000D01*\n\
         X10000000Y3000000D01*\nX10000000Y10000000D01*\nX0Y10000000D01*\nX0Y0D01*\nG37*\nM02*\n"
    );
    let file = reader::parse(&src)?;
    assert_eq!(file.regions().count(), 1);
    let mut bitmap = Bitmap::new([-1.0, -1.0], [11.0, 11.0], 0.01)?;
    raster::render_gerber(&file, &mut bitmap)?;
    close(bitmap.area_mm2(), 84.0, 56.0, 0.01);
    assert!(!bitmap.at([5.0, 5.0]) && bitmap.at([1.0, 5.0]));
    Ok(())
}

#[test]
fn clear_polarity_erases_and_regions_take_several_contours() -> Fallible<()> {
    // Two contours in one region (two 2 × 2 squares), then a clear 1 × 1.
    let src = format!(
        "{HEAD}G36*\nG01*\nX0Y0D02*\nX2000000Y0D01*\nX2000000Y2000000D01*\nX0Y2000000D01*\nX0Y0D01*\n\
         X5000000Y0D02*\nX7000000Y0D01*\nX7000000Y2000000D01*\nX5000000Y2000000D01*\nX5000000Y0D01*\nG37*\n\
         %LPC*%\nG36*\nX500000Y500000D02*\nX1500000Y500000D01*\nX1500000Y1500000D01*\nX500000Y1500000D01*\n\
         X500000Y500000D01*\nG37*\nM02*\n"
    );
    let file = reader::parse(&src)?;
    assert_eq!(file.regions().count(), 3);
    assert!(!file.objects[2].dark);
    let mut bitmap = Bitmap::new([-1.0, -1.0], [8.0, 3.0], 0.01)?;
    raster::render_gerber(&file, &mut bitmap)?;
    close(bitmap.area_mm2(), 7.0, 20.0, 0.01);
    Ok(())
}

#[test]
fn macros_evaluate_variables_rotation_and_exposure() -> Fallible<()> {
    // Centre line 4 × 1 turned 30° about the origin: area 4.
    let src = format!(
        "{HEAD}%AMROT*0 a rotated bar*21,1,$1,$2,0,0,$3*%\n%ADD10ROT,4X1X30*%\nD10*\nX0Y0D03*\nM02*\n"
    );
    let file = reader::parse(&src)?;
    let mut bitmap = Bitmap::new([-3.0, -3.0], [3.0, 3.0], 0.002)?;
    raster::render_gerber(&file, &mut bitmap)?;
    close(bitmap.area_mm2(), 4.0, 10.0, 0.002);
    let along = [
        1.8 * 30f64.to_radians().cos(),
        1.8 * 30f64.to_radians().sin(),
    ];
    assert!(bitmap.at(along) && !bitmap.at([1.8, 0.0]));
    // A ring: a 2 mm disc, then a 1 mm disc of exposure off; and a
    // variable computed from the parameter ($2 = $1 x 2).
    let src = format!(
        "{HEAD}%AMRING*$2=$1x2*1,1,$2,0,0*1,0,$1,0,0*%\n%ADD10RING,1*%\nD10*\nX0Y0D03*\nM02*\n"
    );
    close(
        area(&src, [-2.0, -2.0], [2.0, 2.0], 0.002)?,
        PI * (1.0 - 0.25),
        3.0 * PI,
        0.002,
    );
    // Outline (primitive 4) and vector line (20) with arithmetic.
    let src = format!(
        "{HEAD}%AMSHAPES*4,1,3,0,0,2,0,2,2,0,0,0*20,1,0.5,(0-1),(-2),(3-1)x1,-2,0*%\n\
         %ADD10SHAPES*%\nD10*\nX0Y0D03*\nM02*\n"
    );
    // A right triangle of area 2, and a 3 × 0.5 bar along y = −2.
    close(
        area(&src, [-3.0, -3.0], [3.0, 3.0], 0.002)?,
        2.0 + 1.5,
        7.0 + 7.0,
        0.002,
    );
    // Polygon primitive 5: a hexagon of circumscribed diameter 2.
    let src = format!("{HEAD}%AMHEX*5,1,6,0,0,2,0*%\n%ADD10HEX*%\nD10*\nX0Y0D03*\nM02*\n");
    close(
        area(&src, [-2.0, -2.0], [2.0, 2.0], 0.002)?,
        1.5 * 3f64.sqrt(),
        6.0,
        0.002,
    );
    Ok(())
}

#[test]
fn arcs_are_stroked_in_both_directions_and_full_circles() -> Fallible<()> {
    // A quarter circle of radius 10, counter-clockwise, 0.2 mm wide: the
    // swept band plus two half caps.
    let src = format!(
        "{HEAD}%ADD10C,0.2*%\nD10*\nG75*\nX10000000Y0D02*\nG03*\nX0Y10000000I-10000000J0D01*\nM02*\n"
    );
    let a = area(&src, [-1.0, -1.0], [11.0, 11.0], 0.002)?;
    close(
        a,
        PI / 2.0 * 10.0 * 0.2 + PI * 0.01,
        2.0 * PI * 10.0 / 2.0,
        0.002,
    );
    // The same quarter clockwise from (0, 10) to (10, 0).
    let src = format!(
        "{HEAD}%ADD10C,0.2*%\nD10*\nG75*\nX0Y10000000D02*\nG02*\nX10000000Y0I0J-10000000D01*\nM02*\n"
    );
    let b = area(&src, [-1.0, -1.0], [11.0, 11.0], 0.002)?;
    assert!((a - b).abs() < 1e-3, "{a} {b}");
    // A full circle (end = start): a ring 2π · 10 · 0.2.
    let src = format!(
        "{HEAD}%ADD10C,0.2*%\nD10*\nG75*\nX10000000Y0D02*\nG03*\nX10000000Y0I-10000000J0D01*\nM02*\n"
    );
    let a = area(&src, [-11.0, -11.0], [11.0, 11.0], 0.004)?;
    close(a, 4.0 * PI, 4.0 * PI * 10.0, 0.004);
    // A region bounded by an arc: a half disc of radius 5.
    let src = format!(
        "{HEAD}G75*\nG36*\nX-5000000Y0D02*\nG01*\nX5000000Y0D01*\nG03*\nX-5000000Y0I-5000000J0D01*\nG37*\nM02*\n"
    );
    let a = area(&src, [-6.0, -1.0], [6.0, 6.0], 0.004)?;
    close(a, PI * 12.5, 10.0 + 5.0 * PI, 0.004);
    Ok(())
}

#[test]
fn attributes_are_recorded_per_file_aperture_and_object() -> Fallible<()> {
    let src = format!(
        "{HEAD}%TF.FileFunction,Copper,L1,Top*%\n%TF.FilePolarity,Positive*%\n\
         %TA.AperFunction,SMDPad,CuDef*%\n%ADD10C,1*%\n%TD.AperFunction*%\n%ADD11C,0.2*%\n\
         %TO.N,GND*%\n%TO.P,U1,3*%\n%TO.C,U1*%\nD10*\nX0Y0D03*\n%TD*%\n%TO.N,A\\u002CB*%\nD11*\nG01*\nX0Y0D02*\nX1000000Y0D01*\nM02*\n"
    );
    let file = reader::parse(&src)?;
    assert_eq!(
        file.file_attribute(".FileFunction"),
        Some(&["Copper".to_string(), "L1".into(), "Top".into()][..])
    );
    let pad = &file.objects[0];
    assert_eq!(
        pad.aperture_attributes.get(".AperFunction"),
        Some(&vec!["SMDPad".to_string(), "CuDef".into()])
    );
    assert_eq!(
        pad.object_attributes.get(".P"),
        Some(&vec!["U1".to_string(), "3".into()])
    );
    assert_eq!(
        pad.object_attributes.get(".N"),
        Some(&vec!["GND".to_string()])
    );
    let track = &file.objects[1];
    assert!(track.aperture_attributes.is_empty());
    // TD cleared .P and .C; the new .N is unescaped.
    assert_eq!(track.object_attributes.len(), 1);
    assert_eq!(
        track.object_attributes.get(".N"),
        Some(&vec!["A,B".to_string()])
    );
    Ok(())
}

#[test]
fn inch_files_are_read_in_millimetres() -> Fallible<()> {
    let src = "%FSLAX25Y25*%\n%MOIN*%\n%ADD10C,0.1*%\nD10*\nX100000Y0D03*\nM02*\n";
    let file = reader::parse(src)?;
    let Template::Circle { diameter, .. } = file.apertures[&10].template else {
        return Err("not a circle".into());
    };
    assert!((diameter - 2.54).abs() < 1e-12);
    let Kind::Flash { at } = file.objects[0].kind else {
        return Err("not a flash".into());
    };
    assert!((at[0] - 25.4).abs() < 1e-9);
    Ok(())
}

#[test]
fn malformed_and_unsupported_files_are_refused() {
    for bad in [
        format!("{HEAD}%ADD10C,1*%\nD10*\nX0Y0D03*\n"),
        format!("{HEAD}%LR45*%\nM02*\n"),
        format!("{HEAD}G36*\nX0Y0D02*\nG01*\nX1000000Y0D01*\nX1000000Y1000000D01*\nG37*\nM02*\n"),
        format!("{HEAD}D10*\nM02*\n"),
        format!("{HEAD}%ADD10C,1*%\nD10*\nG03*\nX1000000Y0I1D01*\nM02*\n"),
        "%MOMM*%\n%ADD10C,1*%\nD10*\nX0Y0D03*\nM02*\n".to_string(),
        format!("{HEAD}%ADD10C,1*%\nM02*\nD10*\n"),
    ] {
        assert!(reader::parse(&bad).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn excellon_hits_and_slots_are_read_and_rasterised() -> Fallible<()> {
    let src = "M48\n; #@! TF.FileFunction,Plated,1,4,PTH\nMETRIC,TZ\nFMAT,2\nT1C0.300\nT2C1.000\n%\nG90\nG05\n\
               T1\nX1.0Y1.0\nX3.0Y1.0\nY3.0\nT2\nX10.0Y0.0G85X15.0Y0.0\nM30\n";
    let file = excellon_reader::parse(src)?;
    assert!(file.metric);
    assert_eq!(
        file.attribute(".FileFunction"),
        Some(vec!["Plated".into(), "1".into(), "4".into(), "PTH".into()])
    );
    assert_eq!(file.hits.len(), 4);
    // Modal Y: the third hit keeps X = 3.
    assert_eq!(
        file.hits[2],
        Hit::Round {
            tool: 1,
            at: [3.0, 3.0]
        }
    );
    let mut bitmap = Bitmap::new([-1.0, -2.0], [17.0, 5.0], 0.002)?;
    raster::render_drill(&file, &mut bitmap)?;
    let want = 3.0 * PI * 0.0225 + 5.0 + PI * 0.25;
    close(bitmap.area_mm2(), want, 3.0 * PI * 0.3 + 10.0 + PI, 0.002);
    // Integer coordinates carry 3 implied decimals in mm; routed slots.
    let src = "M48\nMETRIC\nT1C0.5\n%\nT1\nX1000Y2000\nG00X0Y0\nM15\nG01X5000Y0\nM16\nM30\n";
    let file = excellon_reader::parse(src)?;
    assert_eq!(
        file.hits[0],
        Hit::Round {
            tool: 1,
            at: [1.0, 2.0]
        }
    );
    assert_eq!(
        file.hits[1],
        Hit::Slot {
            tool: 1,
            from: [0.0, 0.0],
            to: [5.0, 0.0]
        }
    );
    assert!(excellon_reader::parse("M48\nMETRIC\nT1C0.5\n%\nT2\nX0Y0\nM30\n").is_err());
    assert!(excellon_reader::parse("M48\nMETRIC\nT1C0.5\n%\nT1\nX0Y0\n").is_err());
    Ok(())
}

#[test]
fn comparison_counts_the_area_in_exactly_one() -> Fallible<()> {
    let one = format!("{HEAD}%ADD10R,2X2*%\nD10*\nX0Y0D03*\nM02*\n");
    let two = format!("{HEAD}%ADD10R,2X2*%\nD10*\nX1000000Y0D03*\nM02*\n");
    let mut a = Bitmap::new([-3.0, -3.0], [3.0, 3.0], 0.01)?;
    let mut b = a.blank();
    raster::render_gerber(&reader::parse(&one)?, &mut a)?;
    raster::render_gerber(&reader::parse(&two)?, &mut b)?;
    let c = raster::compare(&a, &b)?;
    // Squares overlapping by half: 2 + 2 differ.
    assert!((c.xor_area - 4.0).abs() < 0.05, "{c:?}");
    assert!((c.xor_share - 1.0).abs() < 0.02, "{c:?}");
    assert_eq!(raster::compare(&a, &a)?.xor_area, 0.0);
    Ok(())
}

/// The expected rasteriser on the showcase: the outline is the 44 × 28 mm
/// board; every via is copper on every layer and a plated drill; tented
/// vias open no mask.
#[test]
fn the_expected_layers_of_the_showcase_hold_its_known_features() -> Fallible<()> {
    let engine = Engine::showcase()?;
    let p = engine.project();
    let pixel = 0.02;
    let mut board = expected::board_bitmap(p, 1.0, pixel)?;
    expected::outline(p, &mut board)?;
    let [w, h] = p.board_size;
    close(board.area_mm2(), w * h, 2.0 * (w + h), pixel);
    let mut drills = board.blank();
    expected::drills(p, true, &mut drills)?;
    let mut mask = board.blank();
    expected::mask(p, Layer::Top, &mut mask)?;
    for layer in p.stackup.layers() {
        let mut copper = board.blank();
        expected::copper(p, layer, &mut copper)?;
        assert!(copper.area_mm2() > 0.0, "{layer:?}");
        for via in &p.vias {
            assert!(copper.at(via.position), "{layer:?}");
            assert!(drills.at(via.position));
            assert_eq!(mask.at(via.position), !via.tented);
        }
    }
    let mut top = board.blank();
    expected::copper(p, CopperLayer::Top, &mut top)?;
    let mut paste = board.blank();
    expected::paste(p, Layer::Top, &mut paste)?;
    // Paste lies on top copper (no reduction by default: equal pads).
    let mut outside = paste.clone();
    outside.subtract(&top)?;
    assert!(outside.area_mm2() < 1e-9, "{}", outside.area_mm2());
    assert!(paste.area_mm2() > 0.0);
    Ok(())
}

/// Scale: 10,000 flashes and as many drill hits over a 200 × 200 mm window
/// at 0.02 mm (10⁸ pixels). The time is printed.
#[test]
fn ten_thousand_flashes_rasterise_quickly() -> Fallible<()> {
    let mut src = format!("{HEAD}%ADD10C,0.6*%\nD10*\n");
    let mut drill = String::from("M48\nMETRIC\nT1C0.3\n%\nT1\n");
    for i in 0..100 {
        for j in 0..100 {
            let (x, y) = (1.0 + 1.98 * f64::from(i), 1.0 + 1.98 * f64::from(j));
            src.push_str(&format!(
                "X{}Y{}D03*\n",
                (x * 1e6).round() as i64,
                (y * 1e6).round() as i64
            ));
            drill.push_str(&format!("X{x:.3}Y{y:.3}\n"));
        }
    }
    src.push_str("M02*\n");
    drill.push_str("M30\n");
    let start = std::time::Instant::now();
    let file = reader::parse(&src)?;
    let mut bitmap = Bitmap::new([0.0, 0.0], [200.0, 200.0], 0.02)?;
    raster::render_gerber(&file, &mut bitmap)?;
    let mut holes = bitmap.blank();
    raster::render_drill(&excellon_reader::parse(&drill)?, &mut holes)?;
    let elapsed = start.elapsed();
    eprintln!("10,000 flashes and hits at 0.02 mm over 200 × 200 mm: {elapsed:?}");
    close(bitmap.area_mm2(), 1e4 * PI * 0.09, 1e4 * PI * 0.6, 0.02);
    close(holes.area_mm2(), 1e4 * PI * 0.0225, 1e4 * PI * 0.3, 0.02);
    assert!(elapsed.as_secs_f64() < 30.0, "{elapsed:?}");
    Ok(())
}
