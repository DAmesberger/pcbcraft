//! Application chrome: app bar, ribbon, the floating Browser and properties
//! cards, the canvas navigation bar and the toast stack. Geometry comes from
//! Solidcraft tokens and constants; content comes from the editors.
use super::{App, browser};
use crate::file_ui::Action;
use crate::issues;
use crate::kit;
use crate::shell::{self, RibbonEvent, Workspace};
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
/// Upstream Browser card width.
const BROWSER_WIDTH: f32 = 260.0;

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
                    if ui_kit::button(ui, "Messages…", Button::Normal)
                        .on_hover_text("Every message of this session, with the full text")
                        .clicked()
                    {
                        self.messages_open = true;
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
        let issues = self.issues.ribbon_toggle(&self.engine, self.workspace);
        match shell::ribbon(ui, self.workspace, &ribbon, &issues) {
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
                self.apply(
                    &ui.ctx().clone(),
                    crate::refusal::Origin::Command,
                    operation,
                );
            }
            Some(RibbonEvent::Issues) => self.issues.toggle(),
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
                    self.issues
                        .badge(ui, layout.canvas, &self.engine, self.workspace);
                    let selection = self.selection();
                    let shown = self.issues.pill(
                        ui,
                        layout.canvas,
                        &self.engine,
                        self.workspace,
                        &selection,
                    );
                    self.issue_request(&ctx, shown);
                    self.navigation_bar(ui, layout.canvas);
                    self.toasts_above_navigation(ui, layout.canvas);
                });
                if let Some(rect) = layout.browser {
                    let summary = browser::summary(&self.engine);
                    let lit = self.lit();
                    let mut pick = None;
                    let mut source_pick = None;
                    let updates = self.sources.updates(&self.engine);
                    let badges = self.issues.badges(&self.engine);
                    shell::card(
                        &ctx,
                        "project-browser",
                        rect,
                        |ui| kit::card_title(ui, "Browser", &summary),
                        |ui| {
                            pick = browser::show(ui, &self.engine, lit, &updates, &badges);
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
                self.column(&ctx, layout.inspector);
            });
    }

    /// The right column: the subject slot above the always-visible Issues
    /// card (docs/sidebar-ia.md, "Layout"). The active editor lays out the
    /// whole slot: its Properties card, a tool card, a docked dialog or a
    /// docked review.
    fn column(&mut self, ctx: &egui::Context, rect: Rect) {
        let wants = kit::card::wants(ctx, 2.0 * f32::from(shell::COLUMN_CARD_INSET));
        let column = kit::card::column(rect, self.issues.expanded(), wants);
        let slot = shell::column_card(
            ctx,
            "properties",
            column.subject,
            shell::COLUMN_CARD_INSET,
            |ui| match self.workspace {
                Workspace::Part => self.part.subject_slot(ui, &self.engine),
                Workspace::Schematic => self.schematic.subject_slot(ui, &self.engine),
                Workspace::Pcb => self.pcb.subject_slot(ui, &self.engine),
            },
        );
        if let Some(slot) = slot {
            if slot.show_issues {
                self.issues.show_selection();
            }
            // Editors mark a field or dialog origin themselves; the rest
            // are commands.
            self.apply(ctx, crate::refusal::Origin::Command, slot.output);
        }
        self.open_new_part();
        if let Some(id) = self.pcb.take_show_in_schematic() {
            self.workspace = Workspace::Schematic;
            self.schematic
                .select_instance(Some(id), self.engine.project());
        }
        self.issues_column(ctx, column);
    }

    /// The Issues card under the subject slot, always shown.
    fn issues_column(&mut self, ctx: &egui::Context, column: kit::card::Column) {
        let selection = self.selection();
        let request = shell::column_card(ctx, "issues", column.issues, 0, |ui| {
            self.issues
                .card(ui, &self.engine, self.workspace, &selection)
        })
        .flatten();
        self.issue_request(ctx, request);
    }

    /// The active editor's selection as issue objects, for the Selection
    /// scope.
    fn selection(&self) -> Vec<pcbcraft_core::ObjectRef> {
        match self.workspace {
            Workspace::Part => {
                issues::selection(self.workspace, self.part.lineage(), None, None, None)
            }
            Workspace::Schematic => issues::selection(
                self.workspace,
                None,
                self.schematic.selected_instance(),
                None,
                self.schematic.selected_net(&self.engine),
            ),
            Workspace::Pcb => issues::selection(
                self.workspace,
                None,
                self.pcb.selected_instance(),
                self.pcb.selected_track(),
                self.pcb.highlighted_net(),
            ),
        }
    }

    /// What the Issues surface asked for: locate an issue's objects in the
    /// workspace that shows them, or run its Fix.
    fn issue_request(&mut self, ctx: &egui::Context, request: Option<issues::Request>) {
        match request {
            Some(issues::Request::Locate {
                refs,
                area,
                pin_mux,
            }) => {
                let mut editors = issues::Editors {
                    part: &mut self.part,
                    schematic: &mut self.schematic,
                    pcb: &mut self.pcb,
                };
                match issues::route(&mut editors, &self.engine, &refs, issues::workspace(area)) {
                    Some(workspace) => {
                        if workspace != self.workspace {
                            self.clear_error();
                        }
                        self.workspace = workspace;
                        // The schematic selected the instance; its card
                        // draws the panel from the next frame.
                        if let Some(target) = pin_mux.filter(|_| workspace == Workspace::Schematic)
                        {
                            crate::pin_mux::reveal(ctx, target.instance, target.terminal);
                        }
                    }
                    None => self.say(
                        crate::kit::severity::Level::Info,
                        "That issue has nothing to select",
                    ),
                }
            }
            Some(issues::Request::Fix(fix)) => self.fix(ctx, fix),
            None => {}
        }
    }

    /// Run an issue's Fix: open its dialog in its workspace, or apply its
    /// typed operation.
    fn fix(&mut self, ctx: &egui::Context, fix: issues::Fix) {
        use issues::{Fix, FixDialog};
        match fix {
            Fix::Apply { operation, .. } => {
                self.apply(ctx, crate::refusal::Origin::Command, Some(*operation));
            }
            Fix::Open { dialog, .. } => match dialog {
                FixDialog::UpdatePcb => {
                    self.workspace = Workspace::Pcb;
                    let operation = self.pcb.on_tool("update", &self.engine);
                    self.apply(ctx, crate::refusal::Origin::Command, operation);
                }
                FixDialog::ResolveParts => {
                    self.workspace = Workspace::Schematic;
                    // The ribbon's Resolve toggles; open it only when shut.
                    let open = self
                        .schematic
                        .ribbon(&self.engine)
                        .groups
                        .iter()
                        .flat_map(|g| &g.tools)
                        .any(|t| t.id == "resolve" && t.selected);
                    if !open {
                        let operation = self.schematic.on_tool("resolve", &self.engine);
                        self.apply(ctx, crate::refusal::Origin::Command, operation);
                    }
                }
                FixDialog::AssignPads
                | FixDialog::ChoosePart
                | FixDialog::UpdateInstances
                | FixDialog::Mapping => {}
            },
        }
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
    /// navigation bar): fit, and the layer popover in PCB 3D.
    fn navigation_bar(&mut self, ui: &mut egui::Ui, canvas: Rect) {
        let t = Tokens::get();
        let three_d = self.model_key().is_some();
        let pcb_3d = self.workspace == Workspace::Pcb && self.pcb.is_3d();
        let pcb_view = self.workspace == Workspace::Pcb && self.pcb.has_view_menu();
        let buttons = if pcb_3d || pcb_view { 2.0 } else { 1.0 };
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
        // The navigation help is the Fit button's hover help (where
        // Solidcraft's navigation bar explains its buttons).
        let help = match (three_d, self.workspace) {
            (true, Workspace::Pcb) => crate::navigation::help_3d(),
            (true, _) => crate::navigation::help_3d_placement(),
            (false, workspace) => crate::navigation::help_2d(workspace == Workspace::Part),
        };
        if shell::icon_button_help(&mut child, "fit", "Fit view", &help, true).clicked() {
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
        if pcb_view {
            // Layers and view toggles (W3): a popover above the bar.
            let response = shell::icon_button(&mut child, "display", "Layers and view", true);
            Container::new(ui.ctx(), Surface::Popup)
                .menu(&response)
                .align(egui::RectAlign::TOP_START)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| self.pcb.view_menu(ui));
        }
        if pcb_3d {
            let mut view = self.pcb.layer_view();
            let copper = self.engine.project().stackup.layers();
            if crate::layer_view::button(&mut child, &mut view, &copper) {
                self.scene.viewport.set_layers(view.clone());
                self.pcb.set_layer_view(view);
            }
        }
    }

    /// The toast stack, bottom-centre of the canvas above the navigation
    /// bar and clear of the bottom-left hint line.
    fn toasts_above_navigation(&mut self, ui: &mut egui::Ui, canvas: Rect) {
        let free = toast_area(canvas);
        if let Some(event) = self.toasts.show(ui.ctx(), free, self.now) {
            self.toast_event(event);
        }
    }
}

/// Where toasts may go in `canvas`: above the navigation bar (its height
/// and gap, see `navigation_bar`).
fn toast_area(canvas: Rect) -> Rect {
    let t = Tokens::get();
    let navigation = t.space_16 + t.control_height + 2.0 * t.space_4;
    Rect::from_min_max(
        canvas.min,
        pos2(
            canvas.right(),
            (canvas.bottom() - navigation).max(canvas.top()),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toasts_stay_above_the_navigation_bar() {
        let canvas = Rect::from_min_size(pos2(10.0, 20.0), vec2(600.0, 400.0));
        let free = toast_area(canvas);
        assert!(canvas.contains_rect(free));
        assert!(free.bottom() < canvas.bottom() - Tokens::get().control_height);
        let flat = toast_area(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 4.0)));
        assert!(flat.height() >= 0.0);
    }

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

    /// The Issues bar sits under Properties, inside the column, at every
    /// window size the layout supports, so blockers stay counted on screen.
    #[test]
    fn the_issues_bar_stays_in_the_column_at_every_window_size() {
        for (w, h) in [
            (1600.0, 900.0),
            (860.0, 1081.0),
            (600.0, 500.0),
            (320.0, 120.0),
        ] {
            let l = layout(
                Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h)),
                BrowserMode::Auto,
            );
            for expanded in [false, true] {
                let c = kit::card::column(l.inspector, expanded, Default::default());
                assert!(
                    c.issues.height() >= kit::issues::ISSUES_BAR_HEIGHT,
                    "{w}x{h}"
                );
                assert_eq!(c.issues.bottom(), l.inspector.bottom());
                assert!(c.subject.bottom() < c.issues.top());
                assert_eq!(c.issues.x_range(), l.inspector.x_range());
            }
        }
    }
}
