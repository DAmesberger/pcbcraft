//! Application chrome: app bar, ribbon, status bar and the floating browser
//! and properties cards. Geometry comes from Solidcraft tokens and constants;
//! feature content comes from the editors.
use super::App;
use crate::file_ui::Action;
use crate::shell::{self, Workspace};
use egui::{Rect, pos2, vec2};
use solidcraft_ui_egui::{
    fonts,
    theme::Tokens,
    titlebar::TRAFFIC_LIGHTS,
    toolbar::APP_BAR_HEIGHT,
    ui_kit::{self, Button, Container, Edge, Surface},
};

/// Gap between the viewport edge and floating cards (Solidcraft's
/// crate-private `shell::SHELL_GAP`).
const SHELL_GAP: f32 = 7.0;
/// The browser shows when the viewport fits it beside the canvas and the
/// properties card.
const BROWSER_MIN_VIEWPORT_CARDS: f32 = 3.5;

/// Where the floating cards and canvas go inside the viewport.
pub(super) struct Layout {
    pub canvas: Rect,
    pub inspector: Rect,
    pub browser: Option<Rect>,
}

pub(super) fn layout(area: Rect, show_browser: bool) -> Layout {
    let t = Tokens::get();
    let area = area.shrink(SHELL_GAP);
    let card = t.form_card_width.min(area.width() * 0.4).max(0.0);
    let inspector = Rect::from_min_max(pos2(area.right() - card, area.top()), area.max);
    let browser = (show_browser && area.width() > card * BROWSER_MIN_VIEWPORT_CARDS).then(|| {
        let rows = 6.0;
        Rect::from_min_size(
            area.min,
            vec2(
                card * 0.8,
                t.form_header_height + rows * t.control_height + t.space_24,
            ),
        )
    });
    let left = browser.map_or(area.left(), |b| b.right() + SHELL_GAP);
    let canvas = Rect::from_min_max(
        pos2(left, area.top()),
        pos2((inspector.left() - SHELL_GAP).max(left), area.bottom()),
    );
    Layout {
        canvas,
        inspector,
        browser,
    }
}

impl App {
    pub(super) fn app_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get();
        let host = Container::new(&ctx, Surface::ApplicationBar);
        host.edge("application", Edge::Top)
            .exact_size(APP_BAR_HEIGHT)
            .frame(host.edge_frame(t.app_bar))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if cfg!(target_os = "macos") {
                        ui.add_space(TRAFFIC_LIGHTS);
                    }
                    ui.label(
                        egui::RichText::new("Pcbcraft")
                            .font(fonts::bold(t.font_heading))
                            .color(t.app_bar_text),
                    );
                    ui.separator();
                    self.file_menu(ui);
                    if shell::icon_button(ui, "save", "Save project", !self.files.busy()).clicked()
                    {
                        self.files.request(Action::Save);
                    }
                    if shell::icon_button(ui, "undo", "Undo", self.engine.can_undo()).clicked() {
                        self.history(false);
                    }
                    if shell::icon_button(ui, "redo", "Redo", self.engine.can_redo()).clicked() {
                        self.history(true);
                    }
                    ui.separator();
                    let file = self
                        .files
                        .path()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| self.engine.project().name.clone());
                    ui.label(format!(
                        "{file}{}",
                        if self.files.dirty() { " •" } else { "" }
                    ));
                    let (_, drag) = ui.allocate_exact_size(
                        vec2(ui.available_width().max(0.0), APP_BAR_HEIGHT - t.space_8),
                        egui::Sense::drag(),
                    );
                    if drag.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                });
            });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let file = ui_kit::button(ui, "File", Button::Ghost);
        let enabled = !self.files.busy();
        Container::new(&ctx, Surface::Popup)
            .menu(&file)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                shell::form(ui, |ui| {
                    for (label, action) in [
                        ("New project", Action::New),
                        ("Open…", Action::Open),
                        ("Save", Action::Save),
                        ("Save as…", Action::SaveAs),
                        ("Export to Solidcraft…", Action::Export),
                    ] {
                        if ui_kit::button(ui, label, Button::Enabled(enabled)).clicked() {
                            self.files.request(action);
                        }
                    }
                    ui.small("Export writes a native, parametric .solidcraft model of the board.");
                });
            });
    }

    pub(super) fn ribbon(&mut self, ui: &mut egui::Ui) {
        let mut workspace = self.workspace;
        shell::ribbon(ui, &mut workspace, |ui| self.tools(ui));
        self.workspace = workspace;
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get();
        let bottom = Container::new(&ctx, Surface::ApplicationBar);
        bottom
            .edge("status", Edge::Bottom)
            .exact_size(t.control_height)
            .frame(bottom.edge_frame(t.app_bar))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.small(&self.status);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if shell::icon_button(ui, "fit", "Fit 3D view", true).clicked() {
                            self.scene.viewport.fit();
                        }
                        ui.small("mm");
                        if self.workspace == Workspace::Pcb
                            && self.pcb.is_3d()
                            && ui
                                .checkbox(&mut self.scene.viewport.hide_substrate, "Hide substrate")
                                .changed()
                        {
                            self.scene.viewport.refresh();
                        }
                    });
                });
            });
    }

    pub(super) fn workspace_area(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get();
        Container::new(&ctx, Surface::Viewport)
            .central()
            .frame(egui::Frame::NONE.fill(t.viewport_bottom))
            .show(ui, |ui| {
                let layout = layout(ui.max_rect(), self.show_browser);
                ui.scope_builder(egui::UiBuilder::new().max_rect(layout.canvas), |ui| {
                    ui.set_clip_rect(layout.canvas);
                    self.canvas(ui);
                });
                if let Some(rect) = layout.browser {
                    shell::card(&ctx, "project-browser", rect, "Browser", "folder", |ui| {
                        self.browser(ui)
                    });
                }
                let title = format!("{} properties", self.workspace.label());
                shell::card(
                    &ctx,
                    "properties",
                    layout.inspector,
                    &title,
                    "settings",
                    |ui| self.inspector(ui),
                );
            });
    }

    fn browser(&mut self, ui: &mut egui::Ui) {
        let project = self.engine.project();
        ui.strong(&project.name);
        ui.separator();
        let rows = [
            (
                Workspace::Part,
                format!("Components  ·  {}", project.parts.len()),
            ),
            (
                Workspace::Schematic,
                format!(
                    "Schematic  ·  {} symbols, {} nets",
                    project.instances.len(),
                    project.nets.len()
                ),
            ),
            (
                Workspace::Pcb,
                format!("PCB  ·  {} tracks", project.tracks.len()),
            ),
        ];
        for (workspace, label) in rows {
            if ui_kit::button(ui, label, Button::Ghost).clicked() {
                self.workspace = workspace;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_cards_and_canvas_apart_and_hides_the_browser_when_narrow() {
        let wide = layout(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0)),
            true,
        );
        assert!(wide.browser.is_some());
        assert!(wide.canvas.left() >= wide.browser.map_or(0.0, |b| b.right()));
        assert!(wide.canvas.right() <= wide.inspector.left());
        let narrow = layout(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 500.0)),
            true,
        );
        assert!(narrow.browser.is_none());
        assert!(narrow.canvas.width() > 0.0);
        let tiny = layout(Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)), true);
        assert!(tiny.canvas.width() >= 0.0);
    }
}
