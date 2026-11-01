//! The desktop application: one engine, three editors and the file workflow.
//!
//! Editors and the 3D view never change the document themselves. They return
//! an [`Operation`], which [`App::apply`] sends through the engine, the same
//! path the CLI and MCP use. Long work (3D builds, file I/O, recovery) runs on
//! background threads and reports back with the input it was made from.
mod browser;
mod chrome;

use crate::file_ui::{self, Effect, FileUi};
use crate::library_sources::{self, BundlePurpose, LibrarySources};
use crate::place_palette::PaletteEvent;
use crate::scene::{Display, ModelKey, SceneController};
use crate::shell::Workspace;
use crate::{part_editor, pcb_editor, schematic_editor, viewport};
use pcbcraft_core::{Engine, Operation, format::EditorFile};
use solidcraft_ui_egui::{fonts, theme};
use std::path::PathBuf;
use std::sync::Arc;

/// An import made for an update review that is still open.
struct PendingImport {
    /// The engine revision right after the import.
    revision: u64,
    name: String,
    target: pcbcraft_core::Revision,
}

pub struct App {
    engine: Engine,
    workspace: Workspace,
    part: part_editor::PartEditor,
    schematic: schematic_editor::SchematicEditor,
    pcb: pcb_editor::PcbEditor,
    scene: SceneController,
    files: FileUi,
    /// Library folders, favourites and recents (S3).
    sources: LibrarySources,
    status: Option<chrome::Status>,
    /// Browser shown by width, or as the user toggled it (⌘B).
    browser: chrome::BrowserMode,
    /// Whether the last layout showed the Browser.
    browser_visible: bool,
    /// "Update project copy" imported a revision at this engine revision;
    /// cancelling its review undoes that import (see `review_closed`).
    pending_import: Option<PendingImport>,
    closing: bool,
    /// Input time of the current frame, for status messages.
    now: f64,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        follow_system_theme(&cc.egui_ctx, true);
        fonts::setup_bundled_fonts(&cc.egui_ctx);
        let render_state = cc.wgpu_render_state.as_ref().ok_or("wgpu is required")?;
        let engine = Engine::demo()?;
        let mut files = FileUi::new(cc, &engine);
        // `pcbcraft FILE.pcbcraft` opens that project through the file workflow.
        if let Some(path) = std::env::args_os()
            .skip(1)
            .map(PathBuf::from)
            .find(|p| p.extension().is_some_and(|e| e == "pcbcraft"))
        {
            files.request(file_ui::Action::OpenPath(path));
        }
        Ok(Self {
            scene: SceneController::new(
                cc.egui_ctx.clone(),
                viewport::Viewport::new(render_state, &cc.egui_ctx),
            ),
            files,
            sources: LibrarySources::from_env(&cc.egui_ctx),
            engine,
            workspace: Workspace::Pcb,
            part: Default::default(),
            schematic: Default::default(),
            pcb: Default::default(),
            status: None,
            browser: chrome::BrowserMode::Auto,
            browser_visible: false,
            pending_import: None,
            closing: false,
            now: 0.0,
        })
    }

    /// The one path from UI gestures to document changes. It carries the
    /// footprint evaluator CLI and MCP use, so sourced footprints edit and
    /// publish the same way here.
    fn apply(&mut self, operation: Option<Operation>) {
        let Some(operation) = operation else {
            return;
        };
        if self.files.busy() {
            self.say("Wait for the file operation to finish", false);
            return;
        }
        match self.engine.apply_with(
            self.engine.revision(),
            operation,
            &pcbcraft_solidcraft::footprint::SketchEvaluator,
        ) {
            Ok(_) => self.clear_error(),
            Err(error) => self.say(error.to_string(), true),
        }
    }

    /// Show a status message in the canvas; errors stay until the next
    /// successful action, a workspace change or a timeout.
    fn say(&mut self, text: impl Into<String>, error: bool) {
        self.status = Some(chrome::Status {
            text: text.into(),
            error,
            at: self.now,
        });
    }

    fn clear_error(&mut self) {
        if self.status.as_ref().is_some_and(|s| s.error) {
            self.status = None;
        }
    }

    /// Requests that need several public commands, such as reserving new
    /// lineages for a component. Each command is its own undo step.
    fn part_requests(&mut self) {
        let Some(request) = self.part.take_request() else {
            return;
        };
        if self.files.busy() {
            self.say("Wait for the file operation to finish", false);
            return;
        }
        match part_editor::run(&mut self.engine, request) {
            Ok(None) => self.clear_error(),
            Ok(Some(created)) => self.say(created, false),
            Err(error) => self.say(error.to_string(), true),
        }
    }

    /// Library sources before the editors draw: finished background work,
    /// and the palette's context when it changed.
    fn library_sources(&mut self, ctx: &egui::Context) {
        let busy = self.files.busy();
        self.sources.tick(ctx, &mut self.engine, busy);
        for notice in self.sources.take_notices() {
            self.say(notice.text, notice.error);
        }
        let updates = self.sources.updates(&self.engine);
        self.schematic.set_updates(updates);
        let sources = &self.sources;
        self.schematic
            .palette_mut()
            .set_context(sources.generation(), || sources.context());
    }

    /// What the palette, the Schematic instance properties and the
    /// Component workspace asked of the library sources this frame. Folder
    /// bundles are read on a worker; the engine work happens when they
    /// arrive (see `library_bundles`).
    fn library_requests(&mut self, ctx: &egui::Context) {
        let events = self.schematic.palette_mut().take_events();
        let action = library_sources::take_component_action(ctx);
        let instance_update = self.schematic.take_library_update();
        let mutating = events.iter().any(|e| {
            matches!(
                e,
                PaletteEvent::PlaceFrom { .. } | PaletteEvent::UpdateProjectCopy(_)
            )
        }) || instance_update.is_some()
            || matches!(
                action,
                Some(library_sources::ComponentAction::Duplicate { .. })
            );
        if mutating && self.files.busy() {
            self.say("Wait for the file operation to finish", false);
            return;
        }
        for event in events {
            match event {
                PaletteEvent::Used(key) => self.sources.used(key),
                PaletteEvent::ToggleFavourite(key) => self.sources.toggle_favourite(key),
                PaletteEvent::PlaceFrom {
                    pick,
                    other_revision,
                } => self
                    .sources
                    .read_bundle(pick, BundlePurpose::Place(other_revision)),
                PaletteEvent::UpdateProjectCopy(pick) => {
                    self.sources.read_bundle(pick, BundlePurpose::Update(None));
                }
                PaletteEvent::ManageSources => self.sources.open_card(),
            }
        }
        if let Some((instance, pick)) = instance_update {
            self.sources
                .read_bundle(pick, BundlePurpose::Update(Some(vec![instance])));
        }
        match action {
            Some(library_sources::ComponentAction::Promote { component, name }) => {
                self.sources.begin_promote(component, name);
            }
            Some(library_sources::ComponentAction::Duplicate { component, name }) => {
                match library_sources::duplicate(&mut self.engine, component, &name) {
                    Ok(lineage) => {
                        self.part.open(lineage, &self.engine);
                        self.say(format!("Duplicated as {name}"), false);
                    }
                    Err(error) => self.say(error, true),
                }
            }
            None => {}
        }
        self.library_bundles();
        self.review_closed();
    }

    /// Folder bundles that finished reading: place or import them now.
    fn library_bundles(&mut self) {
        if self.files.busy() {
            // The file operation owns the document; they wait for it.
            return;
        }
        for read in self.sources.take_bundles() {
            let bundle = match read.result {
                Ok(bundle) => bundle,
                Err(error) => {
                    match read.purpose {
                        BundlePurpose::Place(_) => self.schematic.palette_mut().refuse(error),
                        BundlePurpose::Update(_) => self.say(error, true),
                    }
                    continue;
                }
            };
            match read.purpose {
                BundlePurpose::Place(other_revision) => {
                    self.place_from(read.pick, bundle, other_revision);
                }
                BundlePurpose::Update(only) => {
                    self.update_project_copy(read.pick, bundle, only.as_deref());
                }
            }
        }
    }

    /// `library.place_from` of a folder bundle; another project revision
    /// reopens the palette on the Update project copy… / Keep both / Cancel
    /// choice. A placed instance is selected and brought into view.
    fn place_from(
        &mut self,
        pick: library_sources::FolderPick,
        bundle: pcbcraft_core::library::Bundle,
        other_revision: pcbcraft_core::library::import::OtherRevision,
    ) {
        let position = self.schematic.place_point();
        match library_sources::place(&mut self.engine, bundle, position, other_revision) {
            Ok(library_sources::Placed::Placed {
                designator,
                instance,
                position,
            }) => {
                self.workspace = Workspace::Schematic;
                self.schematic
                    .reveal_placed(instance, position, self.engine.project());
                self.say(format!("Placed {designator} from the library"), false);
            }
            Ok(library_sources::Placed::OtherRevision { message }) => {
                self.schematic.palette_mut().offer_choice(pick, message);
            }
            Err(error) => self.schematic.palette_mut().refuse(error),
        }
    }

    /// "Update project copy": `library.import`, then the update review with
    /// a checkbox per instance. Cancelling the review undoes the import.
    fn update_project_copy(
        &mut self,
        pick: library_sources::FolderPick,
        bundle: pcbcraft_core::library::Bundle,
        only: Option<&[pcbcraft_core::Id]>,
    ) {
        match library_sources::import_for_update(&mut self.engine, bundle, only) {
            Ok(import) => {
                let copied = import.imported.is_some();
                self.pending_import = import.imported.map(|revision| PendingImport {
                    revision,
                    name: pick.name.clone(),
                    target: import.target.revision,
                });
                match import.change {
                    Some(change) => {
                        let note = copied.then(|| {
                            format!(
                                "{} {} was copied into the project as {}. Cancel removes it again.",
                                pick.name, pick.revision, import.target.revision
                            )
                        });
                        self.workspace = Workspace::Schematic;
                        self.schematic.review_update(&self.engine, change, note);
                    }
                    None => {
                        self.pending_import = None;
                        self.say(
                            format!(
                                "{} {} is in the project as {}; nothing placed uses an older revision",
                                pick.name, pick.revision, import.target.revision
                            ),
                            false,
                        );
                    }
                }
            }
            Err(error) => self.say(error, true),
        }
    }

    /// The update review closed: an import made for it stays when it was
    /// applied, and is undone when it was cancelled (while it is still the
    /// last step).
    fn review_closed(&mut self) {
        let Some(closed) = self.schematic.take_review_closed() else {
            return;
        };
        let Some(pending) = self.pending_import.take() else {
            return;
        };
        if closed != crate::update_preview::Closed::Cancelled {
            return;
        }
        if self.engine.revision() == pending.revision && self.engine.undo().is_ok() {
            self.say(
                format!(
                    "Cancelled: {} {} was removed from the project again",
                    pending.name, pending.target
                ),
                false,
            );
        } else {
            self.say(
                format!(
                    "{} {} stays in the project; no instance changed. Undo removes it.",
                    pending.name, pending.target
                ),
                false,
            );
        }
    }

    /// Show or hide the Browser (⌘B, the app bar's Browser button and the
    /// File menu). In a narrow window it shows over the canvas.
    fn toggle_browser(&mut self) {
        self.browser = self.browser.toggled(self.browser_visible);
    }

    fn history(&mut self, redo: bool) {
        let result = if redo {
            self.engine.redo()
        } else {
            self.engine.undo()
        };
        match result {
            Ok(_) => self.clear_error(),
            Err(error) => self.say(error.to_string(), true),
        }
    }

    fn replace(&mut self, document: EditorFile, path: Option<PathBuf>, recovered: bool) {
        let name = path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| document.project.name.clone());
        if let Err(error) = self.engine.replace_document(document) {
            self.say(format!("Open failed: {error}"), true);
            return;
        }
        self.sources.forget_project();
        self.pending_import = None;
        self.pcb.set_selection(None, None, None);
        match self.files.adopt(&self.engine, path, recovered) {
            Ok(()) if recovered => self.say(format!("Recovered {name}; save it to keep it"), false),
            Ok(()) => self.say(format!("Opened {name}"), false),
            Err(error) => self.say(error, true),
        }
    }

    fn file_effects(&mut self, ctx: &egui::Context) {
        for effect in self.files.tick(ctx) {
            match effect {
                Effect::Replace {
                    document,
                    path,
                    recovered,
                } => self.replace(*document, path, recovered),
                Effect::Close => {
                    self.closing = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if let Some(status) = self.files.take_status() {
            let error = status.to_ascii_lowercase().contains("fail");
            self.say(status, error);
        }
    }

    /// The model the active view displays, if it is a 3D view.
    fn model_key(&self) -> Option<ModelKey> {
        match self.workspace {
            Workspace::Part if self.part.is_model_view() => self
                .part
                .preview_package()
                .as_ref()
                .and_then(ModelKey::package),
            Workspace::Pcb if self.pcb.is_3d() => {
                Some(ModelKey::Project(Arc::clone(self.engine.project_arc())))
            }
            _ => None,
        }
    }

    fn request_model(&mut self) {
        if let Some(key) = self.model_key() {
            self.scene
                .request(key, self.part.preview_package().as_ref());
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let operation = match self.workspace {
            Workspace::Part => self.part.inspector(ui, &self.engine),
            Workspace::Schematic => self.schematic.inspector(ui, &self.engine),
            Workspace::Pcb => self.pcb.inspector(ui, &self.engine),
        };
        self.apply(operation);
        if let Some((component, values)) = self.schematic.take_new_part() {
            self.clear_error();
            self.workspace = Workspace::Part;
            self.part.new_part_for(component, values, &self.engine);
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let Some(key) = self.model_key() else {
            let operation = match self.workspace {
                Workspace::Part => self.part.canvas(ui, &self.engine),
                Workspace::Schematic => self.schematic.canvas(ui, &self.engine),
                Workspace::Pcb => self.pcb.canvas(ui, &self.engine),
            };
            self.apply(operation);
            return;
        };
        let display = self.scene.display(&key);
        let note = match self.scene.status() {
            crate::scene::Status::Failed(error) => error.clone(),
            _ => "Updating 3D model…".into(),
        };
        if display == Display::Blank {
            ui.centered_and_justified(|ui| ui.label(note));
            return;
        }
        let area = ui.available_rect_before_wrap();
        // Component previews never change PCB selection.
        let pick = if self.workspace == Workspace::Part {
            // Component 3D shows every layer of its package.
            self.scene.viewport.set_layers(Default::default());
            self.scene.highlight(None, None, None);
            self.part
                .model_view_prepare(&mut self.scene.viewport, &self.engine);
            self.scene.viewport.show(ui);
            let operation = self.part.model_view(ui, &self.scene.viewport, &self.engine);
            self.apply(operation);
            None
        } else {
            crate::part_editor::reset_model_view(&mut self.scene.viewport);
            self.scene.viewport.set_layers(self.pcb.layer_view());
            self.scene.highlight(
                self.pcb.selected_instance(),
                self.pcb.selected_track(),
                self.pcb.highlighted_net(),
            );
            self.scene.viewport.show(ui)
        };
        if display == Display::Stale {
            // The last model stays up while a newer one builds; picks on it
            // could name objects that no longer exist, so they are ignored.
            ui.put(
                egui::Rect::from_center_size(
                    area.center_top() + egui::vec2(0.0, 24.0),
                    egui::vec2(area.width(), 20.0),
                ),
                egui::Label::new(note),
            );
            return;
        }
        if let Some(pick) = pick {
            match pick.and_then(|id| self.scene.source(id)) {
                Some(source) => {
                    let (instance, track, net) = (source.instance, source.track, source.net);
                    self.pcb.set_selection(instance, track, net);
                }
                None => self.pcb.set_selection(None, None, None),
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let pressed = |modifiers, key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(modifiers, key)))
        };
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        if pressed(shift, Key::S) {
            self.files.request(file_ui::Action::SaveAs);
        } else if pressed(Modifiers::COMMAND, Key::S) {
            self.files.request(file_ui::Action::Save);
        }
        if pressed(Modifiers::COMMAND, Key::O) {
            self.files.request(file_ui::Action::Open);
        }
        if pressed(Modifiers::COMMAND, Key::N) {
            self.files.request(file_ui::Action::New);
        }
        if !ctx.egui_wants_keyboard_input() && pressed(Modifiers::COMMAND, Key::B) {
            self.toggle_browser();
        }
        if let Some(redo) = history_shortcut(ctx) {
            self.history(redo);
        }
    }

    /// Closing with unsaved work asks first; the file workflow closes the
    /// window once recovery data is cleaned up.
    fn guard_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.files.request(file_ui::Action::Close);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        follow_system_theme(&ctx, false);
        self.scene.receive();
        self.file_effects(&ctx);
        self.library_sources(&ctx);
        self.app_bar(ui);
        self.ribbon(ui);
        self.workspace_area(ui);
        // Draft edits commit once their gesture ends, as one undo step.
        self.part_requests();
        self.library_requests(&ctx);
        let draft = self.part.commit(&ctx, &self.engine);
        self.apply(draft);
        self.shortcuts(&ctx);
        self.guard_close(&ctx);
        self.files.observe(&self.engine, ctx.input(|i| i.time));
        self.files.dialogs(&ctx);
        self.sources.dialogs(&ctx);
        self.request_model();
    }
}

/// Follow the operating system's light or dark appearance, like Solidcraft's
/// default "system" theme.
fn follow_system_theme(ctx: &egui::Context, force: bool) {
    let dark = ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark);
    if force || dark != theme::is_dark() {
        theme::set_dark(dark);
        theme::apply(ctx);
    }
}

/// `Some(true)` for redo, `Some(false)` for undo. A focused text field owns
/// its own undo keys, as in Solidcraft.
fn history_shortcut(ctx: &egui::Context) -> Option<bool> {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if ctx.egui_wants_keyboard_input() {
        return None;
    }
    if ctx.input_mut(|i| {
        i.consume_shortcut(&KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::Z,
        ))
    }) {
        Some(true)
    } else if ctx
        .input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z)))
    {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Run one headless frame. Its texture changes are cleared: there is no
    /// renderer to apply them, and egui's debug check rejects dropping them.
    fn run(ctx: &egui::Context, input: egui::RawInput, add: impl FnMut(&mut egui::Ui)) {
        ctx.run_ui(input, add).textures_delta.clear();
    }

    #[test]
    fn typing_owns_undo_instead_of_undoing_the_project() {
        let ctx = egui::Context::default();
        let mut text = "10k".to_owned();
        run(&ctx, Default::default(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"))
                .request_focus();
        });
        let mut shortcut = Some(false);
        run(&ctx, undo_input(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id_salt("value"));
            shortcut = history_shortcut(ui.ctx());
        });
        assert_eq!(shortcut, None);
        let unfocused = egui::Context::default();
        run(&unfocused, undo_input(), |ui| {
            shortcut = history_shortcut(ui.ctx());
        });
        assert_eq!(shortcut, Some(false));
    }
}
