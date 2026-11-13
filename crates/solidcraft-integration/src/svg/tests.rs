use super::*;
use sparksmith_core::geometry::segment_distance;
use sparksmith_core::ids::UnitId;
use sparksmith_core::library::graphics::{ItemParams, ShapeNmParams, Side};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

/// Original fixture artwork (see the file's header).
pub const MARK: &str = include_str!("mark.svg");

fn options() -> Options {
    Options::new(0.25)
}

/// The artwork's shapes in millimetres, as stored (rounded to whole
/// nanometres).
fn mm(art: &Artwork) -> Vec<(Shape, f64, Fill)> {
    art.items
        .iter()
        .filter_map(|item| match item {
            Item::Shape {
                shape,
                width_nm,
                fill,
            } => Some((shape.to_mm(), width_nm.to_mm_f64(), *fill)),
            Item::Text(_) => None,
        })
        .collect()
}

fn shapes(owned: &[(Shape, f64, Fill)]) -> Vec<(&Shape, f64, Fill)> {
    owned.iter().map(|(s, w, f)| (s, *w, *f)).collect()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

#[test]
fn the_fixture_becomes_editable_primitives() -> TestResult {
    let art = convert(MARK, &options())?;
    let owned = mm(&art);
    let shapes = shapes(&owned);
    assert_eq!(shapes.len(), 7, "{shapes:#?}");
    // The badge: an exact rounded rectangle, 0.4 mm stroke, unfilled.
    let (Shape::Rect { a, b, radius }, width, Fill::None) = shapes[0] else {
        return Err(format!("badge is a rect: {:?}", shapes[0]).into());
    };
    assert!(near(*radius, 2.0) && near(width, 0.4));
    assert!(near((a[0] - b[0]).abs(), 28.0) && near((a[1] - b[1]).abs(), 18.0));
    // The dot: a filled circle, no outline.
    let (Shape::Circle { radius, .. }, width, Fill::Solid) = shapes[1] else {
        return Err(format!("dot is a filled circle: {:?}", shapes[1]).into());
    };
    assert!(near(*radius, 4.0) && width == 0.0);
    // The wave: one flattened polyline that keeps its 0.6 mm stroke.
    let (Shape::Polyline { points, .. }, width, Fill::None) = shapes[2] else {
        return Err(format!("wave is a polyline: {:?}", shapes[2]).into());
    };
    assert!(points.len() > 8 && near(width, 0.6));
    // The tilted triangle: a filled polygon.
    let (Shape::Polygon { points, radii }, _, Fill::Solid) = shapes[3] else {
        return Err(format!("triangle is a filled polygon: {:?}", shapes[3]).into());
    };
    assert_eq!((points.len(), radii.len()), (3, 0));
    // The ring has a hole: both loops are outlined as circles.
    for (shape, r) in [(shapes[4], 2.5), (shapes[5], 1.0)] {
        let (Shape::Circle { radius, .. }, width, Fill::None) = shape else {
            return Err(format!("ring loop is an outlined circle: {shape:?}").into());
        };
        assert!(near(*radius, r) && near(width, 0.25), "{shape:?}");
    }
    // The tab: tangent arcs become fillet radii of a filled polygon.
    let (Shape::Polygon { points, radii }, _, Fill::Solid) = shapes[6] else {
        return Err(format!("tab is a filled polygon: {:?}", shapes[6]).into());
    };
    assert_eq!(points.len(), 4);
    let rounded: Vec<f64> = radii.iter().copied().filter(|r| *r > 0.0).collect();
    assert!(
        rounded.len() == 2 && rounded.iter().all(|r| near(*r, 1.0)),
        "{radii:?}"
    );
    // Centred, at the badge's size plus its stroke.
    assert!(
        near(art.size[0], 28.4) && near(art.size[1], 18.4),
        "{:?}",
        art.size
    );
    assert!(
        art.notes.iter().any(|n| n.contains("holes")),
        "{:?}",
        art.notes
    );
    Ok(())
}

/// Every point of the true cubic lies within the tolerance of the
/// flattened polyline.
#[test]
fn beziers_flatten_within_the_tolerance() -> TestResult {
    let svg = r#"<svg width="100mm" height="100mm" viewBox="0 0 100 100">
        <path d="M0 0 C 30 90, 70 -40, 100 50" fill="none" stroke="black" stroke-width="0.2"/></svg>"#;
    for tol in [0.1, 0.01, 0.002] {
        let art = convert(
            svg,
            &Options {
                tolerance_mm: tol,
                ..options()
            },
        )?;
        let owned = mm(&art);
        let [(Shape::Polyline { points, .. }, _, _)] = shapes(&owned)[..] else {
            return Err("one polyline".into());
        };
        // The artwork is centred; undo that with the curve's start.
        let first = points.first().copied().ok_or("points")?;
        let curve = |t: f64| {
            let u = 1.0 - t;
            let (b, c, d) = (3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            let x = b * 30.0 + c * 70.0 + d * 100.0;
            let y = b * 90.0 + c * -40.0 + d * 50.0;
            // SVG y points down; the drawing reads upright.
            [x + first[0], -y + first[1]]
        };
        let mut worst: f64 = 0.0;
        for i in 0..=2000 {
            let p = curve(i as f64 / 2000.0);
            let d = points
                .windows(2)
                .map(|w| segment_distance(p, w[0], w[1]))
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(d);
        }
        assert!(worst <= tol + 1e-9, "tol {tol}: deviation {worst}");
        assert!(points.len() <= MAX_CURVE_SEGMENTS + 1);
    }
    Ok(())
}

#[test]
fn circular_arcs_stay_arcs_and_units_set_the_scale() -> TestResult {
    let svg = r#"<svg width="40mm" height="20mm" viewBox="0 0 40 20">
        <path d="M0 10 A 10 10 0 0 1 20 10" fill="none" stroke="black"/></svg>"#;
    let art = convert(svg, &options())?;
    let owned = mm(&art);
    let [
        (
            Shape::Arc {
                radius, sweep_deg, ..
            },
            width,
            Fill::None,
        ),
    ] = shapes(&owned)[..]
    else {
        return Err(format!("one arc: {:?}", art.items).into());
    };
    assert!(near(*radius, 10.0) && near(sweep_deg.abs(), 180.0) && near(width, 1.0));
    // A user scale multiplies size and stroke.
    let art = convert(
        svg,
        &Options {
            scale: 0.5,
            ..options()
        },
    )?;
    let owned = mm(&art);
    let [(Shape::Arc { radius, .. }, width, _)] = shapes(&owned)[..] else {
        return Err("one arc".into());
    };
    assert!(near(*radius, 5.0) && near(width, 0.5));
    // Without units, user units are CSS pixels: 96 per inch.
    let px = r#"<svg><circle cx="0" cy="0" r="96" fill="none" stroke="black"/></svg>"#;
    let art = convert(px, &options())?;
    let owned = mm(&art);
    let [(Shape::Circle { radius, .. }, _, _)] = shapes(&owned)[..] else {
        return Err("one circle".into());
    };
    assert!(near(*radius, 25.4));
    Ok(())
}

#[test]
fn malformed_and_oversized_input_is_refused() -> TestResult {
    let refused = |svg: &str, options: &Options, says: &str| match convert(svg, options) {
        Err(Error::Invalid(message)) if message.contains(says) => Ok(()),
        other => Err(format!("{says}: {other:?}")),
    };
    let o = options();
    refused("<html/>", &o, "not <svg>")?;
    refused("", &o, "no <svg>")?;
    refused("<svg><text>hi</text></svg>", &o, "no drawable")?;
    refused("<svg><path d='M0 0 L1 1'/></g></svg>", &o, "well-formed")?;
    let big = format!("<svg>{}</svg>", " ".repeat(MAX_SVG_BYTES));
    refused(&big, &o, "KiB")?;
    let circles = |n: usize| {
        let body: String = (0..n)
            .map(|i| format!("<circle cx='{}' cy='0' r='1'/>", i * 3))
            .collect();
        format!("<svg>{body}</svg>")
    };
    refused(&circles(MAX_GRAPHIC_EDITS + 1), &o, "at most 256")?;
    refused(&circles(MAX_SVG_ELEMENTS + 1), &o, "drawn elements")?;
    let wide = r#"<svg width="10mm" height="10mm" viewBox="0 0 10 10"><line x1="0" y1="0" x2="5" y2="5" stroke="black" stroke-width="2"/></svg>"#;
    refused(wide, &Options { scale: 4.0, ..o }, "scale the artwork down")?;
    refused(MARK, &Options { scale: 0.0, ..o }, "scale")?;
    refused(
        MARK,
        &Options {
            tolerance_mm: 0.0,
            ..o
        },
        "tolerance",
    )?;
    let huge = r#"<svg width="1000mm" height="1000mm" viewBox="0 0 1 1"><circle cx="0.5" cy="0.5" r="0.5"/></svg>"#;
    refused(huge, &Options { scale: 1000.0, ..o }, "±2 m")?;
    Ok(())
}

#[test]
fn edits_place_the_artwork_on_its_target() -> TestResult {
    let art = convert(MARK, &options())?;
    let target = GraphicTarget::Unit(UnitId(1));
    let edits = art.edits(target, [5.0, -2.5])?;
    assert_eq!(edits.len(), art.items.len());
    let Some(GraphicEdit::Add {
        item:
            ItemParams::Shape {
                shape: ShapeNmParams::Circle { center, .. },
                ..
            },
        ..
    }) = edits.get(1)
    else {
        return Err("the dot is added second".into());
    };
    // The artwork spans x 0.8–29.2 mm (centre 15); the dot is at 7 mm.
    assert!(
        near(center.0.to_mm_f64()[0], 5.0 + 7.0 - 15.0),
        "{center:?}"
    );
    let courtyard = GraphicTarget::Footprint {
        layer: DrawingLayer::Courtyard,
        side: Side::Component,
    };
    assert!(art.edits(courtyard, [0.0, 0.0]).is_err());
    Ok(())
}

/// Hidden and non-rendered content is not drawn; styles inherit.
#[test]
fn presentation_inherits_and_hidden_content_is_skipped() -> TestResult {
    let svg = r##"<svg width="10mm" height="10mm" viewBox="0 0 10 10">
        <defs><circle id="c" cx="1" cy="1" r="1"/></defs>
        <g style="fill:none;stroke:black;stroke-width:0.5">
          <circle cx="5" cy="5" r="2"/>
          <circle cx="5" cy="5" r="3" display="none"/>
        </g>
        <use href="#c"/>
    </svg>"##;
    let art = convert(svg, &options())?;
    let owned = mm(&art);
    let [(Shape::Circle { radius, .. }, width, Fill::None)] = shapes(&owned)[..] else {
        return Err(format!("one circle: {:?}", art.items).into());
    };
    assert!(near(*radius, 2.0) && near(width, 0.5));
    assert!(
        art.notes.iter().any(|n| n.contains("use")),
        "{:?}",
        art.notes
    );
    Ok(())
}
