//! The Schematic editor's Place palette: a searchable sheet over the
//! project's component lineages. Every result list comes from the engine's
//! `library.search` query and every preview from `library.inspect`; the
//! palette only returns what the user chose. The editor turns that into the
//! ordinary `component.place` operation.
//!
//! Rows from library folders (S3) follow the project's, each folder under
//! its own `LIBRARY · <name>` caption. Enter on one asks the app for
//! `library.place_from`; when the project holds another revision of that
//! lineage the sheet offers Keep both / Update project copy / Cancel
//! inline. Cmd+D marks the highlighted row a favourite; favourites and
//! recents rank after the match tier and are kept per user by the app.
//! Below every component, "From template" lists the template rows the same
//! text finds (`template_gallery::find`); choosing one asks the editor to
//! create the component and arm its placement. A text that reads as an
//! on-demand generic part (`template_gallery::find_generic`) lists that part
//! too: first when the project holds it, else after the project's components,
//! "not yet created". Choosing one needs no form. Library folder rows follow
//! everything the project can make itself.
//!
//! In *assign mode* the same search chooses a manufacturer part for one or
//! more instances, as the docked Choose part dialog
//! ([`PlacePalette::choose_dialog`], docs/sidebar-ia.md "Dialogs and
//! reviews"): it opens pre-filled with `kind:part` and the instances'
//! requirements, lists the fitting Parts, and Choose returns the Part,
//! which the editor turns into one `instance.assign_part`. A refusal comes
//! back to the dialog's refusal row; success closes it with a toast.
use crate::kit::dialog::{DialogAction, DialogPanel};
use crate::kit::severity::Level;
use crate::kit::toast::ToastAction;
use crate::kit::{self, Tone};
use crate::library_sources::FolderPick;
use crate::refusal::{self, Origin};
use crate::template_gallery::{self, Found};
use egui::{Rect, Ui, pos2};
use pcbcraft_core::catalog::generic::GenericCandidate;
use pcbcraft_core::catalog::templates::TemplateHit;
use pcbcraft_core::catalog::{
    self, CatalogContext, CatalogEntry, EntryKey, Hit, ItemReport, MAX_CONTEXT, MAX_QUERY,
    SearchReport, Source, UpdateAvailable,
};
use pcbcraft_core::ids::{LibraryId, SourceLineageId};
use pcbcraft_core::issues::MAX_TITLE;
use pcbcraft_core::library::import::OtherRevision;
use pcbcraft_core::library::templates::generic::GenericState;
use pcbcraft_core::{
    ComponentRef, DefinitionId, DefinitionKind, Engine, Id, PartRef, Query, QueryResult, Revision,
};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::collections::BTreeMap;

/// Rows one search asks for.
const ROWS: usize = 40;
/// Key names are spelled out: the bundled fonts have no ⏎ or ⇧ glyphs.
const KEYS: &str = "↑↓ move   Enter place   Shift+Enter place & keep open   click pick   Cmd+D favourite   Esc close";
/// The footer on a library folder row: a click places it like Enter, and
/// a folder Part places its component with the Part assigned.
const KEYS_FOLDER: &str =
    "↑↓ move   Enter place   Shift+Enter place & keep open   Cmd+D favourite   Esc close";
/// The footer on a template row.
const KEYS_TEMPLATE: &str = "↑↓ move   Enter create and place   click pick   Esc close";
/// The footer on a generic part that is not yet created, and on one that is.
const KEYS_GENERIC_NEW: &str = "↑↓ move   Enter create and place   click pick   Esc close";
const KEYS_GENERIC: &str = "↑↓ move   Enter place   click pick   Esc close";
/// The heading of the generic parts not yet in the project.
const GENERIC_HEADING: &str = "Generic parts";
/// The footer while the other-revision choice is open.
const KEYS_CHOICE: &str = "Enter update project copy   Esc cancel";
/// The footer while nothing can be placed.
const KEYS_IDLE: &str = "↑↓ move   Esc close";
/// The Choose part dialog's panel id: its refusals come back here.
pub const CHOOSE_DIALOG: &str = "choose-part";
/// The Choose part dialog's filter hint and its tooltip.
const CHOOSE_HINT: &str = "Filter by value: res=10k, tol<=1%, mpn:EX-";
const CHOOSE_TIP: &str = "Results are manufacturer parts of these instances' component. Field values filter by meaning: res=10k, V>=16, tol<=1%. mpn: matches part numbers only.";
/// The footer of Place mode on a Part row.
const KEYS_PART: &str =
    "↑↓ move   Parts are assigned, not placed: use Choose… on an instance   Esc close";
/// Why Enter on a Part row places nothing.
const PART_NOT_PLACED: &str =
    "Parts are assigned, not placed: place the component, then use Choose… on the instance.";

/// What the palette is choosing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// A component to place.
    #[default]
    Place,
    /// A manufacturer part for these instances.
    Assign { instances: Vec<Id> },
}

/// What the user chose in the palette.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteAction {
    /// Place this revision now; keep the palette open with Shift+Enter.
    Place {
        component: ComponentRef,
        keep_open: bool,
    },
    /// Arm the Place tool with this revision; clicking the sheet places it.
    Pick(ComponentRef),
    /// Assign mode: pin this Part on these instances.
    Assign { instances: Vec<Id>, part: PartRef },
    /// Assign mode: the Part is for a newer revision of the instances'
    /// component; review updating them to `target` with the Part assigned,
    /// as one transaction.
    UpdateAndAssign {
        instances: Vec<Id>,
        part: PartRef,
        target: ComponentRef,
    },
    /// A template row: create the component from it, then arm placement.
    /// `query` is the search that found it, for the form if it takes one.
    FromTemplate {
        hit: Box<TemplateHit>,
        query: String,
    },
    /// A generic part: arm its placement, creating it first if needed, and
    /// give each placed instance the values the text named.
    Generic(Box<GenericCandidate>),
    /// Esc: the palette closed without a choice.
    Close,
}

/// What the palette leaves for the app, which owns settings, folders and
/// the engine (see [`PlacePalette::take_events`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteEvent {
    /// An entry was placed, picked or assigned: remember it as recent.
    Used(EntryKey),
    /// Cmd+D: mark or unmark a favourite.
    ToggleFavourite(EntryKey),
    /// Place a library folder entry through `library.place_from`.
    PlaceFrom {
        pick: FolderPick,
        other_revision: OtherRevision,
    },
    /// "Update project copy": import the folder revision, then review
    /// updating the project's instances to it.
    UpdateProjectCopy(FolderPick),
    /// Open the Library sources card.
    ManageSources,
}

/// What rows say about the project around them, rebuilt with the results:
/// how many instances pin each revision of a lineage, which revision of a
/// folder lineage the project holds, and the folders' names.
#[derive(Clone, Debug, Default)]
pub struct RowContext {
    usage: BTreeMap<DefinitionId, BTreeMap<Revision, usize>>,
    copies: BTreeMap<(LibraryId, SourceLineageId), Revision>,
    names: BTreeMap<LibraryId, String>,
}

impl RowContext {
    fn of(project: &pcbcraft_core::Project, context: &CatalogContext) -> Self {
        let mut usage: BTreeMap<DefinitionId, BTreeMap<Revision, usize>> = BTreeMap::new();
        for instance in &project.instances {
            *usage
                .entry(instance.component.id)
                .or_default()
                .entry(instance.component.revision)
                .or_default() += 1;
            if let Some(part) = instance.part {
                *usage
                    .entry(part.id)
                    .or_default()
                    .entry(part.revision)
                    .or_default() += 1;
            }
        }
        let mut copies: BTreeMap<(LibraryId, SourceLineageId), Revision> = BTreeMap::new();
        for record in &project.library.origins {
            let slot = copies
                .entry((record.origin.library, record.origin.lineage))
                .or_insert(record.local.revision);
            *slot = (*slot).max(record.local.revision);
        }
        Self {
            usage,
            copies,
            names: context
                .sources
                .iter()
                .map(|s| (s.library, s.name.clone()))
                .collect(),
        }
    }

    fn name(&self, library: LibraryId) -> String {
        self.names
            .get(&library)
            .cloned()
            .unwrap_or_else(|| "a library".into())
    }
}

/// Results of one query text at one document revision.
struct Results {
    revision: u64,
    /// The app's and the palette's own context generations.
    context: (u64, u64),
    query: String,
    report: Result<SearchReport, String>,
    /// Place mode: the template rows below the components.
    templates: Found,
    /// Place mode: the generic parts the text reads as.
    generic: Vec<GenericCandidate>,
    /// The rows the highlight moves over, top to bottom.
    order: Vec<Slot>,
}

/// One row of the result list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Hit(usize),
    Generic(usize),
    Template(usize),
}

/// The captioned groups of the result list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    /// The project's (0) or the `i`th folder's (`i + 1`) entries.
    Entries(usize),
    Generic,
    Template,
}

/// The list's order: the project's parts (a generic part it holds first), the
/// generic parts it could make, the template rows, then library folders.
fn row_order(
    hits: &[Hit],
    generic: &[GenericCandidate],
    templates: &[TemplateHit],
    context: &CatalogContext,
) -> Vec<Slot> {
    let held = |c: &GenericCandidate| c.component().is_some();
    let in_project = |h: &&Hit| group_of(&h.entry, context) == 0;
    let index_of = |g: &[GenericCandidate], keep: &dyn Fn(&GenericCandidate) -> bool| {
        (0..g.len())
            .filter(|i| g.get(*i).is_some_and(keep))
            .map(Slot::Generic)
            .collect::<Vec<_>>()
    };
    let mut order = index_of(generic, &held);
    order.extend(
        hits.iter()
            .enumerate()
            .filter(|(_, h)| in_project(h))
            .map(|(i, _)| Slot::Hit(i)),
    );
    order.extend(index_of(generic, &|c| !held(c)));
    order.extend((0..templates.len()).map(Slot::Template));
    order.extend(
        hits.iter()
            .enumerate()
            .filter(|(_, h)| !in_project(h))
            .map(|(i, _)| Slot::Hit(i)),
    );
    order
}

#[derive(Default)]
pub struct PlacePalette {
    open: bool,
    query: String,
    highlighted: usize,
    focus: bool,
    /// The highlighted row changed: scroll it into view.
    scroll: bool,
    results: Option<Results>,
    preview: Option<(u64, ComponentRef, Result<ItemReport, String>)>,
    /// Favourites, recents and folder catalogs, set by the app.
    context: CatalogContext,
    context_generation: u64,
    /// Library folders are still being read: an empty result says
    /// "Searching…", not "No component matches".
    loading: bool,
    /// Bumped by this palette's own favourite and recent changes, so the
    /// ranking follows them before the app reports back.
    local_generation: u64,
    events: Vec<PaletteEvent>,
    /// `library.place_from` refused another project revision: the pick and
    /// the engine's message, while Keep both / Update / Cancel is offered.
    choice: Option<(FolderPick, String)>,
    /// Counts openings and query changes; it salts the result list so each
    /// starts scrolled to the top.
    generation: u64,
    /// Why the last Enter or click placed nothing, shown under the field.
    notice: Option<String>,
    mode: Mode,
    /// Assign mode: the component revisions of the target instances.
    targets: Vec<ComponentRef>,
    /// What the rows say about the project, rebuilt with the results.
    rows: RowContext,
    /// Choose part: the dialog was just opened; its body starts at the top.
    reset: bool,
    /// Choose part: the highlighted Part's details are folded away.
    folded: bool,
    /// Choose part: every fitting Part is listed, not the first eight.
    show_all: bool,
    /// Choose part: an assignment was returned at this document revision;
    /// the next frame closes on success or shows the refusal. The text is
    /// the success toast.
    submitted: Option<(u64, String)>,
    /// Choose part: the engine's refusal of the last Choose.
    refusal: Option<String>,
}

/// What the Choose part dialog returned.
#[derive(Clone, Debug, PartialEq)]
pub enum ChooseAction {
    /// Pin this Part on these instances: one `instance.assign_part`.
    Assign { instances: Vec<Id>, part: PartRef },
    /// The Part is for a newer revision of the instances' component:
    /// review updating them to `target` with the Part assigned.
    UpdateAndAssign {
        instances: Vec<Id>,
        part: PartRef,
        target: ComponentRef,
    },
    /// New Part for these values…: the component lineage and its values.
    NewPart(crate::schematic_editor::NewPartRequest),
    /// Cancel, the header's close or Esc.
    Cancel,
}

impl PlacePalette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open with an empty query, ready for typing.
    pub fn open(&mut self) {
        self.mode = Mode::Place;
        self.reopen(String::new());
    }

    /// Open in assign mode for `instances`, with `query` (`kind:part` and
    /// their requirements) typed in.
    pub fn open_assign(&mut self, query: String, instances: Vec<Id>) {
        self.mode = Mode::Assign { instances };
        self.reopen(query);
        self.reset = true;
        self.folded = false;
        self.show_all = false;
        self.submitted = None;
        self.refusal = None;
    }

    /// Whether the Choose part dialog is open (assign mode).
    pub fn choosing(&self) -> bool {
        self.open && matches!(self.mode, Mode::Assign { .. })
    }

    #[cfg(test)]
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    #[cfg(test)]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The text searched. Assign mode reads a group's `implements:id/rev`
    /// as the whole lineage, so a Part for a newer revision stays listed:
    /// its row is flagged and offers the reviewed update-and-assign.
    fn search_text(&self) -> String {
        if !matches!(self.mode, Mode::Assign { .. }) {
            return self.query.clone();
        }
        self.query
            .split(' ')
            .map(|term| match term.split_once('/') {
                Some((lineage, _)) if lineage.starts_with("implements:") => lineage,
                _ => term,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn reopen(&mut self, query: String) {
        self.open = true;
        self.query = query;
        self.focus = true;
        self.highlighted = 0;
        self.generation = self.generation.wrapping_add(1);
        self.scroll = true;
        self.notice = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focus = false;
        self.choice = None;
    }

    /// Remember a placed, picked or assigned entry for ranking and tell the
    /// app to keep it: under its library identity when it has one (a
    /// folder entry, or a project copy of one), else as a project-only mark
    /// the app keeps for this project alone.
    fn used_entry(&mut self, entry: &CatalogEntry) {
        self.used_key(entry.user_key().unwrap_or_else(|| entry.key()));
    }

    /// Remember an entry for ranking and tell the app to keep it.
    fn used_key(&mut self, key: EntryKey) {
        self.context.recent.retain(|k| *k != key);
        self.context.recent.insert(0, key);
        self.context.recent.truncate(MAX_CONTEXT);
        self.local_generation = self.local_generation.wrapping_add(1);
        self.events.push(PaletteEvent::Used(key));
    }

    /// Cmd+D on an entry.
    fn toggle_favourite(&mut self, key: EntryKey) {
        if !self.context.favourites.remove(&key) {
            self.context.favourites.insert(key);
        }
        self.local_generation = self.local_generation.wrapping_add(1);
        self.events.push(PaletteEvent::ToggleFavourite(key));
    }

    /// The app's favourites, recents and folder catalogs; `generation`
    /// changes whenever they do, and `context` is only built then.
    pub fn set_context(&mut self, generation: u64, context: impl FnOnce() -> CatalogContext) {
        if generation != self.context_generation {
            self.context = context();
            self.context_generation = generation;
        }
    }

    /// Whether library folders are still being read.
    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    /// What the palette left for the app since the last call.
    pub fn take_events(&mut self) -> Vec<PaletteEvent> {
        std::mem::take(&mut self.events)
    }

    /// `library.place_from` refused another project revision of `pick`:
    /// reopen on the inline Keep both / Update project copy / Cancel.
    pub fn offer_choice(&mut self, pick: FolderPick, message: String) {
        self.open = true;
        self.focus = false;
        self.notice = None;
        self.choice = Some((pick, message));
    }

    /// A folder placement failed: reopen with the reason.
    pub fn refuse(&mut self, notice: String) {
        self.open = true;
        self.choice = None;
        self.notice = Some(notice);
    }

    /// The folder caption and source name of an entry's group.
    fn group(&self, entry: &CatalogEntry) -> usize {
        group_of(entry, &self.context)
    }

    /// Run `library.search` when the text or the document changed.
    fn refresh(&mut self, engine: &Engine) {
        let generations = (self.context_generation, self.local_generation);
        let fresh = self.results.as_ref().is_some_and(|r| {
            r.revision == engine.revision() && r.context == generations && r.query == self.query
        });
        if fresh {
            return;
        }
        let kept = self.highlighted_hit().map(|h| h.entry.key());
        let report = match engine.query_with(
            Query::Search {
                query: self.search_text(),
                limit: Some(ROWS),
                include_drafts: false,
            },
            &self.context,
        ) {
            Ok(QueryResult::Search(report)) => Ok(report),
            Ok(_) => Err("unexpected report".into()),
            Err(error) => Err(error.to_string()),
        };
        // A new text starts at the best match; a document or context
        // change keeps the highlighted entry where it still exists.
        let new_text = self.results.as_ref().is_none_or(|r| r.query != self.query);
        if new_text {
            self.highlighted = 0;
            self.generation = self.generation.wrapping_add(1);
            self.scroll = true;
            self.notice = None;
        }
        let report = report.map(|mut report| {
            match &self.mode {
                // Only Parts of the instances' component lineages can be
                // assigned to them.
                Mode::Assign { instances } => {
                    let lineages: Vec<DefinitionId> = instances
                        .iter()
                        .filter_map(|id| engine.project().instance(*id))
                        .map(|i| i.component.id)
                        .collect();
                    // Folder entries are catalog-local: only project Parts
                    // can be assigned.
                    report.results.retain(|h| {
                        h.entry.source == Source::Project
                            && h.entry.implements.is_some_and(|c| lineages.contains(&c.id))
                    });
                    report.total = report.results.len();
                }
                // The project's rows, then each folder's under its caption;
                // components first: Parts are assigned, not placed.
                Mode::Place => report.results.sort_by_key(|h| {
                    (
                        group_of(&h.entry, &self.context),
                        h.entry.kind == DefinitionKind::Part,
                    )
                }),
            }
            report
        });
        self.targets = match &self.mode {
            Mode::Assign { instances } => instances
                .iter()
                .filter_map(|id| engine.project().instance(*id))
                .map(|i| i.component)
                .collect(),
            Mode::Place => Vec::new(),
        };
        self.rows = RowContext::of(engine.project(), &self.context);
        let (mut templates, generic) = match &self.mode {
            Mode::Place => (
                template_gallery::find(engine, &self.query, ROWS, &self.context),
                template_gallery::find_generic(engine, &self.query),
            ),
            Mode::Assign { .. } => (Found::default(), Vec::new()),
        };
        template_gallery::drop_covered(&mut templates, &generic);
        // A text this grammar reads need not be one the search can: "10 kΩ
        // 0603" still finds its generic part. A part the project holds lists
        // once, as that part.
        let report = report
            .or_else(|error| {
                if generic.is_empty() {
                    Err(error)
                } else {
                    Ok(SearchReport::default())
                }
            })
            .map(|mut report| {
                let before = report.results.len();
                report.results.retain(|h| {
                    h.entry.source != Source::Project
                        || !generic
                            .iter()
                            .any(|c| c.component().is_some_and(|r| r.id == h.entry.lineage))
                });
                report.total = report.total.saturating_sub(before - report.results.len());
                report
            });
        let hits = report.as_ref().map_or(&[][..], |r| &r.results[..]);
        let order = row_order(hits, &generic, &templates.hits, &self.context);
        if !new_text
            && let Some(key) = kept
            && let Some(at) = order.iter().position(
                |s| matches!(s, Slot::Hit(i) if hits.get(*i).is_some_and(|h| h.entry.key() == key)),
            )
        {
            self.highlighted = at;
        }
        self.results = Some(Results {
            revision: engine.revision(),
            context: generations,
            query: self.query.clone(),
            report,
            templates,
            generic,
            order,
        });
    }

    /// The highlighted folder component, if the highlighted row is one.
    pub fn highlighted_folder(&self) -> Option<FolderPick> {
        self.highlighted_hit()
            .and_then(|h| FolderPick::of(&h.entry))
    }

    /// The highlighted row.
    fn slot(&self) -> Option<Slot> {
        self.results.as_ref()?.order.get(self.highlighted).copied()
    }

    /// The highlighted search result, if the highlight is on one.
    fn highlighted_hit(&self) -> Option<&Hit> {
        match self.slot()? {
            Slot::Hit(i) => self.hits().get(i),
            _ => None,
        }
    }

    /// The highlighted generic part, if the highlight is on one.
    fn highlighted_generic(&self) -> Option<&GenericCandidate> {
        match self.slot()? {
            Slot::Generic(i) => self.results.as_ref()?.generic.get(i),
            _ => None,
        }
    }

    /// The action of choosing the highlighted generic part; closes the sheet.
    /// One its family refuses, or a draft, stays with the reason.
    fn choose_generic(&mut self) -> Option<PaletteAction> {
        let candidate = self.highlighted_generic()?.clone();
        match &candidate.state {
            GenericState::Refused { reason } => {
                self.notice = Some(reason.clone());
                None
            }
            GenericState::Draft { .. } => {
                self.notice = Some(format!(
                    "{} is a draft: publish it in the Component workspace, then place it.",
                    candidate.name
                ));
                None
            }
            GenericState::Existing { .. } | GenericState::NotCreated => {
                self.close();
                Some(PaletteAction::Generic(Box::new(candidate)))
            }
        }
    }

    /// Assign mode: the revision a Part's component is newer than every
    /// target instance's, which they must be updated to first.
    fn newer_target(&self, entry: &CatalogEntry) -> Option<ComponentRef> {
        newer_than(entry, &self.targets)
    }

    fn hits(&self) -> &[Hit] {
        match self.results.as_ref().map(|r| &r.report) {
            Some(Ok(report)) => &report.results,
            _ => &[],
        }
    }

    /// The template rows below the components.
    fn templates(&self) -> &[TemplateHit] {
        self.results.as_ref().map_or(&[], |r| &r.templates.hits)
    }

    /// Rows the highlight moves over.
    fn row_count(&self) -> usize {
        self.results.as_ref().map_or(0, |r| r.order.len())
    }

    /// The highlighted template row, if the highlight is on one.
    fn highlighted_template(&self) -> Option<&TemplateHit> {
        match self.slot()? {
            Slot::Template(i) => self.templates().get(i),
            _ => None,
        }
    }

    /// The action of choosing the highlighted template row; closes the sheet.
    fn choose_template(&mut self) -> Option<PaletteAction> {
        let hit = Box::new(self.highlighted_template()?.clone());
        let query = self.query.clone();
        self.close();
        Some(PaletteAction::FromTemplate { hit, query })
    }

    /// The highlighted result's exact revision.
    pub fn highlighted(&self) -> Option<ComponentRef> {
        self.highlighted_hit()
            .filter(|h| h.entry.kind == DefinitionKind::Component)
            .and_then(|h| h.entry.component())
    }

    /// The highlighted result's exact Part revision.
    pub fn highlighted_part(&self) -> Option<PartRef> {
        self.highlighted_hit().and_then(|h| part_of(&h.entry))
    }

    /// Assign mode: the highlighted Part for the instances, or a notice.
    fn choose_part(&mut self) -> Option<PaletteAction> {
        let Mode::Assign { instances } = &self.mode else {
            return None;
        };
        let instances = instances.clone();
        let hit = self.highlighted_hit()?;
        let name = hit.entry.name.clone();
        let newer = self.newer_target(&hit.entry);
        match part_of(&hit.entry) {
            Some(part) => {
                self.notice = None;
                Some(match newer {
                    Some(target) => PaletteAction::UpdateAndAssign {
                        instances,
                        part,
                        target,
                    },
                    None => PaletteAction::Assign { instances, part },
                })
            }
            None => {
                self.notice = Some(format!("{name} is not a manufacturer part."));
                None
            }
        }
    }

    /// The highlighted revision if `placeable` accepts it; otherwise the
    /// reason is kept as the palette's notice and nothing is returned.
    fn choose(&mut self, placeable: &dyn Fn(ComponentRef) -> bool) -> Option<ComponentRef> {
        let hit = self.highlighted_hit()?;
        let name = hit.entry.name.clone();
        if hit.entry.kind == DefinitionKind::Part {
            self.notice = Some(PART_NOT_PLACED.into());
            return None;
        }
        if hit.entry.source != Source::Project {
            return None;
        }
        let component = hit.entry.component()?;
        if placeable(component) {
            self.notice = None;
            Some(component)
        } else {
            self.notice = Some(format!("{name} has no symbol to place."));
            None
        }
    }

    /// The sheet at the top of `area`. Owns the navigation keys while open.
    /// `placeable` says whether a revision has a symbol unit to put down; a
    /// row without one keeps the palette open with a notice.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        area: Rect,
        engine: &Engine,
        placeable: &dyn Fn(ComponentRef) -> bool,
    ) -> Option<PaletteAction> {
        // Assign mode is the docked Choose part dialog, not this sheet.
        if !self.open || matches!(self.mode, Mode::Assign { .. }) {
            return None;
        }
        let ctx = ui.ctx().clone();
        let mut keys = kit::search_keys(&ctx);
        if keys.escape && self.choice.is_some() {
            // Esc cancels the other-revision choice, not the palette.
            self.choice = None;
            keys = kit::SearchKeys::default();
        } else if keys.escape {
            self.close();
            return Some(PaletteAction::Close);
        }
        let mut chosen = None;
        if let Some((pick, _)) = &self.choice {
            // Enter takes the default, Update project copy…; the others are
            // its buttons.
            if keys.enter {
                chosen = Some(Choice::Update(pick.clone()));
            }
            keys = kit::SearchKeys::default();
        }
        self.refresh(engine);
        if ctx.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::D,
            ))
        }) && let Some(key) = self
            .highlighted_hit()
            .map(|h| h.entry.user_key().unwrap_or_else(|| h.entry.key()))
        {
            self.toggle_favourite(key);
            self.refresh(engine);
        }
        let count = self.row_count();
        if keys.down && count > 0 {
            self.highlighted = (self.highlighted + 1).min(count - 1);
            self.scroll = true;
            self.notice = None;
        }
        if keys.up {
            self.highlighted = self.highlighted.saturating_sub(1);
            self.scroll = true;
            self.notice = None;
        }
        self.highlighted = self.highlighted.min(count.saturating_sub(1));
        let mut action = None;
        if keys.enter && self.highlighted_generic().is_some() {
            action = self.choose_generic();
        } else if keys.enter && self.highlighted_template().is_some() {
            action = self.choose_template();
        } else if keys.enter && self.highlighted_folder().is_some() {
            action = self.place_folder(keys.keep);
        } else if keys.enter
            && let Some(component) = self.choose(placeable)
        {
            action = Some(PaletteAction::Place {
                component,
                keep_open: keys.keep,
            });
        }
        let t = Tokens::get();
        let width = (t.form_card_width * 2.0 - t.space_8).min(area.width() - t.space_16);
        let top = pos2(area.center().x - width / 2.0, area.top() + t.space_12);
        let list_height = (area.height() - t.space_24 * 7.0).max(kit::search_row_height() * 2.0);
        // Focus is requested until the field holds it: a new sheet's first
        // frame only measures its size.
        let focus = self.focus;
        let scroll = std::mem::take(&mut self.scroll);
        let mut picked = None;
        let mut focused = false;
        let generation = self.generation;
        let highlighted_entry = self.highlighted_hit().map(|h| &h.entry);
        let footer = if self.choice.is_some() {
            KEYS_CHOICE
        } else if let Some(candidate) = self.highlighted_generic() {
            match candidate.state {
                GenericState::Existing { .. } => KEYS_GENERIC,
                GenericState::NotCreated => KEYS_GENERIC_NEW,
                _ => KEYS_IDLE,
            }
        } else if self.highlighted_template().is_some() {
            KEYS_TEMPLATE
        } else if self.highlighted_folder().is_some() {
            KEYS_FOLDER
        } else if self.highlighted().is_some_and(placeable) {
            KEYS
        } else if highlighted_entry.is_some_and(|e| e.kind == DefinitionKind::Part) {
            KEYS_PART
        } else {
            KEYS_IDLE
        };
        let (hint, empty, heading) = (
            "Find a component: 10k 0603, res>=10k, tol<=1%",
            if self.loading {
                "Searching…"
            } else if self.query.trim().is_empty() {
                "Type to find a component or a template"
            } else {
                "No component or template matches"
            },
            "In this project",
        );
        let groups = self.group_names();
        let mut manage = false;
        kit::search_sheet(&ctx, "place-palette", top, width, footer, |ui| {
            let invalid = matches!(self.results.as_ref().map(|r| &r.report), Some(Err(_)));
            let field = kit::search_field(
                ui,
                egui::Id::unique("place-palette-query"),
                &mut self.query,
                hint,
                invalid,
                focus,
                MAX_QUERY,
            );
            focused = field.has_focus();
            // Show the results of what was just typed in this frame.
            if field.changed() {
                self.refresh(engine);
            }
            if let Some(notice) = &self.notice {
                kit::search_message(ui, notice, true);
            }
            if let Some((pick, message)) = &self.choice
                && let Some(clicked) = choice(ui, pick, message)
            {
                chosen = Some(clicked);
            }
            match self.results.as_ref().map(|r| &r.report) {
                Some(Err(error)) => {
                    kit::search_message(ui, error, true);
                }
                Some(Ok(report)) => {
                    let hidden: Vec<String> = report
                        .hidden
                        .iter()
                        .map(|h| format!("{} hidden ({})", h.count, h.reason))
                        .collect();
                    kit::search_chips(ui, &report.chips, &hidden.join(", "));
                    let found = self.results.as_ref().map(|r| &r.templates);
                    let templates = found.map_or(&[][..], |f| &f.hits);
                    let more = found.map_or(0, |f| f.more);
                    let generic = self.results.as_ref().map_or(&[][..], |r| &r.generic[..]);
                    let order = self.results.as_ref().map_or(&[][..], |r| &r.order[..]);
                    if order.is_empty() {
                        kit::search_message(ui, empty, false);
                    } else {
                        // One caption per group; a lone project group keeps
                        // the report's full match count.
                        let group = |hit: &Hit| group_of(&hit.entry, &self.context);
                        let only_project = report.results.iter().all(|h| group(h) == 0);
                        egui::ScrollArea::vertical()
                            .id_salt(("place-palette-rows", generation))
                            .max_height(list_height)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                let held = generic.iter().filter(|c| c.component().is_some());
                                let held = held.count();
                                let mut shown = None;
                                for (at, slot) in order.iter().enumerate() {
                                    let lit = at == self.highlighted;
                                    let caption = match *slot {
                                        Slot::Hit(i) => {
                                            report.results.get(i).map_or(Group::Entries(0), |h| {
                                                Group::Entries(group(h))
                                            })
                                        }
                                        Slot::Generic(i)
                                            if generic
                                                .get(i)
                                                .is_some_and(|c| c.component().is_some()) =>
                                        {
                                            Group::Entries(0)
                                        }
                                        Slot::Generic(_) => Group::Generic,
                                        Slot::Template(_) => Group::Template,
                                    };
                                    if shown != Some(caption) {
                                        shown = Some(caption);
                                        let (label, count) = match caption {
                                            Group::Entries(0) => (
                                                heading.to_owned(),
                                                held + if only_project {
                                                    report.total
                                                } else {
                                                    report
                                                        .results
                                                        .iter()
                                                        .filter(|h| group(h) == 0)
                                                        .count()
                                                },
                                            ),
                                            Group::Entries(g) => (
                                                groups.get(g - 1).map_or_else(
                                                    || "Library".to_owned(),
                                                    |name| format!("Library · {name}"),
                                                ),
                                                report
                                                    .results
                                                    .iter()
                                                    .filter(|h| group(h) == g)
                                                    .count(),
                                            ),
                                            Group::Generic => {
                                                (GENERIC_HEADING.to_owned(), generic.len() - held)
                                            }
                                            Group::Template => (
                                                template_gallery::SEARCH_HEADING.to_owned(),
                                                templates.len() + more,
                                            ),
                                        };
                                        kit::search_heading(ui, &label, count);
                                    }
                                    let response = match *slot {
                                        Slot::Hit(i) => report.results.get(i).map(|hit| {
                                            let favourite =
                                                hit.entry.named_by(&self.context.favourites);
                                            row(
                                                ui,
                                                hit,
                                                &self.query,
                                                lit,
                                                &self.targets,
                                                favourite,
                                                &self.rows,
                                            )
                                        }),
                                        Slot::Generic(i) => generic.get(i).map(|candidate| {
                                            template_gallery::generic_row(
                                                ui,
                                                egui::Id::unique("place-palette-generic").with(i),
                                                candidate,
                                                &self.query,
                                                lit,
                                            )
                                        }),
                                        Slot::Template(i) => templates.get(i).map(|hit| {
                                            let source =
                                                template_gallery::source_label(hit, |library| {
                                                    self.rows.name(library)
                                                });
                                            template_gallery::search_row(
                                                ui,
                                                egui::Id::unique("place-palette-template").with(i),
                                                hit,
                                                &self.query,
                                                lit,
                                                &source,
                                            )
                                        }),
                                    };
                                    if let Some(response) = response {
                                        if scroll && lit {
                                            response.scroll_to_me(None);
                                        }
                                        if response.clicked() {
                                            picked = Some(at);
                                        }
                                    }
                                    let last_template = matches!(slot, Slot::Template(_))
                                        && !matches!(order.get(at + 1), Some(Slot::Template(_)));
                                    if last_template && more > 0 {
                                        kit::note(ui, &format!("{more} more: narrow the search"));
                                    }
                                }
                            });
                    }
                }
                None => {}
            }
            kit::actions(ui, |ui| {
                if ui_kit::button(ui, "Library sources…", Button::Ghost)
                    .on_hover_text("Add, rescan, turn on or off and remove library folders")
                    .clicked()
                {
                    manage = true;
                }
            });
        });
        if manage {
            self.events.push(PaletteEvent::ManageSources);
        }
        if focused {
            self.focus = false;
        }
        if let Some(i) = picked
            && self.choice.is_none()
        {
            self.highlighted = i;
            if self.highlighted_generic().is_some() {
                action = self.choose_generic();
            } else if self.highlighted_template().is_some() {
                action = self.choose_template();
            } else if self.highlighted_folder().is_some() {
                action = self.place_folder(false);
            } else if let Some(component) = self.choose(placeable) {
                action = Some(PaletteAction::Pick(component));
            }
        }
        match chosen {
            Some(Choice::KeepBoth(pick)) => {
                self.events.push(PaletteEvent::PlaceFrom {
                    pick,
                    other_revision: OtherRevision::KeepBoth,
                });
                self.close();
                action = Some(PaletteAction::Close);
            }
            Some(Choice::Update(pick)) => {
                self.events.push(PaletteEvent::UpdateProjectCopy(pick));
                self.close();
                action = Some(PaletteAction::Close);
            }
            Some(Choice::Cancel) => self.choice = None,
            None => {}
        }
        let entry = self.highlighted_hit().map(|h| h.entry.clone());
        match action {
            Some(PaletteAction::Place { keep_open, .. }) => {
                if let Some(entry) = &entry {
                    self.used_entry(entry);
                }
                if !keep_open {
                    self.close();
                }
            }
            Some(
                PaletteAction::Pick(_)
                | PaletteAction::Assign { .. }
                | PaletteAction::UpdateAndAssign { .. },
            ) => {
                if let Some(entry) = &entry {
                    self.used_entry(entry);
                }
                self.close();
            }
            _ => {}
        }
        action
    }

    /// The docked Choose part dialog (assign mode), drawn in the subject
    /// slot `ui` offers: who gets the Part, the filter and its one hint
    /// line, the fitting Parts (the highlighted one expanded with its
    /// rated values and sources), then New Part for these values…. Choose
    /// returns the Part; the assignment's refusal comes back to the
    /// dialog's refusal row and success closes it with an Undo toast.
    pub fn choose_dialog(&mut self, ui: &mut Ui, engine: &Engine) -> Option<ChooseAction> {
        let Mode::Assign { instances } = &self.mode else {
            return None;
        };
        if !self.open {
            return None;
        }
        let instances = instances.clone();
        let ctx = ui.ctx().clone();
        let id = egui::Id::unique(CHOOSE_DIALOG);
        if let Some(refused) = refusal::claim(&ctx, Origin::Dialog(id)) {
            self.refusal = Some(refused);
            self.submitted = None;
        }
        if let Some((revision, done)) = self.submitted.take()
            && engine.revision() != revision
        {
            kit::toast::post(&ctx, Level::Ok, done, vec![ToastAction::Undo]);
            self.close();
            return None;
        }
        self.refresh(engine);
        let (up, down) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            )
        });
        let count = self.hits().len();
        if down && count > 0 {
            self.highlighted = (self.highlighted + 1).min(count - 1);
            self.folded = false;
        }
        if up {
            self.highlighted = self.highlighted.saturating_sub(1);
            self.folded = false;
        }
        self.highlighted = self.highlighted.min(count.saturating_sub(1));
        let project = engine.project();
        let designators: Vec<String> = instances
            .iter()
            .filter_map(|id| project.instance(*id))
            .map(|i| i.designator.clone())
            .collect();
        let who = crate::resolve_parts::designator_summary(&designators);
        let hits: Vec<Hit> = self.hits().to_vec();
        let total = match self.results.as_ref().map(|r| &r.report) {
            Some(Ok(report)) => report.total.max(hits.len()),
            _ => hits.len(),
        };
        let invalid = match self.results.as_ref().map(|r| &r.report) {
            Some(Err(error)) => Some(error.clone()),
            _ => None,
        };
        let highlighted = self.highlighted;
        let hit = hits.get(highlighted);
        let part = self.highlighted_part();
        let newer = hit.and_then(|h| self.newer_target(&h.entry));
        let primary = if newer.is_some() {
            "Review update…"
        } else {
            "Choose"
        };
        let disabled = match (part, count) {
            (Some(_), _) => None,
            (None, 0) => Some("No fitting part to choose"),
            (None, _) => Some("Select a manufacturer part first"),
        };
        let status = newer.map(|t| format!("Updates {who} to {} first", t.revision));
        let shown_refusal = self.refusal.clone();
        let panel = DialogPanel {
            id,
            icon: "library",
            title: "Choose part",
            kind: "Part",
            body_rows: 18.0,
            primary,
            primary_kind: kit::dialog::PrimaryKind::Normal,
            primary_disabled: disabled,
            refusal: shown_refusal.as_deref(),
            reset_scroll: std::mem::take(&mut self.reset),
            status: status.as_deref(),
        };
        let focus = std::mem::take(&mut self.focus);
        let folded = self.folded;
        let max_rows = if self.show_all {
            hits.len()
        } else {
            kit::LIST_MAX_ROWS
        };
        let targets = self.targets.clone();
        let mut changed = false;
        let mut picked = None;
        let mut toggled = None;
        let mut show_all = false;
        let mut new_part = false;
        let query = &mut self.query;
        let action = panel.show(ui, |ui| {
            let t = Tokens::get();
            kit::value_row(ui, "Assign to", &who);
            let field = id.with("query");
            if focus {
                ui.memory_mut(|m| m.request_focus(field));
            }
            let top = ui.cursor().min;
            let width = ui.available_width();
            changed = kit::filter_field(
                ui,
                field,
                query,
                "kind:part res=10k",
                invalid.is_some(),
                MAX_QUERY,
            )
            .changed;
            if let Some(message) = &invalid {
                let ring = ui.interact(
                    Rect::from_min_size(top, egui::vec2(width, t.control_height)),
                    id.with("ring"),
                    egui::Sense::hover(),
                );
                kit::field_error(ui, &ring, message, false);
            }
            kit::hint(ui, CHOOSE_HINT, CHOOSE_TIP);
            kit::section(
                ui,
                "choose-fitting",
                "Fitting parts",
                true,
                Some(total),
                |ui| {
                    if hits.is_empty() {
                        kit::status(
                            ui,
                            "No part of this component meets these values yet",
                            Tone::Dim,
                        );
                    }
                    let frame = kit::list_frame(ui, hits.len(), max_rows, |ui, index| {
                        let Some(hit) = hits.get(index) else {
                            return;
                        };
                        let selected = index == highlighted;
                        let open = selected && !folded;
                        let secondary = choose_secondary(&hit.entry, &targets);
                        let response = kit::tree_row(
                            ui,
                            egui::Id::unique(("choose-part-row", hit.entry.key())),
                            &kit::Row {
                                fold: Some(open),
                                icon: "library",
                                label: &hit.entry.name,
                                secondary: &secondary,
                                selected,
                                ..Default::default()
                            },
                        );
                        if response.fold || (response.clicked && selected) {
                            toggled = Some(index);
                        } else if response.clicked {
                            picked = Some(index);
                        }
                        if open && let Some(part) = part_of(&hit.entry) {
                            part_details(ui, engine, part);
                        }
                    });
                    show_all = frame.show_all;
                    if total > hits.len() {
                        kit::hint(
                            ui,
                            &format!("Showing {} of {total}; narrow the filter", hits.len()),
                            "",
                        );
                    }
                },
            );
            kit::actions(ui, |ui| {
                new_part = ui_kit::button(ui, "New Part for these values…", Button::Normal)
                    .on_hover_text(
                        "Open the component's Parts editor with a draft rated at these values",
                    )
                    .clicked();
            });
        });
        if changed {
            self.refresh(engine);
        }
        if let Some(index) = picked {
            self.highlighted = index;
            self.folded = false;
        }
        if let Some(index) = toggled {
            self.folded = !self.folded;
            self.highlighted = index;
        }
        if show_all {
            self.show_all = true;
        }
        if new_part && let Some(instance) = instances.first().and_then(|id| project.instance(*id)) {
            let values = pcbcraft_core::instances::requirements(&project.library, instance)
                .map(|r| crate::resolve_parts::requirement_values(&r))
                .unwrap_or_default();
            self.close();
            return Some(ChooseAction::NewPart((instance.component.id, values)));
        }
        match action {
            Some(DialogAction::Cancel) => {
                self.close();
                Some(ChooseAction::Cancel)
            }
            Some(DialogAction::Primary) => {
                let entry = self.hits().get(self.highlighted).map(|h| h.entry.clone());
                let chosen = self.choose_part();
                if let (Some(entry), Some(_)) = (&entry, &chosen) {
                    self.used_entry(entry);
                }
                match chosen {
                    Some(PaletteAction::Assign { instances, part }) => {
                        refusal::mark(&ctx, Origin::Dialog(id));
                        self.refusal = None;
                        let name = entry.map_or_else(|| "the part".to_owned(), |e| e.name);
                        self.submitted = Some((
                            engine.revision(),
                            refusal::clip(&format!("Assigned {name} to {who}"), MAX_TITLE),
                        ));
                        Some(ChooseAction::Assign { instances, part })
                    }
                    Some(PaletteAction::UpdateAndAssign {
                        instances,
                        part,
                        target,
                    }) => {
                        self.close();
                        Some(ChooseAction::UpdateAndAssign {
                            instances,
                            part,
                            target,
                        })
                    }
                    _ => None,
                }
            }
            None => None,
        }
    }

    /// Enter or a click on a folder component: ask the app for
    /// `library.place_from`. Without Shift the sheet closes; a refusal
    /// reopens it (see [`Self::offer_choice`]).
    fn place_folder(&mut self, keep_open: bool) -> Option<PaletteAction> {
        let hit = self.highlighted_hit()?;
        let key = hit.entry.key();
        let pick = FolderPick::of(&hit.entry)?;
        self.notice = None;
        self.events.push(PaletteEvent::PlaceFrom {
            pick,
            other_revision: OtherRevision::Refuse,
        });
        self.used_key(key);
        if keep_open {
            None
        } else {
            self.close();
            Some(PaletteAction::Close)
        }
    }

    /// Folder names in group order (group `i + 1` is `names[i]`).
    fn group_names(&self) -> Vec<String> {
        self.context
            .sources
            .iter()
            .map(|s| s.name.clone())
            .collect()
    }

    /// The COMPONENT preview of the highlighted result in the properties
    /// card, from `library.inspect`. Assign mode has no preview: the Choose
    /// part dialog shows its Parts.
    pub fn preview(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> Option<crate::schematic_editor::NewPartRequest> {
        if matches!(self.mode, Mode::Assign { .. }) {
            return None;
        }
        if let Some(candidate) = self.highlighted_generic() {
            generic_preview(ui, candidate);
            return None;
        }
        if let Some(hit) = self.highlighted_template() {
            template_preview(ui, hit);
            return None;
        }
        if let Some(hit) = self
            .highlighted_hit()
            .filter(|h| h.entry.source != Source::Project)
        {
            let source = self
                .group_names()
                .get(self.group(&hit.entry).wrapping_sub(1))
                .cloned();
            folder_preview(ui, &hit.entry, source.as_deref().unwrap_or("Library"));
            return None;
        }
        let Some(component) = self.highlighted() else {
            kit::section(ui, "palette-component", "Component", true, None, |ui| {
                kit::hint(
                    ui,
                    "Type to find a component: 10k 0603, res>=10k, tol<=1%",
                    "Field values filter by meaning: res>=10k, tol<=1%, C:1n..100n. \
                     pkg: matches package names only.",
                );
            });
            return None;
        };
        let update = self
            .highlighted_hit()
            .and_then(|h| h.entry.update.clone().map(|u| (u, h.entry.name.clone())));
        self.component_preview(ui, engine, component, update);
        None
    }

    /// The COMPONENT preview of one published revision: the highlighted
    /// row, or the one the Place tool holds while the palette is closed.
    /// `update` is the Library row of a project copy whose source moved on;
    /// its Update… asks the app for the import-then-review.
    pub fn component_preview(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        component: ComponentRef,
        update: Option<(UpdateAvailable, String)>,
    ) {
        let fresh = self
            .preview
            .as_ref()
            .is_some_and(|(revision, c, _)| *revision == engine.revision() && *c == component);
        if !fresh {
            let report = match engine.query(Query::InspectLineage {
                lineage: component.id,
                revision: Some(component.revision),
            }) {
                Ok(QueryResult::Lineage(report)) => Ok(*report),
                Ok(_) => Err("unexpected report".into()),
                Err(error) => Err(crate::refusal::shown_error(&error)),
            };
            self.preview = Some((engine.revision(), component, report));
        }
        let Some((_, _, report)) = &self.preview else {
            return;
        };
        match report {
            Ok(report) => component_section(ui, report),
            Err(error) => {
                kit::section(ui, "palette-component", "Component", true, None, |ui| {
                    kit::refusal(ui, error);
                });
            }
        }
        if let Some((update, name)) = update
            && let Some(pick) = library_row(ui, &update, &name)
        {
            self.events.push(PaletteEvent::UpdateProjectCopy(pick));
            self.close();
        }
    }
}

/// What choosing the highlighted generic part does.
fn generic_preview(ui: &mut Ui, candidate: &GenericCandidate) {
    kit::section(ui, "palette-generic", "Generic part", true, None, |ui| {
        kit::value_row(ui, "Part", &candidate.name);
        if let GenericState::Refused { reason } = &candidate.state {
            kit::refusal(ui, reason);
            return;
        }
        for badge in &candidate.badges {
            kit::hint(ui, &badge.label, &badge.tip);
        }
        kit::value_row(ui, "Values", &candidate.detail);
        if matches!(candidate.state, GenericState::NotCreated) {
            kit::hint(
                ui,
                "Not a component yet: choosing it creates one, then you place it",
                "It starts with no manufacturer part; choose one per instance when you know it.",
            );
        }
    });
}

/// What choosing the highlighted template row makes.
fn template_preview(ui: &mut Ui, hit: &TemplateHit) {
    kit::section(ui, "palette-template", "From template", true, None, |ui| {
        kit::value_row(ui, "Template", &hit.name);
        for (label, value) in template_gallery::summary(hit, &hit.values) {
            kit::value_row(ui, label, &value);
        }
        kit::value_row(ui, "Values", template_gallery::basis(hit));
        kit::hint(
            ui,
            "Not a component yet: choosing it creates one, then you place it",
            "",
        );
    });
}

/// The badges a row shows, always all four.
fn badges(entry: &CatalogEntry) -> [kit::SearchBadge<'static>; 4] {
    catalog::Badge::ALL.map(|badge| kit::SearchBadge {
        label: badge.label(),
        present: entry.badges.get(badge),
        tip: "",
    })
}

fn detail(entry: &CatalogEntry, rows: &RowContext) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(entry.manufacturer.iter().cloned());
    parts.extend(entry.key_values().into_iter().take(4).map(str::to_owned));
    if let Some(revision) = entry.revision {
        parts.push(match entry.source {
            Source::Project => format!("project {revision}"),
            Source::Library(_) => format!("library {revision}"),
        });
    }
    // The project row and the folder row of one lineage name each other.
    match (entry.source, entry.origin, entry.source_lineage) {
        (Source::Project, Some(origin), _) => {
            parts.push(format!("from {}", rows.name(origin.library)));
        }
        (Source::Library(library), _, Some(lineage)) => {
            if let Some(copy) = rows.copies.get(&(library, lineage)) {
                parts.push(format!("in project {copy}"));
            }
        }
        _ => {}
    }
    match rows
        .usage
        .get(&entry.lineage)
        .filter(|_| entry.source == Source::Project)
    {
        Some(by_revision) if by_revision.len() == 1 => {
            if let Some((revision, count)) = by_revision.iter().next() {
                parts.push(format!("{revision} used {count}×"));
            }
        }
        Some(by_revision) => {
            let each: Vec<String> = by_revision
                .iter()
                .map(|(revision, count)| format!("{revision} ×{count}"))
                .collect();
            parts.push(format!("used {}", each.join(", ")));
        }
        None if entry.used > 0 && entry.source == Source::Project => {
            parts.push(format!("used {}×", entry.used));
        }
        None => {}
    }
    parts.join(" · ")
}

/// A project entry copied from a folder that has moved on, as a chip: the
/// newer revision, or the copied revisions whose records changed on disk.
fn update_chip(entry: &CatalogEntry) -> Option<(String, bool)> {
    let update = entry.update.as_ref()?;
    if !update.modified.is_empty() {
        let revisions: Vec<String> = update.modified.iter().map(Revision::to_string).collect();
        return Some((format!("{} changed", revisions.join(", ")), true));
    }
    (update.latest > update.origin.revision).then(|| (format!("Update {}", update.latest), false))
}

/// The tip of [`update_chip`].
fn update_tip(entry: &CatalogEntry) -> Option<String> {
    let update = entry.update.as_ref()?;
    Some(if update.modified.is_empty() {
        format!(
            "{} has {}; this project has {}.",
            update.source, update.latest, update.local.revision
        )
    } else {
        let revisions: Vec<String> = update.modified.iter().map(Revision::to_string).collect();
        format!(
            "{} of this lineage changed in {} after the project copied it; the project keeps its copy.",
            revisions.join(", "),
            update.source
        )
    })
}

/// 0 for the project's entries, `i + 1` for the `i`th folder's.
fn group_of(entry: &CatalogEntry, context: &CatalogContext) -> usize {
    match entry.source {
        Source::Project => 0,
        Source::Library(library) => context
            .sources
            .iter()
            .position(|s| s.library == library)
            .map_or(context.sources.len() + 1, |i| i + 1),
    }
}

/// The other-revision choice's buttons.
enum Choice {
    KeepBoth(FolderPick),
    Update(FolderPick),
    Cancel,
}

/// The inline Update project copy… / Keep both / Cancel choice under the
/// field, after `library.place_from` refused another project revision.
/// Updating is the default (Enter); keeping two revisions of one part is
/// the riskier choice and stays secondary.
fn choice(ui: &mut Ui, pick: &FolderPick, message: &str) -> Option<Choice> {
    kit::search_message(ui, message, false);
    let mut chosen = None;
    kit::actions(ui, |ui| {
        if kit::primary(ui, "Update project copy…", true)
            .on_hover_text(
                "Copy this revision into the project, then review which instances move to it (Enter)",
            )
            .clicked()
        {
            chosen = Some(Choice::Update(pick.clone()));
        }
        if ui_kit::button(ui, "Keep both", Button::Normal)
            .on_hover_text("Copy this revision beside the project's and place it; both stay in use")
            .clicked()
        {
            chosen = Some(Choice::KeepBoth(pick.clone()));
        }
        if ui_kit::button(ui, "Cancel", Button::Normal).clicked() {
            chosen = Some(Choice::Cancel);
        }
    });
    chosen
}

/// The present badges, then the missing ones: "sym · fp · 3D · no MPN".
fn has_line(badges: &catalog::Badges) -> String {
    let present = catalog::Badge::ALL
        .iter()
        .filter(|b| badges.get(**b))
        .map(|b| b.label().to_owned());
    let missing = catalog::Badge::ALL
        .iter()
        .filter(|b| !badges.get(**b))
        .map(|b| format!("no {}", b.label()));
    present.chain(missing).collect::<Vec<_>>().join(" · ")
}

/// The field values a row shows, shown fields first: "10 kΩ · ±1%".
fn values_line(entry: &CatalogEntry) -> String {
    let shown = entry.fields.iter().filter(|f| f.shown);
    let rest = entry.fields.iter().filter(|f| !f.shown);
    shown
        .chain(rest)
        .map(|f| f.text.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The description as one hint line, its full text (and the provenance)
/// on hover.
fn description_hint(ui: &mut Ui, description: &str, provenance: &str) {
    let line = if description.is_empty() {
        provenance
    } else {
        description
    };
    if line.is_empty() {
        return;
    }
    let tip = [description, provenance]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    kit::hint(ui, line, &tip);
}

/// The COMPONENT preview of a folder entry, from its catalog entry:
/// nothing of it is in the project until it is placed.
fn folder_preview(ui: &mut Ui, entry: &CatalogEntry, source: &str) {
    kit::section(ui, "palette-component", "Component", true, None, |ui| {
        let name = match entry.revision {
            Some(revision) => format!("{} · {revision} (latest)", entry.name),
            None => entry.name.clone(),
        };
        kit::value_row(ui, "Component", &name);
        kit::value_row(ui, "Source", &format!("Library · {source}"));
        let values = values_line(entry);
        if !values.is_empty() {
            kit::value_row(ui, "Values", &values);
        }
        if !entry.packages.is_empty() {
            kit::value_row(ui, "Package", &entry.packages.join(", "));
        }
        kit::value_row(ui, "Has", &has_line(&entry.badges));
        kit::hint(
            ui,
            "Placing copies it into the project",
            "Placing copies this revision and what it needs into the project, \
             which then opens without the folder.",
        );
        description_hint(ui, &entry.description, "");
    });
}

/// The revision a Part entry's component is newer than every one of
/// `targets` (instances of the same lineage), if it is.
fn newer_than(entry: &CatalogEntry, targets: &[ComponentRef]) -> Option<ComponentRef> {
    let implements = entry.implements?;
    (!targets.is_empty()
        && targets
            .iter()
            .all(|t| t.id == implements.id && t.revision < implements.revision))
    .then_some(implements)
}

/// The exact Part revision of a Part entry.
fn part_of(entry: &CatalogEntry) -> Option<PartRef> {
    (entry.kind == DefinitionKind::Part)
        .then_some(entry.revision)
        .flatten()
        .map(|r| PartRef::new(entry.lineage, r))
}

/// A fitting Part's dim trailing text: the revision it needs first, or
/// its maker and revision.
fn choose_secondary(entry: &CatalogEntry, targets: &[ComponentRef]) -> String {
    if let Some(newer) = newer_than(entry, targets) {
        return format!("needs {}", newer.revision);
    }
    let mut parts: Vec<String> = entry.manufacturer.iter().cloned().collect();
    parts.extend(entry.revision.map(|r| r.to_string()));
    parts.join(" · ")
}

/// The highlighted Part's details in its expanded row, as the project
/// library holds it: identity, what it implements, its rated values and
/// its sources.
fn part_details(ui: &mut Ui, engine: &Engine, r: PartRef) {
    let library = &engine.project().library;
    let Some(published) = library.part(r) else {
        kit::status(ui, "This revision is not in the project library", Tone::Dim);
        return;
    };
    let part = &published.definition;
    kit::value_row(ui, "Maker", &part.manufacturer);
    kit::value_row(ui, "MPN", &part.mpn);
    let component = library.component(part.component).map_or_else(
        || part.component.to_string(),
        |c| format!("{} · {}", c.meta.name, part.component.revision),
    );
    kit::value_row(ui, "Implements", &component);
    kit::value_row(
        ui,
        "Footprint",
        &pcbcraft_core::instances::binding_name(library, part.binding),
    );
    for spec in &part.specs {
        let label = library
            .field_view(spec.field)
            .map_or_else(|| spec.field.to_string(), |v| v.field.meta.name.clone());
        kit::value_row(
            ui,
            &label,
            &format!(
                "{} · {}",
                crate::part_editor::spec_text(library, spec),
                crate::part_editor::meaning_label(spec.meaning)
            ),
        );
    }
    for reference in &part.references {
        let shown = reference
            .url
            .clone()
            .or_else(|| reference.asset.as_ref().map(|_| "attached copy".into()))
            .unwrap_or_default();
        kit::value_row(ui, &reference.title, &shown);
    }
    for offer in &part.offers {
        kit::value_row(ui, &offer.distributor, &offer.sku);
    }
}

fn row(
    ui: &mut Ui,
    hit: &Hit,
    query: &str,
    highlighted: bool,
    targets: &[ComponentRef],
    favourite: bool,
    rows: &RowContext,
) -> egui::Response {
    let entry = &hit.entry;
    let badges = badges(entry);
    let mut detail = detail(entry, rows);
    if let Some(newer) = newer_than(entry, targets) {
        detail = format!("needs update to {} · {detail}", newer.revision);
    }
    let chip = update_chip(entry);
    let id = egui::Id::unique(("place-palette-row", entry.key()));
    let shown = kit::SearchRow {
        icon: if entry.kind == DefinitionKind::Part {
            "library"
        } else {
            "component"
        },
        label: &entry.name,
        query,
        detail: &detail,
        badges: &badges,
        highlighted,
    };
    let response = if favourite || chip.is_some() {
        kit::search_row_marked(
            ui,
            id,
            &shown,
            kit::RowMarks {
                star: favourite,
                chip: chip.as_ref().map(|(label, warning)| kit::RowChip {
                    label,
                    warning: *warning,
                }),
            },
        )
    } else {
        kit::search_row(ui, id, &shown)
    };
    match update_tip(entry) {
        Some(tip) => response.on_hover_text(tip),
        None => response,
    }
}

/// The LIBRARY row of a project entry whose source moved on: the newer
/// revision with Update… (the same import-then-review as the
/// other-revision choice), and a hint when copied revisions changed there.
fn library_row(ui: &mut Ui, update: &UpdateAvailable, name: &str) -> Option<FolderPick> {
    let mut pick = None;
    kit::section(ui, "palette-library", "Library", true, None, |ui| {
        if update.latest > update.origin.revision {
            let summary = format!("{} available from {}", update.latest, update.source);
            if kit::summary_row(ui, &summary, Some("Update…")) {
                pick = Some(FolderPick {
                    library: update.origin.library,
                    lineage: update.origin.lineage,
                    revision: update.latest,
                    name: name.to_owned(),
                });
            }
        } else {
            kit::value_row(ui, "Source", &update.source);
        }
        if !update.modified.is_empty() {
            let revisions: Vec<String> = update.modified.iter().map(Revision::to_string).collect();
            let revisions = revisions.join(", ");
            kit::hint(
                ui,
                &format!("{revisions} changed in {} since copied", update.source),
                &format!(
                    "{revisions} changed in {} after the project copied it. The project keeps its copy.",
                    update.source
                ),
            );
        }
    });
    pick
}

/// The COMPONENT preview of one inspected revision: name and revision,
/// key values, package and footprint, what it has, and an Info line for
/// unpublished draft changes.
fn component_section(ui: &mut Ui, report: &ItemReport) {
    let entry = &report.entry;
    kit::section(ui, "palette-component", "Component", true, None, |ui| {
        let revision = entry.revision.map_or_else(
            || "draft".into(),
            |r| {
                if report.latest {
                    format!("{r} (latest)")
                } else {
                    r.to_string()
                }
            },
        );
        kit::value_row(ui, "Component", &format!("{} · {revision}", entry.name));
        let values = values_line(entry);
        if !values.is_empty() {
            kit::value_row(ui, "Values", &values);
        }
        for implementation in &report.implementations {
            let [l, w, h] = implementation.envelope;
            kit::value_row(
                ui,
                "Package",
                &format!(
                    "{} · {} × {} × {} mm",
                    implementation.package_name,
                    kit::number_text(l),
                    kit::number_text(w),
                    kit::number_text(h)
                ),
            );
            for footprint in &implementation.footprints {
                kit::value_row(
                    ui,
                    "Footprint",
                    &format!("{} · {} pads", footprint.name, footprint.pads),
                );
            }
        }
        if report.implementations.is_empty() {
            kit::value_row(ui, "Package", "None · schematic only");
        }
        kit::value_row(ui, "Has", &has_line(&entry.badges));
        if entry.draft {
            kit::hint(
                ui,
                "Unpublished draft changes are not placed",
                "This lineage has unpublished draft changes; the published revision is placed.",
            );
        }
        description_hint(ui, &entry.description, &report.provenance);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::format::{EditorFile, FILE_VERSION};

    /// One frame of the Choose part dialog in a column-sized slot: its
    /// texts with their centres, and what it returned.
    fn run_dialog(
        ctx: &egui::Context,
        palette: &mut PlacePalette,
        engine: &Engine,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, egui::Pos2)>, Option<ChooseAction>) {
        let mut action = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(340.0, 1000.0),
                )),
                events,
                ..Default::default()
            },
            |ui| action = palette.choose_dialog(ui, engine),
        );
        output.textures_delta.clear();
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push((t.galley.job.text.clone(), t.pos + t.galley.size() * 0.5))
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            walk(&shape.shape, &mut out);
        }
        (out, action)
    }

    /// Press and release on the text `label`; returns the release frame's
    /// action.
    fn click_dialog(
        ctx: &egui::Context,
        palette: &mut PlacePalette,
        engine: &Engine,
        label: &str,
    ) -> Result<Option<ChooseAction>, Box<dyn std::error::Error>> {
        let (shown, _) = run_dialog(ctx, palette, engine, vec![]);
        let point = shown
            .iter()
            .find(|(t, _)| t == label)
            .map(|(_, p)| *p)
            .ok_or_else(|| format!("missing {label}: {shown:?}"))?;
        let press = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        run_dialog(
            ctx,
            palette,
            engine,
            vec![egui::Event::PointerMoved(point), press(true)],
        );
        Ok(run_dialog(ctx, palette, engine, vec![press(false)]).1)
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Choose part is a docked dialog: it names who gets the Part, keeps
    /// the query in its filter with one hint line and lists the fitting
    /// Parts, the highlighted one expanded. A component row cannot be
    /// chosen and the disabled primary says why; Choose on a Part returns
    /// it for the instances and the dialog waits for the engine. A refusal
    /// shows once on its refusal row and keeps the dialog open. The canvas
    /// sheet draws nothing in assign mode. The result list is set directly:
    /// which entries `library.search` returns is the catalog's concern.
    #[test]
    fn choose_part_is_a_docked_dialog_that_returns_the_highlighted_part()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let mut engine = Engine::demo()?;
        let part = crate::resolve_parts::demo_part(&mut engine, "EX-R0603-10K0-F")?;
        let QueryResult::Search(mut report) = engine.query(Query::Search {
            query: String::new(),
            limit: None,
            include_drafts: false,
        })?
        else {
            return Err("not a search".into());
        };
        let component = report.results.first().cloned().ok_or("no component")?;
        let mut part_hit = component.clone();
        part_hit.entry.kind = DefinitionKind::Part;
        part_hit.entry.lineage = part.id;
        part_hit.entry.revision = Some(part.revision);
        part_hit.entry.name = "EX-R0603-10K0-F".into();
        report.results = vec![part_hit, component];
        report.total = 2;
        let r1 = engine
            .project()
            .instance_by_designator("R1")
            .ok_or("R1")?
            .id;
        let r2 = engine
            .project()
            .instance_by_designator("R2")
            .ok_or("R2")?
            .id;
        let instances = vec![r1, r2];
        let mut palette = PlacePalette::default();
        palette.open_assign("kind:part res=10k".into(), instances.clone());
        assert!(palette.choosing());
        assert_eq!(palette.query(), "kind:part res=10k");
        palette.results = Some(Results {
            revision: engine.revision(),
            context: (0, 0),
            query: palette.query.clone(),
            generic: Vec::new(),
            order: (0..report.results.len()).map(Slot::Hit).collect(),
            report: Ok(report),
            templates: Found::default(),
        });
        // The canvas sheet is not drawn in assign mode.
        let mut sheet = None;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let area = ui.max_rect();
            sheet = palette.show(ui, area, &engine, &|_| true);
        });
        output.textures_delta.clear();
        assert_eq!(sheet, None);
        assert!(texts(&output).is_empty(), "{:?}", texts(&output));
        let (shown, action) = run_dialog(&ctx, &mut palette, &engine, vec![]);
        assert_eq!(action, None);
        let texts: Vec<&str> = shown.iter().map(|(t, _)| t.as_str()).collect();
        for expected in [
            "Choose part",
            "Assign to",
            "R1, R2",
            CHOOSE_HINT,
            "FITTING PARTS",
            "EX-R0603-10K0-F",
            "Maker",
            "ExampleCo",
            "New Part for these values…",
            "Cancel",
            "Choose",
        ] {
            assert!(texts.contains(&expected), "{expected} in {texts:?}");
        }
        assert_eq!(palette.highlighted_part(), Some(part));
        // The component row is not a Part: Choose does nothing.
        run_dialog(&ctx, &mut palette, &engine, vec![key(egui::Key::ArrowDown)]);
        assert_eq!(palette.highlighted_part(), None);
        assert_eq!(click_dialog(&ctx, &mut palette, &engine, "Choose")?, None);
        assert!(palette.is_open());
        run_dialog(&ctx, &mut palette, &engine, vec![key(egui::Key::ArrowUp)]);
        let action = click_dialog(&ctx, &mut palette, &engine, "Choose")?;
        assert_eq!(
            action,
            Some(ChooseAction::Assign {
                instances: instances.clone(),
                part
            })
        );
        assert!(palette.is_open(), "open until the engine answers");
        // The app refuses it: the refusal returns to the dialog's row.
        let id = egui::Id::unique(CHOOSE_DIALOG);
        crate::refusal::hand_back(
            &ctx,
            crate::refusal::Refused {
                origin: Origin::Dialog(id),
                error: pcbcraft_core::Error::Invalid("The Part does not fit R2".into()),
            },
            false,
        );
        run_dialog(&ctx, &mut palette, &engine, vec![]);
        let (shown, _) = run_dialog(&ctx, &mut palette, &engine, vec![]);
        assert!(palette.is_open());
        assert!(
            shown.iter().any(|(t, _)| t == "The Part does not fit R2"),
            "{shown:?}"
        );
        // Cancel closes without a choice.
        assert_eq!(
            click_dialog(&ctx, &mut palette, &engine, "Cancel")?,
            Some(ChooseAction::Cancel)
        );
        assert!(!palette.is_open());
        // Opening for placement returns to Place mode with an empty query.
        palette.open();
        assert_eq!(palette.mode(), &Mode::Place);
        assert!(!palette.choosing());
        assert_eq!(palette.query(), "");
        Ok(())
    }

    /// Through the real engine: the fitting Part found by R1's own query
    /// is assigned in one undo step, the dialog closes and an Undo toast
    /// says what changed. New Part for these values… asks for a draft of
    /// the instance's component at its values.
    #[test]
    fn choose_assigns_through_the_engine_and_closes_with_a_toast()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let mut engine = Engine::demo()?;
        let part = crate::resolve_parts::demo_part(&mut engine, "EX-R0603-10K0-F")?;
        let r1 = engine
            .project()
            .instances
            .first()
            .ok_or("an instance")?
            .clone();
        let mut palette = PlacePalette::default();
        palette.open_assign(crate::resolve_parts::query_for(&engine, r1.id), vec![r1.id]);
        run_dialog(&ctx, &mut palette, &engine, vec![]);
        assert_eq!(palette.highlighted_part(), Some(part));
        let Some(ChooseAction::Assign {
            instances,
            part: chosen,
        }) = click_dialog(&ctx, &mut palette, &engine, "Choose")?
        else {
            return Err("Choose returned no assignment".into());
        };
        assert_eq!(chosen, part);
        engine.apply(
            engine.revision(),
            crate::resolve_parts::assign(instances, Some(chosen)),
        )?;
        assert_eq!(
            engine.project().instance(r1.id).and_then(|i| i.part),
            Some(part)
        );
        let _ = crate::kit::toast::take_posted(&ctx);
        run_dialog(&ctx, &mut palette, &engine, vec![]);
        assert!(!palette.is_open());
        let posted = crate::kit::toast::take_posted(&ctx);
        let toast = posted.first().ok_or("no toast")?;
        assert_eq!(toast.level, Level::Ok);
        assert_eq!(toast.actions, vec![ToastAction::Undo]);
        assert!(
            toast.text.starts_with("Assigned EX-R0603-10K0-F to "),
            "{toast:?}"
        );
        assert!(toast.text.chars().count() <= MAX_TITLE);
        engine.undo()?;
        assert_eq!(engine.project().instance(r1.id).and_then(|i| i.part), None);
        // New Part for these values…
        palette.open_assign(crate::resolve_parts::query_for(&engine, r1.id), vec![r1.id]);
        let Some(ChooseAction::NewPart((component, values))) =
            click_dialog(&ctx, &mut palette, &engine, "New Part for these values…")?
        else {
            return Err("no New Part request".into());
        };
        assert_eq!(component, r1.component.id);
        assert!(!values.is_empty());
        assert!(!palette.is_open());
        Ok(())
    }

    /// A group's query names its exact component revision, and assign mode
    /// still lists that lineage's Part for another revision, so the flagged
    /// row and its reviewed update stay reachable from Choose…. Place mode
    /// searches the text as typed.
    #[test]
    fn assign_mode_searches_a_groups_implements_as_its_lineage()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let part = crate::resolve_parts::demo_part(&mut engine, "EX-R0603-10K0-F")?;
        let body = engine.project().library.part(part).ok_or("the Part")?;
        let lineage = body.definition.component.id.0;
        let exact = format!("kind:part implements:{lineage}/99 \"two words\"");
        let mut palette = PlacePalette::default();
        palette.open_assign(exact.clone(), vec![]);
        assert_eq!(palette.query(), exact, "the group's text stays as written");
        assert_eq!(
            palette.search_text(),
            format!("kind:part implements:{lineage} \"two words\"")
        );
        let search = |query: String| match engine.query(Query::Search {
            query,
            limit: None,
            include_drafts: false,
        }) {
            Ok(QueryResult::Search(report)) => Ok(report.results.len()),
            other => Err(format!("{other:?}")),
        };
        let text = format!("kind:part implements:{lineage}/99");
        assert_eq!(search(text.clone())?, 0, "no Part pins that revision");
        palette.open_assign(text.clone(), vec![]);
        assert_eq!(search(palette.search_text())?, 1, "listed, then flagged");
        palette.open();
        palette.query = text.clone();
        assert_eq!(palette.search_text(), text, "Place mode is literal");
        Ok(())
    }

    /// A Part for a newer revision of the instances' component is flagged
    /// on its row; the primary becomes Review update… and asks for the
    /// reviewed update-and-assign, never a refused plain assignment. In
    /// Place mode, Enter on a Part row explains that Parts are assigned,
    /// not placed.
    #[test]
    fn a_part_for_a_newer_revision_reviews_an_update_and_parts_are_not_placed()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let mut engine = Engine::demo()?;
        let part = crate::resolve_parts::demo_part(&mut engine, "EX-R0603-10K0-F")?;
        let QueryResult::Search(mut report) = engine.query(Query::Search {
            query: "kind:part".into(),
            limit: None,
            include_drafts: false,
        })?
        else {
            return Err("not a search".into());
        };
        let hit = report.results.first_mut().ok_or("no Part")?;
        assert_eq!(part_of(&hit.entry), Some(part));
        let current = hit.entry.implements.ok_or("a Part implements")?;
        let newer = ComponentRef::new(current.id, pcbcraft_core::Revision(current.revision.0 + 1));
        hit.entry.implements = Some(newer);
        assert!(
            detail(&hit.entry, &RowContext::default()).starts_with("ExampleCo"),
            "the maker leads"
        );
        assert_eq!(
            choose_secondary(&hit.entry, &[current]),
            format!("needs {}", newer.revision)
        );
        let set = |palette: &mut PlacePalette, report: &SearchReport| {
            palette.results = Some(Results {
                revision: engine.revision(),
                context: (0, 0),
                query: palette.query.clone(),
                generic: Vec::new(),
                order: (0..report.results.len()).map(Slot::Hit).collect(),
                report: Ok(report.clone()),
                templates: Found::default(),
            });
        };
        let instance = engine.project().instances.first().ok_or("an instance")?.id;
        let mut palette = PlacePalette::default();
        palette.open_assign("kind:part".into(), vec![instance]);
        set(&mut palette, &report);
        // The dialog's refresh rebuilds the targets from the project.
        run_dialog(&ctx, &mut palette, &engine, vec![]);
        palette.targets = vec![current];
        set(&mut palette, &report);
        let action = click_dialog(&ctx, &mut palette, &engine, "Review update…")?;
        assert_eq!(
            action,
            Some(ChooseAction::UpdateAndAssign {
                instances: vec![instance],
                part,
                target: newer
            })
        );
        assert!(!palette.is_open(), "the review takes over");
        // Place mode: a Part row is not placed.
        palette.open();
        set(&mut palette, &report);
        let run = |palette: &mut PlacePalette, events: Vec<egui::Event>| {
            let mut action = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    let area = ui.max_rect();
                    action = palette.show(ui, area, &engine, &|_| true);
                },
            );
            output.textures_delta.clear();
            action
        };
        assert_eq!(run(&mut palette, vec![key(egui::Key::Enter)]), None);
        assert!(palette.is_open());
        assert_eq!(palette.notice.as_deref(), Some(PART_NOT_PLACED));
        Ok(())
    }

    /// Every drawn text of one frame.
    fn texts(output: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            walk(&shape.shape, &mut out);
        }
        out
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// A folder catalog of `count` resistors made from the synthetic
    /// project's entries, renamed so they are told apart from the
    /// project's own, with catalog-local lineages.
    fn folder_catalog(
        library: pcbcraft_core::ids::LibraryId,
        name: &str,
        count: usize,
    ) -> Result<catalog::SourceCatalog, Box<dyn std::error::Error>> {
        let engine = Engine::from_file(EditorFile {
            version: FILE_VERSION,
            project: catalog::synthetic::project(count)?,
            drafts: Default::default(),
        })?;
        let QueryResult::Search(report) = engine.query(Query::Search {
            query: String::new(),
            limit: None,
            include_drafts: false,
        })?
        else {
            return Err("not a search".into());
        };
        let entries = report
            .results
            .into_iter()
            .take(count)
            .enumerate()
            .map(|(i, hit)| {
                let mut entry = hit.entry;
                entry.name = format!("FOLDER-{i}");
                entry.source = Source::Library(library);
                entry.lineage = DefinitionId(i as u64 + 1);
                entry.source_lineage = Some(pcbcraft_core::ids::SourceLineageId(i as u128 + 100));
                entry.used = 0;
                entry
            })
            .collect();
        Ok(catalog::SourceCatalog {
            library,
            name: name.into(),
            entries,
            ..Default::default()
        })
    }

    /// The project's parts, then the generic parts it could make, then the
    /// template rows, and library folders last, whatever the search ranked.
    #[test]
    fn the_list_keeps_its_order() -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::default();
        let found = template_gallery::find_generic(&engine, "resistor 0603");
        let request = found.first().and_then(|c| c.would_create.clone());
        let request = request.ok_or("a request")?;
        // Another prefix makes it a part of its own, not the generic one.
        let own = pcbcraft_core::library::templates::catalog::NewFromTemplate {
            name: "Resistor 0603 house".into(),
            designator_prefix: Some("RH".into()),
            ..request
        };
        engine.apply_hosted(
            engine.revision(),
            pcbcraft_core::Operation::Library(pcbcraft_core::LibraryCommand::PublishFromTemplate(
                Box::new(own),
            )),
            pcbcraft_core::Evaluators::default(),
        )?;
        let library = pcbcraft_core::ids::LibraryId(0xabc);
        let mut folder = folder_catalog(library, "Team parts", 1)?;
        for entry in &mut folder.entries {
            entry.name = "Resistor 0603 team".into();
        }
        let mut palette = PlacePalette::default();
        palette.set_context(1, || CatalogContext {
            sources: vec![std::sync::Arc::new(folder)],
            ..Default::default()
        });
        palette.open();
        palette.query = "resistor 0603".into();
        palette.refresh(&engine);
        let results = palette.results.as_ref().ok_or("results")?;
        let mut kinds: Vec<&str> = results
            .order
            .iter()
            .map(|slot| match slot {
                Slot::Hit(i) => {
                    match results.report.as_ref().ok().and_then(|r| r.results.get(*i)) {
                        Some(hit) if hit.entry.source == Source::Project => "project",
                        _ => "folder",
                    }
                }
                Slot::Generic(_) => "generic",
                Slot::Template(_) => "template",
            })
            .collect();
        kinds.dedup();
        assert_eq!(kinds, ["project", "generic", "template", "folder"]);
        Ok(())
    }

    /// Folder rows follow the project's under a `LIBRARY · <name>` caption;
    /// Enter on one asks the app for `library.place_from` (never a project
    /// `component.place`), remembers it as recent and closes; Cmd+D marks
    /// a favourite; another project revision reopens the sheet on the
    /// Keep both / Update project copy / Cancel choice, and Esc cancels
    /// only the choice.
    #[test]
    fn folder_rows_place_from_the_library_and_offer_the_other_revision_choice()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let library = pcbcraft_core::ids::LibraryId(0xabc);
        let folder = folder_catalog(library, "Team parts", 2)?;
        let mut palette = PlacePalette::default();
        palette.set_context(1, || CatalogContext {
            sources: vec![std::sync::Arc::new(folder)],
            ..Default::default()
        });
        palette.open();
        palette.query = "FOLDER".into();
        let run = |palette: &mut PlacePalette, events: Vec<egui::Event>| {
            let mut action = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let area = ui.max_rect();
                    action = palette.show(ui, area, &engine, &|_| true);
                },
            );
            output.textures_delta.clear();
            (action, texts(&output))
        };
        // A new sheet's first frame only measures its size.
        run(&mut palette, vec![]);
        let (_, shown) = run(&mut palette, vec![]);
        assert!(
            shown.iter().any(|t| t == "LIBRARY · TEAM PARTS  2"),
            "{shown:?}"
        );
        assert!(shown.iter().any(|t| t == "FOLDER-0"), "{shown:?}");
        let pick = palette.highlighted_folder().ok_or("a folder row")?;
        assert_eq!(pick.library, library);
        assert_eq!(palette.highlighted(), None, "not a project component");

        // Cmd+D: a favourite, shown on the row and left for the app.
        let (_, shown) = run(
            &mut palette,
            vec![key_event(egui::Key::D, egui::Modifiers::COMMAND)],
        );
        let key = EntryKey::Library {
            library,
            lineage: pick.lineage,
        };
        assert_eq!(palette.take_events(), [PaletteEvent::ToggleFavourite(key)]);
        run(&mut palette, vec![]);
        // The row draws a star for it; the detail line stays the entry's own.
        assert!(palette.context.favourites.contains(&key));
        assert!(
            !shown.iter().any(|t| t.starts_with("favourite · ")),
            "{shown:?}"
        );
        assert_eq!(
            palette.highlighted_folder().as_ref(),
            Some(&pick),
            "the favourite keeps its highlight"
        );

        // Enter: library.place_from, recorded as recent; the sheet closes.
        let (action, _) = run(
            &mut palette,
            vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(action, Some(PaletteAction::Close));
        assert!(!palette.is_open());
        assert_eq!(
            palette.take_events(),
            [
                PaletteEvent::PlaceFrom {
                    pick: pick.clone(),
                    other_revision: OtherRevision::Refuse
                },
                PaletteEvent::Used(key)
            ]
        );

        // The app reports another project revision: the choice is inline.
        palette.offer_choice(
            pick.clone(),
            "library.other_revision: r1 is in the project".into(),
        );
        run(&mut palette, vec![]);
        let (_, shown) = run(&mut palette, vec![]);
        assert!(palette.is_open());
        for label in ["Update project copy…", "Keep both", "Cancel"] {
            assert!(shown.iter().any(|t| t == label), "{label}: {shown:?}");
        }
        // Esc cancels only the choice and keeps the sheet.
        let (action, shown) = run(
            &mut palette,
            vec![key_event(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert_eq!(action, None);
        assert!(palette.is_open());
        assert!(!shown.iter().any(|t| t == "Keep both"), "{shown:?}");
        assert!(palette.take_events().is_empty());
        // Enter takes the default, Update project copy…, never Keep both.
        palette.offer_choice(pick.clone(), "r1 is in the project".into());
        run(&mut palette, vec![]);
        let (action, _) = run(
            &mut palette,
            vec![key_event(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(action, Some(PaletteAction::Close));
        assert_eq!(
            palette.take_events(),
            [PaletteEvent::UpdateProjectCopy(pick.clone())]
        );
        Ok(())
    }

    /// A project entry copied from a folder that moved on shows the newer
    /// revision; one whose copied record changed on disk says so.
    #[test]
    fn update_available_marks_lead_the_detail_line() -> Result<(), Box<dyn std::error::Error>> {
        use pcbcraft_core::catalog::UpdateAvailable;
        use pcbcraft_core::library::Origin;
        let engine = Engine::from_file(EditorFile {
            version: FILE_VERSION,
            project: catalog::synthetic::project(3)?,
            drafts: Default::default(),
        })?;
        let QueryResult::Search(report) = engine.query(Query::Search {
            query: "EX-R".into(),
            limit: None,
            include_drafts: false,
        })?
        else {
            return Err("not a search".into());
        };
        let mut entry = report.results[0].entry.clone();
        let local = entry.component().ok_or("a component")?.erased();
        let mut update = UpdateAvailable {
            local,
            origin: Origin {
                library: pcbcraft_core::ids::LibraryId(1),
                lineage: pcbcraft_core::ids::SourceLineageId(2),
                revision: pcbcraft_core::Revision(1),
                content_hash: pcbcraft_core::ids::ContentHash([0; 32]),
            },
            source: "Team parts".into(),
            latest: pcbcraft_core::Revision(2),
            modified: Vec::new(),
        };
        entry.update = Some(update.clone());
        assert_eq!(update_chip(&entry), Some(("Update r2".into(), false)));
        assert_eq!(
            update_tip(&entry).as_deref(),
            Some("Team parts has r2; this project has r1.")
        );
        // The chip names the revision whose record changed.
        update.modified = vec![pcbcraft_core::Revision(1)];
        entry.update = Some(update);
        assert_eq!(update_chip(&entry), Some(("r1 changed".into(), true)));
        entry.update = None;
        assert_eq!(update_chip(&entry), None);
        Ok(())
    }

    /// Assign mode lists project Parts only: a folder entry's ids are
    /// catalog-local and can never name the instances' component.
    #[test]
    fn assign_mode_never_lists_folder_entries() -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let part = crate::resolve_parts::demo_part(&mut engine, "EX-R0603-10K0-F")?;
        let library = pcbcraft_core::ids::LibraryId(7);
        let QueryResult::Search(report) = engine.query(Query::Search {
            query: "kind:part".into(),
            limit: None,
            include_drafts: false,
        })?
        else {
            return Err("not a search".into());
        };
        let mut folder_part = report.results.first().ok_or("a Part")?.entry.clone();
        folder_part.source = Source::Library(library);
        folder_part.source_lineage = Some(pcbcraft_core::ids::SourceLineageId(9));
        folder_part.name = "FOLDER-PART".into();
        let instance = engine
            .project()
            .instances
            .iter()
            .find(|i| Some(i.component) == folder_part.implements)
            .map(|i| i.id)
            .ok_or("an instance of the Part's component")?;
        let mut palette = PlacePalette::default();
        palette.set_context(1, || CatalogContext {
            sources: vec![std::sync::Arc::new(catalog::SourceCatalog {
                library,
                name: "Team".into(),
                entries: vec![folder_part],
                ..Default::default()
            })],
            ..Default::default()
        });
        palette.open_assign("kind:part".into(), vec![instance]);
        palette.refresh(&engine);
        assert!(!palette.hits().is_empty());
        assert!(
            palette
                .hits()
                .iter()
                .all(|h| h.entry.source == Source::Project)
        );
        assert_eq!(palette.highlighted_part(), Some(part));
        Ok(())
    }

    #[test]
    fn rows_list_key_values_revision_and_all_four_badges() -> Result<(), Box<dyn std::error::Error>>
    {
        let engine = Engine::from_file(EditorFile {
            version: FILE_VERSION,
            project: catalog::synthetic::project(3)?,
            drafts: Default::default(),
        })?;
        let QueryResult::Search(report) = engine.query(Query::Search {
            query: "EX-R".into(),
            limit: None,
            include_drafts: false,
        })?
        else {
            return Err("not a search".into());
        };
        let entry = &report.results[0].entry;
        assert_eq!(
            detail(entry, &RowContext::default()),
            "10 kΩ · ±1% · 0603 · project r1"
        );
        let shown: Vec<(&str, bool)> = badges(entry).iter().map(|b| (b.label, b.present)).collect();
        assert_eq!(
            shown,
            [("sym", true), ("fp", true), ("3D", true), ("MPN", false)]
        );
        Ok(())
    }
}
