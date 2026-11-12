//! The Parts and Vias band on its own frames: the ESP32-S31 dev board's
//! parts at the narrowest widths, 10 000 vias virtualised, and selection
//! and hover shared with the Layout canvas without a geometry rebuild.
use super::*;
use crate::kit::list::{ListState, natural_cmp};
use crate::kit::text::tests::{cuts, painted, shows_in_full};
use egui::{FullOutput, Pos2, Rect, pos2, vec2};
use pcbcraft_core::{Engine, Via};
use std::sync::Arc;

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn esp32() -> Fallible<Engine> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    Ok(Engine::from_file(pcbcraft_io::read_project(&path)?)?)
}

/// The canvas above and the band below, as the app lays them out.
struct Frames {
    ctx: egui::Context,
    time: f64,
    width: f32,
}

const CANVAS_H: f32 = 380.0;
const HEIGHT: f32 = 700.0;

impl Frames {
    fn new(width: f32) -> Self {
        let ctx = egui::Context::default();
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        Self {
            ctx,
            time: 0.0,
            width,
        }
    }

    fn band(&self) -> Rect {
        Rect::from_min_max(pos2(0.0, CANVAS_H + 8.0), pos2(self.width, HEIGHT))
    }

    fn frame(
        &mut self,
        editor: &mut PcbEditor,
        engine: &Engine,
        events: Vec<egui::Event>,
    ) -> FullOutput {
        self.time += 0.25;
        let band = self.band();
        let canvas = Rect::from_min_size(Pos2::ZERO, vec2(self.width, CANVAS_H));
        let mut out = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(self.width, HEIGHT))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                editor.begin_hover();
                ui.scope_builder(egui::UiBuilder::new().max_rect(canvas), |ui| {
                    ui.set_clip_rect(canvas);
                    editor.canvas(ui, engine);
                });
                ui.scope_builder(egui::UiBuilder::new().max_rect(band), |ui| {
                    ui.set_clip_rect(band);
                    editor.parts_panel(ui, engine);
                });
            },
        );
        out.textures_delta.clear();
        out
    }

    fn settle(&mut self, editor: &mut PcbEditor, engine: &Engine) -> FullOutput {
        let mut out = self.frame(editor, engine, vec![]);
        for _ in 0..3 {
            out = self.frame(editor, engine, vec![]);
        }
        out
    }

    fn click(&mut self, editor: &mut PcbEditor, engine: &Engine, at: Pos2) -> FullOutput {
        self.frame(editor, engine, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.frame(
                editor,
                engine,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        self.frame(editor, engine, vec![])
    }
}

/// The band's painted texts, top to bottom.
fn band_texts(out: &FullOutput, band: Rect) -> Vec<(String, Rect)> {
    painted(out)
        .into_iter()
        .filter(|(_, rect, _, clip)| band.contains(rect.center()) && clip.intersects(band))
        .map(|(text, rect, ..)| (text, rect))
        .collect()
}

fn designators(engine: &Engine) -> Vec<String> {
    engine
        .project()
        .instances
        .iter()
        .map(|i| i.designator.clone())
        .collect()
}

/// The ESP32 board's parts: every placed instance, in natural designator
/// order, with its value, package and placement as text.
#[test]
fn the_parts_rows_are_in_natural_order_with_value_and_package() -> Fallible {
    let engine = esp32()?;
    let project = engine.project();
    let model = Model::build(project, engine.revision());
    assert_eq!(
        model.parts.len(),
        project.placements.len(),
        "every placed part"
    );
    let refs: Vec<&str> = model
        .parts
        .iter()
        .map(|p| p.id)
        .filter_map(|id| project.instance(id).map(|i| i.designator.as_str()))
        .collect();
    assert!(
        refs.windows(2)
            .all(|w| natural_cmp(w[0], w[1]) != std::cmp::Ordering::Greater),
        "{refs:?}"
    );
    assert!(model.vias.len() == project.vias.len());
    Ok(())
}

/// At the 230 pt Properties width Side, X, Y and Rot leave the row ("+4"),
/// at 420 pt Y and Rot do ("+2"); rows never overlap, and every text cut
/// short shows whole on hover.
#[test]
fn columns_leave_a_narrow_band_with_a_count_and_cut_text_shows_on_hover() -> Fallible {
    let engine = esp32()?;
    for (width, hidden, shown, gone) in [
        (
            230.0,
            "+4",
            &["Ref", "Value", "Package"][..],
            &["Side", "X", "Y", "Rot"][..],
        ),
        (
            420.0,
            "+2",
            &["Ref", "Value", "Package", "Side", "X"][..],
            &["Y", "Rot"][..],
        ),
    ] {
        let mut editor = PcbEditor::default();
        let mut frames = Frames::new(width);
        let out = frames.settle(&mut editor, &engine);
        let texts = band_texts(&out, frames.band());
        let has = |t: &str| texts.iter().any(|(x, _)| x == t);
        assert!(has(hidden), "{width}: {hidden} in {texts:?}");
        for caption in shown {
            assert!(has(caption), "{width}: {caption} shown");
        }
        for caption in gone {
            assert!(!has(caption), "{width}: {caption} left the row");
        }
        // Designator rows 26 pt apart, inside the band.
        let mut ys: Vec<f32> = texts
            .iter()
            .filter(|(t, _)| designators(&engine).contains(t))
            .map(|(_, r)| r.center().y)
            .collect();
        ys.sort_by(f32::total_cmp);
        assert!(ys.len() >= 4, "{width}: rows shown {texts:?}");
        assert!(
            ys.windows(2).all(|w| w[1] - w[0] >= 25.9),
            "{width}: {ys:?}"
        );
        for (_, rect) in &texts {
            assert!(
                rect.right() <= width + 0.5,
                "{width}: painted past the band {rect:?}"
            );
        }
        let band = frames.band();
        for cut in cuts(&out).into_iter().filter(|c| band.contains(c.at)) {
            let mut seen = false;
            frames.frame(
                &mut editor,
                &engine,
                vec![egui::Event::PointerMoved(cut.at)],
            );
            for _ in 0..6 {
                let out = frames.frame(&mut editor, &engine, vec![]);
                seen |= shows_in_full(&out, &cut.text.replace('\u{2026}', ""));
            }
            assert!(
                seen,
                "{width}: {:?} cut without its whole text on hover",
                cut.text
            );
        }
    }
    Ok(())
}

/// 10 000 vias: the rows are built once per revision and a frame builds a
/// screenful plus overscan. The time is a measurement on this machine.
#[test]
fn ten_thousand_vias_build_only_a_screenful() -> Fallible {
    let engine = Engine::demo()?;
    let mut file = engine.to_file();
    let net = file.project.nets.first().ok_or("a net")?.id;
    let [w, h] = file.project.board_size;
    for i in 0..10_000_u32 {
        let id = file.project.allocate()?;
        file.project.vias.push(Via {
            id,
            net,
            position: [
                1.0 + f64::from(i % 100) * (w - 2.0) / 100.0,
                1.0 + f64::from(i / 100) * (h - 2.0) / 100.0,
            ],
            drill: 0.3,
            diameter: 0.6,
            span: pcbcraft_core::ViaSpan::Through,
            tented: true,
            locked: false,
        });
    }
    let engine = Engine::from_file(file)?;
    let mut editor = PcbEditor::default();
    editor.table_tab(Tab::Vias);
    let mut frames = Frames::new(600.0);
    let started = std::time::Instant::now();
    let out = frames.frame(&mut editor, &engine, vec![]);
    let first = started.elapsed();
    let started = std::time::Instant::now();
    frames.frame(&mut editor, &engine, vec![]);
    let next = started.elapsed();
    eprintln!(
        "10 000 vias (measured on this machine, not a gate): first frame {first:?} (model and columns built), next frame {next:?}"
    );
    let rows = band_texts(&out, frames.band())
        .iter()
        .filter(|(t, _)| t == "Through")
        .count();
    let screen = (frames.band().height() / 26.0).ceil() as usize;
    assert!(rows >= 3, "rows shown: {rows}");
    assert!(
        rows <= screen + 2 * crate::kit::list::virt::OVERSCAN,
        "built {rows} rows for a {screen}-row band"
    );
    let count = format!("Vias {}", engine.project().vias.len());
    assert!(
        band_texts(&out, frames.band())
            .iter()
            .any(|(t, _)| *t == count),
        "the Vias tab counts them: {count}"
    );
    Ok(())
}

/// A row click selects the part as a canvas click does; a canvas selection
/// selects its row; hovering a row lights the part on the canvas and
/// hovering the part lights the row. Neither rebuilds the board.
#[test]
fn selection_and_hover_cross_the_band_and_canvas_without_a_rebuild() -> Fallible {
    let engine = esp32()?;
    let project = engine.project();
    let mut editor = PcbEditor::default();
    let mut frames = Frames::new(700.0);
    let out = frames.settle(&mut editor, &engine);
    let graph = Arc::clone(&editor.graph);
    let revision = editor.cached_revision;
    let texts = band_texts(&out, frames.band());
    let first = editor
        .table
        .model
        .parts
        .first()
        .map(|p| p.id)
        .ok_or("a part row")?;
    let reference = project
        .instance(first)
        .ok_or("instance")?
        .designator
        .clone();
    let row = texts
        .iter()
        .find(|(t, _)| *t == reference)
        .ok_or("the first row")?
        .1;
    // A row click selects the part.
    frames.click(&mut editor, &engine, row.center());
    assert_eq!(editor.selected_instance(), Some(first));
    // Hovering the row lights the part on the canvas (next frame).
    frames.frame(
        &mut editor,
        &engine,
        vec![egui::Event::PointerMoved(row.center())],
    );
    assert_eq!(
        editor.hover.elsewhere(HoverFrom::Board),
        Some(Pick::Part(first))
    );
    assert!(editor.lit().parts.contains(&first), "3D lights it too");
    // A canvas selection selects its row.
    let other = project
        .placements
        .iter()
        .map(|p| p.instance)
        .find(|id| *id != first)
        .ok_or("a second part")?;
    editor.set_selection(Some(other), None, None);
    frames.frame(
        &mut editor,
        &engine,
        vec![egui::Event::PointerMoved(pos2(5.0, 5.0))],
    );
    let state = ListState::load(
        &frames.ctx,
        egui::Id::unique(("pcb-parts-table", Tab::Parts)),
    );
    assert_eq!(
        state.selection,
        [crate::kit::list::RowKey::of(("part", other))].into(),
        "the canvas selection selected its row"
    );
    // The pointer on a part on the canvas lights its row.
    let canvas = Rect::from_min_size(Pos2::ZERO, vec2(frames.width, CANVAS_H));
    let view = crate::pcb_editor::board_view(
        canvas,
        project.board_size,
        editor.zoom,
        editor.pan,
        editor.from_bottom,
    );
    let at = view.screen(project.placement_of(other).ok_or("placed")?.position);
    frames.frame(&mut editor, &engine, vec![egui::Event::PointerMoved(at)]);
    assert!(
        matches!(
            editor.hover.elsewhere(HoverFrom::Table),
            Some(Pick::Part(_))
        ),
        "the canvas hover is the band's cross-highlight"
    );
    assert!(
        Arc::ptr_eq(&graph, &editor.graph),
        "the copper graph was not rebuilt"
    );
    assert_eq!(revision, editor.cached_revision);
    Ok(())
}
