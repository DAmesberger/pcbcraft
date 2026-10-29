mod copper_ui;
mod part_editor;
mod pcb_editor;
mod schematic_editor;
mod shell;
mod update_preview;
mod viewport;

use pcbcraft_core::{Engine, Operation, Part, Project};
use pcbcraft_solidcraft::{
    copper::{Geometry, ViewMode},
    project::Source,
};
use shell::Workspace;
use solidcraft_ui_egui::{
    fonts, theme,
    ui_kit::{self, Button, Container, Edge, Surface},
};
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Clone, Debug, PartialEq, Eq)]
enum ModelKey {
    Project(u64),
    Part(Vec<u8>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelKind {
    Project,
    Part,
}

impl ModelKey {
    fn kind(&self) -> ModelKind {
        match self {
            Self::Project(_) => ModelKind::Project,
            Self::Part(_) => ModelKind::Part,
        }
    }
    fn is_current(&self, revision: u64, draft: Option<&Part>) -> bool {
        match self {
            Self::Project(expected) => *expected == revision,
            Self::Part(expected) => {
                draft.and_then(|p| serde_json::to_vec(p).ok()).as_ref() == Some(expected)
            }
        }
    }
}

fn accepts_scene(
    ticket: u64,
    expected_ticket: u64,
    key: Option<&ModelKey>,
    revision: u64,
    draft: Option<&Part>,
) -> bool {
    ticket == expected_ticket && key.is_some_and(|key| key.is_current(revision, draft))
}

/// Return true for redo and false for undo. A focused text editor owns its keys.
fn history_shortcut(ctx: &egui::Context) -> Option<bool> {
    if ctx.egui_wants_keyboard_input() {
        return None;
    }
    if ctx.input_mut(|i| {
        i.consume_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::Z,
        ))
    }) {
        Some(true)
    } else if ctx.input_mut(|i| {
        i.consume_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::COMMAND,
            egui::Key::Z,
        ))
    }) {
        Some(false)
    } else {
        None
    }
}

enum ModelJob {
    Project(u64, Project),
    Part(u64, Part),
    Export {
        revision: u64,
        project: Project,
        path: String,
    },
}
enum ModelUpdate {
    Scene(
        u64,
        std::result::Result<(Vec<Geometry>, Vec<Source>), String>,
    ),
    Saved(u64, std::result::Result<String, String>),
}
struct ModelWorker {
    sender: Sender<ModelJob>,
    receiver: Receiver<ModelUpdate>,
}
impl ModelWorker {
    fn start(ctx: egui::Context) -> Self {
        let (sender, requests) = mpsc::channel();
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(job) = requests.recv() {
                let (ticket, result) = match job {
                    ModelJob::Project(ticket, p) => {
                        (ticket, pcbcraft_solidcraft::project::build(&p))
                    }
                    ModelJob::Part(ticket, p) => {
                        (ticket, pcbcraft_solidcraft::project::preview(&p))
                    }
                    ModelJob::Export {
                        revision,
                        project,
                        path,
                    } => {
                        // Export the exact PCB snapshot captured by the click,
                        // independently of the active editor or preview cache.
                        let result = export_project(&project, &path).map(|()| path);
                        if responses
                            .send(ModelUpdate::Saved(revision, result))
                            .is_err()
                        {
                            break;
                        }
                        ctx.request_repaint();
                        continue;
                    }
                };
                let result = result
                    .map(|model| (model.geometry.clone(), model.sources.clone()))
                    .map_err(|e| e.to_string());
                if responses.send(ModelUpdate::Scene(ticket, result)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self { sender, receiver }
    }
}

fn export_project(project: &Project, path: &str) -> Result<(), String> {
    pcbcraft_solidcraft::project::build(project)
        .and_then(|model| model.save(path))
        .map_err(|error| error.to_string())
}

struct App {
    engine: Engine,
    workspace: Workspace,
    part: part_editor::PartEditor,
    schematic: schematic_editor::SchematicEditor,
    pcb: pcb_editor::PcbEditor,
    copper: copper_ui::CopperUi,
    viewport: viewport::Viewport,
    worker: ModelWorker,
    sources: Vec<Source>,
    model_ticket: u64,
    model_key: Option<ModelKey>,
    model_ready: bool,
    model_kind: Option<ModelKind>,
    status: String,
    project_path: String,
    native_path: String,
    dirty: bool,
    show_browser: bool,
}
impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        theme::set_dark(true);
        theme::apply(&cc.egui_ctx);
        fonts::setup_bundled_fonts(&cc.egui_ctx);
        let rs = cc.wgpu_render_state.as_ref().ok_or("wgpu is required")?;
        let copper = copper_ui::CopperUi::new(cc)?;
        Ok(Self {
            engine: Engine::demo()?,
            workspace: Workspace::Pcb,
            part: Default::default(),
            schematic: Default::default(),
            pcb: Default::default(),
            copper,
            viewport: viewport::Viewport::new(rs),
            worker: ModelWorker::start(cc.egui_ctx.clone()),
            sources: vec![],
            model_ticket: 0,
            model_key: None,
            model_ready: false,
            model_kind: None,
            status: "Ready".into(),
            project_path: default_file_path("untitled.pcbcraft"),
            native_path: default_file_path("board.solidcraft"),
            dirty: false,
            show_browser: true,
        })
    }
    fn changed(&mut self) {
        self.dirty = true;
        self.model_key = None;
        self.model_ready = false;
        self.sources.clear();
    }
    fn apply(&mut self, operation: Option<Operation>) {
        if let Some(operation) = operation {
            match self.engine.apply(self.engine.revision(), operation) {
                Ok(()) => {
                    self.changed();
                    self.status = "Ready".into();
                }
                Err(e) => self.status = e.to_string(),
            }
        }
    }
    fn active_model_key(&self) -> Option<ModelKey> {
        if self.workspace == Workspace::Part && self.part.is_model_view() {
            self.part
                .preview_part()
                .and_then(|part| serde_json::to_vec(part).ok())
                .map(ModelKey::Part)
        } else if self.workspace == Workspace::Pcb && self.pcb.is_3d() {
            Some(ModelKey::Project(self.engine.revision()))
        } else {
            None
        }
    }
    fn model_request(&mut self) {
        let Some(key) = self.active_model_key() else {
            return;
        };
        if self.model_key.as_ref() == Some(&key) {
            return;
        }
        let Some(ticket) = self.model_ticket.checked_add(1) else {
            self.status = "Model request identity exhausted".into();
            self.model_ready = false;
            return;
        };
        let job = match &key {
            ModelKey::Part(_) => match self.part.preview_part() {
                Some(part) => ModelJob::Part(ticket, part.clone()),
                None => return,
            },
            ModelKey::Project(_) => ModelJob::Project(ticket, self.engine.project().clone()),
        };
        self.model_ready = false;
        self.sources.clear();
        match self.worker.sender.send(job) {
            Ok(()) => {
                self.model_ticket = ticket;
                self.model_key = Some(key);
                self.status = "Updating 3D model…".into();
            }
            Err(error) => self.status = error.to_string(),
        }
    }
    fn export_native(&mut self) {
        let job = ModelJob::Export {
            revision: self.engine.revision(),
            project: self.engine.project().clone(),
            path: self.native_path.clone(),
        };
        self.status = match self.worker.sender.send(job) {
            Ok(()) => "Exporting current PCB to Solidcraft…".into(),
            Err(error) => error.to_string(),
        };
    }
    fn receive(&mut self) {
        self.copper.receive();
        while let Ok(message) = self.worker.receiver.try_recv() {
            match message {
                ModelUpdate::Scene(ticket, result)
                    if accepts_scene(
                        ticket,
                        self.model_ticket,
                        self.model_key.as_ref(),
                        self.engine.revision(),
                        self.part.preview_part(),
                    ) =>
                {
                    match result {
                        Ok((geometry, sources)) => {
                            // These colors and camera bounds belong to the request,
                            // never whichever workspace happened to become active.
                            let kind = self.model_key.as_ref().map(ModelKey::kind);
                            let part = kind == Some(ModelKind::Part);
                            let colors = geometry
                                .iter()
                                .map(|g| {
                                    match g
                                        .item
                                        .and_then(|id| sources.iter().find(|s| s.pick_id == id))
                                    {
                                        Some(s) if s.net.is_some() => {
                                            if s.layer == Some(pcbcraft_core::Layer::Bottom) {
                                                [83, 160, 208, 255]
                                            } else {
                                                [196, 139, 70, 255]
                                            }
                                        }
                                        Some(_) => [75, 83, 95, 255],
                                        None => {
                                            if part {
                                                [112, 132, 153, 255]
                                            } else {
                                                [36, 100, 70, 255]
                                            }
                                        }
                                    }
                                })
                                .collect();
                            self.viewport.set_geometry(geometry, ticket);
                            self.viewport.colors(colors);
                            self.viewport.set_mode(ViewMode::Solid);
                            if self.model_kind != kind {
                                self.viewport.fit();
                                self.model_kind = kind;
                            }
                            self.sources = sources;
                            self.model_ready = true;
                            self.status = "Ready".into();
                        }
                        Err(e) => {
                            self.model_ready = false;
                            self.status = e;
                        }
                    }
                }
                ModelUpdate::Saved(revision, result) => {
                    self.status = match result {
                        Ok(path) if revision == self.engine.revision() => {
                            format!("Exported {path}")
                        }
                        Ok(path) => format!(
                            "Exported {path} from project revision {revision}; current revision is {}",
                            self.engine.revision()
                        ),
                        Err(e) => e,
                    }
                }
                _ => {}
            }
        }
    }
    fn save_project(&mut self) {
        match self.engine.save(std::path::Path::new(&self.project_path)) {
            Ok(()) => {
                self.dirty = false;
                self.status = format!("Saved {}", self.project_path);
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    fn app_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = theme::Tokens::get();
        let host = Container::new(&ctx, Surface::ApplicationBar);
        host.edge("application", Edge::Top)
            .exact_size(34.0)
            .frame(host.edge_frame(t.app_bar))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if cfg!(target_os = "macos") {
                        ui.add_space(78.0);
                    }
                    ui.label(
                        egui::RichText::new("Pcbcraft")
                            .font(fonts::bold(t.font_heading))
                            .color(t.app_bar_text),
                    );
                    ui.separator();
                    let file = ui_kit::button(ui, "File", Button::Ghost);
                    Container::new(&ctx, Surface::Popup).menu(&file).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                        shell::form(ui, |ui| {
                            ui.strong("Project");
                            ui.text_edit_singleline(&mut self.project_path);
                            if ui_kit::button(ui, "Save project", Button::Normal).clicked() {
                                self.save_project();
                            }
                            if ui_kit::button(ui, "Open project", Button::Normal).clicked() {
                                match self.engine.open(std::path::Path::new(&self.project_path)) {
                                    Ok(()) => {
                                        self.changed();
                                        self.dirty = false;
                                        self.status = "Opened project".into();
                                        self.pcb.set_selection(None, None, None);
                                    }
                                    Err(e) => self.status = e.to_string(),
                                }
                            }
                            ui.separator();
                            ui.strong("Solidcraft model");
                            ui.text_edit_singleline(&mut self.native_path);
                            if ui_kit::button(
                                ui,
                                "Export .solidcraft",
                                Button::Normal,
                            )
                            .clicked()
                            {
                                self.export_native();
                            }
                            ui.small(
                                "Exports the current PCB project, including placements and copper. Use a new filename.",
                            );
                            ui.separator();
                            if ui_kit::button(ui, "Copper sample · diagnostics", Button::Ghost).clicked() {
                                self.workspace = Workspace::Copper;
                            }
                        });
                    });
                    if shell::icon_button(ui, "save", "Save project", true).clicked() {
                        self.save_project();
                    }
                    if shell::icon_button(ui, "undo", "Undo", self.engine.can_undo()).clicked() {
                        match self.engine.undo() {
                            Ok(()) => self.changed(),
                            Err(e) => self.status = e.to_string(),
                        }
                    }
                    if shell::icon_button(ui, "redo", "Redo", self.engine.can_redo()).clicked() {
                        match self.engine.redo() {
                            Ok(()) => self.changed(),
                            Err(e) => self.status = e.to_string(),
                        }
                    }
                    ui.separator();
                    ui.label(format!(
                        "{}{}",
                        self.engine.project().name,
                        if self.dirty { " •" } else { "" }
                    ));
                    let (_, drag) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width().max(0.0), 26.0),
                        egui::Sense::drag(),
                    );
                    if drag.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                });
            });
    }
    fn tools(&mut self, ui: &mut egui::Ui) {
        let command = match self.workspace {
            Workspace::Part => self.part.tools(ui, &self.engine),
            Workspace::Schematic => self.schematic.tools(ui, &self.engine),
            Workspace::Pcb => self.pcb.tools(ui, &self.engine),
            Workspace::Copper => {
                self.copper.toolbar(ui);
                None
            }
        };
        self.apply(command);
    }
    fn inspector(&mut self, ui: &mut egui::Ui) {
        let command = match self.workspace {
            Workspace::Part => self.part.inspector(ui, &self.engine),
            Workspace::Schematic => self.schematic.inspector(ui, &self.engine),
            Workspace::Pcb => self.pcb.inspector(ui, &self.engine),
            Workspace::Copper => {
                self.copper.inspector(ui);
                None
            }
        };
        self.apply(command);
    }
    fn canvas(&mut self, ui: &mut egui::Ui) {
        if self.workspace == Workspace::Copper {
            self.copper.canvas(ui);
            return;
        }
        let model = self.workspace == Workspace::Pcb && self.pcb.is_3d()
            || self.workspace == Workspace::Part && self.part.is_model_view();
        if model {
            if !self.model_ready || self.model_key != self.active_model_key() {
                ui.centered_and_justified(|ui| {
                    ui.label(&self.status);
                });
                return;
            }
            // Authoring-preview picks must never replace PCB object selection.
            if self.workspace == Workspace::Part {
                self.viewport.highlight(Vec::new());
                self.viewport.show(ui);
                return;
            }
            let selected_instance = self.pcb.selected_instance();
            let selected_track = self.pcb.selected_track();
            let selected_net = self.pcb.highlighted_net();
            let ids = self
                .sources
                .iter()
                .filter(|s| {
                    s.instance.is_some() && s.instance == selected_instance
                        || s.track.is_some() && s.track == selected_track
                        || s.net.as_deref().is_some_and(|n| selected_net == Some(n))
                })
                .map(|s| s.pick_id)
                .collect();
            self.viewport.highlight(ids);
            if let Some(pick) = self.viewport.show(ui) {
                let source = pick.and_then(|id| self.sources.iter().find(|s| s.pick_id == id));
                if let Some(s) = source {
                    self.pcb.set_selection(s.instance, s.track, s.net.clone());
                } else {
                    self.pcb.set_selection(None, None, None);
                }
            }
        } else {
            let command = match self.workspace {
                Workspace::Part => self.part.canvas(ui, &self.engine),
                Workspace::Schematic => self.schematic.canvas(ui, &self.engine),
                Workspace::Pcb => self.pcb.canvas(ui, &self.engine),
                _ => None,
            };
            self.apply(command);
        }
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        // Refresh request identity before draining asynchronous results. This
        // also catches draft edits committed at the end of the previous frame.
        self.model_request();
        self.receive();
        self.app_bar(ui);
        let ctx = ui.ctx().clone();
        let t = theme::Tokens::get();
        let mut workspace = self.workspace;
        shell::ribbon(ui, &mut workspace, |ui| self.tools(ui));
        self.workspace = workspace;
        self.model_request();
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
                            self.viewport.fit();
                        }
                        ui.small("mm");
                        if self.workspace == Workspace::Pcb
                            && ui
                                .checkbox(&mut self.viewport.hide_substrate, "Hide substrate")
                                .changed()
                        {
                            self.viewport.refresh();
                        }
                    });
                });
            });
        Container::new(&ctx, Surface::Viewport)
            .central()
            .frame(egui::Frame::NONE.fill(t.viewport_bottom))
            .show(ui, |ui| {
                let area = ui.max_rect().shrink(t.space_8);
                let side = (area.width() * 0.32).clamp(220.0, 300.0);
                let inspector =
                    egui::Rect::from_min_max(egui::pos2(area.right() - side, area.top()), area.max);
                let browser = self.show_browser && area.width() > 1000.0;
                let left = if browser {
                    area.left() + 230.0 + t.space_8
                } else {
                    area.left()
                };
                let canvas = egui::Rect::from_min_max(
                    egui::pos2(left, area.top()),
                    egui::pos2(inspector.left() - t.space_8, area.bottom()),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(canvas), |ui| {
                    ui.set_clip_rect(canvas);
                    self.canvas(ui);
                });
                if browser {
                    let browser_rect =
                        egui::Rect::from_min_size(area.min, egui::vec2(230.0, 238.0));
                    shell::card(
                        &ctx,
                        "project-browser",
                        browser_rect,
                        "Browser",
                        "folder",
                        |ui| {
                            ui.strong(&self.engine.project().name);
                            ui.separator();
                            for (ws, label) in [
                                (
                                    Workspace::Part,
                                    format!("Components  ·  {}", self.engine.project().parts.len()),
                                ),
                                (
                                    Workspace::Schematic,
                                    format!(
                                        "Schematic  ·  {} symbols",
                                        self.engine.project().instances.len()
                                    ),
                                ),
                                (
                                    Workspace::Pcb,
                                    format!(
                                        "PCB  ·  {} tracks",
                                        self.engine.project().tracks.len()
                                    ),
                                ),
                            ] {
                                if ui_kit::button(ui, label, Button::Ghost).clicked() {
                                    self.workspace = ws;
                                }
                            }
                            ui.separator();
                            ui.small("Project snapshots · exact revisions");
                        },
                    );
                }
                shell::card(
                    &ctx,
                    "properties",
                    inspector,
                    &format!("{} properties", self.workspace.label()),
                    "settings",
                    |ui| self.inspector(ui),
                );
            });
        if ctx.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::S,
            ))
        }) {
            self.save_project();
        }
        // Solidcraft lib.rs scopes document shortcuts with this same typing
        // guard. Text fields retain their own local undo history.
        if let Some(redo) = history_shortcut(&ctx) {
            let result = if redo {
                self.engine.redo()
            } else {
                self.engine.undo()
            };
            match result {
                Ok(()) => self.changed(),
                Err(e) => self.status = e.to_string(),
            }
        }
        // Inspector edits and browser workspace changes happen after canvas.
        // Invalidate their preview immediately, before the next interaction.
        self.model_request();
    }
}
// Bundled macOS apps start in `/`, which cannot be used as a save directory.
fn default_file_path(name: &str) -> String {
    let directory = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    directory.join(name).to_string_lossy().into_owned()
}

fn main() -> eframe::Result {
    let compact = std::env::args().any(|argument| argument == "--compact");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(if compact {
                [900.0, 680.0]
            } else {
                [1380.0, 900.0]
            })
            .with_min_inner_size([760.0, 560.0])
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_fullsize_content_view(true),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Pcbcraft",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)?))),
    )
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn document_edits_and_newer_requests_reject_old_preview_results()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let key = ModelKey::Project(engine.revision());
        assert!(accepts_scene(10, 10, Some(&key), engine.revision(), None));
        engine.apply(
            engine.revision(),
            Operation::Layout(pcbcraft_core::layout::LayoutCommand::Move {
                instance: 2,
                position: [12.0, 12.0],
                rotation_deg: 0.0,
                layer: pcbcraft_core::Layer::Top,
            }),
        )?;
        assert!(!accepts_scene(10, 10, Some(&key), engine.revision(), None));
        assert!(!accepts_scene(
            10,
            11,
            Some(&ModelKey::Project(engine.revision())),
            engine.revision(),
            None
        ));
        assert!(!accepts_scene(10, 10, None, engine.revision(), None));
        Ok(())
    }

    #[test]
    fn edited_component_draft_rejects_its_previous_preview()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut draft = pcbcraft_core::parts::passive_template("R");
        let key = ModelKey::Part(serde_json::to_vec(&draft)?);
        assert!(accepts_scene(1, 1, Some(&key), 7, Some(&draft)));
        draft.package.body_size[0] += 1.0;
        assert!(!accepts_scene(1, 1, Some(&key), 7, Some(&draft)));
        assert_ne!(key, ModelKey::Project(7));
        Ok(())
    }

    fn undo_input() -> egui::RawInput {
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn typing_owns_undo_instead_of_undoing_the_project() {
        let ctx = egui::Context::default();
        let mut text = "10k".to_owned();
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"))
                .request_focus();
        });
        let mut shortcut = Some(false);
        let _ = ctx.run_ui(undo_input(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"));
            shortcut = history_shortcut(ui.ctx());
        });
        assert_eq!(shortcut, None);
        let unfocused = egui::Context::default();
        let _ = unfocused.run_ui(undo_input(), |ui| {
            shortcut = history_shortcut(ui.ctx());
        });
        assert_eq!(shortcut, Some(false));
    }

    #[test]
    fn export_validates_its_pcb_snapshot_even_after_a_valid_part_preview()
    -> Result<(), Box<dyn std::error::Error>> {
        let worker = ModelWorker::start(egui::Context::default());
        worker.sender.send(ModelJob::Part(
            1,
            pcbcraft_core::parts::passive_template("Preview"),
        ))?;
        let response = worker
            .receiver
            .recv_timeout(std::time::Duration::from_secs(30))?;
        assert!(matches!(response, ModelUpdate::Scene(1, Ok(_))));
        // Invalid PCB geometry must fail instead of exporting the worker's
        // previous, valid component model under the board filename.
        let mut project = Project::default();
        project.board_size[0] = 0.0;
        worker.sender.send(ModelJob::Export {
            revision: 9,
            project,
            path: "invalid-export.solidcraft".into(),
        })?;
        let response = worker
            .receiver
            .recv_timeout(std::time::Duration::from_secs(30))?;
        match response {
            ModelUpdate::Saved(9, Err(message)) => assert!(message.contains("board dimensions")),
            _ => return Err("export must reject its invalid PCB snapshot".into()),
        }
        Ok(())
    }
}
