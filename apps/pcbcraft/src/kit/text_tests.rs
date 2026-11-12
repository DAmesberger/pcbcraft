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
use pcbcraft_ui::theme::Tokens;
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

pub(crate) fn painted(output: &FullOutput) -> Vec<(String, Rect, bool, Rect)> {
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
        // A field not being edited: its long text ends in "…".
        let mut project = "ESP32-S31 sensor development board, long name".to_owned();
        text_row(ui, "Project", &mut project);
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
            egui::Id::unique("tree"),
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
            egui::Id::unique("pick"),
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
        // A list row whose name runs past two lines, whose driven value
        // and plain value are too long for their column.
        let parts = [
            "Top-front edge of the shell of the USB-C receptacle",
            "bottom edge of the board outline",
        ];
        let mut row = ItemRow::new(&parts, 90.0);
        row.action = Some(RowAction::delete("Delete dimension"));
        row.badge = Some((Level::Warning, "Inexact"));
        let mut driven = "board_width / 2 - connector_offset".to_owned();
        item_row(ui, &row, |ui| {
            value_field(ui, "driven", &mut driven, false, Some("12.3456 mm"))
        });
        let mut plain = "123456.789012 mm".to_owned();
        item_row(ui, &ItemRow::new(&["Pad 1"], 60.0), |ui| {
            value_field(ui, "plain", &mut plain, false, None)
        });
        item_row(ui, &ItemRow::new(&["Pad 2"], 60.0), |ui| {
            value_text(ui, "Unrouted on two layers")
        });
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
                    egui::Id::unique("d"),
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
                egui::Id::unique("issues"),
                std::slice::from_ref(&group),
                |_, r| rows.get(r).cloned().unwrap_or_else(|| rows[0].clone()),
            );
        })
    });
    assert!(cut >= 3, "{cut}");
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");
}

/// In a column so narrow that a hovered row's action covers where its
/// cut title was, the full title still shows on hover.
#[test]
fn a_row_action_over_the_cut_title_still_shows_it_in_full() -> Result<(), String> {
    let mut harness = Harness::new(150.0, 300.0);
    tooltips_at_once(harness.ctx());
    let title = "J1 ↔ R4 courtyards overlap on the top side";
    let draw = |ui: &mut Ui| {
        form(ui, |ui| {
            pick_row(
                ui,
                egui::Id::unique("narrow"),
                &PickRow {
                    level: Level::Error,
                    title,
                    tip: "",
                    depth: 1,
                    fold: None,
                    count: None,
                    tick: Some(Tick::Off),
                    action: Some("Waive…"),
                },
            );
        });
    };
    let rest = harness.output(vec![], draw);
    let at = cuts(&rest)
        .into_iter()
        .find(|c| c.text == title)
        .map(|c| c.at)
        .ok_or("the title is cut at rest")?;
    // The pointer rests where the cut title was; Waive… now sits there.
    harness.output(vec![Event::PointerMoved(at)], draw);
    let hovered = harness.output(vec![], draw);
    let waive = painted(&hovered)
        .into_iter()
        .find(|(t, ..)| t == "Waive…")
        .map(|(_, rect, ..)| rect)
        .ok_or("Waive… shows on hover")?;
    assert!(waive.expand(8.0).contains(at), "{waive:?} covers {at:?}");
    // One tooltip per window: the title's own closes, then Waive… shows
    // the whole title, and keeps it while the pointer rests.
    let frames: Vec<bool> = (0..4)
        .map(|_| shows_in_full(&harness.output(vec![], draw), title))
        .collect();
    assert_eq!(frames[2..], [true, true], "{frames:?}");
    Ok(())
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
                egui::Id::unique("pick"),
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
        Ok(pcbcraft_solidcraft::viewport::png(w, h, rgba).ok_or("png encoding failed")?)
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
        pcbcraft_ui::fonts::setup_bundled_fonts(&ctx);
        // The icons the app installs at startup (`App::new`).
        pcbcraft_ui::icon_svg::install();
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

    /// `output` rasterized at one pixel per point, as RGBA rows from the
    /// top left.
    pub(crate) fn rgba(&self, output: &FullOutput) -> Vec<u8> {
        let primitives = self.ctx.tessellate(output.shapes.clone(), 1.0);
        let size = (self.size.x as usize, self.size.y as usize);
        rasterize(
            &primitives,
            &self.atlas.borrow().0,
            size,
            1.0,
            Tokens::get().panel,
        )
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

/// Finding 9 of the second UX run: a long tooltip near the window's right
/// edge wraps inside the window instead of running off it, in a window as
/// wide as the run's and in one narrower than egui's tooltip width.
#[test]
fn a_tooltip_at_the_window_edge_stays_inside_the_window() -> Result<(), String> {
    let title = "Function ‘PA05’ of terminal ‘PA05’: bidirectional with digital \
                 signal and no drive";
    let tip = "Set the Drive of the function (push-pull, open drain or open source) \
               in the Function card, or change its direction";
    for (width, column) in [(1380.0, 240.0), (300.0, 200.0)] {
        let mut harness = Harness::new(width, 400.0);
        let ctx = harness.ctx().clone();
        tooltips_at_once(&ctx);
        let mut draw = |ui: &mut Ui| {
            // As the app does at the start of every frame.
            fit_tooltips(ui.ctx());
            let rect =
                Rect::from_min_size(Pos2::new(width - column, 40.0), egui::vec2(column, 300.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            form(&mut child, |ui| {
                pick_row(
                    ui,
                    egui::Id::unique("pick"),
                    &PickRow {
                        level: Level::Error,
                        title,
                        tip,
                        depth: 0,
                        fold: None,
                        count: None,
                        tick: None,
                        action: None,
                    },
                );
            });
        };
        let rest = harness.output(vec![], &mut draw);
        let at = cuts(&rest)
            .into_iter()
            .find(|c| c.text == title)
            .map(|c| c.at)
            .ok_or("the title is cut in the column")?;
        harness.output(vec![Event::PointerMoved(at)], &mut draw);
        harness.output(vec![], &mut draw);
        let shown = harness.output(vec![], &mut draw);
        let tooltip = painted(&shown)
            .into_iter()
            .find(|(t, _, elided, _)| !elided && t.contains(tip))
            .ok_or("no tooltip")?;
        let (_, rect, _, clip) = tooltip;
        assert!(
            rect.left() >= 0.0 && rect.right() <= width + 0.5,
            "{width}: the tooltip runs off the window: {rect:?}"
        );
        assert!(
            clip.contains_rect(rect.shrink(0.5)),
            "{width}: the tooltip is clipped: {rect:?} in {clip:?}"
        );
    }
    Ok(())
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

/// The strongest contrast with `page` of any pixel of `rect` in an RGBA
/// frame: how much its ink stands out, in either theme.
fn strongest_ink(rgba: &[u8], width: usize, rect: Rect, page: Color32) -> f32 {
    let mut best = 1.0_f32;
    for y in rect.top() as usize..rect.bottom() as usize {
        for x in rect.left() as usize..rect.right() as usize {
            let at = (y * width + x) * 4;
            if let Some(px) = rgba.get(at..at + 3) {
                let ink = Color32::from_rgb(px[0], px[1], px[2]);
                best = best.max(contrast(ink, page));
            }
        }
    }
    best
}

/// F5: a command that cannot run reads as off in both themes: its name,
/// reason and key all stand out less than a command's that can, measured
/// on the raster.
#[test]
fn an_unavailable_command_row_is_dimmed_to_the_key() {
    for dark in [false, true] {
        pcbcraft_ui::theme::set_dark(dark);
        let screen = Screen::new(egui::vec2(420.0, 90.0));
        let row = |ui: &mut Ui, name: &str, enabled: bool| {
            let row = CommandRow {
                icon: "plane",
                label: name,
                query: "",
                detail: "PCB",
                shortcut: Some("D"),
                enabled,
                highlighted: false,
            };
            command_row(ui, egui::Id::unique(name), &row).rect
        };
        let mut rects = Vec::new();
        let mut draw = |ui: &mut Ui| {
            rects = vec![row(ui, "Available", true), row(ui, "Unavailable", false)];
        };
        screen.run(vec![], &mut draw);
        let output = screen.run(vec![], &mut draw);
        let rgba = screen.rgba(&output);
        let page = Tokens::get().panel;
        let ink = |rect: Rect| strongest_ink(&rgba, screen.size.x as usize, rect, page);
        let name = |rect: Rect| {
            Rect::from_min_max(
                rect.min + egui::vec2(50.0, 0.0),
                egui::pos2(rect.right() - 40.0, rect.center().y),
            )
        };
        let detail = |rect: Rect| {
            Rect::from_min_max(
                egui::pos2(rect.left() + 50.0, rect.center().y),
                egui::pos2(rect.right() - 40.0, rect.bottom()),
            )
        };
        let key = |rect: Rect| {
            Rect::from_min_max(egui::pos2(rect.right() - 24.0, rect.top()), rect.max).shrink(8.0)
        };
        let (on, off) = (rects[0], rects[1]);
        for (part, on, off) in [
            ("name", ink(name(on)), ink(name(off))),
            ("detail", ink(detail(on)), ink(detail(off))),
            ("key", ink(key(on)), ink(key(off))),
        ] {
            assert!(off < on * 0.9, "dark {dark}, {part}: {off} against {on}");
            assert!(off > 1.3, "dark {dark}, {part} still shows: {off}");
        }
    }
}

/// F8: hovered, a group title keeps its lines: "Waive all…" takes its room
/// by cutting a line with "…", never by wrapping in the middle of a word,
/// and the row stays as tall. The whole title then shows on hover.
#[test]
fn a_hovered_group_title_keeps_its_lines() {
    let mut harness = Harness::new(190.0, 120.0);
    let title = "Unconnected terminals and more";
    let height = std::cell::Cell::new(0.0_f32);
    let mut draw = |ui: &mut Ui| {
        let before = ui.cursor().top();
        pick_row(
            ui,
            egui::Id::unique("group"),
            &PickRow {
                level: Level::Error,
                title,
                tip: "",
                depth: 0,
                fold: Some(false),
                count: Some(3),
                tick: Some(Tick::Off),
                action: Some("Waive all…"),
            },
        );
        height.set(ui.cursor().top() - before);
    };
    let rest = harness.frame(vec![Event::PointerGone], &mut draw);
    let at_rest = height.get();
    let rest = rest.iter().find(|p| p.text == title).map(|p| p.rows);
    assert_eq!(rest, Some(2), "the narrow column wraps it at rest");
    let hovered = harness.frame(vec![Event::PointerMoved(Pos2::new(120.0, 12.0))], &mut draw);
    let hovered = harness
        .frame(vec![], &mut draw)
        .into_iter()
        .chain(hovered)
        .collect::<Vec<_>>();
    assert_eq!(height.get(), at_rest, "the row does not change height");
    let lines: Vec<&crate::kit::rows::tests::Painted> = hovered
        .iter()
        .filter(|p| title.contains(p.text.trim()) && !p.text.trim().is_empty())
        .collect();
    assert!(!lines.is_empty());
    for line in lines {
        assert_eq!(line.rows, 1, "{line:?}");
        // Each painted line is whole words of the title.
        assert!(
            line.text
                .split_whitespace()
                .all(|word| title.split(' ').any(|w| w == word)),
            "split a word: {line:?}"
        );
    }
}

/// F7: a refusal's whole sentence reaches the place it shows. A field's
/// refusal wraps all of it; a one-line place cuts it with its hover.
#[test]
fn a_long_refusal_is_read_in_full_where_it_shows() {
    let long = "100 tracks on USB_DP, USB_DN and 19 more nets and 27 vias on USB_DN, GND and 19 \
                more nets would lie outside 20 × 64 mm";
    let shown = crate::refusal::shown_error(&pcbcraft_core::Error::Invalid(long.to_owned()));
    assert_eq!(shown, long, "nothing is cut before it is shown");
    let screen = Screen::new(egui::vec2(240.0, 200.0));
    let output = screen.run(vec![], |ui| {
        let field = ui.label("field");
        field_refused(ui, &field, &shown);
    });
    assert!(shows_in_full(&output, long));
    assert!(cuts(&output).is_empty());
    let mut draw = |ui: &mut Ui| {
        refusal(ui, &shown);
    };
    let (found, missing) = cuts_without_hover(&screen.ctx, |events| screen.run(events, &mut draw));
    assert_eq!((found, missing), (1, Vec::<String>::new()));
}

/// N5: a toast's text keeps clear of its box on every side, measured on
/// the raster: a margin of the box's fill runs round the ink.
#[test]
fn a_toast_has_room_round_its_text() -> Result<(), &'static str> {
    use crate::kit::toast::Toasts;
    let t = Tokens::get();
    let screen = Screen::new(egui::vec2(500.0, 200.0));
    let mut toasts = Toasts::default();
    toasts.push(Level::Ok, "Marked no-connect", vec![], 0.0);
    let canvas = Rect::from_min_size(Pos2::ZERO, screen.size);
    let mut draw = |ui: &mut Ui| {
        ui.painter().rect_filled(canvas, 0.0, Color32::BLACK);
        toasts.show(ui.ctx(), canvas, 0.0);
    };
    screen.run(vec![], &mut draw);
    let output = screen.run(vec![], &mut draw);
    let text = painted(&output)
        .into_iter()
        .find(|(text, ..)| text == "Marked no-connect")
        .map(|(_, rect, ..)| rect)
        .ok_or("the toast's text")?;
    let rgba = screen.rgba(&output);
    let width = screen.size.x as usize;
    let filled = |x: f32, y: f32| {
        let at = (y as usize * width + x as usize) * 4;
        rgba.get(at..at + 3).is_some_and(|p| p != [0, 0, 0])
    };
    let y = text.center().y;
    let room = t.space_8;
    assert!(filled(text.right() + room, y), "right of the text");
    assert!(filled(text.center().x, text.top() - t.space_4), "above it");
    assert!(
        filled(text.center().x, text.bottom() + t.space_4),
        "below it"
    );
    Ok(())
}

/// N11: the PCB status line fits beside the canvas buttons in the window
/// the check ran in (1380 points wide: a canvas of 830), with the longest
/// tool name and layer focus it had.
#[test]
fn the_route_status_line_is_not_cut() {
    let screen = Screen::new(egui::vec2(830.0, 200.0));
    let line = "Route · Top copper · All layers (⇧L) · grid 0.5 mm · 0 airwires";
    let output = screen.run(vec![], |ui| {
        canvas_hint(ui.painter(), ui.ctx().content_rect(), 16.0, line);
    });
    assert!(shows_in_full(&output, line), "{:?}", cuts(&output));
}

/// F2: the canvas hint sits on the panel colour, a little wider than its
/// text, so it stays readable over copper or any other drawing under it.
#[test]
fn the_canvas_hint_sits_on_a_panel_backdrop() -> Result<(), Box<dyn std::error::Error>> {
    let t = Tokens::get();
    let screen = Screen::new(egui::vec2(830.0, 200.0));
    let line = "Select / move · Top copper";
    let output = screen.run(vec![], |ui| {
        canvas_hint(ui.painter(), ui.ctx().content_rect(), 16.0, line);
    });
    let text = painted(&output)
        .into_iter()
        .find(|(text, ..)| text == line)
        .map(|(_, rect, ..)| rect)
        .ok_or("the hint's text")?;
    let backdrop = output.shapes.iter().find_map(|s| match &s.shape {
        Shape::Rect(r) if r.fill == t.panel && r.rect.contains_rect(text) => Some(r.rect),
        _ => None,
    });
    let backdrop = backdrop.ok_or("no panel backdrop behind the hint")?;
    assert!(backdrop.width() > text.width(), "{backdrop:?} {text:?}");
    Ok(())
}
