//! Application chrome: app bar, ribbon, the floating Browser and properties
//! cards, the canvas navigation bar and the toast stack. Geometry comes from
//! Solidcraft tokens and constants; content comes from the editors.
use super::{App, browser};
use crate::commands::AppCommand;
use crate::file_ui::Action;
use crate::issues;
use crate::kit;
use crate::reveal::{self, Target};
use crate::shell::{self, Workspace};
use egui::{Rect, pos2, vec2};
use solidcraft_theme::Tokens;
#[cfg(test)]
use solidcraft_ui_egui::fonts;
use solidcraft_ui_egui::ui_kit::type_face;
use solidcraft_ui_egui::{
    toolbar::APP_BAR_HEIGHT,
    ui_kit::{Container, Edge, Surface},
    window_chrome::TRAFFIC_LIGHTS,
};

/// Gap between the viewport edge and floating cards (Solidcraft's
/// crate-private `shell::SHELL_GAP`).
const SHELL_GAP: f32 = 7.0;
/// Application-bar room the document tab leaves to drag the window by.
const TAB_DRAG_ROOM: f32 = 96.0;
/// The document tab's narrowest width.
const TAB_MIN_WIDTH: f32 = 96.0;
/// The browser shows when the viewport fits it beside the canvas and the
/// properties card.
const BROWSER_MIN_VIEWPORT_CARDS: f32 = 3.5;
/// Upstream Browser card width: the width before the user drags one.
pub(super) const BROWSER_WIDTH: f32 = 260.0;
/// How narrow the Browser may be dragged, and how wide (upstream
/// `browser::MIN_BROWSER_WIDTH` / `MAX_BROWSER_WIDTH`). A docked Browser
/// also leaves the canvas at least half the room beside Properties.
pub(super) const MIN_BROWSER_WIDTH: f32 = 170.0;
pub(super) const MAX_BROWSER_WIDTH: f32 = 620.0;

/// Where the floating cards and canvas go inside the viewport.
pub(super) struct Layout {
    pub canvas: Rect,
    pub inspector: Rect,
    pub browser: Option<Rect>,
    /// The widths the Browser may be dragged to in this window.
    pub browser_range: std::ops::RangeInclusive<f32>,
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

/// The widths a Browser may take in `room` (the width left of
/// Properties): docked it leaves the canvas at least half of it, as an
/// overlay it may cover it.
pub(super) fn browser_range(room: f32, overlay: bool) -> std::ops::RangeInclusive<f32> {
    let room = if room.is_finite() { room.max(0.0) } else { 0.0 };
    let max = if overlay { room } else { room / 2.0 }.min(MAX_BROWSER_WIDTH);
    MIN_BROWSER_WIDTH.min(max)..=max
}

/// The cards and canvas for a Browser of the remembered `width` (clamped to
/// what the window fits; a non-finite width is the default).
pub(super) fn layout(area: Rect, mode: BrowserMode, width: f32) -> Layout {
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
    let browser_range = browser_range(inspector.left() - SHELL_GAP - area.left(), overlay);
    let width = if width.is_finite() {
        width
    } else {
        BROWSER_WIDTH
    }
    .clamp(*browser_range.start(), *browser_range.end());
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
        browser_range,
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
                            .font(type_face::bold(t.font_heading))
                            .color(t.app_bar_text),
                    );
                    ui.add_space(t.space_8);
                    self.file_menu(ui);
                    let save = self.with_shortcut("Save project", AppCommand::Save);
                    let button = bar_icon(ui, "save", &save, !self.files.busy());
                    reveal::register(&ctx, Target::App(AppCommand::Save), button.rect);
                    if button.clicked() {
                        self.files.request(Action::Save);
                    }
                    let undo = self.with_shortcut("Undo", AppCommand::Undo);
                    let button = bar_icon(ui, "undo", &undo, self.engine.can_undo());
                    reveal::register(&ctx, Target::App(AppCommand::Undo), button.rect);
                    if button.clicked() {
                        self.history(false);
                    }
                    let redo = self.with_shortcut("Redo", AppCommand::Redo);
                    let button = bar_icon(ui, "redo", &redo, self.engine.can_redo());
                    reveal::register(&ctx, Target::App(AppCommand::Redo), button.rect);
                    if button.clicked() {
                        self.history(true);
                    }
                    let browser = self.with_shortcut(
                        if self.browser_visible {
                            "Hide Browser"
                        } else {
                            "Show Browser"
                        },
                        AppCommand::ToggleBrowser,
                    );
                    let button = bar_icon(ui, "list", &browser, true);
                    reveal::register(&ctx, Target::App(AppCommand::ToggleBrowser), button.rect);
                    if button.clicked() {
                        self.toggle_browser();
                    }
                    let palette = self.with_shortcut("Command palette", AppCommand::Palette);
                    let button = bar_icon(ui, "zoom", &palette, true);
                    reveal::register(&ctx, Target::App(AppCommand::Palette), button.rect);
                    if button.clicked() {
                        self.palette.toggle();
                    }
                    ui.add_space(t.space_8);
                    self.document_tab(ui);
                    self.live_indicator(ui);
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
        let tip = match self.files.path() {
            Some(path) => path.display().to_string(),
            None if self.files.dirty() => "Unsaved project".to_owned(),
            None => String::new(),
        };
        tab(ui, &label, &tip);
    }

    /// The in-window File menu, as Solidcraft's: plain rows with their
    /// keys, groups apart. Each row runs its registry command, as the menu
    /// bar and the key do.
    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let file = bar_text_button(ui, "File");
        let rows = self.menu_now(&ctx, |now| {
            crate::menu_bar::rows(now, crate::menu_bar::IN_WINDOW_FILE)
        });
        let mut chosen = None;
        Container::new(&ctx, Surface::Popup)
            .menu(&file)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .show(|ui| chosen = crate::menu_bar::show_rows(ui, &rows));
        if let Some(id) = chosen {
            self.run_command(&ctx, id);
        }
    }

    pub(super) fn ribbon(&mut self, ui: &mut egui::Ui) {
        let mut ribbon = match self.workspace {
            Workspace::Part => self.part.ribbon(&self.engine),
            Workspace::Schematic => self.schematic.ribbon(&self.engine),
            Workspace::Pcb => self.pcb.ribbon(&self.engine),
        };
        self.annotate_ribbon(&mut ribbon);
        self.refresh_tips(&ribbon);
        let issues = self.issues.ribbon_toggle(&self.engine, self.workspace);
        if let Some(event) = shell::ribbon(ui, self.workspace, &ribbon, &issues) {
            // The ribbon was drawn before its event: draw it again in its
            // new state (the 3D tab chosen) rather than when input next
            // arrives.
            ui.ctx().request_repaint();
            self.ribbon_event(&ui.ctx().clone(), event);
        }
    }

    pub(super) fn workspace_area(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get();
        Container::new(&ctx, Surface::Viewport)
            .central()
            .frame(egui::Frame::NONE.fill(t.viewport_bottom))
            .show(ui, |ui| {
                let layout = layout(ui.max_rect(), self.browser, self.browser_width);
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
                    // The remembered width is what the user dragged; this
                    // window may show it narrower.
                    let mut width = rect.width();
                    let resized = shell::card(
                        &ctx,
                        "project-browser",
                        rect,
                        Some(shell::Resize {
                            width: &mut width,
                            range: layout.browser_range.clone(),
                        }),
                        |ui| kit::card_title(ui, "Browser", &summary),
                        |ui| {
                            pick = browser::show(
                                ui,
                                &self.engine,
                                lit,
                                &updates,
                                &badges,
                                &mut self.browser_menu,
                            );
                            source_pick = browser::show_sources(ui, self.sources.sources());
                        },
                    );
                    if resized.changed {
                        self.browser_width = width;
                    }
                    if resized.stopped {
                        self.sources.set_browser_width(self.browser_width);
                    }
                    if let Some(browser::Pick::Locate(id)) = pick {
                        // As an issue's locate: select and frame it.
                        self.issue_request(
                            &ctx,
                            Some(issues::Request::Locate {
                                refs: vec![pcbcraft_core::ObjectRef::Instance { id }],
                                area: issues::area(Workspace::Schematic),
                                pin_mux: None,
                            }),
                        );
                    } else if let Some(browser::Pick::Command(id)) = pick {
                        // A folder's menu: as from the palette.
                        self.run_command(&ctx, id);
                    } else if let Some(pick) = pick {
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
            Fix::Apply {
                operation, done, ..
            } => {
                let before = self.engine.revision();
                self.apply(ctx, crate::refusal::Origin::Command, Some(*operation));
                if self.engine.revision() != before {
                    self.say_with(
                        crate::kit::severity::Level::Ok,
                        done,
                        vec![crate::kit::toast::ToastAction::Undo],
                    );
                }
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
                FixDialog::DesignRules(topic) => {
                    self.workspace = Workspace::Pcb;
                    self.pcb.show_rule(topic);
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
                component: self.browser_component,
                instance: self.schematic.selected_instance(),
                net: self.schematic.selected_net(&self.engine),
                sheet: Some(self.schematic.active_sheet()),
                ..Default::default()
            },
            Workspace::Pcb => browser::Lit {
                component: self.browser_component,
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
    pub(super) fn pick(&mut self, pick: browser::Pick) {
        use browser::Pick;
        self.browser_component = None;
        match pick {
            // Selecting a component changes no workspace and drafts nothing.
            Pick::Component(id) => self.browser_component = Some(id),
            Pick::EditComponent(id) => {
                self.workspace = Workspace::Part;
                self.part.open(id, &self.engine);
            }
            Pick::Instance(id) | Pick::Locate(id) => {
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
            Pick::Sheet(sheet) => {
                self.workspace = Workspace::Schematic;
                self.schematic.show_sheet(sheet, self.engine.project());
            }
            Pick::Remove(id) => {
                self.workspace = Workspace::Schematic;
                self.schematic.review_removal(&self.engine, id);
            }
            // Run where the context is at hand (the Browser's call site).
            Pick::Command(_) => {}
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
        let fit = self.with_shortcut("Fit view", AppCommand::FitView);
        let button = shell::icon_button_help(&mut child, "fit", &fit, &help, true);
        reveal::register(ui.ctx(), Target::App(AppCommand::FitView), button.rect);
        if button.clicked() {
            self.fit_view();
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
            let stack = pcbcraft_solidcraft::stack::Stack::of(&self.engine.project().stackup);
            if crate::layer_view::button(&mut child, &mut view, &stack) {
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

/// The document tab showing `label`: a long name ends in "…", keeping room
/// to drag the window by, and shows in full on hover above `tip`.
/// An icon button on the always dark application bar, as Solidcraft's
/// (`toolbar.rs`): the dark theme's artwork, its bar hover, and 35 % opacity
/// when disabled.
fn bar_icon(ui: &mut egui::Ui, icon: &str, label: &str, enabled: bool) -> egui::Response {
    let t = Tokens::get();
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(t.control_height, t.control_height), sense);
    if enabled && (response.hovered() || response.has_focus()) {
        ui.painter()
            .rect_filled(rect, t.radius_sm, t.app_bar_hover_button);
    }
    solidcraft_ui_egui::icons::paint_on(
        ui.painter(),
        rect.shrink(t.space_4),
        icon,
        Some(true),
        if enabled { 1.0 } else { 0.35 },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    response.on_hover_text(label)
}

/// A text button on the application bar (File), in the bar's text token.
fn bar_text_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let t = Tokens::get();
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(t.font_button),
        t.app_bar_text,
    );
    let size = vec2(galley.size().x + t.space_16, t.control_height);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, t.radius_sm, t.app_bar_hover);
    }
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, t.app_bar_text);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    response
}

fn tab(ui: &mut egui::Ui, label: &str, tip: &str) -> egui::Response {
    let t = Tokens::get();
    let room = (ui.available_width() - TAB_DRAG_ROOM).max(TAB_MIN_WIDTH);
    let galley = kit::text::one_line(
        ui,
        label,
        type_face::semibold(t.font_label),
        t.app_bar_text,
        room - 2.0 * t.space_12,
    );
    let cut = galley.elided;
    let size = vec2(
        galley.size().x + 2.0 * t.space_12,
        APP_BAR_HEIGHT - t.space_8,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, t.radius_md, t.app_bar_tab_active);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, t.app_bar_text);
    kit::text::hover(response, label, tip, cut)
}

/// Where toasts may go in `canvas`: above the navigation bar (its height
/// and gap, see `navigation_bar`).
fn toast_area(canvas: Rect) -> Rect {
    let navigation = crate::navigation::bar_band();
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

    /// N2: the application bar's text and enabled icons stand out from the
    /// always dark bar in both themes, and a disabled icon is fainter but
    /// still there. Measured on the raster, against the bar's own token.
    #[test]
    fn the_application_bar_is_legible() -> Result<(), Box<dyn std::error::Error>> {
        use crate::kit::text::tests::Screen;
        for dark in [false, true] {
            solidcraft_theme::set_dark(dark);
            let t = Tokens::get();
            assert!(kit::contrast(t.app_bar_text, t.app_bar) >= 4.5);
            let screen = Screen::new(vec2(200.0, 40.0));
            let mut rects = Vec::new();
            let mut draw = |ui: &mut egui::Ui| {
                ui.painter()
                    .rect_filled(ui.ctx().content_rect(), 0.0, t.app_bar);
                ui.horizontal(|ui| {
                    rects = vec![
                        bar_text_button(ui, "File").rect,
                        bar_icon(ui, "undo", "Undo", true).rect,
                        bar_icon(ui, "undo", "Undo", false).rect,
                        bar_icon(ui, "save", "Save", true).rect,
                        bar_icon(ui, "zoom", "Find", false).rect,
                    ];
                });
            };
            screen.run(vec![], &mut draw);
            let output = screen.run(vec![], &mut draw);
            let rgba = screen.rgba(&output);
            let strongest = |rect: Rect| {
                // Inside the control: the fill's feathered edge is not ink.
                let rect = rect.shrink(2.0);
                let mut best = 1.0_f32;
                for y in rect.top() as usize..rect.bottom() as usize {
                    for x in rect.left() as usize..rect.right() as usize {
                        let at = (y * screen.size.x as usize + x) * 4;
                        if let Some(px) = rgba.get(at..at + 3) {
                            let ink = egui::Color32::from_rgb(px[0], px[1], px[2]);
                            best = best.max(kit::contrast(ink, t.app_bar));
                        }
                    }
                }
                best
            };
            let [file, undo, off, save, find] = rects[..] else {
                return Err("five controls".into());
            };
            assert!(strongest(file) >= 4.5, "File {}", strongest(file));
            assert!(strongest(undo) >= 3.0, "Undo {}", strongest(undo));
            assert!(strongest(save) >= 3.0, "Save {}", strongest(save));
            // WCAG exempts disabled controls, but they must still show.
            for (name, rect) in [("Undo", off), ("Find", find)] {
                assert!(strongest(rect) >= 1.5, "off {name} {}", strongest(rect));
            }
            screen.picture(if dark { "app-bar" } else { "app-bar-light" }, &output, 3.0)?;
            assert!(strongest(off) < strongest(undo), "disabled reads fainter");
        }
        Ok(())
    }

    /// A 170-character file name in a narrow application bar: the tab
    /// stays inside the bar with room left to drag the window by, ends in
    /// "…" and shows the whole name and the path on hover.
    #[test]
    fn a_long_tab_title_is_cut_with_its_full_name_on_hover() {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let name = "ESP32-S31 sensor board ".repeat(8);
        let path = format!("/Users/agent/prec/{name}.pcbcraft");
        let bar = Rect::from_min_size(pos2(0.0, 0.0), vec2(700.0, APP_BAR_HEIGHT));
        let mut tab_rect = Rect::NOTHING;
        let (cuts, missing) = kit::text::tests::cuts_without_hover(&ctx, |events| {
            let input = egui::RawInput {
                screen_rect: Some(bar),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| tab_rect = tab(ui, &name, &path).rect);
            });
            output.textures_delta.clear();
            output
        });
        assert_eq!(cuts, 1, "the name is cut");
        assert!(missing.is_empty(), "{missing:?}");
        assert!(
            tab_rect.right() <= bar.right() - TAB_DRAG_ROOM + 1.0,
            "{tab_rect:?}"
        );
    }

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
            BROWSER_WIDTH,
        );
        assert!(wide.browser.is_some());
        assert!(wide.canvas.left() >= wide.browser.map_or(0.0, |b| b.right()));
        assert!(wide.canvas.right() <= wide.inspector.left());
        let narrow = layout(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 500.0)),
            BrowserMode::Auto,
            BROWSER_WIDTH,
        );
        assert!(narrow.browser.is_none());
        assert!(narrow.canvas.width() > 0.0);
        let tiny = layout(
            Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0)),
            BrowserMode::Auto,
            BROWSER_WIDTH,
        );
        assert!(tiny.canvas.width() >= 0.0);
    }

    /// At half a laptop screen the Browser is hidden by width, but the
    /// toggle shows it over the canvas, beside Properties; hidden on
    /// request it stays hidden however wide the window.
    #[test]
    fn the_browser_toggle_overlays_it_when_narrow() {
        let half = Rect::from_min_size(pos2(0.0, 0.0), vec2(860.0, 900.0));
        assert!(
            layout(half, BrowserMode::Auto, BROWSER_WIDTH)
                .browser
                .is_none()
        );
        let shown = layout(half, BrowserMode::Shown, BROWSER_WIDTH);
        let browser = shown.browser.unwrap_or(Rect::NOTHING);
        assert!(shown.overlay);
        assert!(browser.width() > 100.0);
        assert!(browser.right() <= shown.inspector.left());
        assert!(shown.canvas.width() > 0.0, "the canvas keeps its width");
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0));
        assert!(
            layout(wide, BrowserMode::Hidden, BROWSER_WIDTH)
                .browser
                .is_none()
        );
        assert!(!layout(wide, BrowserMode::Shown, BROWSER_WIDTH).overlay);
        assert_eq!(BrowserMode::Auto.toggled(false), BrowserMode::Shown);
        assert_eq!(BrowserMode::Shown.toggled(true), BrowserMode::Hidden);
    }

    /// The remembered Browser width is shown inside its range: at least
    /// the upstream minimum, at most the upstream maximum and, docked, half
    /// the room beside Properties so the canvas keeps the rest.
    #[test]
    fn the_browser_width_stays_inside_its_range() {
        let huge = Rect::from_min_size(pos2(0.0, 0.0), vec2(2600.0, 900.0));
        let width = |area, w| {
            layout(area, BrowserMode::Auto, w)
                .browser
                .map_or(0.0, |b| b.width())
        };
        assert_eq!(width(huge, 50.0), MIN_BROWSER_WIDTH);
        assert_eq!(width(huge, 333.0), 333.0);
        assert_eq!(width(huge, 5000.0), MAX_BROWSER_WIDTH);
        assert_eq!(width(huge, f32::NAN), BROWSER_WIDTH);
        let laptop = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        let l = layout(laptop, BrowserMode::Auto, 5000.0);
        let browser = l.browser.unwrap_or(Rect::NOTHING);
        assert!(browser.width() < MAX_BROWSER_WIDTH);
        assert_eq!(browser.width(), *l.browser_range.end());
        assert!(l.canvas.width() >= browser.width() - SHELL_GAP);
        // As an overlay it may cover the canvas, never Properties.
        let half = Rect::from_min_size(pos2(0.0, 0.0), vec2(860.0, 900.0));
        let shown = layout(half, BrowserMode::Shown, 5000.0);
        let browser = shown.browser.unwrap_or(Rect::NOTHING);
        assert!(browser.right() <= shown.inspector.left());
        let range = browser_range(0.0, false);
        assert!(range.start() <= range.end());
        assert_eq!(shell::resized_width(300.0, 1e9, &(170.0..=620.0)), 620.0);
    }

    /// Dragging the Browser's right edge changes its width, clamped to the
    /// range, and reports the end of the drag (when the width is saved).
    #[test]
    fn dragging_the_browser_edge_resizes_it_within_its_range() {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let range = MIN_BROWSER_WIDTH..=400.0;
        let mut width = BROWSER_WIDTH;
        let mut time = 0.0;
        let mut frame = |width: &mut f32, events: Vec<egui::Event>| {
            time += 0.05;
            let mut resized = shell::Resized::default();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let rect = Rect::from_min_size(pos2(7.0, 7.0), vec2(*width, 600.0));
                    resized = shell::card(
                        ui.ctx(),
                        "drag-test",
                        rect,
                        Some(shell::Resize {
                            width,
                            range: range.clone(),
                        }),
                        |ui| kit::card_title(ui, "Browser", ""),
                        |ui| {
                            ui.label("Body");
                        },
                    );
                },
            );
            output.textures_delta.clear();
            resized
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Drag the edge right by 60, then far past the maximum.
        let mut x = 7.0 + width - 2.0;
        let y = 20.0;
        // The card's area is sized on its first frames.
        frame(&mut width, Vec::new());
        frame(&mut width, Vec::new());
        frame(&mut width, vec![egui::Event::PointerMoved(pos2(x, y))]);
        frame(&mut width, vec![button(pos2(x, y), true)]);
        let start = width;
        for _ in 0..6 {
            x += 10.0;
            frame(&mut width, vec![egui::Event::PointerMoved(pos2(x, y))]);
        }
        assert!(width > start + 40.0 && width <= start + 60.0, "{width}");
        for _ in 0..10 {
            x += 100.0;
            frame(&mut width, vec![egui::Event::PointerMoved(pos2(x, y))]);
        }
        assert_eq!(width, 400.0, "the maximum holds");
        let end = frame(&mut width, vec![button(pos2(x, y), false)]);
        assert!(end.stopped, "the end of the drag is reported");
        // And far to the left: the minimum holds.
        let mut x = 7.0 + width - 2.0;
        frame(&mut width, vec![egui::Event::PointerMoved(pos2(x, y))]);
        frame(&mut width, vec![button(pos2(x, y), true)]);
        for _ in 0..10 {
            x -= 100.0;
            frame(&mut width, vec![egui::Event::PointerMoved(pos2(x, y))]);
        }
        frame(&mut width, vec![button(pos2(x, y), false)]);
        assert_eq!(width, MIN_BROWSER_WIDTH, "the minimum holds");
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
                BROWSER_WIDTH,
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
