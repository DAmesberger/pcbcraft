//! Layer focus measured on the painted canvas: the active layer's copper
//! is never covered by another layer's tracks or fills, from either side;
//! each focus mode paints what it says; keepouts show over all copper in
//! every mode.
use super::layer_focus::{
    DIM_FILL_TINT, DIM_SHADE, FILL_TINT, KEEPOUT_HATCH_HERE, KEEPOUT_HATCH_OTHER, keepout_color,
};
use super::raster_tests::{SCREEN, picture, view_of, with_project};
use super::*;
use crate::commands::{PcbView, Registry};
use sparksmith_core::library::{Outline, Segment};
use sparksmith_core::model::Keepout;
use sparksmith_core::model::fill::{KeepoutLayers, KeepoutRules};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The whole showcase framed, with the drawings and airwires that lie
/// over copper by design hidden.
fn bare(engine: &Engine) -> PcbEditor {
    let mut editor = PcbEditor::default();
    let board = engine.project().board_size;
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(SCREEN[0], SCREEN[1]));
    (editor.zoom, editor.pan) = frame_view(rect, board, [[0.0, 0.0], board], false);
    editor.layers.silk_top = false;
    editor.layers.silk_bottom = false;
    editor.layers.courtyard = false;
    editor.layers.assembly = false;
    editor.show_airwires = false;
    editor
}

/// Whether `p` lies on or near a pad (its number), a via or a hole,
/// which the canvas draws over copper.
fn near_drilled(editor: &PcbEditor, project: &Project, p: Point, margin: f64) -> bool {
    editor
        .board_pads
        .iter()
        .any(|b| b.outline.distance(p, true) < margin)
        || project
            .vias
            .iter()
            .any(|v| sparksmith_core::geometry::distance(v.position, p) < v.diameter / 2.0 + margin)
}

/// Every shape of a frame, flattened, in paint order.
fn shapes(output: &egui::FullOutput) -> Vec<Shape> {
    fn walk(shape: &Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            other => out.push(other.clone()),
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// Every colour a shape paints with.
fn colors_of(shape: &Shape) -> Vec<Color32> {
    match shape {
        Shape::Path(p) => {
            let mut v = vec![p.fill];
            if let egui::epaint::ColorMode::Solid(c) = p.stroke.color {
                v.push(c);
            }
            v
        }
        Shape::Mesh(m) => m.vertices.iter().map(|v| v.color).collect(),
        Shape::Circle(c) => vec![c.fill, c.stroke.color],
        Shape::LineSegment { stroke, .. } => vec![stroke.color],
        _ => vec![],
    }
}

fn frame(editor: &mut PcbEditor, engine: &Engine, ctx: &egui::Context) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(SCREEN[0], SCREEN[1]),
            )),
            ..Default::default()
        },
        |ui| {
            editor.canvas(ui, engine);
        },
    );
    output.textures_delta.clear();
    output
}

/// With each stackup layer active, seen from the top and from the bottom,
/// every pixel well inside the active layer's tracks is that layer's full
/// colour, and so is its fill copper when it is the near side: no other
/// layer's track or pour covers it. (The active layer's pours on the far
/// side or inside are translucent; `a_far_active_pour_lets_the_near_side_read`
/// covers them.)
#[test]
fn no_other_layer_covers_the_active_layers_copper() -> TestResult {
    let engine = Engine::showcase()?;
    let project = engine.project();
    let ink = BoardInk::get();
    let colors = crate::palette::board();
    let mut fills_seen = 0;
    for from_bottom in [false, true] {
        for layer in project.stackup.layers() {
            let mut editor = bare(&engine);
            editor.from_bottom = from_bottom;
            // The first frame syncs the stackup; the active layer then
            // stays.
            picture(&mut editor, &engine);
            editor.layer = layer;
            let picture = picture(&mut editor, &engine);
            let view = view_of(&editor, &engine);
            let scale = f64::from(view.view.scale);
            let full = ink.on_board(colors.copper_layer(layer), 1.0);
            let mut checked = 0;
            for track in project.tracks.iter().filter(|t| t.layer == layer) {
                // Only tracks whose middle is a clear pixel inside.
                if track.width / 2.0 * scale < 1.6 {
                    continue;
                }
                for k in 1..20 {
                    let s = f64::from(k) / 20.0;
                    let p = [
                        track.start[0] + (track.end[0] - track.start[0]) * s,
                        track.start[1] + (track.end[1] - track.start[1]) * s,
                    ];
                    if near_drilled(&editor, project, p, 0.3) {
                        continue;
                    }
                    assert_eq!(
                        picture.at(view.screen(p)),
                        Some(full),
                        "{layer:?} track at {p:?} (from bottom: {from_bottom})"
                    );
                    checked += 1;
                }
            }
            assert!(checked > 10, "{layer:?}: {checked} track pixels");
            let step = 2.5 / scale;
            let near = CopperLayer::from(if from_bottom {
                Layer::Bottom
            } else {
                Layer::Top
            });
            for fill in project
                .fills
                .iter()
                .filter(|f| f.layer == layer && layer == near)
            {
                let Some(mesh) = editor.fill_meshes.mesh(fill.id) else {
                    continue;
                };
                let mut x = 0.0;
                while x < project.board_size[0] {
                    let mut y = 0.0;
                    while y < project.board_size[1] {
                        let p = [x, y];
                        let inside = [
                            [0.0, 0.0],
                            [step, 0.0],
                            [-step, 0.0],
                            [0.0, step],
                            [0.0, -step],
                        ]
                        .iter()
                        .all(|d| mesh.contains([p[0] + d[0], p[1] + d[1]]));
                        if inside && !near_drilled(&editor, project, p, 0.3) {
                            assert_eq!(
                                picture.at(view.screen(p)),
                                Some(full),
                                "{layer:?} fill at {p:?} (from bottom: {from_bottom})"
                            );
                            fills_seen += 1;
                        }
                        y += 0.7;
                    }
                    x += 0.7;
                }
            }
        }
    }
    assert!(fills_seen > 1000, "{fills_seen} fill pixels checked");
    Ok(())
}

/// All layers dims the others beneath the active one, their pours a
/// translucent tint; Dim others dims them strongly; Active only draws the
/// active layer alone and picks only its copper. Vias through the active
/// layer show in every mode. One registry command cycles the modes.
#[test]
fn each_focus_mode_paints_what_it_says() -> TestResult {
    let engine = Engine::showcase()?;
    let project = engine.project();
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let ink = BoardInk::get();
    let colors = crate::palette::board();
    let mut editor = bare(&engine);
    frame(&mut editor, &engine, &ctx);
    editor.layer = CopperLayer::Inner(1);
    let others = [CopperLayer::Top, CopperLayer::Inner(2), CopperLayer::Bottom];
    let shade = |l: CopperLayer, s: f32| ink.on_board(colors.copper_layer(l), s);
    let painted = |editor: &mut PcbEditor| -> Vec<Color32> {
        shapes(&frame(editor, &engine, &ctx))
            .iter()
            .flat_map(colors_of)
            .collect()
    };
    let through_active = project
        .vias
        .iter()
        .filter(|v| v.on(CopperLayer::Inner(1), &editor.stack))
        .count();
    assert!(through_active > 0);
    let drills = |all: &[Color32]| all.iter().filter(|c| **c == ink.drill).count();

    // All layers: the far side and the other layers dimmed, pours tinted.
    assert_eq!(editor.focus, LayerFocus::All);
    let all = painted(&mut editor);
    assert!(all.contains(&shade(CopperLayer::Inner(1), 1.0)));
    assert!(all.contains(&shade(CopperLayer::Bottom, FAR_SHADE)));
    assert!(all.contains(&shade(CopperLayer::Top, OTHER_SHADE)));
    assert!(all.contains(&shade(CopperLayer::Inner(2), OTHER_SHADE)));
    let tint = colors
        .copper_layer(CopperLayer::Bottom)
        .gamma_multiply(FILL_TINT);
    assert!(all.contains(&tint), "the bottom pour is a translucent tint");
    for other in others {
        assert!(
            !all.contains(&shade(other, 1.0)),
            "{other:?} at full strength"
        );
    }
    assert!(drills(&all) >= through_active);

    // Dim others, through the registry command's action.
    editor.view_toggle(PcbView::LayerFocus);
    assert_eq!(editor.focus, LayerFocus::Dim);
    let dim = painted(&mut editor);
    assert!(dim.contains(&shade(CopperLayer::Inner(1), 1.0)));
    for other in others {
        assert!(dim.contains(&shade(other, DIM_SHADE)), "{other:?} dimmed");
        for s in [1.0, FAR_SHADE, OTHER_SHADE] {
            assert!(!dim.contains(&shade(other, s)), "{other:?} at {s}");
        }
    }
    let dim_tint = colors
        .copper_layer(CopperLayer::Bottom)
        .gamma_multiply(DIM_FILL_TINT);
    assert!(dim.contains(&dim_tint));

    // Active only: no colour of another layer at any strength.
    let track = project
        .tracks
        .iter()
        .find(|t| t.layer == CopperLayer::Bottom)
        .ok_or("a bottom track")?;
    let mid = [
        (track.start[0] + track.end[0]) / 2.0,
        (track.start[1] + track.end[1]) / 2.0,
    ];
    let metric = TextMetric::default();
    editor.select_at(project, mid, 0.0, metric);
    assert_eq!(editor.track, Some(track.id), "picked under Dim others");
    editor.set_selection(None, None, None);
    editor.view_toggle(PcbView::LayerFocus);
    assert_eq!(editor.focus, LayerFocus::Only);
    let only = painted(&mut editor);
    assert!(only.contains(&shade(CopperLayer::Inner(1), 1.0)));
    for other in others {
        for s in [1.0, FAR_SHADE, OTHER_SHADE, DIM_SHADE] {
            assert!(!only.contains(&shade(other, s)), "{other:?} at {s}");
        }
        for t in [FILL_TINT, DIM_FILL_TINT] {
            assert!(!only.contains(&colors.copper_layer(other).gamma_multiply(t)));
        }
    }
    assert!(drills(&only) >= through_active, "vias through it show");
    editor.select_at(project, mid, 0.0, metric);
    assert_ne!(editor.track, Some(track.id), "a hidden layer's track");

    // The cycle closes; the key is the registry's.
    editor.view_toggle(PcbView::LayerFocus);
    assert_eq!(editor.focus, LayerFocus::All);
    let registry = Registry::standard();
    let command = registry.get("pcb.layer-focus").ok_or("the command")?;
    assert_eq!(
        registry.shortcut(command),
        Some(
            crate::commands::Chord::key(egui::Key::L)
                .with_shift()
                .shown()
        )
    );
    Ok(())
}

fn rectangle(lo: Point, hi: Point) -> Outline {
    Outline {
        start: lo,
        segments: [[hi[0], lo[1]], hi, [lo[0], hi[1]], lo]
            .into_iter()
            .map(|to| Segment::Line { to })
            .collect(),
    }
}

/// Keepouts draw over all copper, hatched, with the layers they apply to,
/// in every focus mode, whichever layer is active.
#[test]
fn keepouts_show_over_all_copper_in_every_mode() -> TestResult {
    let engine = Engine::showcase()?;
    let mut project = engine.project().clone();
    for (name, layers, lo, hi) in [
        (
            "Antenna",
            KeepoutLayers::Only(vec![CopperLayer::Inner(2)]),
            [30.0, 2.0],
            [40.0, 8.0],
        ),
        ("", KeepoutLayers::All, [2.0, 22.0], [8.0, 27.0]),
    ] {
        let id = project.allocate()?;
        project.keepouts.push(Keepout {
            id,
            name: name.into(),
            outline: rectangle(lo, hi),
            layers,
            forbid: KeepoutRules::default(),
        });
    }
    let engine = with_project(&engine, project)?;
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let color = keepout_color();
    let inks = [
        color,
        color.gamma_multiply(KEEPOUT_HATCH_HERE),
        color.gamma_multiply(KEEPOUT_HATCH_OTHER),
    ];
    let colors = crate::palette::board();
    for layer in [CopperLayer::Top, CopperLayer::Inner(2), CopperLayer::Bottom] {
        for focus in LayerFocus::ALL {
            let mut editor = bare(&engine);
            frame(&mut editor, &engine, &ctx);
            editor.layer = layer;
            editor.focus = focus;
            let output = frame(&mut editor, &engine, &ctx);
            let all = shapes(&output);
            let texts: Vec<String> = all
                .iter()
                .filter_map(|s| match s {
                    Shape::Text(t) => Some(t.galley.job.text.clone()),
                    _ => None,
                })
                .collect();
            assert!(texts.iter().any(|t| t == "Antenna · Inner 2"), "{texts:?}");
            assert!(texts.iter().any(|t| t == "Keepout · all layers"));
            let copper = |s: &Shape| {
                let active = BoardInk::get().on_board(colors.copper_layer(layer), 1.0);
                colors_of(s).contains(&active)
            };
            let last_copper = all.iter().rposition(copper).ok_or("active copper")?;
            let keepout_lines = all
                .iter()
                .enumerate()
                .filter(|(_, s)| colors_of(s).iter().any(|c| inks.contains(c)))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            assert!(
                keepout_lines.len() > 20,
                "{layer:?} {focus:?}: outlines and hatching ({})",
                keepout_lines.len()
            );
            assert!(
                keepout_lines.iter().all(|i| *i > last_copper),
                "{layer:?} {focus:?}: over the copper"
            );
        }
    }
    Ok(())
}

/// Whether `p` is clear of every pad, via and track by `margin` mm.
fn clear_of_copper(editor: &PcbEditor, project: &Project, p: Point, margin: f64) -> bool {
    !near_drilled(editor, project, p, margin)
        && project.tracks.iter().all(|t| {
            sparksmith_core::geometry::segment_distance(p, t.start, t.end) > t.width / 2.0 + margin
        })
}

/// A point well inside fill `mesh` and clear of other copper.
fn open_pour(
    editor: &PcbEditor,
    project: &Project,
    mesh: &super::fill_paint::FillMesh,
) -> Option<Point> {
    let [w, h] = project.board_size;
    (1..40)
        .flat_map(|i| (1..40).map(move |j| [w * f64::from(i) / 40.0, h * f64::from(j) / 40.0]))
        .find(|p| {
            [[0.0, 0.0], [0.5, 0.0], [-0.5, 0.0], [0.0, 0.5], [0.0, -0.5]]
                .iter()
                .all(|d| mesh.contains([p[0] + d[0], p[1] + d[1]]))
                && clear_of_copper(editor, project, *p, 1.0)
        })
}

/// Seen from the top with the bottom layer active (as the Fill tool leaves
/// it), the bottom pour is translucent: the top pads over it still read as
/// pads. Highlighting a pour's net outlines it in the selection colour and
/// leaves its copper as it was, so the board is not flooded.
#[test]
fn a_far_active_pour_lets_the_near_side_read() -> TestResult {
    let engine = Engine::showcase()?;
    let project = engine.project();
    let ink = BoardInk::get();
    let colors = crate::palette::board();
    let mut editor = bare(&engine);
    picture(&mut editor, &engine);
    editor.layer = CopperLayer::Bottom;
    let plain = picture(&mut editor, &engine);
    let view = view_of(&editor, &engine);
    let pour = project
        .fills
        .iter()
        .find(|f| f.layer == CopperLayer::Bottom)
        .ok_or("a bottom pour")?;
    let mesh = editor.fill_meshes.mesh(pour.id).ok_or("bottom copper")?;
    let open = open_pour(&editor, project, mesh).ok_or("open pour")?;
    let pour_pixel = plain.at(view.screen(open)).ok_or("on screen")?;
    let full = ink.on_board(colors.copper_layer(CopperLayer::Bottom), 1.0);
    assert_ne!(pour_pixel, full, "the far pour is not opaque");
    // Top surface pads over the bottom copper, sampled off their numbers.
    let mut pads = 0;
    for pad in &editor.board_pads {
        let placed = &pad.placed;
        if placed.layer != Layer::Top || placed.pad.kind.through_hole() {
            continue;
        }
        let b = pad.outline.bounds(0.0);
        let at = [b.min[0] + 0.2 * (b.max[0] - b.min[0]), b.center()[1]];
        // Off the active layer's tracks, which lie on top by design.
        let on_track = project.tracks.iter().any(|t| {
            sparksmith_core::geometry::segment_distance(at, t.start, t.end) < t.width / 2.0 + 0.2
        });
        if on_track
            || !pad.contains(at)
            || !mesh.contains(at)
            || (b.max[0] - b.min[0]) * f64::from(view.view.scale) < 8.0
        {
            continue;
        }
        let pixel = plain.at(view.screen(at)).ok_or("on screen")?;
        assert_ne!(pixel, full, "pad at {at:?} is covered");
        assert_ne!(pixel, pour_pixel, "pad at {at:?} reads apart from the pour");
        pads += 1;
    }
    assert!(pads > 0, "top pads over the bottom pour");

    // Highlight each pour's net with its layer active: its open copper
    // keeps its colour, and its outline is in the selection colour.
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let selected = ink.on_board(colors.selected, 1.0);
    let mut checked = 0;
    for fill in project.fills.iter().filter(|f| f.net.is_some()) {
        let Some(mesh) = editor.fill_meshes.mesh(fill.id) else {
            continue;
        };
        let Some(open) = open_pour(&editor, project, mesh) else {
            continue;
        };
        editor.layer = fill.layer;
        editor.set_selection(None, None, None);
        let before = picture(&mut editor, &engine);
        editor.set_selection(None, None, fill.net);
        let lit = picture(&mut editor, &engine);
        assert_eq!(
            lit.at(view.screen(open)),
            before.at(view.screen(open)),
            "{:?} pour at {open:?} is not flooded",
            fill.layer
        );
        let outlined = shapes(&frame(&mut editor, &engine, &ctx))
            .iter()
            .any(|s| matches!(s, Shape::Path(p) if p.stroke.color == egui::epaint::ColorMode::Solid(selected) && p.closed));
        assert!(outlined, "{:?} pour outlined", fill.layer);
        checked += 1;
    }
    assert!(checked > 0);
    Ok(())
}
