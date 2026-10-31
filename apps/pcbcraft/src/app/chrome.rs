//! Application chrome: app bar, ribbon, the floating Browser and properties
//! cards, the canvas navigation bar and status messages. Geometry comes from
//! Solidcraft tokens and constants; content comes from the editors.
use super::{App, browser};
use crate::file_ui::Action;
use crate::kit;
use crate::shell::{self, RibbonEvent, Workspace};
use egui::{FontId, Rect, pos2, vec2};
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
/// Upstream Browser card width.
const BROWSER_WIDTH: f32 = 260.0;
/// Informational status messages fade after this many seconds; errors stay
/// until the next successful action, a workspace change or this many
/// seconds, whichever comes first.
const STATUS_SECONDS: f64 = 6.0;
const ERROR_SECONDS: f64 = 12.0;

/// Where the floating cards and canvas go inside the viewport.
pub(super) struct Layout {
    pub canvas: Rect,
    pub inspector: Rect,
    pub browser: Option<Rect>,
    /// The Browser floats over the canvas (shown on request in a window too
    /// narrow to dock it beside the canvas).
    pub overlay: bool,
}

/// Whether the Browser shows: by width (`Auto`), or as the user toggled it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum BrowserMode {
    #[default]
    Auto,
    Shown,
    Hidden,
}

impl BrowserMode {
    /// The mode after the Browser toggle, from what is on screen now.
    pub fn toggled(self, visible: bool) -> Self {
        if visible { Self::Hidden } else { Self::Shown }
    }
}

/// Whether a viewport of `width` docks the Browser beside the canvas.
fn docks(width: f32, card: f32) -> bool {
    width > card * BROWSER_MIN_VIEWPORT_CARDS
}

pub(super) fn layout(area: Rect, mode: BrowserMode) -> Layout {
    let t = Tokens::get();
    let area = area.shrink(SHELL_GAP);
    let card = t.form_card_width.min(area.width() * 0.4).max(0.0);
    let inspector = Rect::from_min_max(pos2(area.right() - card, area.top()), area.max);
    let docked = docks(area.width(), card);
    let shown = match mode {
        BrowserMode::Auto => docked,
        BrowserMode::Shown => true,
        BrowserMode::Hidden => false,
    };
    let overlay = shown && !docked;
    // Docked, it takes the canvas's left edge; as an overlay it floats over
    // the canvas at the same place, as wide as fits beside Properties.
    let width = if overlay {
        BROWSER_WIDTH.min((inspector.left() - SHELL_GAP - area.left()).max(0.0))
    } else {
        BROWSER_WIDTH.min(card)
    };
    let browser = shown.then(|| Rect::from_min_size(area.min, vec2(width, area.height())));
    let left = match browser {
        Some(b) if !overlay => b.right() + SHELL_GAP,
        _ => area.left(),
    };
    let canvas = Rect::from_min_max(
        pos2(left, area.top()),
        pos2((inspector.left() - SHELL_GAP).max(left), area.bottom()),
    );
    Layout {
        canvas,
        inspector,
        browser,
        overlay,
    }
}

/// How a status message reads.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Status {
    pub text: String,
    pub error: bool,
    pub at: f64,
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
                    ui.add_space(t.space_8);
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
                    let visible = self.browser_visible;
                    if shell::icon_button(
                        ui,
                        "list",
                        if visible {
                            "Hide Browser (⌘B)"
                        } else {
                            "Show Browser (⌘B)"
                        },
                        true,
                    )
                    .clicked()
                    {
                        self.toggle_browser();
                    }
                    ui.add_space(t.space_8);
                    self.document_tab(ui);
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

    /// The open document as an application-bar tab, with the unsaved dot.
    fn document_tab(&self, ui: &mut egui::Ui) {
        let t = Tokens::get();
        let name = self
            .files
            .path()
            .and_then(|p| p.file_stem())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.engine.project().name.clone());
        let label = if self.files.dirty() {
            format!("{name} •")
        } else {
            name
        };
        let galley = ui.painter().layout_no_wrap(
            label.clone(),
            fonts::semibold(t.font_label),
            t.app_bar_text,
        );
        let size = vec2(
            galley.size().x + 2.0 * t.space_12,
            APP_BAR_HEIGHT - t.space_8,
        );
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, t.radius_md, t.app_bar_tab_active);
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, t.app_bar_text);
        if let Some(path) = self.files.path() {
            response.on_hover_text(path.display().to_string());
        } else if self.files.dirty() {
            response.on_hover_text("Unsaved project");
        }
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let file = ui_kit::button(ui, "File", Button::Ghost);
        let enabled = !self.files.busy();
        Container::new(&ctx, Surface::Popup)
            .menu(&file)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| {
                kit::form(ui, |ui| {
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
                    kit::note(
                        ui,
                        "Export writes a native, parametric .solidcraft model of the board.",
                    );
                    kit::rule(ui);
                    if ui_kit::button(ui, "Library sources…", Button::Normal)
                        .on_hover_text("Add, rescan, turn on or off and remove library folders")
                        .clicked()
                    {
                        self.sources.open_card();
                    }
                    let label = if self.browser_visible {
                        "Hide Browser"
                    } else {
                        "Show Browser"
                    };
                    if ui_kit::button(ui, label, Button::Normal)
                        .on_hover_text("⌘B")
                        .clicked()
                    {
                        self.toggle_browser();
                    }
                });
            });
    }

    pub(super) fn ribbon(&mut self, ui: &mut egui::Ui) {
        let ribbon = match self.workspace {
            Workspace::Part => self.part.ribbon(&self.engine),
            Workspace::Schematic => self.schematic.ribbon(&self.engine),
            Workspace::Pcb => self.pcb.ribbon(&self.engine),
        };
        match shell::ribbon(ui, self.workspace, &ribbon) {
            Some(RibbonEvent::Workspace(workspace)) => {
                if workspace != self.workspace {
                    self.clear_error();
                }
                self.workspace = workspace;
            }
            Some(RibbonEvent::Tab(tab)) => match self.workspace {
                Workspace::Part => self.part.on_tab(tab),
                Workspace::Schematic => {}
                Workspace::Pcb => self.pcb.on_tab(tab),
            },
            Some(RibbonEvent::Tool(tool)) => {
                let operation = match self.workspace {
                    // The file service owns the native picker.
                    Workspace::Part if tool == crate::part_editor::IMPORT_SVG => {
                        self.part.import_svg(self.files.pick_svg());
                        None
                    }
                    Workspace::Part => self.part.on_tool(tool, &self.engine),
                    Workspace::Schematic => self.schematic.on_tool(tool, &self.engine),
                    Workspace::Pcb => self.pcb.on_tool(tool, &self.engine),
                };
                self.apply(operation);
            }
            None => {}
        }
    }

    pub(super) fn workspace_area(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get();
        Container::new(&ctx, Surface::Viewport)
            .central()
            .frame(egui::Frame::NONE.fill(t.viewport_bottom))
            .show(ui, |ui| {
                let layout = layout(ui.max_rect(), self.browser);
                self.browser_visible = layout.browser.is_some();
                ui.scope_builder(egui::UiBuilder::new().max_rect(layout.canvas), |ui| {
                    ui.set_clip_rect(layout.canvas);
                    self.canvas(ui);
                    self.navigation_bar(ui, layout.canvas);
                    self.status_chip(ui, layout.canvas);
                });
                if let Some(rect) = layout.browser {
                    let summary = browser::summary(&self.engine);
                    let lit = self.lit();
                    let mut pick = None;
                    let mut source_pick = None;
                    let updates = self.sources.updates(&self.engine);
                    shell::card(
                        &ctx,
                        "project-browser",
                        rect,
                        |ui| kit::card_title(ui, "Browser", &summary),
                        |ui| {
                            pick = browser::show(ui, &self.engine, lit, &updates);
                            source_pick = browser::show_sources(ui, self.sources.sources());
                        },
                    );
                    if let Some(pick) = pick {
                        self.pick(pick);
                        // An overlay Browser gives the canvas back once used.
                        if layout.overlay {
                            self.browser = BrowserMode::Auto;
                        }
                    }
                    if source_pick.is_some() {
                        self.sources.open_card();
                    }
                }
                let header = match self.workspace {
                    Workspace::Part => self.part.header(&self.engine),
                    Workspace::Schematic => self.schematic.header(&self.engine),
                    Workspace::Pcb => self.pcb.header(&self.engine),
                };
                shell::card(
                    &ctx,
                    "properties",
                    layout.inspector,
                    |ui| kit::inspector_header(ui, header.icon, &header.title, header.kind),
                    |ui| self.inspector(ui),
                );
            });
    }

    /// The selection the Browser marks, from the active workspace.
    fn lit(&self) -> browser::Lit {
        match self.workspace {
            Workspace::Part => browser::Lit {
                component: self.part.lineage(),
                ..Default::default()
            },
            Workspace::Schematic => browser::Lit {
                instance: self.schematic.selected_instance(),
                net: self.schematic.selected_net(&self.engine),
                ..Default::default()
            },
            Workspace::Pcb => browser::Lit {
                placement: self.pcb.selected_instance(),
                track: self.pcb.selected_track(),
                net: self.pcb.highlighted_net(),
                board: self.pcb.selected_instance().is_none()
                    && self.pcb.selected_track().is_none(),
                ..Default::default()
            },
        }
    }

    /// Open a Browser row in its workspace and select it there.
    fn pick(&mut self, pick: browser::Pick) {
        use browser::Pick;
        match pick {
            Pick::Component(id) => {
                self.workspace = Workspace::Part;
                self.part.open(id, &self.engine);
            }
            Pick::Instance(id) => {
                self.workspace = Workspace::Schematic;
                self.schematic
                    .select_instance(Some(id), self.engine.project());
            }
            Pick::Net(id) => {
                if self.workspace == Workspace::Pcb {
                    self.pcb.set_selection(None, None, Some(id));
                } else {
                    self.workspace = Workspace::Schematic;
                    self.schematic.select_net(id, self.engine.project());
                }
            }
            Pick::Placement(id) => {
                self.workspace = Workspace::Pcb;
                self.pcb.set_selection(Some(id), None, None);
            }
            Pick::Track(id) => {
                self.workspace = Workspace::Pcb;
                let net = self
                    .engine
                    .project()
                    .tracks
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| t.net);
                self.pcb.set_selection(None, Some(id), net);
            }
            Pick::Board => {
                self.workspace = Workspace::Pcb;
                self.pcb.set_selection(None, None, None);
            }
        }
    }

    /// Floating view controls at the bottom of the canvas (Solidcraft's
    /// navigation bar): fit, and the substrate toggle in PCB 3D.
    fn navigation_bar(&mut self, ui: &mut egui::Ui, canvas: Rect) {
        let t = Tokens::get();
        let three_d = self.model_key().is_some();
        let pcb_3d = self.workspace == Workspace::Pcb && self.pcb.is_3d();
        let buttons = if pcb_3d { 2.0 } else { 1.0 };
        let size = vec2(
            buttons * t.control_height + (buttons + 1.0) * t.space_4,
            t.control_height + 2.0 * t.space_4,
        );
        let rect = Rect::from_center_size(
            pos2(
                canvas.center().x,
                canvas.bottom() - t.space_16 - size.y / 2.0,
            ),
            size,
        );
        if rect.width() > canvas.width() {
            return;
        }
        ui.painter().rect(
            rect,
            t.radius_lg,
            t.toolbar,
            t.container_border(t.border),
            egui::StrokeKind::Inside,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink(t.space_4))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = t.space_4;
        if shell::icon_button(&mut child, "fit", "Fit view", true).clicked() {
            if three_d {
                self.scene.viewport.fit();
            } else {
                match self.workspace {
                    Workspace::Part => self.part.fit(),
                    Workspace::Schematic => self.schematic.fit(),
                    Workspace::Pcb => self.pcb.fit(),
                }
            }
        }
        if pcb_3d {
            let hidden = self.scene.viewport.hide_substrate();
            let label = if hidden {
                "Show substrate"
            } else {
                "Hide substrate"
            };
            let response = shell::icon_button(
                &mut child,
                if hidden {
                    "style_translucent"
                } else {
                    "style_shaded"
                },
                label,
                true,
            );
            if hidden {
                child.painter().rect_stroke(
                    response.rect,
                    t.radius_sm,
                    egui::Stroke::new(1.0, t.accent),
                    egui::StrokeKind::Inside,
                );
            }
            if response.clicked() {
                self.scene.viewport.set_hide_substrate(!hidden);
            }
        }
    }

    /// The latest status message, bottom-left in the canvas. Errors stay
    /// longer than other messages; both fade.
    fn status_chip(&mut self, ui: &mut egui::Ui, canvas: Rect) {
        let t = Tokens::get();
        let Some(status) = &self.status else {
            return;
        };
        let now = ui.input(|i| i.time);
        let lasts = if status.error {
            ERROR_SECONDS
        } else {
            STATUS_SECONDS
        };
        if now - status.at > lasts {
            self.status = None;
            return;
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(
                lasts - (now - status.at) + 0.05,
            ));
        let ink = if status.error { t.error } else { t.text };
        let mut job = egui::text::LayoutJob::simple(
            status.text.clone(),
            FontId::proportional(t.font_label),
            ink,
            (canvas.width() * 0.45).max(120.0),
        );
        job.wrap.max_rows = 3;
        let galley = ui.painter().layout_job(job);
        let rect = Rect::from_min_size(
            pos2(
                canvas.left() + t.space_12,
                canvas.bottom() - t.space_16 - galley.size().y - 2.0 * t.space_8,
            ),
            galley.size() + vec2(2.0 * t.space_12, 2.0 * t.space_8),
        );
        ui.painter().rect(
            rect,
            t.radius_md,
            t.toolbar,
            t.container_border(if status.error { t.error } else { t.border }),
            egui::StrokeKind::Inside,
        );
        ui.painter()
            .galley(rect.min + vec2(t.space_12, t.space_8), galley, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_cards_and_canvas_apart_and_hides_the_browser_when_narrow() {
        let wide = layout(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0)),
            BrowserMode::Auto,
        );
        assert!(wide.browser.is_some());
        assert!(wide.canvas.left() >= wide.browser.map_or(0.0, |b| b.right()));
        assert!(wide.canvas.right() <= wide.inspector.left());
        let narrow = layout(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 500.0)),
            BrowserMode::Auto,
        );
        assert!(narrow.browser.is_none());
        assert!(narrow.canvas.width() > 0.0);
        let tiny = layout(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)),
            BrowserMode::Auto,
        );
        assert!(tiny.canvas.width() >= 0.0);
    }

    /// At half a laptop screen the Browser is hidden by width, but the
    /// toggle shows it over the canvas, beside Properties; hidden on
    /// request it stays hidden however wide the window.
    #[test]
    fn the_browser_toggle_overlays_it_when_narrow() {
        let half = Rect::from_min_size(pos2(0.0, 0.0), vec2(860.0, 900.0));
        assert!(layout(half, BrowserMode::Auto).browser.is_none());
        let shown = layout(half, BrowserMode::Shown);
        let browser = shown.browser.unwrap_or(Rect::NOTHING);
        assert!(shown.overlay);
        assert!(browser.width() > 100.0);
        assert!(browser.right() <= shown.inspector.left());
        assert!(shown.canvas.width() > 0.0, "the canvas keeps its width");
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0));
        assert!(layout(wide, BrowserMode::Hidden).browser.is_none());
        assert!(!layout(wide, BrowserMode::Shown).overlay);
        assert_eq!(BrowserMode::Auto.toggled(false), BrowserMode::Shown);
        assert_eq!(BrowserMode::Shown.toggled(true), BrowserMode::Hidden);
    }
}
