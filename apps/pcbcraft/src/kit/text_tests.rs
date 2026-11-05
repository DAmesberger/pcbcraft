//! Text that may not fit, checked as a person sees it: every text a frame
//! paints cut short is hovered, and the full text must then show in a
//! tooltip that closes when the pointer leaves. The kit's widgets are
//! checked here, narrow enough to cut; feature cards call
//! [`cuts_without_hover`] from their own tests. [`png`] turns a frame into
//! a picture for a person to look at (`write_text_pictures`).
use super::*;
use crate::kit::issues::{IssueGroupView, IssueRowView};
use crate::kit::rows::tests::Harness;
use crate::kit::severity::Level;
use crate::kit::*;
use egui::epaint::{ClippedPrimitive, Primitive};
use egui::{Event, FullOutput, Pos2, Shape};
use solidcraft_ui_egui::theme::Tokens;
use std::collections::HashMap;

/// A text painted cut short: its full text and where it shows.
#[derive(Clone, Debug)]
pub(crate) struct Cut {
    pub text: String,
    pub at: Pos2,
}

fn walk(shape: &Shape, clip: Rect, out: &mut Vec<(String, Rect, bool, Rect)>) {
    match shape {
        Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, clip, out)),
        Shape::Text(t) => out.push((
            t.galley.text().to_owned(),
            t.galley.rect.translate(t.pos.to_vec2()),
            t.galley.elided,
            clip,
        )),
        _ => {}
    }
}

fn painted(output: &FullOutput) -> Vec<(String, Rect, bool, Rect)> {
    let mut out = Vec::new();
    for shape in &output.shapes {
        walk(&shape.shape, shape.clip_rect, &mut out);
    }
    out
}

/// The texts `output` paints cut short where they can be seen.
pub(crate) fn cuts(output: &FullOutput) -> Vec<Cut> {
    let mut out: Vec<Cut> = Vec::new();
    for (text, rect, elided, clip) in painted(output) {
        let seen = rect.intersect(clip);
        if elided && seen.is_positive() && !out.iter().any(|c| c.text == text) {
            out.push(Cut {
                text,
                at: seen.center(),
            });
        }
    }
    out
}

/// Whether `output` shows `text` in full: a painted text, not cut, that
/// contains it (a tooltip).
pub(crate) fn shows_in_full(output: &FullOutput, text: &str) -> bool {
    painted(output)
        .iter()
        .any(|(t, _, elided, _)| !elided && t.contains(text))
}

/// Tooltips show at once in these tests, as after the pointer rests.
pub(crate) fn tooltips_at_once(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.interaction.tooltip_delay = 0.0;
        s.interaction.show_tooltips_only_when_still = false;
    });
}

/// Hover every text cut short at rest and return those whose full text
/// did not then show, with how many were cut. `run` draws one frame with
/// the given events.
pub(crate) fn cuts_without_hover(
    ctx: &egui::Context,
    mut run: impl FnMut(Vec<Event>) -> FullOutput,
) -> (usize, Vec<String>) {
    tooltips_at_once(ctx);
    run(vec![Event::PointerGone]);
    let rest = run(vec![]);
    let found = cuts(&rest);
    let mut missing = Vec::new();
    for cut in &found {
        run(vec![Event::PointerMoved(cut.at)]);
        run(vec![]);
        let hovered = run(vec![]);
        if !shows_in_full(&hovered, &cut.text) {
            missing.push(cut.text.clone());
        }
    }
    run(vec![Event::PointerGone]);
    (found.len(), missing)
}

/// Every kit widget, given long text in a narrow column.
fn widgets(ui: &mut Ui) {
    let long = "Terminal ‘RESET’ has no pin in symbol ‘Temp sensor LM75B symbol’ draft";
    form(ui, |ui| {
        inspector_header(
            ui,
            "component",
            "Temp sensor LM75B with a long name",
            "Component",
        );
        card_title(ui, "Browser of this project", "12 instances, 3 sheets");
        section_with_summary(ui);
        row(ui, "A label far too long for its column", |ui| {
            ui.label("value");
        });
        value_row(ui, "Body L × W × H", "8.65 × 6 × 1.625 mm");
        value_row(ui, "Result", &long.repeat(3));
        summary_row(
            ui,
            "VBUS: 4 pads unrouted · waived: prototype",
            Some("Restore"),
        );
        let field = ui.label("field");
        field_error(ui, &field, long, true);
        refusal(ui, long);
        hint(ui, &long.repeat(5), "");
        tree_row(
            ui,
            egui::Id::new("tree"),
            &Row {
                depth: 1,
                icon: "component",
                label: "Chip capacitor 0603 (1608 metric)",
                secondary: "r1 · draft",
                ..Default::default()
            },
        );
        pick_row(
            ui,
            egui::Id::new("pick"),
            &PickRow {
                level: Level::Error,
                title: "U2 pin RESET unconnected: mark it no-connect",
                tip: "Route the connection",
                depth: 1,
                fold: None,
                count: None,
                tick: Some(Tick::Off),
                action: Some("Waive…"),
            },
        );
        table(
            ui,
            &[
                TableColumn {
                    title: "A very long caption",
                    weight: 1.0,
                },
                TableColumn {
                    title: "Value",
                    weight: 1.0,
                },
            ],
            1,
            |ui, _, _| {
                ui.label("x");
            },
            None,
        );
    });
}

fn section_with_summary(ui: &mut Ui) {
    // Folded, so its summary shows beside the caption.
    let id = ui.make_persistent_id(("section", "folded-summary"));
    ui.ctx().data_mut(|d| d.insert_persisted(id, false));
    summary_section(
        ui,
        "folded-summary",
        "Placement",
        false,
        "Held 2 mm from the bottom edge, rotated 90°",
        |_| {},
    );
}

#[test]
fn every_kit_text_cut_short_shows_in_full_on_hover() {
    let mut harness = Harness::new(220.0, 1400.0);
    let ctx = harness.ctx().clone();
    let (cut, missing) = cuts_without_hover(&ctx, |events| harness.output(events, widgets));
    assert!(cut >= 8, "the column is narrow enough to cut: {cut}");
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");
}

#[test]
fn a_dialog_header_and_status_and_a_tool_card_name_show_in_full_on_hover() {
    let mut harness = Harness::new(200.0, 500.0);
    let ctx = harness.ctx().clone();
    let (cut, missing) = cuts_without_hover(&ctx, |events| {
        harness.output(events, |ui| {
            let panel = dialog::DialogPanel {
                kind: "Output",
                status: Some("Step 3 of 17 · writing the bottom copper layer"),
                ..dialog::DialogPanel::new(
                    egui::Id::new("d"),
                    "export",
                    "Production files for the board",
                    "Export",
                )
            };
            ui.allocate_ui(egui::vec2(200.0, 220.0), |ui| {
                panel.show(ui, |ui| {
                    ui.label("body");
                });
            });
            tool_card(
                ui,
                &ToolCard {
                    icon: "polygon",
                    name: "Mounting hole pattern",
                    key: "H",
                    hint: "",
                    tip: "",
                    finish: None,
                },
                |_| {},
            );
        })
    });
    assert!(cut >= 3, "{cut}");
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");
}

#[test]
fn issue_rows_and_groups_show_their_whole_title_on_hover() {
    let mut harness = Harness::new(200.0, 400.0);
    let ctx = harness.ctx().clone();
    let rows: Vec<IssueRowView> = (1..=3)
        .map(|n| IssueRowView {
            level: Level::Error,
            title: format!("Terminal ‘{n}’ has no pin in symbol ‘Temp sensor LM75B’"),
            count: 1,
            fix: None,
            open: false,
            detail: Default::default(),
            objects: Vec::new(),
        })
        .collect();
    let group = IssueGroupView {
        level: Level::Error,
        title: "Terminals without a pin on this symbol".into(),
        rows: rows.len(),
        open: true,
        fix: None,
    };
    let (cut, missing) = cuts_without_hover(&ctx, |events| {
        harness.output(events, |ui| {
            issues_list(
                ui,
                egui::Id::new("issues"),
                std::slice::from_ref(&group),
                |_, r| rows.get(r).cloned().unwrap_or_else(|| rows[0].clone()),
            );
        })
    });
    assert!(cut >= 3, "{cut}");
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");
}

#[test]
fn hints_and_values_wrap_where_there_is_room() {
    let mut harness = Harness::new(240.0, 600.0);
    let hint_text = "Open a component, or start a new one, on the start page";
    let keys = "End the route (or right-click); again: Select";
    let painted = harness.frame(vec![], |ui| {
        form(ui, |ui| {
            hint(ui, hint_text, "Search finds this project's components");
            value_row(ui, "Esc", keys);
        });
    });
    for text in [hint_text, keys] {
        let p = painted.iter().find(|p| p.text == text);
        assert!(
            p.is_some_and(|p| !p.elided && p.rows > 1 && p.rect.right() <= 240.5),
            "{text} wraps inside the column: {p:?}"
        );
    }
}

#[test]
fn a_tooltip_closes_when_the_pointer_leaves_and_never_stacks() -> Result<(), String> {
    let mut harness = Harness::new(220.0, 300.0);
    let ctx = harness.ctx().clone();
    tooltips_at_once(&ctx);
    let title = "VBUS: 4 pads unrouted, from J1 pin 1 to U1 pin 3";
    let mut draw = |ui: &mut Ui| {
        form(ui, |ui| {
            pick_row(
                ui,
                egui::Id::new("pick"),
                &PickRow {
                    level: Level::Error,
                    title,
                    tip: "Route the connection",
                    depth: 0,
                    fold: None,
                    count: None,
                    tick: Some(Tick::Off),
                    action: Some("Waive…"),
                },
            );
            ui.add_space(120.0);
        });
    };
    let rest = harness.output(vec![], &mut draw);
    let at = cuts(&rest)
        .into_iter()
        .find(|c| c.text == title)
        .map(|c| c.at);
    let at = at.ok_or("the title is cut in a narrow row")?;
    harness.output(vec![Event::PointerMoved(at)], &mut draw);
    let shown = harness.output(vec![], &mut draw);
    let full = format!("{title}\nRoute the connection");
    assert!(
        shows_in_full(&shown, &full),
        "one tooltip: title, then the fix"
    );
    let tooltips = painted(&shown)
        .iter()
        .filter(|(t, _, elided, _)| !elided && t.contains("Route the connection"))
        .count();
    assert_eq!(tooltips, 1, "tooltips never stack");
    // The pointer leaves the row for empty space: the tooltip closes.
    harness.output(
        vec![Event::PointerMoved(Pos2::new(110.0, 250.0))],
        &mut draw,
    );
    let left = harness.output(vec![], &mut draw);
    assert!(!shows_in_full(&left, title), "the tooltip closed");
    // The pointer leaves the window: nothing stays either.
    harness.output(vec![Event::PointerMoved(at)], &mut draw);
    harness.output(vec![], &mut draw);
    harness.output(vec![Event::PointerGone], &mut draw);
    let gone = harness.output(vec![], &mut draw);
    assert!(!shows_in_full(&gone, title), "the tooltip closed");
    Ok(())
}

/// Fill egui's tessellated meshes into an RGBA image at `ppp` pixels per
/// point over `page` (adapted from the schematic canvas pictures).
fn rasterize(
    primitives: &[ClippedPrimitive],
    textures: &HashMap<egui::TextureId, egui::ColorImage>,
    (w, h): (usize, usize),
    ppp: f32,
    page: egui::Color32,
) -> Vec<u8> {
    let bg = [page.r(), page.g(), page.b()].map(|c| f32::from(c) / 255.0);
    let mut px = vec![[bg[0], bg[1], bg[2], 1.0_f32]; w * h];
    for p in primitives {
        let Primitive::Mesh(mesh) = &p.primitive else {
            continue;
        };
        let clip = p.clip_rect;
        let (cx0, cy0) = ((clip.left() * ppp).max(0.0), (clip.top() * ppp).max(0.0));
        let (cx1, cy1) = (clip.right() * ppp, clip.bottom() * ppp);
        let texture = textures.get(&mesh.texture_id);
        let sample = |uv: Pos2| -> [f32; 4] {
            let Some(t) = texture else {
                return [1.0; 4];
            };
            let [tw, th] = t.size;
            let x = ((uv.x * tw as f32) as usize).min(tw.saturating_sub(1));
            let y = ((uv.y * th as f32) as usize).min(th.saturating_sub(1));
            let c = t.pixels[y * tw + x];
            [c.r(), c.g(), c.b(), c.a()].map(|v| f32::from(v) / 255.0)
        };
        for tri in mesh.indices.chunks_exact(3) {
            let v: Vec<&egui::epaint::Vertex> = tri
                .iter()
                .filter_map(|i| mesh.vertices.get(*i as usize))
                .collect();
            if v.len() != 3 {
                continue;
            }
            let q = |i: usize| (v[i].pos.x * ppp, v[i].pos.y * ppp);
            let (a, b, c) = (q(0), q(1), q(2));
            let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
            if area.abs() < 1e-9 {
                continue;
            }
            let x0 = a.0.min(b.0).min(c.0).max(cx0).floor().max(0.0) as usize;
            let x1 = (a.0.max(b.0).max(c.0).min(cx1).ceil().max(0.0) as usize).min(w);
            let y0 = a.1.min(b.1).min(c.1).max(cy0).floor().max(0.0) as usize;
            let y1 = (a.1.max(b.1).max(c.1).min(cy1).ceil().max(0.0) as usize).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    let (sx, sy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((b.0 - sx) * (c.1 - sy) - (b.1 - sy) * (c.0 - sx)) / area;
                    let w1 = ((c.0 - sx) * (a.1 - sy) - (c.1 - sy) * (a.0 - sx)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let col = |k: usize| -> [f32; 4] {
                        let c = v[k].color;
                        [c.r(), c.g(), c.b(), c.a()].map(|v| f32::from(v) / 255.0)
                    };
                    let (c0, c1, c2) = (col(0), col(1), col(2));
                    let uv = egui::pos2(
                        v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2,
                        v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2,
                    );
                    let t = sample(uv);
                    let dst = &mut px[y * w + x];
                    let src: [f32; 4] =
                        std::array::from_fn(|i| (c0[i] * w0 + c1[i] * w1 + c2[i] * w2) * t[i]);
                    for i in 0..4 {
                        dst[i] = src[i] + dst[i] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
    px.iter()
        .flat_map(|p| [p[0], p[1], p[2], 1.0].map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8))
        .collect()
}

/// Keeps egui's font atlas between frames, so a frame can be pictured.
#[derive(Default)]
pub(crate) struct Atlas(HashMap<egui::TextureId, egui::ColorImage>);

impl Atlas {
    /// Take the texture updates of `output` (call it for every frame).
    pub(crate) fn take(&mut self, output: &mut FullOutput) {
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                let egui::ImageData::Color(image) = &delta.image;
                let image: &egui::ColorImage = image;
                match delta.pos {
                    None => {
                        self.0.insert(*id, image.clone());
                    }
                    Some([x0, y0]) => {
                        if let Some(t) = self.0.get_mut(id) {
                            for y in 0..image.size[1] {
                                for x in 0..image.size[0] {
                                    let at = (y0 + y) * t.size[0] + x0 + x;
                                    if let Some(d) = t.pixels.get_mut(at) {
                                        *d = image.pixels[y * image.size[0] + x];
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        output.textures_delta.clear();
    }

    /// `output` as PNG bytes, `size` points at `ppp` pixels per point.
    pub(crate) fn png(
        &self,
        ctx: &egui::Context,
        output: &FullOutput,
        size: egui::Vec2,
        ppp: f32,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let primitives = ctx.tessellate(output.shapes.clone(), ppp);
        let (w, h) = ((size.x * ppp) as usize, (size.y * ppp) as usize);
        let rgba = rasterize(&primitives, &self.0, (w, h), ppp, Tokens::get().panel);
        Ok(pcbcraft_solidcraft::render::png(w, h, rgba).ok_or("png encoding failed")?)
    }
}

/// A headless screen of `size` points for a card's text tests: each frame
/// keeps egui's font atlas, so any frame can be pictured.
pub(crate) struct Screen {
    pub ctx: egui::Context,
    pub size: egui::Vec2,
    atlas: std::cell::RefCell<Atlas>,
}

impl Screen {
    pub(crate) fn new(size: egui::Vec2) -> Self {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        Self {
            ctx,
            size,
            atlas: Default::default(),
        }
    }

    /// One frame with `events`; `add` draws into the screen's root Ui.
    pub(crate) fn run(&self, events: Vec<Event>, add: impl FnMut(&mut Ui)) -> FullOutput {
        let mut out = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                events,
                ..Default::default()
            },
            add,
        );
        self.atlas.borrow_mut().take(&mut out);
        out
    }

    /// With `PCBCRAFT_TEXT_PICTURES=<dir>`, write `output` there as
    /// `<name>.png` at `ppp` pixels per point.
    pub(crate) fn picture(
        &self,
        name: &str,
        output: &FullOutput,
        ppp: f32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(dir) = std::env::var_os("PCBCRAFT_TEXT_PICTURES") {
            let png = self.atlas.borrow().png(&self.ctx, output, self.size, ppp)?;
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png)?;
        }
        Ok(())
    }
}

/// The kit's widgets at rest and with a cut row hovered; with
/// `PCBCRAFT_TEXT_PICTURES=<dir>` it writes both pictures.
#[test]
fn the_kit_widgets_picture() -> Result<(), Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("PCBCRAFT_TEXT_PICTURES") else {
        return Ok(());
    };
    let size = egui::vec2(220.0, 900.0);
    let mut harness = Harness::new(size.x, size.y);
    let ctx = harness.ctx().clone();
    tooltips_at_once(&ctx);
    harness.output(vec![], widgets);
    let rest = harness.output(vec![], widgets);
    let write = |name: &str,
                 harness: &Harness,
                 out: &FullOutput|
     -> Result<(), Box<dyn std::error::Error>> {
        let png = harness.atlas.png(&ctx, out, size, 2.0)?;
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.png")), png)?;
        Ok(())
    };
    write("kit-at-rest", &harness, &rest)?;
    let at = cuts(&rest)
        .into_iter()
        .find(|c| c.text.starts_with("U2 pin RESET"))
        .map(|c| c.at)
        .ok_or("the pick row is cut")?;
    harness.output(vec![Event::PointerMoved(at)], widgets);
    let hovered = harness.output(vec![], widgets);
    write("kit-hover-pick-row", &harness, &hovered)?;
    Ok(())
}
