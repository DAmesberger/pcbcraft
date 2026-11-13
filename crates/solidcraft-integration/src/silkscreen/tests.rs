use super::*;
use sparksmith_core::library::graphics::{HAlign, Justify, TextMetric, VAlign};
use sparksmith_core::{Engine, Track};

/// The test prototype profile's silkscreen rules (1/7 of the height, at
/// least 0.1 mm).
fn silk() -> SilkscreenRules {
    sparksmith_core::rules::fixtures::prototype().map_or_else(
        |_| sparksmith_core::rules::DesignRules::unset().silkscreen,
        |p| p.rules().silkscreen,
    )
}

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
    text_loops(&mut upright, &text("R10", false, 0.0), &silk());
    assert!(!upright.is_empty());
    let box_ = text("R10", false, 0.0).bounds(TextMetric::default());
    let [x0, y0, x1, y1] = extent(&upright);
    let half = silk().text_stroke(1.0) / 2.0;
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
    text_loops(&mut mirrored, &text("R10", true, 0.0), &silk());
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
    text_loops(&mut turned, &text("R10", false, 90.0), &silk());
    let [t0, t1, t2, t3] = extent(&turned);
    assert!(((t2 - t0) - (y1 - y0)).abs() < 1e-9 && ((t3 - t1) - (x1 - x0)).abs() < 1e-9);
}

#[test]
fn unknown_characters_print_nothing_but_keep_their_place() {
    let mut known = Vec::new();
    text_loops(&mut known, &text("R\u{2126}1", false, 0.0), &silk());
    let mut spaced = Vec::new();
    text_loops(&mut spaced, &text("R 1", false, 0.0), &silk());
    assert_eq!(extent(&known), extent(&spaced));
}

/// The unsigned area of a loop.
fn loop_area(l: &[Point]) -> f64 {
    let n = l.len();
    (0..n)
        .map(|i| l[i][0] * l[(i + 1) % n][1] - l[(i + 1) % n][0] * l[i][1])
        .sum::<f64>()
        .abs()
        / 2.0
}

/// A raised film's area: its pieces less their holes.
fn film_area(f: &Raised) -> f64 {
    f.fills.iter().map(|l| loop_area(l)).sum::<f64>()
        - f.cuts.iter().map(|l| loop_area(l)).sum::<f64>()
}

#[test]
fn raised_ink_keeps_the_overlap_only() {
    let square = vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
    let shifted = vec![[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]];
    let mut clockwise = shifted.clone();
    clockwise.reverse();
    for clip in [&shifted, &clockwise] {
        let films = raised(std::slice::from_ref(&square), &[vec![clip.clone()]]);
        let area: f64 = films.iter().map(film_area).sum();
        assert!((area - 1.0).abs() < 1e-9, "{area}");
    }
    let far = vec![[5.0, 5.0], [6.0, 5.0], [6.0, 6.0]];
    assert!(raised(&[square], &[vec![far]]).is_empty());
}

/// Ink over a pour with a clearance hole, and a track's cap inside that
/// hole: the pour's piece keeps its hole, and the cap's piece inside it is
/// a film of its own, never cut by the hole.
#[test]
fn raised_ink_keeps_pour_holes_and_the_caps_inside_them() {
    let square = |lo: f64, hi: f64| vec![[lo, lo], [hi, lo], [hi, hi], [lo, hi]];
    let mut hole = square(3.0, 7.0);
    hole.reverse();
    let pour = vec![square(0.0, 10.0), hole];
    let cap = vec![square(4.0, 6.0)];
    let ink = vec![square(1.0, 9.0)];
    let films = raised(&ink, &[pour, cap]);
    assert_eq!(films.len(), 2);
    assert!((film_area(&films[0]) - (64.0 - 16.0)).abs() < 1e-6);
    assert!((film_area(&films[1]) - 4.0).abs() < 1e-6);
    assert!(raised(&ink, &[]).is_empty());
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
    let crown = 1.6 + 0.035 + project.stackup.mask_um / 1000.0;
    let raised: Vec<&&Geometry> = films
        .iter()
        // 15 µm of legend ink (the shipped finishes).
        .filter(|g| (g.max[2] - (crown + 0.015)).abs() < 1e-9)
        .collect();
    assert_eq!(raised.len(), 1, "one raised piece, over the track only");
    let piece = raised[0];
    assert!(piece.min[1] >= 9.6 - 0.125 - CAP_MARGIN_MM - 1e-9);
    assert!(piece.max[1] <= 9.6 + 0.125 + CAP_MARGIN_MM + 1e-9);
    // Same part, same pick, as the flat film.
    assert!(films.iter().filter(|g| g.item == piece.item).count() >= 2);
    Ok(())
}

/// The legend is clipped clear of mask openings by the rules'
/// silkscreen-to-pad clearance: a wider clearance prints less ink.
#[test]
fn the_legend_clears_mask_openings_by_the_rule() -> Result<()> {
    use sparksmith_core::rules::{RuleKey, RuleOverride, RuleValue};
    let mut project = Engine::demo()
        .map_err(|e| crate::Error::Verification(e.to_string()))?
        .project()
        .clone();
    project.rules = sparksmith_core::rules::fixtures::rules()
        .map_err(|e| crate::Error::Verification(e.to_string()))?;
    let ink = |project: &Project| -> Result<f64> {
        Ok(crate::project::build(project)?
            .geometry
            .iter()
            .filter(|g| g.kind == BodyKind::Silkscreen)
            .map(|g| g.volume())
            .sum())
    };
    let close = ink(&project)?;
    project.rules.overrides.push(RuleOverride {
        key: RuleKey::SilkPadClearance,
        value: RuleValue::Length(0.3),
    });
    let clear = ink(&project)?;
    assert!(clear < close - 1e-6, "{clear} vs {close}");
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
    use sparksmith_core::layout::LayoutCommand;
    use sparksmith_core::{Layer, Operation};
    let fail = |e: sparksmith_core::Error| crate::Error::Verification(e.to_string());
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

/// The 3D legend reads `layout::placed_graphics`, so a designator moved
/// with `instance.set_text_placement` prints there: the part's film reaches
/// the text's new line and no further than its height.
#[test]
fn a_moved_designator_moves_its_legend_film() -> Result<()> {
    use sparksmith_core::layout::LayoutCommand;
    use sparksmith_core::model::TextPlacement;
    use sparksmith_core::{InstanceCommand, Layer, Operation};
    let fail = |e: sparksmith_core::Error| crate::Error::Verification(e.to_string());
    let mut engine = Engine::demo().map_err(fail)?;
    let project = engine.project();
    let [w, h] = project.board_size;
    let instance = project
        .placements
        .iter()
        .map(|p| p.instance)
        .find(|id| sparksmith_core::instances::text::target(project, *id, None).is_ok())
        .ok_or_else(|| crate::Error::Verification("no designator".into()))?;
    let revision = engine.revision();
    engine
        .apply(
            revision,
            Operation::Layout(LayoutCommand::Move {
                instance,
                position: [w / 2.0, h / 2.0],
                rotation_deg: 0.0,
                layer: Layer::Top,
            }),
        )
        .map_err(fail)?;
    let top = |project: &sparksmith_core::Project| -> Result<f64> {
        let model = crate::project::build(project)?;
        Ok(model
            .geometry
            .iter()
            .filter(|g| g.kind == BodyKind::Silkscreen)
            .map(|g| g.max[1])
            .fold(f64::NEG_INFINITY, f64::max))
    };
    let before = top(engine.project())?;
    // Up the board by the room left above everything printed so far.
    let dy = (h - 1.0) - before;
    assert!(dy > 0.5, "the fixture leaves {dy} mm above the legend");
    let revision = engine.revision();
    engine
        .apply(
            revision,
            Operation::Instance(InstanceCommand::SetTextPlacement {
                instance,
                graphic: None,
                placement: Some(TextPlacement {
                    offset: [0.0, dy],
                    ..Default::default()
                }),
            }),
        )
        .map_err(fail)?;
    let after = top(engine.project())?;
    assert!(after > before + 0.5 * dy, "{before} to {after}, moved {dy}");
    assert!(
        after <= before + dy + 1e-6,
        "{before} to {after}, moved {dy}"
    );
    Ok(())
}

/// Run 4, finding 21: a designator over a top pour stands on the pour's
/// mask cap, so it shows in 3D; ink on the laminate's mask alone would lie
/// under the cap.
#[test]
fn a_designator_over_a_pour_stands_on_its_mask() -> Result<()> {
    use sparksmith_core::layout::{BoardDrawing, PlacedItem};
    use sparksmith_core::model::fill::{FilledCopper, FilledRegion};
    use sparksmith_core::{Layer, Project};
    let mut project: Project = Engine::showcase()
        .map_err(|e| crate::Error::Verification(e.to_string()))?
        .project()
        .clone();
    let [w, h] = project.board_size;
    // No track caps or via domes to stand on: only the pour lifts the ink.
    project.tracks.clear();
    project.vias.clear();
    // The showcase's pours are inside and below: one moved to the top.
    let fill = project
        .fills
        .first_mut()
        .ok_or_else(|| crate::Error::Verification("no pour".into()))?;
    fill.layer = CopperLayer::Top;
    let fill_id = fill.id;
    fill.computed = Some(FilledCopper {
        key: "test".into(),
        regions: vec![FilledRegion {
            outer: vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]],
            holes: Vec::new(),
        }],
        removed_islands: 0,
    });
    let (instance, centre) = sparksmith_core::layout::placed_graphics(&project)
        .into_iter()
        .find_map(|g| match (&g.layer, &g.item) {
            (BoardDrawing::Silkscreen(Layer::Top), PlacedItem::Text(t)) => Some((g.instance, t.at)),
            _ => None,
        })
        .ok_or_else(|| crate::Error::Verification("no top designator".into()))?;
    let model = crate::project::build(&project)?;
    let part = model
        .sources
        .iter()
        .find(|s| s.instance == Some(instance) && s.pad.is_none())
        .map(|s| s.pick_id);
    let pour = model
        .sources
        .iter()
        .find(|s| s.fill == Some(fill_id))
        .map(|s| s.pick_id);
    let mask = model
        .geometry
        .iter()
        .filter(|g| g.kind == BodyKind::MaskCap && g.item == pour)
        .map(|g| g.max[2])
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(mask.is_finite(), "the pour wears a mask cap");
    let over = model.geometry.iter().any(|g| {
        g.kind == BodyKind::Silkscreen
            && g.item == part
            && g.max[2] > mask
            && (g.min[0]..=g.max[0]).contains(&centre[0])
            && (g.min[1]..=g.max[1]).contains(&centre[1])
    });
    assert!(over, "the designator's ink stands above the pour's mask");
    Ok(())
}
