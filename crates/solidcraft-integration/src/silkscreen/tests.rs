use super::*;
use pcbcraft_core::library::graphics::{Justify, VAlign};
use pcbcraft_core::{Engine, Track};

fn text(s: &str, mirrored: bool, rotation_deg: f64) -> PlacedText {
    PlacedText {
        graphic: None,
        text: s.into(),
        at: [10.0, 5.0],
        height: 1.0,
        rotation_deg,
        justify: Justify {
            h: HAlign::Center,
            v: VAlign::Middle,
        },
        mirrored,
        line_spacing: 1.6,
    }
}

fn extent(loops: &[Vec<Point>]) -> [f64; 4] {
    loops
        .iter()
        .flatten()
        .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
            [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ]
        })
}

#[test]
fn legend_text_fills_the_core_metric_box_and_mirrors_within_it() {
    let mut upright = Vec::new();
    text_loops(&mut upright, &text("R10", false, 0.0));
    assert!(!upright.is_empty());
    let box_ = text("R10", false, 0.0).bounds(TextMetric::default());
    let [x0, y0, x1, y1] = extent(&upright);
    let half = (TEXT_STROKE_SHARE * 1.0).max(MIN_STROKE_MM) / 2.0;
    // Inside the core's box, grown by half a stroke, and filling most of it.
    assert!(x0 >= box_.min[0] - half - 1e-9 && x1 <= box_.max[0] + half + 1e-9);
    assert!(y0 >= box_.min[1] - half - 1e-9 && y1 <= box_.max[1] + half + 1e-9);
    assert!(x1 - x0 > 0.8 * (box_.max[0] - box_.min[0]));
    assert!(
        (y1 - y0 - 1.0 - 2.0 * half).abs() < 1e-9,
        "cap height plus the stroke"
    );
    // Mirrored: the same box, the glyphs reflected (R's bowl now faces
    // left, so the left half holds more ink than before).
    let mut mirrored = Vec::new();
    text_loops(&mut mirrored, &text("R10", true, 0.0));
    let [m0, _, m1, _] = extent(&mirrored);
    assert!((m0 - x0).abs() < 1e-9 && (m1 - x1).abs() < 1e-9);
    let reflected: Vec<Vec<Point>> = upright
        .iter()
        .map(|l| l.iter().map(|p| [20.0 - p[0], p[1]]).collect())
        .collect();
    let key = |loops: &[Vec<Point>]| {
        let mut points: Vec<(i64, i64)> = loops
            .iter()
            .flatten()
            .map(|p| ((p[0] * 1e6).round() as i64, (p[1] * 1e6).round() as i64))
            .collect();
        points.sort_unstable();
        points
    };
    assert_eq!(key(&reflected), key(&mirrored));
    // Turned a quarter: the box turns with it.
    let mut turned = Vec::new();
    text_loops(&mut turned, &text("R10", false, 90.0));
    let [t0, t1, t2, t3] = extent(&turned);
    assert!(((t2 - t0) - (y1 - y0)).abs() < 1e-9 && ((t3 - t1) - (x1 - x0)).abs() < 1e-9);
}

#[test]
fn unknown_characters_print_nothing_but_keep_their_place() {
    let mut known = Vec::new();
    text_loops(&mut known, &text("R\u{2126}1", false, 0.0));
    let mut spaced = Vec::new();
    text_loops(&mut spaced, &text("R 1", false, 0.0));
    assert_eq!(extent(&known), extent(&spaced));
}

#[test]
fn convex_clips_keep_the_overlap_only() {
    let square = vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
    let shifted = vec![[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]];
    let mut clockwise = shifted.clone();
    clockwise.reverse();
    for clip in [&shifted, &clockwise] {
        let piece = clip_convex(&square, clip).unwrap_or_default();
        let n = piece.len();
        let area: f64 = (0..n)
            .map(|i| piece[i][0] * piece[(i + 1) % n][1] - piece[(i + 1) % n][0] * piece[i][1])
            .sum::<f64>()
            / 2.0;
        assert!((area.abs() - 1.0).abs() < 1e-9, "{area}");
    }
    let far = vec![[5.0, 5.0], [6.0, 5.0], [6.0, 6.0]];
    assert!(clip_convex(&square, &far).is_none());
}

/// A track running under R1's designator: the legend climbs its mask cap,
/// so the ink over it stands on the cap's crown, never under it.
#[test]
fn legend_over_a_track_stands_on_its_mask_cap() -> Result<()> {
    let mut project = Engine::demo()
        .map_err(|e| crate::Error::Verification(e.to_string()))?
        .project()
        .clone();
    let net = project.nets.first().map(|n| n.id).unwrap_or_default();
    let id = project
        .allocate()
        .map_err(|e| crate::Error::Verification(e.to_string()))?;
    project.tracks.push(Track {
        id,
        net,
        start: [5.0, 9.6],
        end: [11.0, 9.6],
        width: 0.25,
        layer: CopperLayer::Top,
        locked: false,
    });
    let model = crate::project::build(&project)?;
    let films: Vec<&Geometry> = model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::Silkscreen)
        .collect();
    let crown = 1.6 + 0.035 + MASK_CAP_MM;
    let raised: Vec<&&Geometry> = films
        .iter()
        .filter(|g| (g.max[2] - (crown + INK_THICKNESS_MM)).abs() < 1e-9)
        .collect();
    assert_eq!(raised.len(), 1, "one raised piece, over the track only");
    let piece = raised[0];
    assert!(piece.min[1] >= 9.6 - 0.125 - CAP_MARGIN_MM - 1e-9);
    assert!(piece.max[1] <= 9.6 + 0.125 + CAP_MARGIN_MM + 1e-9);
    // Same part, same pick, as the flat film.
    assert!(films.iter().filter(|g| g.item == piece.item).count() >= 2);
    Ok(())
}

#[test]
fn a_side_without_silkscreen_prints_nothing() -> Result<()> {
    let mut project = Engine::demo()
        .map_err(|e| crate::Error::Verification(e.to_string()))?
        .project()
        .clone();
    project.stackup.silkscreen_top = false;
    let model = crate::project::build(&project)?;
    assert!(
        model
            .geometry
            .iter()
            .all(|g| g.kind != BodyKind::Silkscreen)
    );
    Ok(())
}

/// U2 moved to (30, 12) on the showcase sets its legend against its
/// neighbours' and over copper; the build still succeeds and every
/// silkscreen film is a closed outward volume (a review found the whole
/// native build failing here).
#[test]
fn a_moved_part_never_fails_the_build_through_its_legend() -> Result<()> {
    use pcbcraft_core::layout::LayoutCommand;
    use pcbcraft_core::{Layer, Operation};
    let fail = |e: pcbcraft_core::Error| crate::Error::Verification(e.to_string());
    let mut engine = Engine::showcase().map_err(fail)?;
    let u2 = engine
        .project()
        .instance_by_designator("U2")
        .map(|i| i.id)
        .ok_or_else(|| crate::Error::Verification("U2".into()))?;
    engine
        .apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Move {
                instance: u2,
                position: [30.0, 12.0],
                rotation_deg: 0.0,
                layer: Layer::Top,
            }),
        )
        .map_err(fail)?;
    let model = crate::project::build(engine.project())?;
    let films: Vec<&Geometry> = model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::Silkscreen)
        .collect();
    assert!(films.iter().all(|g| g.volume() > 0.0));
    // U2's legend is built whole: its flat film and the piece standing on
    // the mask caps it crosses (the piece that failed), not left out.
    let pick = model
        .sources
        .iter()
        .find(|s| s.instance == Some(u2) && s.pad.is_none())
        .map(|s| s.pick_id)
        .ok_or_else(|| crate::Error::Verification("U2 pick".into()))?;
    assert_eq!(films.iter().filter(|g| g.item == Some(pick)).count(), 2);
    Ok(())
}
