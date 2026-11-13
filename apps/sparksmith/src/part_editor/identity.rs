//! Slice A1: the Component workspace's identity bar and subject.
//!
//! The workspace names exactly one subject, chosen explicitly: a project
//! component lineage, or a library-folder item shown read-only (decision
//! F4). Without one it shows the start page ([`super::start`]). Nothing is
//! ever opened implicitly, so a draft nobody opened is listed (catalog,
//! start page), never shown.
//!
//! The bar is a slim strip above the canvas: back and forward through the
//! open history (⌘[ and ⌘]), source › name, a state chip (`published r3`,
//! `r3 → draft`, `new · never published` or `read-only · Sample Parts r5`),
//! an origin chip, and Switch with the recent subjects, Open… and New….
//! The Properties title stays the selection's, so the two never repeat.
use super::PartEditor;
use super::folder_preview::{self, Preview};
use crate::kit::{self, severity::Level};
use crate::library_sources::FolderPick;
use egui::{RichText, Ui, vec2};
use sparksmith_core::catalog::CatalogContext;
use sparksmith_core::ids::LibraryId;
use sparksmith_core::library::bundle::Direction;
use sparksmith_core::{DefinitionId, Engine, Operation};
use sparksmith_ui::theme::Tokens;
use sparksmith_ui::ui_kit::type_face;
use sparksmith_ui::ui_kit::{self, Button};
use std::sync::mpsc::Receiver;

/// The most subjects the open history keeps.
pub const MAX_HISTORY: usize = 64;
/// The most recent subjects Switch and the start page list.
pub const MAX_RECENTS: usize = 12;

/// What the workspace can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    /// A component lineage of the open project (drafted when opened).
    Project(DefinitionId),
    /// A library-folder item, shown read-only until Edit a copy.
    Folder(FolderPick),
}

/// A folder request the app carries out: it owns the sources and the
/// project engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderAction {
    /// Read the item's bundle and build its read-only preview
    /// ([`folder_preview::spawn`], then [`PartEditor::preview_loading`]).
    Preview(FolderPick),
    /// Import the previewed bundle and draft it ([`PartEditor::preview_bundle`]).
    EditCopy,
    /// Place the previewed item in the schematic (`library.place_from`).
    Place(FolderPick),
}

/// The bar's per-session state: the subject, open history and recents.
#[derive(Default)]
pub struct Identity {
    history: Vec<Subject>,
    /// The history entry shown; on the start page, the one Back returns to.
    at: Option<usize>,
    /// The start page is shown although a history entry exists.
    on_start: bool,
    recents: Vec<Subject>,
    /// The folder item shown read-only, kept while its engine is lent out.
    pub(super) previewing: Option<FolderPick>,
    pub(super) preview: Option<Box<Preview>>,
    /// The shown folder item's inspect report, kept while the preview is
    /// lent out.
    report: Option<std::sync::Arc<sparksmith_core::catalog::ItemReport>>,
    /// A preview being read.
    loading: Option<(FolderPick, Option<Receiver<folder_preview::Built>>)>,
    /// Bumped on every subject change; a lent preview whose subject
    /// changed is dropped rather than restored.
    pub(super) epoch: u64,
    actions: Vec<FolderAction>,
    /// Library names, and whether each is writable, by identity.
    names: Vec<(LibraryId, String, bool)>,
    /// The catalog context the start page searches with, and its
    /// generation.
    pub(super) context: (u64, CatalogContext),
    /// Tests: a sample-template New request left; the first component
    /// lineage that appears opens. (The gallery names what it created:
    /// `TemplateGallery::take_created`.)
    #[cfg(test)]
    pub(super) expect_new: bool,
    /// The component draft lineages at the last sync.
    #[cfg(test)]
    pub(super) known: std::collections::BTreeSet<DefinitionId>,
}

impl Identity {
    fn current(&self) -> Option<&Subject> {
        if self.on_start {
            return None;
        }
        self.at.and_then(|at| self.history.get(at))
    }

    fn can_back(&self) -> bool {
        match self.at {
            Some(_) if self.on_start => true,
            Some(at) => at > 0,
            None => false,
        }
    }

    fn can_forward(&self) -> bool {
        self.at.is_some_and(|at| at + 1 < self.history.len())
    }

    /// Record `subject` as opened: it becomes the history's newest entry
    /// (dropping the forward entries) and the first recent.
    pub(super) fn visit(&mut self, subject: Subject) {
        self.on_start = false;
        if self.current() == Some(&subject) {
            return;
        }
        let keep = self.at.map_or(0, |at| at + 1);
        self.history.truncate(keep);
        if self.history.last() != Some(&subject) {
            self.history.push(subject.clone());
        }
        if self.history.len() > MAX_HISTORY {
            let extra = self.history.len() - MAX_HISTORY;
            self.history.drain(..extra);
        }
        self.at = self.history.len().checked_sub(1);
        self.recents.retain(|s| *s != subject);
        self.recents.insert(0, subject);
        self.recents.truncate(MAX_RECENTS);
    }

    pub(super) fn recents(&self) -> &[Subject] {
        &self.recents
    }

    /// A library's name, else its identity.
    pub(super) fn library_name(&self, library: LibraryId) -> String {
        self.names
            .iter()
            .find(|(l, _, _)| *l == library)
            .map_or_else(|| library.to_string(), |(_, n, _)| n.clone())
    }
}

impl PartEditor {
    /// Open `subject`: a project lineage at once, a folder item once its
    /// bundle is read.
    pub fn open_subject(&mut self, subject: Subject, engine: &Engine) {
        match subject {
            Subject::Project(id) => self.open(id, engine),
            Subject::Folder(pick) => self.open_folder(pick),
        }
    }

    /// Show a library-folder item read-only. The app reads it
    /// ([`FolderAction::Preview`]); until then the canvas says so.
    pub fn open_folder(&mut self, pick: FolderPick) {
        self.identity.visit(Subject::Folder(pick.clone()));
        self.leave_subject();
        self.identity
            .actions
            .push(FolderAction::Preview(pick.clone()));
        self.identity.loading = Some((pick, None));
    }

    /// The worker reading the folder item being opened.
    pub fn preview_loading(
        &mut self,
        pick: &FolderPick,
        receiver: Receiver<folder_preview::Built>,
    ) {
        if let Some((loading, slot)) = &mut self.identity.loading
            && loading == pick
        {
            *slot = Some(receiver);
        }
    }

    /// A folder item could not be read or opened.
    pub fn preview_failed(&mut self, pick: &FolderPick, error: String) {
        if self
            .identity
            .loading
            .as_ref()
            .is_some_and(|(loading, _)| loading == pick)
        {
            self.identity.loading = None;
            self.tell(Level::Error, error);
        }
    }

    /// Take in a finished preview read. Call once per frame before the
    /// editor draws.
    pub fn tick(&mut self) {
        let Some((pick, Some(receiver))) = &self.identity.loading else {
            return;
        };
        let Ok((read, result)) = receiver.try_recv() else {
            return;
        };
        if read != *pick {
            return;
        }
        self.identity.loading = None;
        match result {
            Ok(preview) => self.show_preview(preview),
            Err(error) => self.tell(Level::Error, error),
        }
    }

    /// Show a built preview as the subject.
    pub(super) fn show_preview(&mut self, preview: Box<Preview>) {
        self.leave_subject();
        self.lineage = Some(preview.root);
        self.identity.previewing = Some(preview.pick.clone());
        self.identity.report = preview.report.clone();
        self.identity.preview = Some(preview);
        self.view = super::View::Details;
    }

    /// Drop the shown subject and its per-subject state.
    pub(super) fn leave_subject(&mut self) {
        self.identity.epoch = self.identity.epoch.wrapping_add(1);
        self.identity.preview = None;
        self.identity.previewing = None;
        self.identity.report = None;
        self.identity.loading = None;
        #[cfg(test)]
        {
            self.identity.expect_new = false;
        }
        self.lineage = None;
        self.source = None;
        self.draft = None;
        self.baseline = None;
        self.parts = Default::default();
        self.update = Default::default();
        self.template_picker = Default::default();
    }

    /// Show the start page; Back returns to the subject left.
    pub fn show_start(&mut self) {
        self.leave_subject();
        self.identity.on_start = self.identity.at.is_some();
        self.start.focus = true;
    }

    /// Whether a library-folder item is shown read-only.
    pub fn is_read_only(&self) -> bool {
        self.identity.previewing.is_some()
    }

    /// The folder item shown read-only, if any.
    pub fn previewing(&self) -> Option<&FolderPick> {
        self.identity.previewing.as_ref()
    }

    /// The folder item shown read-only or being read, with its
    /// `inspect_bundle` report once read (the catalog strip's summary).
    pub fn folder_subject(
        &self,
    ) -> Option<(
        &FolderPick,
        Option<&std::sync::Arc<sparksmith_core::catalog::ItemReport>>,
    )> {
        match (&self.identity.previewing, &self.identity.loading) {
            (Some(pick), _) => Some((pick, self.identity.report.as_ref())),
            (None, Some((pick, _))) => Some((pick, None)),
            (None, None) => None,
        }
    }

    /// The bundle of the previewed folder item, for Edit a copy.
    pub fn preview_bundle(&self) -> Option<sparksmith_core::library::Bundle> {
        self.identity.preview.as_ref().map(|p| p.bundle.clone())
    }

    /// The scratch engine of a folder preview, lent to the app for the 3D
    /// view: give it back with [`Self::restore_preview`].
    pub fn lend_preview(&mut self) -> Option<Box<Preview>> {
        self.identity.preview.take()
    }

    /// Give a lent preview back. It is kept only while the same folder
    /// item is still the subject (as [`Self::scoped`] keeps it only
    /// within one subject epoch); a subject change meanwhile drops it.
    pub fn restore_preview(&mut self, preview: Option<Box<Preview>>) {
        let Some(preview) = preview else {
            return;
        };
        if self.identity.preview.is_none()
            && self.identity.previewing.as_ref() == Some(&preview.pick)
        {
            self.identity.preview = Some(preview);
        }
    }

    /// Folder requests for the app, in order.
    pub fn take_folder_actions(&mut self) -> Vec<FolderAction> {
        std::mem::take(&mut self.identity.actions)
    }

    /// Library names for the bar's chips, and the start page's catalog
    /// context (re-read only when `generation` changes).
    pub fn set_sources(
        &mut self,
        names: Vec<(LibraryId, String, bool)>,
        generation: u64,
        context: impl FnOnce() -> CatalogContext,
    ) {
        self.identity.names = names;
        if self.identity.context.0 != generation {
            self.identity.context = (generation, context());
        }
        let shared = &self.identity.context.1;
        self.gallery.set_context(generation, || shared.clone());
    }

    /// Folder template rows the gallery needs read (io `read_bundle`, off
    /// the UI thread); hand each back with [`Self::template_row_read`].
    pub fn take_template_reads(&mut self) -> Vec<FolderPick> {
        self.gallery.take_reads()
    }

    /// A folder template row's bundle, read for `pick`.
    pub fn template_row_read(
        &mut self,
        pick: FolderPick,
        result: Result<sparksmith_core::library::Bundle, String>,
    ) {
        self.gallery.bundle_read(pick, result);
    }

    pub(super) fn go_back(&mut self, engine: &Engine) {
        let Some(at) = self.identity.at else {
            return;
        };
        let target = if self.identity.on_start {
            at
        } else {
            at.saturating_sub(1)
        };
        self.go_to(target, engine);
    }

    pub(super) fn go_forward(&mut self, engine: &Engine) {
        if let Some(at) = self.identity.at.filter(|_| self.identity.can_forward()) {
            self.go_to(at + 1, engine);
        }
    }

    /// Show history entry `at` without changing the history.
    fn go_to(&mut self, at: usize, engine: &Engine) {
        let Some(subject) = self.identity.history.get(at).cloned() else {
            return;
        };
        let history = std::mem::take(&mut self.identity.history);
        self.open_subject(subject, engine);
        self.identity.history = history;
        self.identity.at = Some(at);
        self.identity.on_start = false;
    }

    /// The source › name and the state chip of the subject shown.
    pub fn identity_text(&self, engine: &Engine) -> Option<(String, String)> {
        let set = self.draft.as_ref()?;
        let name = set.metas.component.name.clone();
        if let Some(pick) = &self.identity.previewing {
            let source = self.identity.library_name(pick.library);
            return Some((
                format!("{source} › {name}"),
                format!("read-only · {source} {}", pick.revision),
            ));
        }
        let base = engine.drafts().get(set.ids.component).and_then(|d| d.base);
        let state = match base {
            None => "new · never published".to_owned(),
            Some(base) if set.unpublished(engine) => format!("{base} → draft"),
            Some(base) => format!("published {base}"),
        };
        Some((format!("Project › {name}"), state))
    }

    /// Where the open project component came from or went to.
    pub fn origin_text(&self, engine: &Engine) -> Option<String> {
        if self.identity.previewing.is_some() {
            return None;
        }
        let set = self.draft.as_ref()?;
        let library = &engine.project().library;
        let record = library
            .origins
            .iter()
            .filter(|o| o.local.id == set.ids.component)
            .max_by_key(|o| o.local.revision)?;
        let source = self.identity.library_name(record.origin.library);
        Some(match record.direction {
            Direction::Promoted => format!("published to {source} {}", record.origin.revision),
            Direction::Imported => {
                let latest = library.latest(set.ids.component).map(|(_, r)| r);
                if latest != Some(record.local.revision) || set.unpublished(engine) {
                    format!(
                        "modified since import from {source} {}",
                        record.origin.revision
                    )
                } else {
                    format!("from {source} {}", record.origin.revision)
                }
            }
        })
    }

    /// The identity bar above the canvas.
    pub(super) fn identity_bar(&mut self, ui: &mut Ui, engine: &Engine) {
        self.history_keys(ui.ctx(), engine);
        let t = Tokens::get();
        let mut back = false;
        let mut forward = false;
        let mut details = false;
        let mut chosen = None;
        let mut open = false;
        let mut new = false;
        let mut copy = false;
        let mut place = false;
        let text = self.identity_text(engine);
        let origin = self.origin_text(engine);
        let loading = self.identity.loading.as_ref().map(|(p, _)| p.name.clone());
        let recents: Vec<(Subject, String)> = self
            .identity
            .recents
            .iter()
            .map(|s| (s.clone(), self.subject_name(s, engine)))
            .collect();
        let read_only = self.is_read_only();
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), t.control_height + t.space_4),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = t.space_4;
                back = ui
                    .add_enabled_ui(self.identity.can_back(), |ui| {
                        ui_kit::button(ui, "‹", Button::Ghost).on_hover_text("Back (⌘[)")
                    })
                    .inner
                    .clicked();
                forward = ui
                    .add_enabled_ui(self.identity.can_forward(), |ui| {
                        ui_kit::button(ui, "›", Button::Ghost).on_hover_text("Forward (⌘])")
                    })
                    .inner
                    .clicked();
                ui.add_space(t.space_4);
                match (&text, &loading) {
                    (_, Some(name)) => {
                        ui.label(RichText::new(format!("Opening {name}…")).color(t.text_dim));
                    }
                    (Some((title, state)), None) => {
                        ui.label(
                            RichText::new(title)
                                .font(type_face::semibold(t.font_panel))
                                .color(t.text),
                        );
                        details |= chip(ui, state, read_only).clicked();
                        if let Some(origin) = &origin {
                            details |= chip(ui, origin, false)
                                .on_hover_text("Show the Library section")
                                .clicked();
                        }
                    }
                    (None, None) => {
                        ui.label(RichText::new("No component open").color(t.text_dim));
                    }
                }
                // Left to right: the canvas issue badge holds the right end.
                ui.add_space(t.space_8);
                ui.menu_button("Switch", |ui| {
                    for (subject, name) in &recents {
                        if ui.button(name).clicked() {
                            chosen = Some(subject.clone());
                            ui.close();
                        }
                    }
                    if !recents.is_empty() {
                        kit::rule(ui);
                    }
                    if ui.button("Open…").on_hover_text(crate::keys::shown(ui.ctx(), "component.open").unwrap_or_default()).clicked() {
                        open = true;
                        ui.close();
                    }
                    if ui.button("New…").on_hover_text(crate::keys::shown(ui.ctx(), "component.new").unwrap_or_default()).clicked() {
                        new = true;
                        ui.close();
                    }
                })
                .response
                .on_hover_text("Recent components, Open… and New…");
                if read_only {
                    copy = kit::primary(ui, "Edit a copy in this project", true)
                        .on_hover_text(
                            "Import this revision and draft it; library items are never edited in place",
                        )
                        .clicked();
                    place = ui_kit::button(ui, "Place in schematic", Button::Normal)
                        .on_hover_text("Place this library item in the schematic")
                        .clicked();
                }
            },
        );
        kit::rule(ui);
        if back {
            self.go_back(engine);
        } else if forward {
            self.go_forward(engine);
        } else if let Some(subject) = chosen {
            self.open_subject(subject, engine);
        } else if open || new {
            self.show_start();
        } else if copy {
            self.identity.actions.push(FolderAction::EditCopy);
        } else if place && let Some(pick) = self.identity.previewing.clone() {
            self.identity.actions.push(FolderAction::Place(pick));
        } else if details {
            self.view = super::View::Details;
        }
    }

    /// The name a recent subject shows.
    pub(super) fn subject_name(&self, subject: &Subject, engine: &Engine) -> String {
        match subject {
            Subject::Project(id) => {
                let drafts = engine.drafts();
                drafts
                    .get(*id)
                    .map(|d| d.meta.name.clone())
                    .or_else(|| {
                        let library = &engine.project().library;
                        library
                            .latest_components()
                            .into_iter()
                            .find(|c| c.id == *id)
                            .map(|c| c.meta.name.clone())
                    })
                    .unwrap_or_else(|| "Removed component".into())
            }
            Subject::Folder(pick) => format!(
                "{} · {} {}",
                pick.name,
                self.identity.library_name(pick.library),
                pick.revision
            ),
        }
    }

    /// ⌘[ and ⌘] walk the open history; ⇧⌘O opens a component (the start
    /// page with its catalog search focused) and ⇧⌘N starts one from a
    /// template, while the Component workspace is shown. The workspace
    /// draws before the app's file shortcuts, so ⇧⌘N never reaches ⌘N.
    fn history_keys(&mut self, ctx: &egui::Context, engine: &Engine) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |id| crate::keys::pressed(ctx, id);
        if pressed("component.new") {
            self.open_template_gallery();
        } else if pressed("component.open") {
            self.show_start();
        } else if pressed("component.back") {
            self.go_back(engine);
        } else if pressed("component.forward") {
            self.go_forward(engine);
        }
    }

    /// Tests: open the first component lineage created since a sample New
    /// request, and remember the component drafts for the next comparison.
    #[cfg(test)]
    pub(super) fn open_created(&mut self, engine: &Engine) {
        let now: std::collections::BTreeSet<DefinitionId> = engine
            .drafts()
            .items
            .iter()
            .filter(|d| d.definition.kind() == sparksmith_core::DefinitionKind::Component)
            .map(|d| d.id)
            .collect();
        let created = now
            .iter()
            .find(|id| {
                !self.identity.known.contains(id)
                    && engine.drafts().get(**id).is_some_and(|d| d.base.is_none())
            })
            .copied();
        self.identity.known = now;
        if self.identity.expect_new
            && let Some(id) = created
        {
            self.identity.expect_new = false;
            self.open(id, engine);
        }
    }
}

#[cfg(test)]
impl PartEditor {
    /// Open the project's first complete component draft, as a person
    /// would pick it in the catalog.
    pub fn open_first(&mut self, engine: &Engine) {
        if let Some(set) = super::WorkingSet::find(engine.drafts(), None) {
            self.open(set.ids.component, engine);
        }
    }
}

/// A state chip's fill and text, as Solidcraft's state pills
/// (`AssistantTone`): neutral, or the selection tint for read-only.
/// (`Tokens::chip` is the saturated fill of a selection, for `on_accent`.)
pub(super) fn chip_colours(t: &Tokens, strong: bool) -> (egui::Color32, egui::Color32) {
    if strong {
        (t.accent_soft, t.selected_ink)
    } else {
        (t.dialog_footer, t.form_dim)
    }
}

/// A state or origin chip; `strong` marks read-only.
fn chip(ui: &mut Ui, text: &str, strong: bool) -> egui::Response {
    let t = Tokens::get();
    let (fill, ink) = chip_colours(&t, strong);
    ui.add(
        egui::Button::new(RichText::new(text).size(t.font_label).color(ink))
            .fill(fill)
            .corner_radius(t.radius_lg)
            .small(),
    )
}

impl PartEditor {
    /// A library source's name for messages.
    pub fn library_name(&self, library: LibraryId) -> String {
        self.identity.library_name(library)
    }

    /// Ask the app for Edit a copy of the previewed item.
    pub(super) fn identity_edit_copy(&mut self) {
        self.identity.actions.push(FolderAction::EditCopy);
    }
}

/// The public entry points, run against the subject's engine: the
/// project's, or a read-only preview's scratch engine. A preview never
/// sends an operation to the project.
impl PartEditor {
    /// Run `f` with the subject's engine.
    fn scoped<R>(&mut self, engine: &Engine, f: impl FnOnce(&mut Self, &Engine) -> R) -> R {
        let Some(preview) = self.identity.preview.take() else {
            return f(self, engine);
        };
        let epoch = self.identity.epoch;
        let out = f(self, &preview.engine);
        // A subject change inside `f` (Open, Back, a recent) drops it.
        if self.identity.epoch == epoch {
            self.identity.preview = Some(preview);
        }
        out
    }

    /// The operation committing a finished edit, if any. Call once per
    /// frame after the editor's widgets ran. A read-only subject commits
    /// nothing and drops any local change.
    pub fn commit(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        if self.is_read_only() {
            return self.scoped(engine, |this, engine| {
                this.sync(engine);
                this.flush(ctx);
                this.draft.clone_from(&this.baseline);
                None
            });
        }
        self.commit_inner(ctx, engine)
    }

    /// Ribbon tabs are the views of one component; groups are commands.
    pub fn ribbon(&mut self, engine: &Engine) -> crate::shell::Ribbon {
        self.scoped(engine, |this, engine| this.ribbon_inner(engine))
    }

    /// See [`Self::inspector_card_inner`].
    pub fn inspector_card(&mut self, engine: &Engine) -> kit::card::Inspector {
        self.scoped(engine, |this, engine| this.inspector_card_inner(engine))
    }

    /// See [`Self::inspector_region_inner`].
    pub fn inspector_region(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        region: kit::card::Region,
    ) -> Option<Operation> {
        let read_only = self.is_read_only();
        let out = self.scoped(engine, |this, engine| {
            if read_only {
                ui.disable();
            }
            this.inspector_region_inner(ui, engine, region)
        });
        out.filter(|_| !read_only)
    }

    /// See [`Self::subject_slot_inner`].
    pub fn subject_slot(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> kit::card::CardResponse<Operation> {
        let read_only = self.is_read_only();
        let mut response = self.scoped(engine, |this, engine| {
            if read_only {
                ui.disable();
            }
            this.subject_slot_inner(ui, engine)
        });
        if read_only {
            response.output = None;
        }
        response
    }

    /// The identity bar, above the canvas of every view.
    pub fn identity_strip(&mut self, ui: &mut Ui, engine: &Engine) {
        self.scoped(engine, |this, engine| this.identity_bar(ui, engine));
    }

    /// The canvas: the start page or the open tab's view.
    pub fn canvas(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let read_only = self.is_read_only();
        let out = self.scoped(engine, |this, engine| this.canvas_inner(ui, engine));
        out.filter(|_| !read_only)
    }
}
