//! Layout copper measured in pixels: the canvas's own egui output,
//! tessellated by egui and filled by a small CPU rasterizer (premultiplied
//! source-over blending, as the window composites it). Tracks joined at
//! 30°, 45° and 90°, crossing tracks and a track over a pad must read as
//! one uniform colour per layer, with copper wherever the round-capped
//! outline is and nowhere else: no gap or notch at a corner, no darker
//! overlap.
//!
//! With `PCBCRAFT_PICTURES` set, `layout_pictures` writes PNGs of the
//! showcase and of the corner board there.
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(super) const SCREEN: [f32; 2] = [900.0, 700.0];

/// An RGBA picture, unpremultiplied bytes, row-major from the top left.
pub(crate) struct Picture {
    pub(super) width: usize,
    pub(super) height: usize,
    pixels: Vec<[f32; 4]>,
}

impl Picture {
    pub(super) fn at(&self, p: Pos2) -> Option<Color32> {
        let (x, y) = (p.x.floor(), p.y.floor());
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.width || y >= self.height {
            return None;
        }
        let [r, g, b, _] = self.pixels[y * self.width + x];
        let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        Some(Color32::from_rgb(byte(r), byte(g), byte(b)))
    }

    pub(crate) fn png(&self) -> Option<Vec<u8>> {
        let bytes: Vec<u8> = self
            .pixels
            .iter()
            .flat_map(|[r, g, b, _]| {
                [*r, *g, *b, 1.0].map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
            })
            .collect();
        pcbcraft_solidcraft::viewport::png(self.width, self.height, bytes)
    }
}

/// Textures egui has uploaded so far, by id.
#[derive(Default)]
struct Textures(std::collections::HashMap<egui::TextureId, egui::ColorImage>);

impl Textures {
    fn apply(&mut self, delta: &egui::TexturesDelta) {
        for (id, image) in delta
            .set
            .iter()
            .flat_map(|(id, d)| d.iter().map(move |i| (id, i)))
        {
            let egui::ImageData::Color(source) = &image.image;
            match image.pos {
                None => {
                    self.0.insert(*id, (**source).clone());
                }
                Some([x0, y0]) => {
                    if let Some(target) = self.0.get_mut(id) {
                        for y in 0..source.size[1] {
                            for x in 0..source.size[0] {
                                let (tx, ty) = (x0 + x, y0 + y);
                                if tx < target.size[0] && ty < target.size[1] {
                                    target.pixels[ty * target.size[0] + tx] =
                                        source.pixels[y * source.size[0] + x];
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Nearest texel at `uv`, premultiplied, as 0–1 floats.
    fn sample(&self, id: egui::TextureId, uv: Pos2) -> [f32; 4] {
        let Some(image) = self.0.get(&id) else {
            return [1.0; 4];
        };
        let [w, h] = image.size;
        let x = ((uv.x * w as f32).floor().max(0.0) as usize).min(w.saturating_sub(1));
        let y = ((uv.y * h as f32).floor().max(0.0) as usize).min(h.saturating_sub(1));
        image
            .pixels
            .get(y * w + x)
            .map_or([1.0; 4], |c| c.to_array().map(|v| f32::from(v) / 255.0))
    }
}

/// Fill one frame's tessellated meshes over `background`.
fn rasterize(
    ctx: &egui::Context,
    output: egui::FullOutput,
    textures: &Textures,
    background: Color32,
    size: [f32; 2],
) -> Picture {
    let (width, height) = (size[0] as usize, size[1] as usize);
    let bg = background.to_array().map(|v| f32::from(v) / 255.0);
    let mut pixels = vec![[bg[0], bg[1], bg[2], 1.0]; width * height];
    for clipped in ctx.tessellate(output.shapes, output.pixels_per_point) {
        let egui::epaint::Primitive::Mesh(mesh) = clipped.primitive else {
            continue;
        };
        let clip = clipped.clip_rect;
        for tri in mesh.indices.chunks_exact(3) {
            let v = [tri[0], tri[1], tri[2]].map(|i| mesh.vertices[i as usize]);
            let [a, b, c] = v.map(|v| v.pos);
            let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            if area.abs() < 1e-9 {
                continue;
            }
            let min_x = a.x.min(b.x).min(c.x).max(clip.min.x).max(0.0).floor() as usize;
            let max_x = a.x.max(b.x).max(c.x).min(clip.max.x).min(size[0]).ceil() as usize;
            let min_y = a.y.min(b.y).min(c.y).max(clip.min.y).max(0.0).floor() as usize;
            let max_y = a.y.max(b.y).max(c.y).min(clip.max.y).min(size[1]).ceil() as usize;
            for y in min_y..max_y.min(height) {
                for x in min_x..max_x.min(width) {
                    let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                    if !clip.contains(p) {
                        continue;
                    }
                    let edge = |p0: Pos2, p1: Pos2| {
                        ((p1.x - p0.x) * (p.y - p0.y) - (p1.y - p0.y) * (p.x - p0.x)) / area
                    };
                    let (wa, wb, wc) = (edge(b, c), edge(c, a), edge(a, b));
                    // Half-open on the shared edges, so touching triangles
                    // of one mesh never fill a pixel twice.
                    if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                        continue;
                    }
                    let colors = v.map(|v| v.color.to_array().map(|c| f32::from(c) / 255.0));
                    let uv = egui::pos2(
                        v[0].uv.x * wa + v[1].uv.x * wb + v[2].uv.x * wc,
                        v[0].uv.y * wa + v[1].uv.y * wb + v[2].uv.y * wc,
                    );
                    let texel = textures.sample(mesh.texture_id, uv);
                    let src: [f32; 4] = std::array::from_fn(|k| {
                        (colors[0][k] * wa + colors[1][k] * wb + colors[2][k] * wc) * texel[k]
                    });
                    let dst = &mut pixels[y * width + x];
                    for k in 0..3 {
                        dst[k] = src[k] + dst[k] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
    Picture {
        width,
        height,
        pixels,
    }
}

/// Draw the canvas twice (the first frame uploads the fonts) and fill it.
pub(super) fn picture(editor: &mut PcbEditor, engine: &Engine) -> Picture {
    picture_of(SCREEN, Tokens::get().viewport_bottom, |ui| {
        editor.canvas(ui, engine);
    })
}

/// Draw `ui` on the test screen over the viewport background. The schematic
/// canvas uses it too.
pub(crate) fn render(draw: impl FnMut(&mut egui::Ui)) -> Picture {
    picture_of(SCREEN, Tokens::get().viewport_bottom, draw)
}

/// Draw `draw` in a `size` screen twice (the first frame uploads the fonts
/// and icons) and fill it over `background`.
pub(crate) fn picture_of(
    size: [f32; 2],
    background: Color32,
    mut draw: impl FnMut(&mut egui::Ui),
) -> Picture {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut textures = Textures::default();
    let mut output = None;
    for _ in 0..3 {
        let mut frame = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                ..Default::default()
            },
            |ui| draw(ui),
        );
        textures.apply(&frame.textures_delta);
        frame.textures_delta.clear();
        output = Some(frame);
    }
    match output {
        Some(output) => rasterize(&ctx, output, &textures, background, size),
        None => Picture {
            width: 0,
            height: 0,
            pixels: vec![],
        },
    }
}

pub(super) fn view_of(editor: &PcbEditor, engine: &Engine) -> BoardView {
    board_view(
        Rect::from_min_size(Pos2::ZERO, egui::vec2(SCREEN[0], SCREEN[1])),
        engine.project().board_size,
        editor.zoom,
        editor.pan,
        editor.from_bottom,
    )
}

/// The demo board, empty of tracks, with `paths` routed on `layer` (one
/// track per leg) on one net.
fn corner_board(layer: CopperLayer, paths: &[&[Point]], width: f64) -> Result<Engine, String> {
    let engine = if layer.side().is_some() {
        Engine::demo()
    } else {
        Engine::showcase()
    }
    .map_err(|e| e.to_string())?;
    let mut project = engine.project().clone();
    project.tracks.clear();
    project.vias.clear();
    project.placements.clear();
    // The showcase's pours would cover the inner layer's tracks.
    project.fills.clear();
    let net = project.nets.first().map(|n| n.id).ok_or("a net")?;
    for path in paths {
        for leg in path.windows(2) {
            let id = project.allocate().map_err(|e| e.to_string())?;
            project.tracks.push(pcbcraft_core::Track {
                id,
                net,
                start: leg[0],
                end: leg[1],
                width,
                layer,
                locked: false,
            });
        }
    }
    with_project(&engine, project).map_err(|e| e.to_string())
}

/// An engine holding `project` in place of `engine`'s document.
pub(super) fn with_project(engine: &Engine, project: Project) -> pcbcraft_core::Result<Engine> {
    let mut file = engine.to_file();
    file.project = project;
    Engine::from_file(file)
}

/// Every pixel at least a pixel inside the copper of `tracks` is `inside`;
/// every pixel at least a pixel outside is not copper (`inside` absent).
/// Returns how many copper pixels were checked.
fn assert_copper(
    picture: &Picture,
    view: BoardView,
    tracks: &[pcbcraft_core::Track],
    inside: Color32,
) -> usize {
    let scale = f64::from(view.view.scale);
    let margin = 1.5 / scale;
    let mut checked = 0;
    for y in 0..picture.height {
        for x in 0..picture.width {
            let p = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
            let world = view.world(p);
            let depth = tracks
                .iter()
                .map(|t| {
                    pcbcraft_core::geometry::segment_distance(world, t.start, t.end) - t.width / 2.0
                })
                .fold(f64::INFINITY, f64::min);
            let Some(color) = picture.at(p) else {
                continue;
            };
            if depth < -margin {
                assert_eq!(color, inside, "copper at {world:?} reads one colour");
                checked += 1;
            } else if depth > margin
                && tracks.iter().any(|t| {
                    pcbcraft_core::geometry::segment_distance(world, t.start, t.end) < t.width * 2.0
                })
            {
                assert_ne!(color, inside, "no copper at {world:?}");
            }
        }
    }
    checked
}

fn framed(engine: &Engine, bounds: [Point; 2]) -> PcbEditor {
    let mut editor = PcbEditor::default();
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(SCREEN[0], SCREEN[1]));
    (editor.zoom, editor.pan) = frame_view(rect, engine.project().board_size, bounds, false);
    editor.layers.assembly = false;
    editor.layers.courtyard = false;
    editor.show_airwires = false;
    editor
}

/// A path turning 30°, 45° and 90° (and back), and two tracks crossing it,
/// on the top layer: one uniform opaque colour, round corners with no gap.
#[test]
fn corners_at_every_angle_are_seamless_and_one_colour() -> TestResult {
    let (deg30, deg45) = (30f64.to_radians(), 45f64.to_radians());
    let leg = 3.0;
    let start = [6.0, 12.0];
    let a = [start[0] + leg, start[1]];
    let b = [a[0] + leg * deg30.cos(), a[1] + leg * deg30.sin()];
    let c = [b[0] + leg * deg30.cos(), b[1] + leg * deg30.sin() + leg];
    let d = [c[0] + leg * deg45.cos(), c[1] + leg * deg45.sin()];
    let e = [d[0] + leg, d[1] - 2.0 * leg];
    let path: &[Point] = &[start, a, b, c, d, e];
    let crossing: &[Point] = &[[8.0, 9.0], [8.0, 16.0]];
    let engine = corner_board(CopperLayer::Top, &[path, crossing], 0.8)?;
    let mut editor = framed(&engine, [[5.0, 8.0], [20.0, 20.0]]);
    let picture = picture(&mut editor, &engine);
    let view = view_of(&editor, &engine);
    let ink = BoardInk::get();
    let top = ink.on_board(crate::palette::board().copper_layer(CopperLayer::Top), 1.0);
    let checked = assert_copper(&picture, view, &engine.project().tracks, top);
    assert!(checked > 1000, "the board is drawn large enough to measure");
    // The outside of every corner is copper out to the track's half-width:
    // butt ends left a wedge-shaped gap there.
    for w in path.windows(3) {
        let (u, v) = (unit(sub(w[1], w[0])), unit(sub(w[2], w[1])));
        let outside = unit([u[0] - v[0], u[1] - v[1]]);
        let at = [w[1][0] + outside[0] * 0.35, w[1][1] + outside[1] * 0.35];
        assert_eq!(
            picture.at(view.screen(at)),
            Some(top),
            "corner at {:?}",
            w[1]
        );
    }
    Ok(())
}

/// A dimmed inner layer's overlaps read as one colour too, and a selected
/// net's joins show no seam in the selection colour.
#[test]
fn inner_and_selected_copper_have_no_darker_overlaps() -> TestResult {
    let path: &[Point] = &[[10.0, 10.0], [14.0, 10.0], [17.0, 13.0], [17.0, 17.0]];
    let crossing: &[Point] = &[[12.0, 7.0], [12.0, 14.0]];
    let engine = corner_board(CopperLayer::Inner(1), &[path, crossing], 0.6)?;
    let mut editor = framed(&engine, [[8.0, 6.0], [19.0, 18.0]]);
    let ink = BoardInk::get();
    let colors = crate::palette::board();
    let inner = ink.on_board(colors.copper_layer(CopperLayer::Inner(1)), OTHER_SHADE);
    let picture1 = picture(&mut editor, &engine);
    let view = view_of(&editor, &engine);
    let tracks = &engine.project().tracks;
    assert!(assert_copper(&picture1, view, tracks, inner) > 1000);
    let net = tracks.first().map(|t| t.net);
    editor.set_selection(None, tracks.first().map(|t| t.id), net);
    let picture2 = picture(&mut editor, &engine);
    let selected = ink.on_board(colors.selected, OTHER_SHADE);
    assert!(assert_copper(&picture2, view, tracks, selected) > 1000);
    Ok(())
}

/// A track ending on a pad of its layer: the pad and the track are one
/// colour where they overlap.
#[test]
fn a_track_over_its_pad_is_one_colour() -> TestResult {
    let engine = Engine::demo()?;
    let project = engine.project();
    let pad = layout::placed_pads(project)
        .into_iter()
        .find(|p| p.layer == Layer::Top && !p.pad.kind.through_hole())
        .ok_or("a top pad")?;
    let c = pad.center();
    let mut project = project.clone();
    project.tracks.clear();
    let net = project.nets.first().map(|n| n.id).ok_or("a net")?;
    let id = project.allocate()?;
    let track = pcbcraft_core::Track {
        id,
        net,
        start: c,
        end: [c[0] + 3.0, c[1] + 1.0],
        width: 0.4,
        layer: CopperLayer::Top,
        locked: false,
    };
    project.tracks.push(track.clone());
    let engine = with_project(&engine, project)?;
    let mut editor = framed(
        &engine,
        [[c[0] - 2.0, c[1] - 2.0], [c[0] + 4.0, c[1] + 3.0]],
    );
    editor.layers.silk_top = false;
    let picture = picture(&mut editor, &engine);
    let view = view_of(&editor, &engine);
    let top = BoardInk::get().on_board(crate::palette::board().copper(Layer::Top), 1.0);
    // Inside the track, including where it lies over the pad (away from
    // the pad number drawn at its centre).
    let mut seen = 0;
    for k in 1..20 {
        let t = 0.15 + 0.85 * f64::from(k) / 20.0;
        let at = [
            track.start[0] + (track.end[0] - track.start[0]) * t * 0.4,
            track.start[1] + (track.end[1] - track.start[1]) * t * 0.4,
        ];
        if pad_number_near(&engine, view, at) {
            continue;
        }
        assert_eq!(picture.at(view.screen(at)), Some(top), "at {at:?}");
        seen += 1;
    }
    assert!(seen > 10);
    Ok(())
}

/// Whether a pad number's text may cover `at` (within 6 points of a pad
/// centre).
fn pad_number_near(engine: &Engine, view: BoardView, at: Point) -> bool {
    layout::placed_pads(engine.project())
        .iter()
        .any(|p| view.screen(p.center()).distance(view.screen(at)) < 6.0)
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn unit(v: Point) -> Point {
    let l = v[0].hypot(v[1]).max(1e-12);
    [v[0] / l, v[1] / l]
}

/// Evidence pictures, written only with `PCBCRAFT_PICTURES` set.
#[test]
fn layout_pictures() -> TestResult {
    let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") else {
        return Ok(());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let engine = Engine::showcase()?;
    let mut editor = PcbEditor::default();
    let board = engine.project().board_size;
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(SCREEN[0], SCREEN[1]));
    let mut shots: Vec<(&str, PcbEditor)> = Vec::new();
    (editor.zoom, editor.pan) = frame_view(rect, board, [[0.0, 0.0], board], false);
    shots.push(("layout-showcase", editor));
    let mut close = PcbEditor::default();
    let middle = [board[0] / 2.0, board[1] / 2.0];
    (close.zoom, close.pan) = frame_view(
        rect,
        board,
        [
            [middle[0] - 8.0, middle[1] - 6.0],
            [middle[0] + 8.0, middle[1] + 6.0],
        ],
        false,
    );
    shots.push(("layout-showcase-zoom", close));
    let mut closer = PcbEditor::default();
    (closer.zoom, closer.pan) = frame_view(
        rect,
        board,
        [
            [middle[0] - 4.0, middle[1] - 3.0],
            [middle[0] + 4.0, middle[1] + 3.0],
        ],
        false,
    );
    shots.push(("layout-showcase-close", closer));
    for (name, mut editor) in shots {
        let picture = picture(&mut editor, &engine);
        let png = picture.png().ok_or("png encoding failed")?;
        std::fs::write(dir.join(format!("{name}.png")), png)?;
    }
    Ok(())
}

/// Evidence of e2e findings 12 and 18, written only with
/// `PCBCRAFT_PICTURES` set: the showcase seen from the top with the bottom
/// layer active (its pour plain and with the pour's net highlighted), and
/// half its parts staged beside the board by Update PCB.
#[test]
fn pour_and_staging_pictures() -> TestResult {
    let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") else {
        return Ok(());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(SCREEN[0], SCREEN[1]));
    let save = |name: &str, editor: &mut PcbEditor, engine: &Engine| -> TestResult {
        let png = picture(editor, engine).png().ok_or("png encoding failed")?;
        std::fs::write(dir.join(format!("{name}.png")), png)?;
        Ok(())
    };
    let engine = Engine::showcase()?;
    let board = engine.project().board_size;
    let mut editor = PcbEditor::default();
    (editor.zoom, editor.pan) = frame_view(rect, board, [[0.0, 0.0], board], false);
    picture(&mut editor, &engine);
    for layer in [CopperLayer::Top, CopperLayer::Bottom] {
        editor.layer = layer;
        let side = if layer == CopperLayer::Top {
            "top"
        } else {
            "bottom"
        };
        editor.set_selection(None, None, None);
        save(&format!("pour-{side}-active"), &mut editor, &engine)?;
        let net = engine
            .project()
            .fills
            .iter()
            .find(|f| f.layer == layer)
            .and_then(|f| f.net);
        editor.set_selection(None, None, net);
        save(&format!("pour-{side}-active-net-lit"), &mut editor, &engine)?;
    }
    let mut project = engine.project().clone();
    project.placements = project.placements.iter().step_by(2).cloned().collect();
    let staged = with_project(&engine, project)?;
    let mut staged = staged;
    staged.apply(
        staged.revision(),
        pcbcraft_core::Operation::Layout(layout::LayoutCommand::Sync),
    )?;
    let boxes = layout::staging::part_boxes(staged.project());
    let all = boxes
        .values()
        .fold(
            pcbcraft_core::geometry::Bounds::point([0.0, 0.0]),
            |a, b| a.union(*b),
        )
        .including(board);
    let mut editor = PcbEditor::default();
    (editor.zoom, editor.pan) = overview(rect, board, [all.min, all.max], false);
    save("update-pcb-staged", &mut editor, &staged)?;
    Ok(())
}

/// Finding 8 of the second UX run: the Route card, with every key shown,
/// is taller than its slot; it stays inside the slot (its rows scroll)
/// instead of rising over the ribbon and the title bar above it.
#[test]
fn the_route_card_stays_inside_its_slot() -> TestResult {
    let engine = Engine::demo()?;
    let mut editor = PcbEditor::default();
    editor.on_tool("route", &engine);
    editor.route.keys_all = true;
    let size = [400.0, 700.0];
    // As in the app, the slot reaches almost to the window's bottom, so a
    // card that outgrew it would be pushed up the window.
    let slot = Rect::from_min_max(Pos2::new(60.0, 200.0), Pos2::new(340.0, 680.0));
    let background = Tokens::get().viewport_bottom;
    let shown = picture_of(size, background, |ui| {
        crate::shell::column_card(
            ui.ctx(),
            "properties",
            slot,
            crate::shell::COLUMN_CARD_INSET,
            |ui| editor.subject_slot(ui, &engine),
        );
    });
    // Drawn: clearly unlike the background (the card's faint shadow is not
    // the card).
    let unlike = |c: Color32| {
        [
            (c.r(), background.r()),
            (c.g(), background.g()),
            (c.b(), background.b()),
        ]
        .iter()
        .any(|(a, b)| a.abs_diff(*b) > 12)
    };
    let painted = |rows: std::ops::Range<usize>, columns: std::ops::Range<usize>| {
        rows.flat_map(|y| columns.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| {
                shown
                    .at(Pos2::new(x as f32 + 0.5, y as f32 + 0.5))
                    .is_some_and(unlike)
            })
            .count()
    };
    let (left, right) = (slot.left() as usize, slot.right() as usize);
    assert!(
        painted(slot.top() as usize..slot.bottom() as usize, left..right) > 1000,
        "the card is drawn in its slot"
    );
    assert_eq!(
        painted(0..slot.top() as usize - 1, 0..size[0] as usize),
        0,
        "nothing is drawn above the slot"
    );
    assert_eq!(
        painted(
            slot.bottom() as usize + 1..size[1] as usize,
            0..size[0] as usize
        ),
        0,
        "nothing is drawn below the slot"
    );
    Ok(())
}
