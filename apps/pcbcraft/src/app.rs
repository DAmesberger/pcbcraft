//! The desktop application: one engine, three editors and the file workflow.
//!
//! Editors and the 3D view never change the document themselves. They return
//! an [`Operation`], which [`App::apply`] sends through the engine, the same
//! path the CLI and MCP use. Long work (3D builds, file I/O, recovery) runs on
//! background threads and reports back with the input it was made from.
mod browser;
mod browser_catalog;
mod chrome;
mod command_dispatch;
mod command_palette;
mod key_reveal;
mod list_prefs;
mod live_mcp;
mod open_timing;
mod settings_window;
mod shortcut_sheet;
mod system_windows;

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
use pcbcraft_core::layout::fill::{FillCommand, RefillTarget, StoredFill};
use pcbcraft_core::{Engine, Operation, format::EditorFile};
use pcbcraft_ui::fonts;
use pcbcraft_ui::theme::{self as theme};
use std::path::PathBuf;
use std::sync::Arc;

/// An import made for an update review that is still open.
struct PendingImport {
    /// The engine revision right after the import.
    revision: u64,
    name: String,
    target: pcbcraft_core::Revision,
}

/// What [`App::with_parts`] builds the app around.
struct Parts {
    engine: Engine,
    files: FileUi,
    sources: LibrarySources,
    registry: Arc<crate::commands::Registry>,
    keymap_problems: Vec<crate::keymap::Unapplied>,
    messages: Messages,
    browser_width: f32,
    #[cfg(target_os = "macos")]
    native_menu: Option<crate::native_menu::NativeMenu>,
}

pub struct App {
    engine: Engine,
    workspace: Workspace,
    part: part_editor::PartEditor,
    schematic: schematic_editor::SchematicEditor,
    pcb: pcb_editor::PcbEditor,
    scene: SceneController,
    /// Stale copper fills refill off the UI thread.
    fills: crate::fill_worker::FillWorker,
    files: FileUi,
    /// Library folders, favourites and recents (S3).
    sources: LibrarySources,
    /// Command results and refusals in the canvas (slice K3).
    toasts: Toasts,
    /// Every message of this run, with the full engine text.
    messages: Messages,
    messages_open: bool,
    /// Messages, Keyboard shortcuts, Settings and Connect AI assistants
    /// open last frame, in that order (`system_windows`).
    system_windows: [bool; 4],
    /// Live MCP: assistants editing this window's project (off at start).
    live: live_mcp::LiveMcp,
    /// The system's fallback fonts (CJK and others) are installed.
    system_fonts: bool,
    /// The Issues card, canvas badge and pill, and their cached answers.
    issues: crate::issues::Surface,
    /// The engine revision a toast's Undo would undo; Undo is withdrawn
    /// once another step follows.
    undo_revision: Option<u64>,
    /// The last undo or redo toast's text: the next one replaces it, so a
    /// held ⌘Z shows one toast.
    history_toast: Option<String>,
    /// Browser shown by width, or as the user toggled it (⌘B).
    browser: chrome::BrowserMode,
    /// Whether the last layout showed the Browser.
    browser_visible: bool,
    /// The component row selected in the Browser (a click selects it; a
    /// double-click opens it for editing).
    browser_component: Option<pcbcraft_core::DefinitionId>,
    /// A Browser row's right-click menu.
    browser_menu: crate::context_menu::State<browser::MenuTarget>,
    /// The Browser's width as last dragged (kept per user in the
    /// settings); the layout clamps it to what the window fits.
    browser_width: f32,
    /// The Parts and Vias band under the PCB canvas (⇧⌘L) and its height.
    parts_band: bool,
    parts_height: f32,
    /// "Update project copy" imported a revision at this engine revision;
    /// cancelling its review undoes that import (see `review_closed`).
    pending_import: Option<PendingImport>,
    closing: bool,
    /// Input time of the current frame, for status messages.
    now: f64,
    /// Time from opening the PCB 3D view to its first frame.
    open_timing: open_timing::OpenTiming,
    /// Every command, its context and its key (`crate::commands`).
    /// Shared with every key reader through `crate::keys::publish`.
    registry: Arc<crate::commands::Registry>,
    /// Saved key changes that could not be applied (kept in the settings).
    keymap_problems: Vec<crate::keymap::Unapplied>,
    palette: command_palette::CommandPalette,
    /// ⌘ commands pressed over a focused field, run once it committed.
    after_commit: command_dispatch::AfterCommit,
    shortcut_sheet: shortcut_sheet::ShortcutSheet,
    settings_window: settings_window::SettingsWindow,
    /// Hold ⌘ for key badges, tap ⌥ for key tips.
    reveal: key_reveal::KeyReveal,
    /// The macOS menu bar, built from the registry (`crate::menu_bar`).
    #[cfg(target_os = "macos")]
    native_menu: Option<crate::native_menu::NativeMenu>,
    /// Commands chosen in the menu bar, run in the next update.
    #[cfg(target_os = "macos")]
    menu_runs: Vec<String>,
    /// Recent projects chosen in the menu bar, opened in the next update.
    #[cfg(target_os = "macos")]
    menu_recent: Vec<std::path::PathBuf>,
    /// Which File › Open Recent files are gone.
    recent: crate::recent_projects::Availability,
    /// The project file last kept in Open Recent.
    recent_seen: Option<std::path::PathBuf>,
    /// This frame's "Updating" chip over a 3D view: the view's area and
    /// the error when the newer model failed. It is drawn after the issues
    /// badge, beside it.
    updating: Option<(egui::Rect, Option<String>)>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        follow_system_theme(&cc.egui_ctx, true);
        // Solidcraft's icons and glyphs, which its UI kit draws only once
        // a host installs them (once per process).
        pcbcraft_ui::icon_svg::install();
        fonts::setup_bundled_fonts(&cc.egui_ctx);
        fonts::load_system_fonts_in_background(&cc.egui_ctx);
        let render_state = cc.wgpu_render_state.as_ref().ok_or("wgpu is required")?;
        let mut engine = file_ui::new_project();
        let mut files = FileUi::new(cc, &engine);
        // `pcbcraft FILE.pcbcraft` opens that project through the file workflow.
        if let Some(path) = std::env::args_os()
            .skip(1)
            .map(PathBuf::from)
            .find(|p| crate::open_documents::is_project(p))
        {
            files.request(file_ui::Action::OpenPath(path));
        }
        // Files opened from Finder wake the UI (src/open_documents.rs).
        crate::open_documents::set_ui(&cc.egui_ctx);
        let sources = LibrarySources::from_env(&cc.egui_ctx);
        // The user's keymap over the defaults; what cannot apply is
        // reported and kept.
        let mut registry = crate::commands::Registry::standard();
        let keymap_problems = crate::keymap::apply(&mut registry, &sources.keyboard().bindings);
        // A keymap conflict is a defect (the registry tests forbid one);
        // report it instead of failing silently.
        let mut messages = Messages::default();
        for problem in &keymap_problems {
            messages.push(
                Level::Warning,
                &format!(
                    "Saved key for {} not applied: {}",
                    problem.binding.command, problem.reason
                ),
                0.0,
            );
        }
        for conflict in registry.conflicts() {
            let title = |id: &str| {
                registry
                    .get(id)
                    .map_or_else(String::new, |c| c.title.clone())
            };
            messages.push(
                Level::Warning,
                &format!(
                    "Shortcut {} is taken by both {} and {}",
                    conflict.chord.shown(),
                    title(&conflict.first),
                    title(&conflict.second)
                ),
                0.0,
            );
        }
        let registry = Arc::new(registry);
        crate::keys::publish(&cc.egui_ctx, &registry);
        quit_through_registry(&cc.egui_ctx);
        #[cfg(target_os = "macos")]
        let native_menu = match crate::native_menu::NativeMenu::new(
            &cc.egui_ctx,
            &crate::menu_bar::Now {
                registry: &registry,
                active: &crate::commands::active_contexts(crate::commands::Context::PcbLayout),
                typing: false,
                recording: false,
                browser_shown: true,
                issues_shown: false,
                available: &|_| true,
            },
        ) {
            Ok(menu) => Some(menu),
            Err(error) => {
                messages.push(
                    Level::Warning,
                    &format!("App menu unavailable: {error}"),
                    0.0,
                );
                None
            }
        };
        let browser_width = sources.browser_width().unwrap_or(chrome::BROWSER_WIDTH);
        engine.require_waiver_reason(sources.require_waiver_reason());
        Ok(Self::with_parts(
            &cc.egui_ctx,
            viewport::Viewport::new(render_state, &cc.egui_ctx),
            Parts {
                engine,
                files,
                sources,
                registry,
                keymap_problems,
                messages,
                browser_width,
                #[cfg(target_os = "macos")]
                native_menu,
            },
        ))
    }

    /// The app around what [`Self::new`] set up (a window's; a test's has
    /// none).
    fn with_parts(ctx: &egui::Context, viewport: viewport::Viewport, parts: Parts) -> Self {
        let Parts {
            engine,
            files,
            sources,
            registry,
            keymap_problems,
            messages,
            browser_width,
            #[cfg(target_os = "macos")]
            native_menu,
        } = parts;
        let mut pcb = pcb_editor::PcbEditor::default();
        *pcb.filter_mut() = sources.selection_filter();
        Self {
            scene: SceneController::new(ctx.clone(), viewport),
            fills: crate::fill_worker::FillWorker::start(ctx.clone()),
            files,
            sources,
            engine,
            workspace: Workspace::Pcb,
            part: Default::default(),
            schematic: Default::default(),
            pcb,
            toasts: Toasts::default(),
            messages,
            messages_open: false,
            system_windows: [false; 4],
            live: Default::default(),
            system_fonts: false,
            issues: Default::default(),
            undo_revision: None,
            history_toast: None,
            browser: chrome::BrowserMode::Auto,
            browser_visible: false,
            browser_component: None,
            browser_menu: Default::default(),
            browser_width,
            parts_band: false,
            parts_height: chrome::PARTS_HEIGHT,
            pending_import: None,
            closing: false,
            now: 0.0,
            open_timing: open_timing::OpenTiming::default(),
            registry,
            keymap_problems,
            palette: Default::default(),
            after_commit: Default::default(),
            shortcut_sheet: Default::default(),
            settings_window: Default::default(),
            reveal: Default::default(),
            #[cfg(target_os = "macos")]
            native_menu,
            #[cfg(target_os = "macos")]
            menu_runs: Vec::new(),
            #[cfg(target_os = "macos")]
            menu_recent: Vec::new(),
            recent: Default::default(),
            recent_seen: None,
            updating: None,
        }
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
        let action = refusal::take_action(ctx).or_else(|| refusal::titled_action(&operation));
        if self.files.busy() {
            self.say(Level::Info, "Wait for the file operation to finish");
            return;
        }
        // An explicit refill computes on the fill worker; its copper comes
        // back as one undoable `fill.store_all` (`refill_fills`).
        if let Operation::Fill(FillCommand::Refill { target }) = &operation
            && self.fills.available()
            && !matches!(target, RefillTarget::One(id) if self.engine.project().fill(*id).is_none())
        {
            if self.fills.refilling() {
                self.say(Level::Info, "A refill is already running");
                return;
            }
            if self.fills.refill(&self.engine, *target) {
                self.say(Level::Info, "Refilling in the background");
                return;
            }
        }
        // Core refuses a direct schematic edit that merges named nets; the
        // review then opens for it, whatever tool sent it.
        let schematic = match &operation {
            Operation::Schematic(command) => Some(command.clone()),
            _ => None,
        };
        match self.engine.apply_hosted(
            self.engine.revision(),
            operation,
            pcbcraft_core::Evaluators {
                footprint: Some(&pcbcraft_solidcraft::footprint::SketchEvaluator),
                board: crate::pcb_editor::board_shape::host_evaluator(),
            },
        ) {
            Ok(outcome) => {
                if let pcbcraft_core::Outcome::Pasted { report } = &outcome {
                    self.schematic.select_pasted(self.engine.project(), report);
                }
                if let pcbcraft_core::Outcome::PartsKept { parts } = &outcome {
                    self.say(
                        Level::Info,
                        pcbcraft_core::layout::several::kept_line(parts),
                    );
                }
                self.clear_error();
                // Moved parts' dragged copper the engine re-routed, or
                // left unrouted, in the same undo step.
                let rerouted = self.engine.rerouted();
                if let Some(line) = rerouted.line() {
                    let level = if rerouted.unrouted.is_empty() {
                        Level::Info
                    } else {
                        Level::Warning
                    };
                    self.say_with(level, line, vec![ToastAction::Undo]);
                }
            }
            Err(_)
                if schematic.as_ref().is_some_and(|command| {
                    self.schematic.review_named_merge(&self.engine, command)
                }) => {}
            Err(error) => self.refuse(ctx, origin, error, action.as_deref()),
        }
    }

    /// A refused operation: Messages keeps the full engine text; a field or
    /// dialog gets it back on the next frame and shows it itself; a canvas
    /// gesture or a command shows a toast (the gesture's editor may still
    /// claim it, to keep its gesture going).
    /// A toast names the refused `action` ("Can't rotate J1: …").
    fn refuse(
        &mut self,
        ctx: &egui::Context,
        origin: Origin,
        error: pcbcraft_core::Error,
        action: Option<&str>,
    ) {
        let level = refusal::level(&error);
        self.messages.push(level, &error.to_string(), self.now);
        match origin {
            Origin::Field(_) | Origin::Dialog(_) => {
                refusal::hand_back(ctx, Refused { origin, error }, false);
            }
            Origin::Canvas => {
                self.toast(
                    level,
                    refusal::refused(action, &error),
                    vec![ToastAction::Details],
                );
                refusal::hand_back(ctx, Refused { origin, error }, true);
            }
            Origin::Command => self.command_refused(error, false, action),
        }
    }

    /// A ribbon, menu or keyboard command was refused: a toast, and the
    /// full engine text in Messages unless `refuse` already put it there.
    fn command_refused(&mut self, error: pcbcraft_core::Error, record: bool, action: Option<&str>) {
        let level = refusal::level(&error);
        if record {
            self.messages.push(level, &error.to_string(), self.now);
        }
        self.toast(
            level,
            refusal::refused(action, &error),
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

    /// Store fill copper the worker computed for this revision (no undo
    /// step), and queue the revision. Undo and a pending import review
    /// still name the step before the refill.
    ///
    /// A finished explicit refill is committed here as one undoable
    /// `fill.store_all`.
    fn refill_fills(&mut self, ctx: &egui::Context) {
        if self.files.busy() {
            return;
        }
        let before = self.engine.revision();
        match self.fills.poll(&mut self.engine) {
            Ok(polled) => {
                if polled.stored > 0 {
                    let now = self.engine.revision();
                    self.pcb.fills_stored(before, now);
                    if self.undo_revision == Some(before) {
                        self.undo_revision = Some(now);
                    }
                    if let Some(pending) = &mut self.pending_import
                        && pending.revision == before
                    {
                        pending.revision = now;
                    }
                }
                match polled.refilled {
                    Some(copper) if copper.is_empty() => {
                        self.say(Level::Info, "No fill to refill");
                    }
                    Some(copper) => {
                        let count = copper.len();
                        let revision = self.engine.revision();
                        self.apply(
                            ctx,
                            Origin::Command,
                            Some(Operation::Fill(FillCommand::StoreAll {
                                fills: copper
                                    .into_iter()
                                    .map(|(fill, copper)| StoredFill { fill, copper })
                                    .collect(),
                            })),
                        );
                        if self.engine.revision() != revision {
                            let text = if count == 1 {
                                "Refilled 1 fill".to_owned()
                            } else {
                                format!("Refilled {count} fills")
                            };
                            self.say_with(Level::Info, text, vec![ToastAction::Undo]);
                        }
                    }
                    None => {}
                }
            }
            Err(text) => self.say(Level::Warning, text),
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
        // The toast wraps it and shows all of it on hover (`kit::toast`).
        let shown = refusal::clip(text.lines().next().unwrap_or_default(), refusal::MAX_SHOWN);
        if actions.contains(&ToastAction::Undo) {
            self.undo_revision = Some(self.engine.revision());
        }
        self.toast(level, shown, actions);
    }

    /// What an undo or redo did (`done`, "Undid"), by the step's title and
    /// workspace, as a Messages line and a toast that replaces the last
    /// one of its kind.
    fn say_step(&mut self, done: &str, step: pcbcraft_core::Step) {
        let text = format!(
            "{done}: {} · {}",
            step.line(),
            crate::shell::area_label(step.area)
        );
        self.messages.push(Level::Info, &text, self.now);
        let shown = refusal::clip(&text, refusal::MAX_SHOWN);
        if let Some(last) = self.history_toast.take() {
            self.toasts.remove_newest(&last);
        }
        self.toast(Level::Info, shown.clone(), Vec::new());
        self.history_toast = Some(shown);
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

    /// The system's fallback fonts once a background search found them, as
    /// Solidcraft installs them: CJK, Hangul, Thai, Hebrew and Arabic text
    /// then draws instead of empty boxes. Nothing is bundled for them.
    fn system_fonts(&mut self, ctx: &egui::Context) {
        if !self.system_fonts && fonts::system_fonts_ready() {
            self.system_fonts = true;
            fonts::setup_fonts_without_waiting(ctx, "en");
        }
    }

    /// The Messages window, while open.
    fn messages_window(&mut self, ctx: &egui::Context) {
        if self.messages_open && self.messages.window(ctx).closed {
            self.messages_open = false;
        }
    }

    /// Component workspace requests the app runs on the engine (drafting a
    /// published component: one `library.draft_closure`).
    fn part_requests(&mut self) {
        let Some(request) = self.part.take_request() else {
            return;
        };
        if self.files.busy() {
            self.say(Level::Info, "Wait for the file operation to finish");
            return;
        }
        let result = part_editor::run(&mut self.engine, request);
        match result {
            Ok(None) => self.clear_error(),
            Ok(Some(created)) => self.say(Level::Ok, created),
            Err(error) => self.command_refused(error, true, None),
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
        // The part-file chooser's recent files are kept per user.
        let chooser = self.part.part_chooser_mut();
        if let Some(recent) = chooser.take_recent_changed() {
            self.sources.set_recent_part_files(&recent);
        }
        chooser.set_recent(self.sources.recent_part_files());
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
                        BundlePurpose::Template => {
                            self.part.template_row_read(read.pick, Err(error))
                        }
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
                BundlePurpose::Template => self.part.template_row_read(read.pick, Ok(bundle)),
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

    /// Undo or redo one engine step and say which, by its history title.
    /// Undoing an AI assistant's edit says so.
    fn history(&mut self, redo: bool) {
        let assistant = !redo && self.live_edit_at(self.engine.revision());
        let (step, result) = if redo {
            (self.engine.redo_step(), self.engine.redo())
        } else {
            (self.engine.undo_step(), self.engine.undo())
        };
        match result {
            Ok(()) => {
                self.clear_error();
                let done = match (redo, assistant) {
                    (true, _) => "Redid",
                    (false, true) => "Undid AI assistant edit",
                    (false, false) => "Undid",
                };
                if let Some(step) = step {
                    self.say_step(done, step);
                }
            }
            Err(error) => self.command_refused(error, true, None),
        }
    }

    /// The Component workspace's subject requests: open requests from
    /// other surfaces, and folder previews, Edit a copy and Place.
    fn component_subject(&mut self, ctx: &egui::Context) {
        let names = self
            .sources
            .sources()
            .iter()
            .filter_map(|s| {
                s.library()
                    .map(|library| (library, s.name.clone(), s.setting.writable))
            })
            .collect();
        let sources = &self.sources;
        self.part
            .set_sources(names, sources.generation(), || sources.context());
        for pick in self.part.take_template_reads() {
            self.sources.read_bundle(pick, BundlePurpose::Template);
        }
        for action in self.part.take_folder_actions() {
            match action {
                part_editor::FolderAction::Preview(pick) => {
                    let folder = self.sources.sources().iter().find_map(|s| match &s.status {
                        library_sources::Status::Ready { folder, .. }
                            if folder.manifest.library == pick.library =>
                        {
                            Some(folder.clone())
                        }
                        _ => None,
                    });
                    match folder {
                        Some(folder) => {
                            self.sources
                                .used(pcbcraft_core::catalog::EntryKey::Library {
                                    library: pick.library,
                                    lineage: pick.lineage,
                                });
                            let receiver =
                                part_editor::spawn_preview(folder, pick.clone(), ctx.clone());
                            self.part.preview_loading(&pick, receiver);
                        }
                        None => self.part.preview_failed(
                            &pick,
                            format!("{} is not in a ready library source", pick.name),
                        ),
                    }
                }
                part_editor::FolderAction::EditCopy => self.edit_copy(),
                part_editor::FolderAction::Place(pick) => self
                    .sources
                    .read_bundle(pick, BundlePurpose::Place(Default::default())),
            }
        }
        self.part.tick();
    }

    /// "Edit a copy in this project": import the previewed folder item and
    /// draft it, as one undo step, then author the copy.
    fn edit_copy(&mut self) {
        if self.files.busy() {
            self.say(Level::Info, "Wait for the file operation to finish");
            return;
        }
        let (Some(bundle), Some(pick)) =
            (self.part.preview_bundle(), self.part.previewing().cloned())
        else {
            return;
        };
        match part_editor::edit_copy(&mut self.engine, bundle) {
            Ok(copied) => {
                self.part.open(copied.lineage, &self.engine);
                let source = self.part.library_name(pick.library);
                let text = format!(
                    "Copied {} {} from {source} into the project",
                    pick.name, pick.revision
                );
                if copied.changed {
                    self.say_with(Level::Ok, text, vec![ToastAction::Undo]);
                } else {
                    self.say(
                        Level::Info,
                        format!("{} {} is already in the project", pick.name, pick.revision),
                    );
                }
            }
            Err(error) => self.say(Level::Error, error),
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
        self.pcb.document_opened();
        self.scene.document_opened();
        self.schematic.document_opened(self.engine.project());
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
        self.opened_documents();
    }

    /// Files opened from Finder or the Dock go through File › Open's path,
    /// which asks about unsaved work first. They wait while a file operation
    /// or a file question is open, so none is dropped.
    fn opened_documents(&mut self) {
        if self.closing || self.files.busy() || self.files.prompting() {
            return;
        }
        let arrivals = crate::open_documents::sort(crate::open_documents::take());
        for path in arrivals.skipped {
            let why = if crate::open_documents::is_project(&path) {
                "another project opened at the same time"
            } else {
                concat!(pcbcraft_core::product_name!(), " opens .pcbcraft projects")
            };
            self.say(
                Level::Warning,
                format!("Not opened: {} ({why})", path.display()),
            );
        }
        if let Some(path) = arrivals.open {
            self.files.request(file_ui::Action::OpenPath(path));
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

    /// Ask for the model the active 3D view shows; while the board is
    /// edited in 2D, prepare its model in the background (debounced), so
    /// switching to 3D shows it at once.
    fn request_model(&mut self, ctx: &egui::Context) {
        if let Some(key) = self.model_key() {
            let package = self.part.preview_package();
            let engine = &self.engine;
            self.scene
                .request(key, package.as_ref(), || match &package {
                    Some(package) => crate::scene::library_asset(engine, package),
                    None => Ok(None),
                });
        } else if self.workspace == Workspace::Pcb {
            let key = ModelKey::Project(Arc::clone(self.engine.project_arc()));
            if let Some(wait) = self.scene.prefetch(key, self.now) {
                ctx.request_repaint_after(wait);
            }
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
        if self.workspace == Workspace::Part {
            // Which component is open, above every view of it.
            self.part.identity_strip(ui, &self.engine);
        }
        let Some(key) = self.model_key() else {
            let operation = match self.workspace {
                Workspace::Part => self.part.canvas(ui, &self.engine),
                Workspace::Schematic => {
                    // The Place palette's template rows: the same form,
                    // drawn over the sheet.
                    let ctx = ui.ctx().clone();
                    if let Some((hit, query)) = self.schematic.take_template_form() {
                        self.part.open_template_to_place(&hit, query);
                    }
                    let sheet = self.part.template_sheet(&ctx, &self.engine);
                    if let Some(component) = self.part.take_template_to_place() {
                        self.schematic.arm_created(&ctx, &self.engine, component);
                    }
                    let canvas = self.schematic.canvas(ui, &self.engine);
                    if sheet.is_some() {
                        // The sheet's operation goes instead of a placement.
                        self.schematic.take_template_join();
                    }
                    sheet.or(canvas)
                }
                Workspace::Pcb => self.pcb.canvas(ui, &self.engine),
            };
            let ctx = ui.ctx().clone();
            self.apply(&ctx, Origin::Canvas, operation);
            // The first placement of a just-created part joins its create:
            // one undo takes back both (refused, harmlessly, if it failed).
            if let Some(revision) = self.schematic.take_template_join() {
                let _ = self.engine.join_last_step(revision);
            }
            if let Some(revision) = self.schematic.take_template_undo()
                && revision == self.engine.revision()
                && self.engine.undo().is_ok()
            {
                self.say(Level::Info, "Cancelled: the component was not kept");
            }
            self.canvas_menu_requests(&ctx);
            return;
        };
        let display = self.scene.display(&key);
        if self.workspace == Workspace::Pcb {
            self.open_timing.drew(display);
        }
        let failed = match self.scene.status() {
            crate::scene::Status::Failed(error) => Some(error.clone()),
            _ => None,
        };
        if display == Display::Blank {
            let note = failed.unwrap_or_else(|| "Preparing 3D model…".into());
            ui.centered_and_justified(|ui| ui.label(note));
            return;
        }
        let area = ui.available_rect_before_wrap();
        // Component previews never change PCB selection.
        let pick = if self.workspace == Workspace::Part {
            // Component 3D shows every layer of its package.
            self.scene.viewport.set_layers(Default::default());
            self.scene
                .highlight(&Default::default(), None, &Default::default());
            // A read-only library item builds from its scratch engine and
            // never sends an operation to the project.
            let preview = self.part.lend_preview();
            let engine = preview.as_ref().map_or(&self.engine, |p| &p.engine);
            self.part
                .model_view_prepare(&mut self.scene.viewport, engine);
            self.scene.viewport.show(ui, viewport::NavCaps::PLACEMENT);
            let operation = self.part.model_view(ui, &self.scene.viewport, engine);
            let read_only = preview.is_some();
            self.part.restore_preview(preview);
            self.apply(
                &ui.ctx().clone(),
                Origin::Canvas,
                operation.filter(|_| !read_only),
            );
            None
        } else {
            crate::part_editor::reset_model_view(&mut self.scene.viewport);
            self.scene.viewport.set_layers(self.pcb.layer_view());
            self.scene.highlight(
                &self.pcb.selected(),
                self.pcb.highlighted_net(),
                &self.pcb.lit(),
            );
            let pick = self.scene.viewport.show(ui, viewport::NavCaps::BOARD);
            // How to move the view; the Fit view button's hover help lists
            // every binding.
            if let Some(rect) = self.scene.viewport.rect() {
                crate::kit::canvas_hint(
                    &ui.painter_at(rect),
                    rect,
                    pcbcraft_ui::theme::Tokens::get().space_16,
                    crate::navigation::hint_3d(),
                );
            }
            pick
        };
        if display == Display::Stale {
            // The last model stays up while a newer one builds, under a
            // small chip at the top right; picks on it could name objects
            // that no longer exist, so they are ignored.
            self.scene.viewport.set_hover(None);
            if self.workspace == Workspace::Pcb {
                // The dimensions follow the document at once: an edited
                // value shows at its new place while the model updates.
                self.pcb.board_3d_updating(ui, &self.scene, &self.engine);
            }
            self.updating = Some((area, failed));
            return;
        }
        if self.workspace == Workspace::Pcb {
            // A pick selects as a Layout click on the same object does, or
            // picks for the Dimension tool; the selection's dimensions
            // are drawn over the model.
            let ctx = ui.ctx().clone();
            let operation = self.pcb.board_3d(ui, &mut self.scene, &self.engine, pick);
            self.apply(&ctx, Origin::Canvas, operation);
        }
    }

    /// What a canvas's right-click menu asked the app to run: a registry
    /// command, as from the palette, or Edit component, as the Browser's
    /// double-click.
    fn canvas_menu_requests(&mut self, ctx: &egui::Context) {
        use schematic_editor::MenuRequest;
        match self.schematic.take_menu_request() {
            Some(MenuRequest::Command(id)) => self.run_command(ctx, id),
            Some(MenuRequest::EditComponent(id)) => self.pick(browser::Pick::EditComponent(id)),
            None => {}
        }
        if let Some(id) = self.pcb.take_menu_command() {
            self.run_command(ctx, id);
        }
    }

    /// The menu bar's commands chosen since the last frame, run as the
    /// registry's (after a focused field commits, as for a key), and the
    /// menu's states brought up to date.
    fn native_menu(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "macos")]
        {
            let runs = std::mem::take(&mut self.menu_runs);
            // A menu-bar key (⌘Z) with a right-click menu open only closes
            // the menu; nothing acts underneath it.
            if !runs.is_empty() && crate::context_menu::is_open(ctx) {
                crate::context_menu::close_all(ctx);
            } else {
                for id in runs {
                    if let Some(id) = command_dispatch::route(ctx, &mut self.after_commit, id) {
                        self.run_command(ctx, &id);
                    }
                }
            }
            for path in std::mem::take(&mut self.menu_recent) {
                self.open_recent(path);
            }
            if let Some(mut menu) = self.native_menu.take() {
                self.menu_now(ctx, |now| menu.update(now));
                if let Err(error) = menu.update_recent(&self.recent_items()) {
                    self.say(Level::Warning, format!("Open Recent not updated: {error}"));
                }
                self.native_menu = Some(menu);
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = ctx;
    }

    /// Esc closes the shown workspace's review card, before a canvas sees
    /// it. A window, popup or field over the review keeps its own Esc.
    fn escape_review(&mut self, ctx: &egui::Context) {
        if self.palette.is_open()
            || self.settings_window.is_open()
            || self.shortcut_sheet.is_open()
            || ctx.any_popup_open()
            || ctx.egui_wants_keyboard_input()
        {
            return;
        }
        let review = match self.workspace {
            Workspace::Part => self.part.review_mut(),
            Workspace::Schematic => self.schematic.review_mut(),
            Workspace::Pcb => self.pcb.review_mut(),
        };
        review.escape(ctx);
    }

    /// What the menus depend on now.
    /// Whether the shown canvas has a selection its delete key removes.
    fn can_delete(&self) -> bool {
        match self.workspace {
            Workspace::Part => self.part.can_delete(),
            Workspace::Schematic => self.schematic.can_delete(self.engine.project()),
            Workspace::Pcb => self.pcb.can_delete(self.engine.project()),
        }
    }

    fn menu_now<R>(&self, ctx: &egui::Context, f: impl FnOnce(&crate::menu_bar::Now) -> R) -> R {
        let active = self.active_contexts();
        let available = |command: &crate::commands::Command| {
            if command.id.ends_with(".delete") {
                self.can_delete()
            } else {
                self.availability(command, &[]).is_ok()
            }
        };
        f(&crate::menu_bar::Now {
            registry: &self.registry,
            active: &active,
            typing: ctx.egui_wants_keyboard_input(),
            recording: self.settings_window.is_recording(),
            browser_shown: self.browser_visible,
            issues_shown: self.issues.expanded(),
            available: &available,
        })
    }

    /// Keep the project file just opened or saved first in File › Open
    /// Recent, and check which recent files are gone.
    fn recent_projects(&mut self, ctx: &egui::Context) {
        let path = self.files.path();
        if path != self.recent_seen.as_deref() {
            if let Some(path) = path {
                self.sources.remember_project(path);
            }
            self.recent_seen = path.map(std::path::Path::to_path_buf);
        }
        self.recent
            .poll(ctx, self.sources.recent_projects(), self.now);
    }

    /// File › Open Recent's rows now, newest first.
    fn recent_items(&self) -> Vec<crate::recent_projects::RecentItem> {
        crate::recent_projects::items(self.sources.recent_projects(), self.recent.missing())
    }

    /// Open a recent project as File › Open does (asking about unsaved
    /// work first). One that is gone is removed from the list, with a toast.
    fn open_recent(&mut self, path: std::path::PathBuf) {
        if path.try_exists().unwrap_or(false) {
            self.files.request(file_ui::Action::OpenPath(path));
            return;
        }
        self.sources.forget_projects(Some(&path));
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        self.say(
            Level::Warning,
            format!("{name} is no longer there; removed from Open Recent"),
        );
    }

    /// Keep the production folder chosen for this project file per user,
    /// and hand the kept one to the next Production files dialog. A
    /// project never saved has no file to keep it by.
    fn production_folder(&mut self) {
        // The board interface publishes beside the project file.
        if self.pcb.project_path.as_deref() != self.files.path() {
            self.pcb.project_path = self.files.path().map(std::path::Path::to_path_buf);
        }
        let Some(project) = self.files.path() else {
            self.pcb.production_folder = None;
            return;
        };
        if let Some(folder) = self.pcb.take_chosen_production_folder() {
            self.sources.set_production_folder(project, folder);
        }
        let kept = self.sources.production_folder(project);
        if self.pcb.production_folder.as_deref() != kept {
            self.pcb.production_folder = kept.map(std::path::Path::to_path_buf);
        }
    }
}

/// ⌘Q (Ctrl+Q) is the registry's `app.quit`, rebindable and asking about
/// unsaved work like every other quit; egui's own quit key, which closes
/// the window directly, is turned off.
fn quit_through_registry(ctx: &egui::Context) {
    ctx.options_mut(|o| o.quit_shortcuts.clear());
}

/// Closing the window (its close button, or the system closing it) with
/// unsaved work asks first: the close is cancelled and the file workflow
/// closes the window once answered. `closing`: the answer was given.
fn guard_close(ctx: &egui::Context, files: &mut FileUi, closing: bool) {
    if ctx.input(|i| i.viewport().close_requested()) && !closing {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        files.request(file_ui::Action::Close);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        follow_system_theme(&ctx, false);
        self.system_fonts(&ctx);
        crate::kit::text::fit_tooltips(&ctx);
        // An open right-click menu owns the keys; before anyone reads them.
        crate::context_menu::hold_keys(&ctx);
        let pcb_3d = self.workspace == Workspace::Pcb && self.pcb.is_3d();
        open_timing::report(self.open_timing.frame(pcb_3d, std::time::Instant::now()));
        self.scene.receive();
        self.refill_fills(&ctx);
        self.file_effects(&ctx);
        self.recent_projects(&ctx);
        // Assistants' requests, between the last frame and this one's
        // editors.
        self.serve_live(&ctx);
        self.library_sources(&ctx);
        self.component_subject(&ctx);
        // A2: the Component workspace catalog's action and feed.
        self.catalog(&ctx);
        // The key reveal watches ⌘ and ⌥ before anyone takes a key; key
        // tips, while shown, take the keys themselves.
        self.reveal_input(&ctx);
        self.list_prefs(&ctx);
        // A key being recorded in Settings reaches nothing else.
        self.record_key(&ctx);
        // The palette's keys come before any canvas sees them.
        self.early_keys(&ctx);
        self.command_palette(&ctx);
        self.system_windows(&ctx);
        self.escape_review(&ctx);
        self.app_bar(ui);
        self.ribbon(ui);
        self.workspace_area(ui);
        self.production_folder();
        // Draft edits commit once their gesture ends, as one undo step.
        self.part_requests();
        self.library_requests(&ctx);
        let draft = self.part.commit(&ctx, &self.engine);
        self.apply(&ctx, Origin::Command, draft);
        // A command held for a field's commit runs next: the file workflow
        // must already hold the document the field committed to.
        self.files.observe(&self.engine, ctx.input(|i| i.time));
        self.dispatch_keys(&ctx);
        self.list_changes(&ctx);
        self.native_menu(&ctx);
        guard_close(&ctx, &mut self.files, self.closing);
        self.files.observe(&self.engine, ctx.input(|i| i.time));
        self.files.dialogs(&ctx);
        self.sources.dialogs(&ctx);
        self.messages_window(&ctx);
        let active = self.active_contexts();
        self.shortcut_sheet.show(&ctx, &self.registry, &active);
        self.settings(&ctx);
        self.live_windows(&ctx);
        self.reveal_overlay(&ctx);
        self.feedback(&ctx);
        self.unclaimed_refusals(&ctx);
        self.request_model(&ctx);
        if self.open_timing.pending() {
            ctx.request_repaint();
        }
    }

    /// Items chosen in the macOS menu bar, before egui reads this frame's
    /// input: a field's own keys join the input; commands wait for `ui`,
    /// and a press of the same key is dropped, so it runs once.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        #[cfg(target_os = "macos")]
        if let Some(mut menu) = self.native_menu.take() {
            let choices = self.menu_now(ctx, |now| menu.take(now));
            self.native_menu = Some(menu);
            for choice in choices {
                if let crate::menu_bar::Choice::OpenRecent(path) = choice {
                    self.menu_recent.push(path);
                    continue;
                }
                crate::menu_bar::deliver(choice, &self.registry, raw, &mut self.menu_runs, || {
                    arboard::Clipboard::new()
                        .and_then(|mut clipboard| clipboard.get_text())
                        .ok()
                });
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (ctx, raw);
    }

    /// The app ends without asking only when the system ends it (Dock ›
    /// Quit on macOS, a logout): unsaved work then goes to this session's
    /// recovery snapshot, which the next launch offers to restore.
    fn on_exit(&mut self) {
        self.files
            .keep_for_recovery(std::time::Duration::from_secs(5));
    }
}

/// The chip over a 3D view showing its last model while a newer one is
/// prepared (or failed: the error, the last model still shown), at the
/// top right of `area`, left of the issues badge (`badge`) when one shows.
fn updating_chip(
    ui: &mut egui::Ui,
    area: egui::Rect,
    badge: Option<egui::Rect>,
    failed: Option<&str>,
) {
    let t = pcbcraft_ui::theme::Tokens::get();
    let text = failed.unwrap_or("Updating");
    let right = badge.map_or(area.right(), |b| b.left());
    let width = if failed.is_some() {
        (right - area.left() - 2.0 * t.space_8).clamp(0.0, 480.0)
    } else {
        ui.painter()
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(t.font_small),
                t.text_dim,
            )
            .size()
            .x
            + t.font_small
            + 3.0 * t.space_8
    };
    let rect = egui::Rect::from_min_size(
        egui::pos2(right - width - t.space_8, area.top() + t.space_8),
        egui::vec2(width, t.control_height),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let level = failed.map(|_| crate::kit::severity::Level::Error);
    crate::kit::status_chip(&mut child, level, text);
}

/// Follow the operating system's light or dark appearance, like Solidcraft's
/// default "system" theme. `theme::apply` sets egui's theme to light or
/// dark, and egui would pass that on to the native window; a macOS window
/// given an appearance no longer reports the system's changes, so the app
/// would keep its launch theme. The window is left to follow the system,
/// as Solidcraft's `prefs::sync_window_theme` does (MIT option; see
/// ATTRIBUTION.md).
fn follow_system_theme(ctx: &egui::Context, force: bool) {
    if force {
        ctx.options_mut(|o| o.sync_window_theme = false);
    }
    let dark = ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark);
    if force || dark != theme::is_dark() {
        theme::set_dark(dark);
        theme::apply(ctx);
    }
}

/// The shortcuts sheet's rows for `filter`, every group flattened (tests
/// outside the app module check the sheet follows the keymap).
#[cfg(test)]
pub(crate) fn sheet_rows(
    registry: &crate::commands::Registry,
    filter: &str,
    active: &[crate::commands::Context],
) -> Vec<(String, String)> {
    shortcut_sheet::groups(registry, filter, active)
        .into_iter()
        .flat_map(|g| g.rows)
        .collect()
}

#[cfg(test)]
#[path = "app/dimension_2d_tests.rs"]
mod dimension_2d_tests;
#[cfg(test)]
#[path = "app/dimension_3d_tests.rs"]
mod dimension_3d_tests;
#[cfg(test)]
#[path = "app/dimension_list_tests.rs"]
mod dimension_list_tests;
#[cfg(test)]
#[path = "app/flow_tests.rs"]
mod flow_tests;
#[cfg(test)]
#[path = "app/parts_band_tests.rs"]
mod parts_band_tests;

#[cfg(test)]
#[path = "app/ax_harness.rs"]
pub(crate) mod ax_harness;

#[cfg(test)]
#[path = "app/ax_tests.rs"]
mod ax_tests;

// The font tests rely on macOS system fonts, so the module only exists there.
#[cfg(all(test, target_os = "macos"))]
mod font_tests {
    use pcbcraft_ui::fonts;
    use pcbcraft_ui::ui_kit::type_face;

    /// The bundled faces have no CJK glyphs; the system fallbacks the app
    /// installs once found do (macOS ships Hiragino), in every family a
    /// name may use, the monospace of fields too.
    #[test]
    #[cfg(target_os = "macos")]
    fn japanese_names_draw_with_the_system_fonts() {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let frame = |ctx: &egui::Context| {
            ctx.run_ui(Default::default(), |_| {})
                .textures_delta
                .clear();
        };
        frame(&ctx);
        let draws = |ctx: &egui::Context, font: &egui::FontId| {
            ctx.fonts_mut(|f| f.has_glyphs(font, "日本語"))
        };
        let sans = egui::FontId::proportional(13.0);
        let mono = egui::FontId::monospace(13.0);
        assert!(!draws(&ctx, &sans), "the bundled faces lack CJK");
        install_system_fonts(&ctx);
        frame(&ctx);
        assert!(draws(&ctx, &sans) && draws(&ctx, &mono));
    }

    /// Hiragino's ⇧ stands half as tall again as a capital; the status line
    /// and menus show it beside letters, so it must keep to their size, with
    /// one point for rounding. Measured on the glyphs as rasterized, for each
    /// family that shows shortcuts.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_shift_glyph_stays_the_size_of_a_capital_with_the_system_fonts() {
        let ctx = egui::Context::default();
        install_system_fonts(&ctx);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        for font in [
            egui::FontId::proportional(13.0),
            egui::FontId::proportional(11.0),
            type_face::semibold(13.0),
            egui::FontId::monospace(13.0),
        ] {
            let galley = ctx.fonts_mut(|f| {
                f.layout_no_wrap("L⇧".to_owned(), font.clone(), egui::Color32::WHITE)
            });
            let heights: Vec<f32> = galley.rows[0]
                .row
                .glyphs
                .iter()
                .map(|glyph| glyph.uv_rect.size.y)
                .collect();
            assert_eq!(heights.len(), 2, "{font:?}");
            let (capital, shift) = (heights[0], heights[1]);
            assert!(
                capital > 0.0 && shift <= capital + 1.0,
                "{font:?}: ⇧ is {shift} tall beside L at {capital}"
            );
        }
    }

    /// The system's fallback fonts in `ctx`, once the search found them.
    #[cfg(target_os = "macos")]
    fn install_system_fonts(ctx: &egui::Context) {
        fonts::load_system_fonts_in_background(ctx);
        for _ in 0..600 {
            if fonts::system_fonts_ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(fonts::system_fonts_ready(), "the search finished");
        fonts::setup_fonts_without_waiting(ctx, "en");
    }
}

#[cfg(test)]
mod edit_copy_tests {
    use pcbcraft_core::Engine;

    /// Edit a copy is one `library.edit_copy`: one engine Undo takes the
    /// import and the drafts back, one Redo brings them back, and a
    /// repeat changes nothing.
    #[test]
    fn edit_a_copy_is_one_undo_step() -> Result<(), Box<dyn std::error::Error>> {
        let (_, bundle) = crate::part_editor::sample_item()?;
        let mut engine = Engine::default();
        let kept = (engine.project().clone(), engine.drafts().clone());
        let before = engine.revision();
        let copied = crate::part_editor::edit_copy(&mut engine, bundle.clone())?;
        assert!(copied.changed);
        assert_eq!(engine.revision(), before + 1, "one engine step");
        let copy = (engine.project().clone(), engine.drafts().clone());
        engine.undo()?;
        assert_eq!((engine.project().clone(), engine.drafts().clone()), kept);
        engine.redo()?;
        assert_eq!((engine.project().clone(), engine.drafts().clone()), copy);
        let again = crate::part_editor::edit_copy(&mut engine, bundle)?;
        assert!(!again.changed);
        assert_eq!(again.lineage, copied.lineage);
        Ok(())
    }
}

#[cfg(test)]
mod quit_tests {
    use super::{file_ui, guard_close, quit_through_registry};
    use crate::commands::{Chord, Context, Registry, active_contexts};
    use pcbcraft_core::{Engine, Operation, schematic::SchematicCommand};

    type Fallible<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn input(events: Vec<egui::Event>, close: bool) -> egui::RawInput {
        let mut input = egui::RawInput {
            events,
            ..Default::default()
        };
        if close {
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
        }
        input
    }

    fn commands(output: &egui::FullOutput) -> Vec<egui::ViewportCommand> {
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default()
    }

    /// The window's close button with unsaved work: the close is cancelled
    /// and the unsaved-changes question is asked instead. Once answered
    /// (`closing`), a close request goes through.
    #[test]
    fn the_close_button_asks_about_unsaved_work() -> Fallible {
        let dir = tempfile::tempdir()?;
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut files = file_ui::FileUi::with_directory(
            &ctx,
            rfd::AsyncFileDialog::new(),
            &engine,
            dir.path().to_path_buf(),
        );
        engine.apply(
            engine.revision(),
            Operation::Schematic(SchematicCommand::RenameProject {
                name: "Unsaved".into(),
            }),
        )?;
        files.observe(&engine, 0.0);
        let mut output = ctx.run_ui(input(vec![], true), |ui| {
            guard_close(ui.ctx(), &mut files, false);
        });
        output.textures_delta.clear();
        assert!(commands(&output).contains(&egui::ViewportCommand::CancelClose));
        assert!(files.prompting(), "no unsaved-changes question");
        let mut output = ctx.run_ui(input(vec![], true), |ui| {
            guard_close(ui.ctx(), &mut files, true);
        });
        output.textures_delta.clear();
        assert!(!commands(&output).contains(&egui::ViewportCommand::CancelClose));
        Ok(())
    }

    /// ⌘Q is the registry's `app.quit`, in every context and while typing.
    /// egui's own ⌘Q, which would close the window without asking, is off.
    #[test]
    fn quit_key_is_the_registry_command() -> Fallible {
        let registry = Registry::standard();
        let quit = Chord::cmd(egui::Key::Q);
        for context in Context::ALL {
            let command = registry
                .resolve(quit, &active_contexts(context))
                .ok_or_else(|| format!("⌘Q does nothing in {context:?}"))?;
            assert_eq!(command.id, "app.quit", "{context:?}");
            assert!(super::command_dispatch::dispatchable(
                command, quit, true, true
            ));
        }
        let press = egui::Event::Key {
            key: egui::Key::Q,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        let plain = egui::Context::default();
        let mut output = plain.run_ui(input(vec![press.clone()], false), |_| {});
        output.textures_delta.clear();
        assert!(
            commands(&output).contains(&egui::ViewportCommand::Close),
            "egui's default quit key closes directly"
        );
        let ctx = egui::Context::default();
        quit_through_registry(&ctx);
        let mut output = ctx.run_ui(input(vec![press], false), |_| {});
        output.textures_delta.clear();
        assert!(!commands(&output).contains(&egui::ViewportCommand::Close));
        Ok(())
    }
}

#[cfg(test)]
mod theme_tests {
    /// The app follows the system's appearance, so it never tells the
    /// native window to be light or dark: a macOS window given an
    /// appearance stops reporting the system's changes, and the app would
    /// keep its launch theme.
    #[test]
    fn following_the_system_theme_leaves_the_window_appearance_alone() {
        let ctx = egui::Context::default();
        super::follow_system_theme(&ctx, true);
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    system_theme: Some(egui::Theme::Dark),
                    ..Default::default()
                },
                |ui| super::follow_system_theme(ui.ctx(), false),
            );
            output.textures_delta.clear();
            let told: Vec<_> = output
                .viewport_output
                .values()
                .flat_map(|v| v.commands.iter())
                .filter(|c| matches!(c, egui::ViewportCommand::SetTheme(_)))
                .collect();
            assert!(told.is_empty(), "{told:?}");
        }
    }
}
