//! The desktop application: one engine, three editors and the file workflow.
//!
//! Editors and the 3D view never change the document themselves. They return
//! an [`Operation`], which [`App::apply`] sends through the engine, the same
//! path the CLI and MCP use. Long work (3D builds, file I/O, recovery) runs on
//! background threads and reports back with the input it was made from.
mod browser;
mod browser_catalog;
mod chrome;

use crate::file_ui::{self, Effect, FileUi};
use crate::kit::messages::Messages;
use crate::kit::severity::Level;
use crate::kit::toast::{ToastAction, ToastEvent, Toasts};
use crate::library_sources::{self, BundlePurpose, LibrarySources};
use crate::place_palette::PaletteEvent;
use crate::refusal::{self, Origin, Refused};
use crate::scene::{Display, ModelKey, SceneController};
use crate::shell::Workspace;
use crate::{part_editor, pcb_editor, schematic_editor, viewport};
use pcbcraft_core::{Engine, Operation, format::EditorFile};
use solidcraft_ui_egui::{fonts, theme};
use std::path::PathBuf;
use std::sync::Arc;

/// The longest toast text; Messages keeps the whole message.
const MAX_TOAST_CHARS: usize = pcbcraft_core::issues::MAX_TITLE;

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
    /// Command results and refusals in the canvas (slice K3).
    toasts: Toasts,
    /// Every message of this run, with the full engine text.
    messages: Messages,
    messages_open: bool,
    /// The Issues card, canvas badge and pill, and their cached answers.
    issues: crate::issues::Surface,
    /// The engine revision a toast's Undo would undo; Undo is withdrawn
    /// once another step follows.
    undo_revision: Option<u64>,
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
            toasts: Toasts::default(),
            messages: Messages::default(),
            messages_open: false,
            issues: Default::default(),
            undo_revision: None,
            browser: chrome::BrowserMode::Auto,
            browser_visible: false,
            pending_import: None,
            closing: false,
            now: 0.0,
        })
    }

    /// The one path from UI gestures to document changes. It carries the
    /// footprint evaluator CLI and MCP use, so sourced footprints edit and
    /// publish the same way here. A refusal goes back to `default` unless
    /// the editor marked another origin this frame (`refusal::mark`).
    fn apply(&mut self, ctx: &egui::Context, default: Origin, operation: Option<Operation>) {
        let Some(operation) = operation else {
            return;
        };
        let origin = refusal::take_mark(ctx).unwrap_or(default);
        if self.files.busy() {
            self.say(Level::Info, "Wait for the file operation to finish");
            return;
        }
        match self.engine.apply_with(
            self.engine.revision(),
            operation,
            &pcbcraft_solidcraft::footprint::SketchEvaluator,
        ) {
            Ok(_) => self.clear_error(),
            Err(error) => self.refuse(ctx, origin, error),
        }
    }

    /// A refused operation: Messages keeps the full engine text; a field or
    /// dialog gets it back on the next frame and shows it itself; a canvas
    /// gesture or a command shows a toast (the gesture's editor may still
    /// claim it, to keep its gesture going).
    fn refuse(&mut self, ctx: &egui::Context, origin: Origin, error: pcbcraft_core::Error) {
        let level = refusal::level(&error);
        self.messages.push(level, &error.to_string(), self.now);
        match origin {
            Origin::Field(_) | Origin::Dialog(_) => {
                refusal::hand_back(ctx, Refused { origin, error }, false);
            }
            Origin::Canvas => {
                self.toast(
                    level,
                    refusal::shown_error(&error),
                    vec![ToastAction::Details],
                );
                refusal::hand_back(ctx, Refused { origin, error }, true);
            }
            Origin::Command => self.command_refused(error, false),
        }
    }

    /// A ribbon, menu or keyboard command was refused: a toast, and the
    /// full engine text in Messages unless `refuse` already put it there.
    fn command_refused(&mut self, error: pcbcraft_core::Error, record: bool) {
        let level = refusal::level(&error);
        if record {
            self.messages.push(level, &error.to_string(), self.now);
        }
        self.toast(
            level,
            refusal::shown_error(&error),
            vec![ToastAction::Details],
        );
    }

    /// Refusals handed back last frame that no editor claimed: they show
    /// as toasts after all (Messages already has them).
    fn unclaimed_refusals(&mut self, ctx: &egui::Context) {
        for refused in refusal::unclaimed(ctx) {
            let level = refusal::level(&refused.error);
            self.toast(
                level,
                refusal::shown_error(&refused.error),
                vec![ToastAction::Details],
            );
        }
    }

    /// Tell the person something: a toast now, and the same text in
    /// Messages. The severity is explicit, never guessed from the text.
    fn say(&mut self, level: Level, text: impl Into<String>) {
        self.say_with(level, text, Vec::new());
    }

    /// [`Self::say`] with toast actions. Undo applies to the step just made.
    fn say_with(&mut self, level: Level, text: impl Into<String>, actions: Vec<ToastAction>) {
        let text = text.into();
        self.messages.push(level, &text, self.now);
        let shown = refusal::clip(text.lines().next().unwrap_or_default(), MAX_TOAST_CHARS);
        if actions.contains(&ToastAction::Undo) {
            self.undo_revision = Some(self.engine.revision());
        }
        self.toast(level, shown, actions);
    }

    /// A toast only: its full text is in Messages already.
    fn toast(&mut self, level: Level, text: String, actions: Vec<ToastAction>) {
        self.toasts.push(level, text, actions, self.now);
    }

    /// A successful action or a workspace change makes error toasts stale.
    fn clear_error(&mut self) {
        self.toasts.dismiss_errors();
    }

    /// Toasts editors posted this frame, and Undo withdrawn once the step it
    /// names is no longer the last.
    fn feedback(&mut self, ctx: &egui::Context) {
        for posted in crate::kit::toast::take_posted(ctx) {
            self.say_with(posted.level, posted.text, posted.actions);
        }
        if self
            .undo_revision
            .is_some_and(|revision| revision != self.engine.revision())
        {
            self.toasts.withdraw(ToastAction::Undo);
            self.undo_revision = None;
        }
    }

    /// What a toast's click asked for.
    pub(super) fn toast_event(&mut self, event: ToastEvent) {
        match event {
            ToastEvent::Action {
                action: ToastAction::Undo,
                ..
            } => {
                if self.undo_revision == Some(self.engine.revision()) {
                    self.toasts.withdraw(ToastAction::Undo);
                    self.undo_revision = None;
                    self.history(false);
                }
            }
            ToastEvent::Action {
                action: ToastAction::Details,
                ..
            }
            | ToastEvent::OpenMessages => self.messages_open = true,
            // Show opens Issues, where every finding locates.
            ToastEvent::Action {
                action: ToastAction::Show,
                ..
            } => self.issues.open(),
        }
    }

    /// The Messages window, while open.
    fn messages_window(&mut self, ctx: &egui::Context) {
        if self.messages_open && self.messages.window(ctx).closed {
            self.messages_open = false;
        }
    }

    /// Requests that need several public commands, such as reserving new
    /// lineages for a component. Each command is its own undo step.
    fn part_requests(&mut self) {
        let Some(request) = self.part.take_request() else {
            return;
        };
        if self.files.busy() {
            self.say(Level::Info, "Wait for the file operation to finish");
            return;
        }
        match part_editor::run(&mut self.engine, request) {
            Ok(None) => self.clear_error(),
            Ok(Some(created)) => self.say(Level::Ok, created),
            Err(error) => self.command_refused(error, true),
        }
    }

    /// Library sources before the editors draw: finished background work,
    /// and the palette's context when it changed.
    fn library_sources(&mut self, ctx: &egui::Context) {
        let busy = self.files.busy();
        self.sources.tick(ctx, &mut self.engine, busy);
        for notice in self.sources.take_notices() {
            let level = if notice.error {
                Level::Error
            } else {
                Level::Ok
            };
            self.say(level, notice.text);
        }
        let updates = self.sources.updates(&self.engine);
        self.schematic.set_updates(updates);
        let sources = &self.sources;
        self.schematic
            .palette_mut()
            .set_context(sources.generation(), || sources.context());
        let loading = sources.loading();
        self.schematic.palette_mut().set_loading(loading);
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
            self.say(Level::Info, "Wait for the file operation to finish");
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
                        self.say_with(
                            Level::Ok,
                            format!("Duplicated as {name}"),
                            vec![ToastAction::Undo],
                        );
                    }
                    Err(error) => self.say(Level::Error, error),
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
                        BundlePurpose::Update(_) => self.say(Level::Error, error),
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
                self.say_with(
                    Level::Ok,
                    format!("Placed {designator} from the library"),
                    vec![ToastAction::Undo],
                );
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
                            Level::Info,
                            format!(
                                "{} {} is in the project as {}; nothing placed uses an older revision",
                                pick.name, pick.revision, import.target.revision
                            ),
                        );
                    }
                }
            }
            Err(error) => self.say(Level::Error, error),
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
                Level::Info,
                format!(
                    "Cancelled: {} {} was removed from the project again",
                    pending.name, pending.target
                ),
            );
        } else {
            self.say(
                Level::Info,
                format!(
                    "{} {} stays in the project; no instance changed. Undo removes it.",
                    pending.name, pending.target
                ),
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
            Err(error) => self.command_refused(error, true),
        }
    }

    fn replace(&mut self, document: EditorFile, path: Option<PathBuf>, recovered: bool) {
        let name = path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| document.project.name.clone());
        if let Err(error) = self.engine.replace_document(document) {
            self.say(Level::Error, format!("Open failed: {error}"));
            return;
        }
        self.sources.forget_project();
        self.pending_import = None;
        self.pcb.set_selection(None, None, None);
        match self.files.adopt(&self.engine, path, recovered) {
            Ok(()) if recovered => {
                self.say(Level::Info, format!("Recovered {name}; save it to keep it"));
            }
            Ok(()) => self.say(Level::Ok, format!("Opened {name}")),
            Err(error) => self.say(Level::Error, error),
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
        if let Some((level, status)) = self.files.take_status() {
            self.say(level, status);
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

    /// New Part… from the schematic opens the Part dialog; its return link
    /// comes back to the instance.
    fn open_new_part(&mut self) {
        if let Some((component, values)) = self.schematic.take_new_part() {
            self.clear_error();
            // The Part dialog returns to the instance the request came from.
            let back = self.schematic.selected_instance().map_or_else(
                crate::part_editor::PartReturn::default,
                |id| crate::part_editor::PartReturn {
                    instance: Some(id),
                    designator: self
                        .engine
                        .project()
                        .instance(id)
                        .map(|i| i.designator.clone())
                        .unwrap_or_default(),
                },
            );
            self.workspace = Workspace::Part;
            self.part.new_part_for(component, values, &self.engine);
            self.part.part_return_to(Some(back));
        }
        if let Some(back) = self.part.take_part_return() {
            self.workspace = Workspace::Schematic;
            if back.instance.is_some() {
                self.schematic
                    .select_instance(back.instance, self.engine.project());
            }
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let Some(key) = self.model_key() else {
            let operation = match self.workspace {
                Workspace::Part => self.part.canvas(ui, &self.engine),
                Workspace::Schematic => self.schematic.canvas(ui, &self.engine),
                Workspace::Pcb => self.pcb.canvas(ui, &self.engine),
            };
            self.apply(&ui.ctx().clone(), Origin::Canvas, operation);
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
            self.apply(&ui.ctx().clone(), Origin::Canvas, operation);
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
                // A via body selects the via (and lights its net).
                Some(source) if source.via.is_some() && source.instance.is_none() => {
                    if let Some(via) = source.via {
                        self.pcb.select_via(via, source.net);
                    }
                }
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
        self.apply(&ctx, Origin::Command, draft);
        self.shortcuts(&ctx);
        self.guard_close(&ctx);
        self.files.observe(&self.engine, ctx.input(|i| i.time));
        self.files.dialogs(&ctx);
        self.sources.dialogs(&ctx);
        self.messages_window(&ctx);
        self.feedback(&ctx);
        self.unclaimed_refusals(&ctx);
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
