use super::*;
use crate::Engine;

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn showcase() -> Fallible<Project> {
    Ok(Engine::showcase()?.project().clone())
}

fn row<'a>(rows: &'a [PlacementRow], designator: &str) -> Fallible<&'a PlacementRow> {
    rows.iter()
        .find(|r| r.designator == designator)
        .ok_or_else(|| format!("no row {designator}").into())
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn every_placed_part_has_one_row_at_its_placement_origin() -> Fallible<()> {
    let p = showcase()?;
    let rows = rows(&p, &PlacementConvention::default())?;
    // The showcase places nine parts (J1, U1, U2, C1–C3, R1–R3), each with
    // a footprint.
    assert_eq!(rows.len(), 9);
    assert_eq!(rows.len(), p.placements.len());
    for placement in &p.placements {
        let r = rows
            .iter()
            .find(|r| r.instance == placement.instance)
            .ok_or("missing row")?;
        assert!(close(r.x, placement.position[0]) && close(r.y, placement.position[1]));
        assert_eq!(r.side, placement.layer);
        assert!(!r.package.is_empty(), "{r:?}");
        // No value is set and no Part chosen: every part reads nothing,
        // never its component name (run 3, finding 25).
        assert!(r.value.is_empty(), "{r:?}");
    }
    let order: Vec<&str> = rows.iter().map(|r| r.designator.as_str()).collect();
    assert_eq!(
        order,
        ["C1", "C2", "C3", "J1", "R1", "R2", "R3", "U1", "U2"]
    );
    let u1 = row(&rows, "U1")?;
    assert!(close(u1.x, 20.0) && close(u1.y, 15.0) && close(u1.rotation_deg, 90.0));
    Ok(())
}

#[test]
fn bottom_rotation_follows_the_convention() -> Fallible<()> {
    let mut p = showcase()?;
    let r3 = p.instance_by_designator("R3").map(|i| i.id).ok_or("R3")?;
    for placement in &mut p.placements {
        if placement.instance == r3 {
            placement.rotation_deg = 30.0;
        }
    }
    let top_view = rows(&p, &PlacementConvention::default())?;
    let r = row(&top_view, "R3")?;
    assert_eq!(r.side, Layer::Bottom);
    assert!(close(r.rotation_deg, 30.0) && close(r.x, 24.0) && close(r.y, 24.0));
    // Seen from below (board flipped about y): 180 − 30 = 150, x negated.
    let bottom_view = PlacementConvention {
        bottom: BottomRotation::ViewedFromBottom,
        mirror_bottom_x: true,
        ..Default::default()
    };
    let rows_b = rows(&p, &bottom_view)?;
    let r = row(&rows_b, "R3")?;
    assert!(close(r.rotation_deg, 150.0), "{r:?}");
    assert!(close(r.x, -24.0) && close(r.y, 24.0));
    // Top parts are untouched by the bottom rules.
    let u2 = row(&rows_b, "U2")?;
    assert!(close(u2.x, 36.0) && close(u2.rotation_deg, 0.0));
    Ok(())
}

#[test]
fn origins_and_package_offsets_shift_rows() -> Fallible<()> {
    let p = showcase()?;
    let min = p.board().bounds().min;
    let base = rows(&p, &PlacementConvention::default())?;
    let shifted = rows(
        &p,
        &PlacementConvention {
            origin: PlacementOrigin::Point { at: [5.0, 2.0] },
            ..Default::default()
        },
    )?;
    let lower_left = rows(
        &p,
        &PlacementConvention {
            origin: PlacementOrigin::OutlineLowerLeft,
            ..Default::default()
        },
    )?;
    for ((a, b), c) in base.iter().zip(&shifted).zip(&lower_left) {
        assert!(close(b.x, a.x - 5.0) && close(b.y, a.y - 2.0));
        assert!(close(c.x, a.x - min[0]) && close(c.y, a.y - min[1]));
    }
    // An offset by package glob turns only the matching parts, wrapping
    // at 360.
    let u1 = row(&base, "U1")?;
    let pattern = format!("{}*", u1.package.chars().take(3).collect::<String>());
    let turned = rows(
        &p,
        &PlacementConvention {
            rotation_offsets: vec![RotationOffset {
                package: pattern.clone(),
                degrees: 270.0,
            }],
            ..Default::default()
        },
    )?;
    for (a, b) in base.iter().zip(&turned) {
        let expected = if glob_matches(&pattern, &a.package) {
            (a.rotation_deg + 270.0).rem_euclid(360.0)
        } else {
            a.rotation_deg
        };
        assert!(close(b.rotation_deg, expected), "{a:?} {b:?}");
    }
    assert!(close(row(&turned, "U1")?.rotation_deg, 0.0));
    // A bad convention is refused.
    let bad = PlacementConvention {
        origin: PlacementOrigin::Point {
            at: [f64::NAN, 0.0],
        },
        ..Default::default()
    };
    assert!(rows(&p, &bad).is_err());
    Ok(())
}

#[test]
fn csv_fields_are_quoted_per_rfc_4180() {
    assert_eq!(csv_field("10k"), "10k");
    assert_eq!(csv_field("10k, 1%"), "\"10k, 1%\"");
    assert_eq!(csv_field("2\" header"), "\"2\"\" header\"");
    assert_eq!(csv_field("a\nb"), "\"a\nb\"");
    let rows = [PlacementRow {
        instance: 1,
        designator: "R1".into(),
        value: "10k, 1%".into(),
        package: "0603".into(),
        x: -0.0,
        y: 1.23456,
        rotation_deg: 0.0,
        side: Layer::Top,
    }];
    assert_eq!(
        placement_csv(&rows),
        "Designator,Value,Package,X (mm),Y (mm),Rotation (deg),Side\n\
         R1,\"10k, 1%\",0603,0.0000,1.2346,0.000,Top\n"
    );
}

#[test]
fn bom_files_cover_every_instance_and_are_deterministic() -> Fallible<()> {
    let p = showcase()?;
    let options = FabricationOptions::default();
    let a = files(&p, &options, "showcase")?;
    let b = files(&p, &options, "showcase")?;
    assert_eq!(a, b);
    let names: Vec<&str> = a.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "showcase-pos-top.csv",
            "showcase-pos-bottom.csv",
            "showcase-bom.csv",
            "showcase-bom-parts.csv"
        ]
    );
    let text = |name: &str| -> Fallible<String> {
        let f = a.iter().find(|f| f.name == name).ok_or("file")?;
        Ok(String::from_utf8(f.bytes.clone())?)
    };
    // 8 top rows and R3 alone on the bottom, plus headers.
    assert_eq!(text("showcase-pos-top.csv")?.lines().count(), 9);
    let bottom = text("showcase-pos-bottom.csv")?;
    assert_eq!(bottom.lines().count(), 2);
    assert!(
        bottom
            .lines()
            .nth(1)
            .is_some_and(|l| l.starts_with("R3,") && l.ends_with(",Bottom"))
    );
    // Grouped quantities add up to every instance; the per-part BOM has a
    // row for each, all fitted.
    let bom = text("showcase-bom.csv")?;
    let quantity: u32 = bom
        .lines()
        .skip(1)
        .filter_map(|l| l.split(',').nth(1)?.parse::<u32>().ok())
        .sum();
    assert_eq!(quantity as usize, p.instances.len());
    let parts = text("showcase-bom-parts.csv")?;
    assert_eq!(parts.lines().count(), p.instances.len() + 1);
    assert!(parts.lines().skip(1).all(|l| l.ends_with(",yes")));
    Ok(())
}
