//! SVG paint coverage: each feature against hand-derived areas, colours
//! and places. All artwork here is written for the tests.
use super::super::fill::{Flat, fill_with};
use super::*;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn svg(body: &str) -> String {
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{body}</svg>")
}

fn filled(group: &Painted) -> std::result::Result<Flat, Box<dyn std::error::Error>> {
    let mut flat = Flat::default();
    fill_with(&group.loops, group.rule, &mut flat, 1 << 20)?;
    Ok(flat)
}

fn area(group: &Painted) -> std::result::Result<f64, Box<dyn std::error::Error>> {
    Ok(filled(group)?.area())
}

fn covers(flat: &Flat, p: Point) -> bool {
    flat.triangles.iter().any(|t| {
        let q = t.map(|i| flat.points[i as usize]);
        let s = |a: Point, b: Point| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        s(q[0], q[1]) >= 0.0 && s(q[1], q[2]) >= 0.0 && s(q[2], q[0]) >= 0.0
    })
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn box_of(groups: &[Painted]) -> [f64; 4] {
    bounds_of(groups).unwrap_or([0.0; 4])
}

/// One element at 1:1: a 10 × 10 artwork drawn into a 10 × 10 decal.
fn one(body: &str) -> std::result::Result<Vec<Painted>, Box<dyn std::error::Error>> {
    Ok(artwork(&svg(body), [0.0, 0.0], [10.0, 10.0])?)
}

#[test]
fn holes_stay_open_by_winding_and_by_even_odd() -> TestResult {
    // Outer square and an inner one drawn the other way: a hole under
    // either rule.
    let reversed = one(r#"<path d="M0 0 H10 V10 H0 Z M3 3 V7 H7 V3 Z"/>"#)?;
    assert_eq!(reversed.len(), 1);
    assert!(near(area(&reversed[0])?, 84.0, 1e-9));
    assert!(!covers(&filled(&reversed[0])?, [0.0, 0.0]));
    assert!(covers(&filled(&reversed[0])?, [4.0, 4.0]));
    // Drawn the same way: nonzero fills it, even-odd opens it.
    let same = r#"<path d="M0 0 H10 V10 H0 Z M3 3 H7 V7 H3 Z"/>"#;
    assert!(near(area(&one(same)?[0])?, 100.0, 1e-9));
    let odd = one(&same.replace("<path ", r#"<path fill-rule="evenodd" "#))?;
    assert_eq!(odd[0].rule, FillRule::EvenOdd);
    assert!(near(area(&odd[0])?, 84.0, 1e-9));
    // The same through `style`.
    let styled = one(&same.replace("<path ", r#"<path style="fill-rule:evenodd" "#))?;
    assert!(near(area(&styled[0])?, 84.0, 1e-9));
    Ok(())
}

#[test]
fn a_star_differs_between_nonzero_and_even_odd() -> TestResult {
    // A pentagram: its centre pentagon is wound twice.
    let star = r#"<polygon points="50,0 79,90 2,35 98,35 21,90"/>"#;
    let nonzero = one(star)?;
    let odd = one(&star.replace("<polygon ", r#"<polygon fill-rule="evenodd" "#))?;
    let (a, b) = (area(&nonzero[0])?, area(&odd[0])?);
    assert!(b < a * 0.8, "{b} against {a}");
    // The centre of the decal is the star's centre region: filled only
    // by nonzero.
    let centre = {
        let bb = box_of(&nonzero);
        [(bb[0] + bb[2]) / 2.0, bb[1] + 0.55 * (bb[3] - bb[1])]
    };
    assert!(covers(&filled(&nonzero[0])?, centre));
    assert!(!covers(&filled(&odd[0])?, centre));
    Ok(())
}

#[test]
fn basic_shapes_have_their_areas() -> TestResult {
    let pi = std::f64::consts::PI;
    // A circle of r 5 fills the decal: πr², short of it by at most the
    // chord tolerance times the perimeter (2πr) for the inscribed polygon.
    let circle = one(r#"<circle cx="20" cy="20" r="5"/>"#)?;
    assert!(near(area(&circle[0])?, pi * 25.0, CHORD_MM * 10.0 * pi));
    // An ellipse 10 × 5 into a 10 × 10 decal: 1:1, π·5·2.5.
    let ellipse = one(r#"<ellipse cx="0" cy="0" rx="5" ry="2.5"/>"#)?;
    assert!(near(area(&ellipse[0])?, pi * 12.5, CHORD_MM * 10.0 * pi));
    // A rounded square: 100 − (4 − π)·r².
    let rect = one(r#"<rect width="10" height="10" rx="2"/>"#)?;
    assert!(near(
        area(&rect[0])?,
        100.0 - (4.0 - pi) * 4.0,
        CHORD_MM * 4.0 * pi
    ));
    // A half disc by an arc: πr²/2.
    let half = one(r#"<path d="M0 5 A5 5 0 0 1 10 5 Z"/>"#)?;
    assert!(near(area(&half[0])?, pi * 25.0 / 2.0, CHORD_MM * 5.0 * pi));
    // Relative commands, H/V, a quadratic and a smooth cubic all read.
    let rel = one(r#"<path d="m0 0 h10 v10 h-10 z"/>"#)?;
    assert!(near(area(&rel[0])?, 100.0, 1e-9));
    let curves = one(r#"<path d="M0 0 Q5 10 10 0 T20 0 C20 5 15 5 10 5 S0 5 0 0Z"/>"#)?;
    assert!(area(&curves[0])? > 1.0);
    // A polyline's fill closes it; a line draws nothing without a stroke.
    let tri = one(r#"<polyline points="0,0 10,0 10,10"/>"#)?;
    assert!(near(area(&tri[0])?, 50.0, 1e-9));
    assert!(one(r#"<line x1="0" y1="0" x2="10" y2="0"/>"#)?.is_empty());
    Ok(())
}

#[test]
fn strokes_are_round_capped_segments_scaled_by_the_transform() -> TestResult {
    // A 10 mm line stroked 2 wide: 20 plus a 12-gon of radius 1 for the
    // two half-round caps (CAP_STEPS = 6 per half): 3·sin 30° · 12 / 2.
    let line = r##"<line x1="0" y1="0" x2="10" y2="0" stroke="#000" stroke-width="2"/>"##;
    let out = artwork(&svg(line), [0.0, 0.0], [12.0, 2.0])?;
    assert_eq!(out.len(), 1);
    assert!(near(area(&out[0])?, 20.0 + 3.0, 1e-6), "{}", area(&out[0])?);
    // Doubled by a scale, the stroke doubles too: the shape is the same,
    // so fitted to the same decal it has the same area.
    let scaled = format!(r#"<g transform="scale(2)">{line}</g>"#);
    let out = artwork(&svg(&scaled), [0.0, 0.0], [12.0, 2.0])?;
    assert!(near(area(&out[0])?, 23.0, 1e-6));
    // A filled and stroked shape: the fill first, then its stroke.
    let both =
        one(r##"<rect width="8" height="8" fill="#ff0000" stroke="#0000ff" stroke-width="1"/>"##)?;
    assert_eq!(both.len(), 2);
    assert_eq!(both[0].colour, Some([255, 0, 0]));
    assert_eq!(both[1].colour, Some([0, 0, 255]));
    Ok(())
}

#[test]
fn colours_inherit_and_opacity_hides_faint_paint() -> TestResult {
    let colour =
        |body: &str| -> std::result::Result<Vec<Option<[u8; 3]>>, Box<dyn std::error::Error>> {
            Ok(one(body)?.into_iter().map(|p| p.colour).collect())
        };
    assert_eq!(
        colour(r#"<rect width="1" height="1"/>"#)?,
        [Some([0, 0, 0])]
    );
    assert_eq!(
        colour(r##"<rect width="1" height="1" fill="#f80"/>"##)?,
        [Some([255, 136, 0])]
    );
    assert_eq!(
        colour(r#"<rect width="1" height="1" style="fill: rgb(10, 20, 30)"/>"#)?,
        [Some([10, 20, 30])]
    );
    assert_eq!(
        colour(r#"<rect width="1" height="1" fill="rgb(100%, 0%, 50%)"/>"#)?,
        [Some([255, 0, 128])]
    );
    assert_eq!(
        colour(r#"<g fill="navy"><g><rect width="1" height="1"/></g></g>"#)?,
        [Some([0, 0, 128])]
    );
    // currentColor: the `color` when set, else the finish's own colour.
    assert_eq!(
        colour(r#"<rect width="1" height="1" fill="currentColor"/>"#)?,
        [None]
    );
    assert_eq!(
        colour(r##"<g color="#123456"><rect width="1" height="1" fill="currentColor"/></g>"##)?,
        [Some([0x12, 0x34, 0x56])]
    );
    // fill="none" draws nothing; faint paint is not drawn.
    assert!(one(r#"<rect width="1" height="1" fill="none"/>"#)?.is_empty());
    assert!(one(r#"<rect width="1" height="1" fill-opacity="0.3"/>"#)?.is_empty());
    assert!(one(r#"<g opacity="0.6"><rect width="1" height="1" opacity="0.6"/></g>"#)?.is_empty());
    assert_eq!(
        one(r#"<rect width="1" height="1" opacity="0.6"/>"#)?.len(),
        1
    );
    // Hidden elements and defs are not drawn.
    assert!(one(r#"<rect width="1" height="1" display="none"/><defs><rect width="1" height="1"/></defs>"#)?.is_empty());
    Ok(())
}

#[test]
fn gradients_paint_their_mean_stop_colour() -> TestResult {
    let body = r##"<defs>
        <linearGradient id="g"><stop offset="0" stop-color="#000000"/><stop offset="1" style="stop-color:#ffffff"/></linearGradient>
        <radialGradient id="r" href="#g"/>
        <linearGradient id="f"><stop stop-color="#ff0000"/><stop stop-color="#00ff00" stop-opacity="0.1"/></linearGradient>
      </defs>
      <rect width="1" height="1" fill="url(#g)"/>
      <rect x="2" width="1" height="1" fill="url('#r')"/>
      <rect x="4" width="1" height="1" fill="url(#f)"/>
      <rect x="6" width="1" height="1" fill="url(#missing)"/>"##;
    let (out, notes) = artwork_with_notes(&svg(body), [0.0, 0.0], [10.0, 10.0])?;
    let colours: Vec<_> = out.iter().map(|p| p.colour).collect();
    // 127.5 rounds up; a faint stop is left out.
    assert_eq!(colours, [Some([128; 3]), Some([128; 3]), Some([255, 0, 0])]);
    assert!(
        notes.iter().any(|n| n.contains("paint servers")),
        "{notes:?}"
    );
    Ok(())
}

#[test]
fn transforms_nest_and_move_shapes() -> TestResult {
    // Two unit squares 9 apart: a 10-wide artwork at 1:1, centred.
    let out = one(
        r#"<rect width="1" height="1"/><g transform="translate(4 0)"><rect transform="translate(5,0)" width="1" height="1"/></g>"#,
    )?;
    assert_eq!(out.len(), 2);
    let centre = |p: &Painted| {
        let b = box_of(std::slice::from_ref(p));
        [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]
    };
    assert!(near(centre(&out[0])[0], -4.5, 1e-9) && near(centre(&out[1])[0], 4.5, 1e-9));
    // Turned a quarter, a 2 × 1 bar stands 1 × 2: fitted to 10 × 10 it
    // is 5 wide and 10 tall.
    let out = one(r#"<rect width="2" height="1" transform="rotate(90)"/>"#)?;
    let b = box_of(&out);
    assert!(near(b[2] - b[0], 5.0, 1e-9) && near(b[3] - b[1], 10.0, 1e-9));
    // rotate about a point, matrix and skew read too.
    let out = one(
        r#"<rect width="2" height="1" transform="rotate(90 1 0.5) matrix(1 0 0 1 0 0) skewX(0)"/>"#,
    )?;
    let b = box_of(&out);
    assert!(near(b[2] - b[0], 5.0, 1e-9));
    // y runs up on the decal: the square drawn at the top of the SVG
    // (small y) lands at +y.
    let out = one(r#"<rect width="1" height="1"/><rect y="9" x="9" width="1" height="1"/>"#)?;
    assert!(centre(&out[0])[1] > centre(&out[1])[1]);
    // A malformed transform skips its element, said so.
    let (out, notes) = artwork_with_notes(
        &svg(r#"<rect width="1" height="1" transform="spin(3)"/>"#),
        [0.0; 2],
        [1.0; 2],
    )?;
    assert!(out.is_empty() && !notes.is_empty());
    Ok(())
}

#[test]
fn a_stretched_view_box_changes_the_aspect() -> TestResult {
    let text = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" width="20mm" height="10mm" preserveAspectRatio="none"><rect width="10" height="10"/></svg>"#;
    let b = box_of(&artwork(text, [0.0, 0.0], [10.0, 10.0])?);
    assert!(near(b[2] - b[0], 10.0, 1e-9) && near(b[3] - b[1], 5.0, 1e-9));
    // Kept aspect by default.
    let kept = text.replace(r#" preserveAspectRatio="none""#, "");
    let b = box_of(&artwork(&kept, [0.0, 0.0], [10.0, 10.0])?);
    assert!(near(b[2] - b[0], 10.0, 1e-9) && near(b[3] - b[1], 10.0, 1e-9));
    Ok(())
}

#[test]
fn text_is_drawn_in_the_built_in_font_with_its_counters_open() -> TestResult {
    let out = artwork(
        &svg(r##"<text x="0" y="0" font-size="10" fill="#333">O<tspan>K</tspan></text>"##),
        [0.0, 0.0],
        [20.0, 10.0],
    )?;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].colour, Some([0x33; 3]));
    let b = box_of(&out);
    // Fitted: at most the decal, touching it in one direction.
    assert!(b[2] - b[0] <= 20.0 + 1e-9 && b[3] - b[1] <= 10.0 + 1e-9);
    assert!(near(b[2] - b[0], 20.0, 1e-6) || near(b[3] - b[1], 10.0, 1e-6));
    let flat = filled(&out[0])?;
    // The O's counter is open: a point in the middle of the left half.
    let o_centre = [b[0] + 0.24 * (b[2] - b[0]), (b[1] + b[3]) / 2.0];
    assert!(!covers(&flat, o_centre));
    // An anchor in the middle centres the text on x (only the shape's own
    // bounds matter once fitted, so check it still draws).
    let mid = one(r#"<text x="5" y="5" text-anchor="middle">AB</text>"#)?;
    assert!(!mid.is_empty());
    Ok(())
}

fn png(
    width: u32,
    height: u32,
    rgba: &[u8],
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    use base64::Engine as _;
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba)?;
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[test]
fn embedded_png_images_become_runs_of_quantised_colour() -> TestResult {
    // 4 × 2: top row red red clear blue, bottom row red, then 3 white.
    let (r, w, b, c) = (
        [255, 0, 0, 255],
        [255, 255, 255, 255],
        [0, 0, 255, 255],
        [9, 9, 9, 0],
    );
    let pixels: Vec<u8> = [r, r, c, b, r, w, w, w].concat();
    let data = png(4, 2, &pixels)?;
    let body =
        format!(r#"<image x="0" y="0" width="4" height="2" href="data:image/png;base64,{data}"/>"#);
    let out = artwork(&svg(&body), [0.0, 0.0], [4.0, 2.0])?;
    let colours: Vec<_> = out.iter().map(|p| p.colour).collect();
    assert_eq!(
        colours,
        [Some([255, 0, 0]), Some([0, 0, 255]), Some([255, 255, 255])]
    );
    // Red: two cells of the top row and one of the bottom.
    assert!(near(area(&out[0])?, 3.0, 1e-9));
    assert!(near(area(&out[1])?, 1.0, 1e-9));
    assert!(near(area(&out[2])?, 3.0, 1e-9));
    // The clear cell is left out: the top row's third cell (x 0..1 of
    // the decal's −2..2, y 0..1) is covered by nothing.
    assert!(
        out.iter()
            .all(|p| filled(p).is_ok_and(|f| !covers(&f, [0.5, 0.5])))
    );
    // A larger image averages down to at most IMAGE_CELLS a side.
    let big = [0u8, 0, 0, 255].repeat(200 * 100);
    let data = png(200, 100, &big)?;
    let body = format!(r#"<image width="200" height="100" href="data:image/png;base64,{data}"/>"#);
    let out = artwork(&svg(&body), [0.0, 0.0], [2.0, 1.0])?;
    assert_eq!(out.len(), 1);
    // f = ceil(200 / 96) = 3: 67 × 34 cells, one run a row.
    assert_eq!(out[0].loops.len(), 34);
    assert!(near(area(&out[0])?, 2.0, 1e-6));
    // Other images are skipped with a note; bad data is refused.
    let (out, notes) = artwork_with_notes(
        &svg(r#"<image width="1" height="1" href="logo.png"/>"#),
        [0.0; 2],
        [1.0; 2],
    )?;
    assert!(out.is_empty() && notes.iter().any(|n| n.contains("images")));
    assert!(
        artwork(
            &svg(r#"<image width="1" height="1" href="data:image/png;base64,!!!"/>"#),
            [0.0; 2],
            [1.0; 2]
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn unsupported_features_are_noted_not_drawn() -> TestResult {
    let (out, notes) = artwork_with_notes(
        &svg(
            r##"<use href="#a"/><rect width="1" height="1" clip-path="url(#c)" filter="url(#f)"/>"##,
        ),
        [0.0; 2],
        [1.0; 2],
    )?;
    assert_eq!(out.len(), 1);
    for what in ["use", "clip", "filter"] {
        assert!(
            notes.iter().any(|n| n.contains(what)),
            "{what} in {notes:?}"
        );
    }
    Ok(())
}

#[test]
fn limits_refuse_the_whole_artwork() {
    // Too many bytes.
    let huge = svg(&" ".repeat(MAX_SVG_BYTES));
    assert!(artwork(&huge, [0.0; 2], [1.0; 2]).is_err());
    // Too many elements.
    let many = svg(&r#"<rect width="1" height="1"/>"#.repeat(MAX_ELEMENTS + 1));
    assert!(artwork(&many, [0.0; 2], [1.0; 2]).is_err());
    // Too much text.
    let text = svg(&format!("<text>{}</text>", "A".repeat(MAX_TEXT_CHARS + 1)));
    assert!(artwork(&text, [0.0; 2], [1.0; 2]).is_err());
    // Too many images.
    let image = r#"<image width="1" height="1" href="data:image/png;base64,AAAA"/>"#;
    assert!(artwork(&svg(&image.repeat(MAX_IMAGES + 1)), [0.0; 2], [1.0; 2]).is_err());
    // Not SVG, malformed XML, a bad size.
    assert!(artwork("<html/>", [0.0; 2], [1.0; 2]).is_err());
    assert!(artwork("<svg><rect></svg>", [0.0; 2], [1.0; 2]).is_err());
    assert!(artwork(&svg(""), [0.0; 2], [f64::NAN, 1.0]).is_err());
    // Nothing painted is nothing, not an error.
    assert!(artwork(&svg(""), [0.0; 2], [1.0; 2]).is_ok_and(|o| o.is_empty()));
}

#[test]
fn fill_with_keeps_triangles_counter_clockwise_for_either_winding() -> TestResult {
    let square = vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let mut flat = Flat::default();
        fill_with(std::slice::from_ref(&square), rule, &mut flat, 16)?;
        assert!(near(flat.area(), 1.0, 1e-12), "{rule:?}");
    }
    Ok(())
}
