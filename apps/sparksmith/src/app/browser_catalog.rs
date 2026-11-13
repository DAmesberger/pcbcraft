//! The Component workspace's Browser as the catalog (slice A2): the
//! project's components and every library source's items in one tree,
//! searched with the engine's `library.search` over the same
//! [`CatalogContext`] as the Place palette, with Favourites, Recent and the
//! open and place actions.
//!
//! - Clicking a project item opens it (`PartEditor::open`).
//! - Clicking a folder item opens it read-only in the Component workspace
//!   (`PartEditor::open_folder`): its bundle is read on a worker into a
//!   scratch engine and inspected (`catalog::preview::inspect_bundle`);
//!   nothing reaches the project (F4). The identity bar offers Edit a copy
//!   in this project; the strip under the tree shows the inspect summary.
//! - Place in schematic is `library.place_from`: a project revision in
//!   place, a folder item through the bundle read the Place palette uses.
//!
//! [`Catalog`] is rebuilt every frame. Its state lives in egui memory: the
//! app publishes a [`Feed`] before the Browser draws (only in the Component
//! workspace, so the other workspaces keep their design tree), the search
//! text and its cached report are kept per document revision and source
//! generation, and a click leaves one [`Action`] that the app applies on
//! the next frame through the one engine.
use super::{App, Level, ToastAction};
use crate::kit::{self, Row};
use crate::library_sources::{self, BundlePurpose, FolderPick};
use crate::shell::Workspace;
use sparksmith_core::catalog::{
    CatalogContext, CatalogEntry, EntryKey, ItemReport, MAX_QUERY, MAX_RESULTS, Source,
};
use sparksmith_core::ids::LibraryId;
use sparksmith_core::{ComponentRef, DefinitionId, DefinitionKind, Engine, Query, QueryResult};
use sparksmith_ui::ui_kit::{self, Button};
use std::sync::Arc;

/// The search text, kept in egui memory.
const SEARCH: &str = "browser-catalog-search";

fn feed_slot() -> egui::Id {
    egui::Id::unique("browser-catalog-feed")
}

fn action_slot() -> egui::Id {
    egui::Id::unique("browser-catalog-action")
}

/// One configured source as the catalog lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRow {
    /// `None` until its manifest has been read.
    pub library: Option<LibraryId>,
    pub name: String,
    /// "ro" or "rw" when ready, else the state ("loading…", the reason).
    pub state: String,
    pub reason: Option<String>,
}

impl SourceRow {
    fn of(view: &library_sources::SourceView) -> Self {
        let reason = view.reason();
        let state = match (&reason, view.ready()) {
            (None, true) if view.setting.writable => "rw".into(),
            (None, true) => "ro".into(),
            _ => view.short(),
        };
        Self {
            library: view.library(),
            name: view.name.clone(),
            state,
            reason,
        }
    }
}

/// What the app publishes for the catalog before the Browser draws.
#[derive(Clone)]
pub struct Feed {
    /// The sources' generation the context was built at.
    pub generation: u64,
    pub context: Arc<CatalogContext>,
    pub sources: Arc<Vec<SourceRow>>,
    /// Some source is still being read.
    pub loading: bool,
    /// The project lineage open in the workspace.
    pub open: Option<DefinitionId>,
    /// The folder item open read-only (or being read).
    pub folder: Option<OpenFolder>,
}

/// The folder item the Component workspace shows read-only, with its
/// `inspect_bundle` report once read.
#[derive(Clone, Debug)]
pub struct OpenFolder {
    pub pick: FolderPick,
    pub report: Option<Arc<ItemReport>>,
}

/// A catalog item an action names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A project lineage, with its placeable revision when it has one (a
    /// draft-only lineage has none). A project Part names its component.
    Project {
        lineage: DefinitionId,
        component: Option<ComponentRef>,
        key: EntryKey,
    },
    /// A folder component or Part.
    Folder { pick: FolderPick, key: EntryKey },
}

impl Item {
    /// The item an entry opens; `None` for a kind the catalog does not open.
    pub fn of(entry: &CatalogEntry) -> Option<Self> {
        let key = entry.user_key().unwrap_or_else(|| entry.key());
        match entry.source {
            Source::Project => match entry.kind {
                DefinitionKind::Component => Some(Self::Project {
                    lineage: entry.lineage,
                    component: entry.component(),
                    key,
                }),
                DefinitionKind::Part => entry.implements.map(|c| Self::Project {
                    lineage: c.id,
                    component: Some(c),
                    key,
                }),
                _ => None,
            },
            Source::Library(_) => FolderPick::of(entry).map(|pick| Self::Folder { pick, key }),
        }
    }

    fn key(&self) -> EntryKey {
        match self {
            Self::Project { key, .. } | Self::Folder { key, .. } => *key,
        }
    }

    fn placeable(&self) -> bool {
        match self {
            Self::Project { component, .. } => component.is_some(),
            Self::Folder { .. } => true,
        }
    }
}

/// What the user asked of the catalog this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Open(Item),
    Place(Item),
    Favourite(EntryKey),
    ManageSources,
}

fn post(ctx: &egui::Context, action: Action) {
    ctx.data_mut(|d| d.insert_temp(action_slot(), action));
    ctx.request_repaint();
}

/// The catalog's last action, once.
pub fn take_action(ctx: &egui::Context) -> Option<Action> {
    ctx.data_mut(|d| {
        let action = d.get_temp::<Action>(action_slot());
        d.remove::<Action>(action_slot());
        action
    })
}

/// Whether the catalog draws this frame (the Component workspace), so the
/// Browser leaves out the design tree and the separate sources branch.
pub fn active(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Feed>(feed_slot())).is_some()
}

/// Publish the catalog's feed, or withdraw it (`None`) outside the
/// Component workspace.
pub fn publish(ctx: &egui::Context, feed: Option<Feed>) {
    ctx.data_mut(|d| {
        if let Some(feed) = feed {
            d.insert_temp(feed_slot(), feed);
        } else {
            d.remove::<Feed>(feed_slot());
        }
    });
}

/// One search per document revision, source generation and text.
#[derive(Clone)]
struct Searched {
    revision: u64,
    generation: u64,
    text: String,
    result: Arc<Result<Found, String>>,
}

#[derive(Clone, Debug, PartialEq)]
struct Found {
    /// In `library.search` rank order.
    entries: Vec<CatalogEntry>,
    total: usize,
}

/// `library.search` over the project (drafts included) and every ready
/// source, ranked by the engine; the catalog groups it without
/// reordering inside a group.
fn search(
    ctx: &egui::Context,
    engine: &Engine,
    feed: &Feed,
    text: &str,
) -> Arc<Result<Found, String>> {
    let key = egui::Id::unique((SEARCH, "result"));
    if let Some(cached) = ctx.data(|d| d.get_temp::<Searched>(key))
        && cached.revision == engine.revision()
        && cached.generation == feed.generation
        && cached.text == text
    {
        return cached.result;
    }
    let result = match engine.query_with(
        Query::Search {
            query: text.into(),
            limit: Some(MAX_RESULTS),
            include_drafts: true,
        },
        &feed.context,
    ) {
        Ok(QueryResult::Search(report)) => Ok(Found {
            entries: report.results.into_iter().map(|h| h.entry).collect(),
            total: report.total,
        }),
        Ok(_) => Err("unexpected report".into()),
        Err(error) => Err(error.to_string()),
    };
    let result = Arc::new(result);
    ctx.data_mut(|d| {
        d.insert_temp(
            key,
            Searched {
                revision: engine.revision(),
                generation: feed.generation,
                text: text.into(),
                result: Arc::clone(&result),
            },
        );
    });
    result
}

/// The entries of one group, in search order: the project's (`None`) or
/// one library's.
fn group(found: &Found, library: Option<LibraryId>) -> Vec<&CatalogEntry> {
    found
        .entries
        .iter()
        .filter(|e| match (e.source, library) {
            (Source::Project, None) => true,
            (Source::Library(l), Some(wanted)) => l == wanted,
            _ => false,
        })
        .collect()
}

/// The Favourites and Recent groups: entries the context names, Recent in
/// most-recent-first order.
fn favourites<'a>(found: &'a Found, context: &CatalogContext) -> Vec<&'a CatalogEntry> {
    found
        .entries
        .iter()
        .filter(|e| e.named_by(&context.favourites))
        .collect()
}

fn recent<'a>(found: &'a Found, context: &CatalogContext) -> Vec<&'a CatalogEntry> {
    context
        .recent
        .iter()
        .filter_map(|key| {
            found
                .entries
                .iter()
                .find(|e| e.key() == *key || e.user_key() == Some(*key))
        })
        .collect()
}

/// The catalog tree's per-frame view.
#[derive(Default)]
pub struct Catalog {}

impl Catalog {
    /// Draw the catalog in the Component workspace; `false` (elsewhere)
    /// keeps the design tree.
    pub fn show(&mut self, ui: &mut egui::Ui, engine: &Engine) -> bool {
        let Some(feed) = ui.ctx().data(|d| d.get_temp::<Feed>(feed_slot())) else {
            return false;
        };
        ui.spacing_mut().item_spacing.y = 0.0;
        let ctx = ui.ctx().clone();
        let mut text = ctx
            .data(|d| d.get_temp::<String>(egui::Id::unique(SEARCH)))
            .unwrap_or_default();
        let found = search(&ctx, engine, &feed, &text);
        ui.add_space(2.0);
        let field = kit::filter_field(
            ui,
            egui::Id::unique((SEARCH, "field")),
            &mut text,
            "Search: name, 10k, res>=1k, pkg:0603",
            found.is_err(),
            MAX_QUERY,
        );
        if field.changed {
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique(SEARCH), text.clone()));
        }
        ui.add_space(2.0);
        let found = match found.as_ref() {
            Ok(found) => found,
            Err(error) => {
                kit::search_message(ui, error, true);
                return true;
            }
        };
        let searching = !text.trim().is_empty();
        if searching && found.entries.is_empty() {
            kit::search_message(ui, "Nothing matches.", false);
        } else if found.total > found.entries.len() {
            kit::search_message(
                ui,
                &format!(
                    "Showing the best {} of {} matches; refine the search.",
                    found.entries.len(),
                    found.total
                ),
                false,
            );
        }
        if feed.loading {
            kit::search_message(ui, "Reading library folders…", false);
        }
        let mut action = None;
        let project = group(found, None);
        let count = project.len().to_string();
        if super::browser::folder_labelled(
            ui,
            "catalog-project",
            0,
            "This project",
            &count,
            feed.open.is_some(),
            true,
        ) {
            entries(ui, "project", &project, &feed, &mut action);
        }
        for source in feed.sources.iter() {
            let key = format!("catalog-source-{}", source.name);
            let Some(library) = source.library.filter(|_| source.reason.is_none()) else {
                // Listed with its reason, never left out (S3).
                let tip = source.reason.clone().unwrap_or_default();
                kit::tree_row(
                    ui,
                    egui::Id::unique(("browser", key.as_str())),
                    &Row {
                        depth: 0,
                        icon: if source.reason.is_some() {
                            "status_warning"
                        } else {
                            "library"
                        },
                        label: &source.name,
                        secondary: &source.state,
                        dim: true,
                        tip: &tip,
                        ..Default::default()
                    },
                );
                continue;
            };
            let items = group(found, Some(library));
            let secondary = format!("{} · {}", source.state, items.len());
            let lit = feed
                .folder
                .as_ref()
                .is_some_and(|p| p.pick.library == library);
            if super::browser::folder_labelled(
                ui,
                &key,
                0,
                &source.name,
                &secondary,
                lit,
                !searching || !items.is_empty(),
            ) {
                entries(ui, &key, &items, &feed, &mut action);
            }
        }
        if !searching {
            let marked = favourites(found, &feed.context);
            if !marked.is_empty()
                && super::browser::folder_labelled(
                    ui,
                    "catalog-favourites",
                    0,
                    "Favourites",
                    &marked.len().to_string(),
                    false,
                    false,
                )
            {
                entries(ui, "favourites", &marked, &feed, &mut action);
            }
            let used = recent(found, &feed.context);
            if !used.is_empty()
                && super::browser::folder_labelled(
                    ui,
                    "catalog-recent",
                    0,
                    "Recent",
                    &used.len().to_string(),
                    false,
                    false,
                )
            {
                entries(ui, "recent", &used, &feed, &mut action);
            }
        }
        if kit::tree_row(
            ui,
            egui::Id::unique("browser-catalog-manage"),
            &Row {
                depth: 0,
                icon: "folder",
                label: "Library sources…",
                dim: true,
                tip: "Add a library folder, turn sources on or off, rescan",
                ..Default::default()
            },
        )
        .clicked
        {
            action = Some(Action::ManageSources);
        }
        ui.spacing_mut().item_spacing.y = sparksmith_ui::theme::Tokens::get().space_4;
        if let Some(chosen) = details(ui, engine, found, &feed) {
            action = Some(chosen);
        }
        if let Some(action) = action {
            post(&ctx, action);
        }
        true
    }
}

/// Whether `entry` is the item open in the workspace.
fn is_open(entry: &CatalogEntry, feed: &Feed) -> bool {
    match (&feed.folder, Item::of(entry)) {
        (Some(folder), Some(Item::Folder { pick, .. })) => folder.pick == pick,
        (Some(_), _) => false,
        (None, Some(Item::Project { lineage, .. })) => {
            entry.kind == DefinitionKind::Component && feed.open == Some(lineage)
        }
        (None, _) => false,
    }
}

/// The rows of one group; a click opens the item.
fn entries(
    ui: &mut egui::Ui,
    group: &str,
    entries: &[&CatalogEntry],
    feed: &Feed,
    action: &mut Option<Action>,
) {
    for entry in entries {
        let Some(item) = Item::of(entry) else {
            continue;
        };
        let label = match (&entry.manufacturer, &entry.mpn) {
            (Some(m), Some(mpn)) => format!("{m} {mpn}"),
            _ => entry.name.clone(),
        };
        let secondary = match (entry.source, entry.revision, entry.draft) {
            (Source::Project, Some(r), true) => format!("{r} · draft"),
            (Source::Project, None, _) => "draft".into(),
            (_, Some(r), _) => r.to_string(),
            (_, None, _) => String::new(),
        };
        let tip = match &item {
            Item::Folder { .. } => {
                "Opens read-only; Edit a copy in this project to change it".to_owned()
            }
            Item::Project { .. } if entry.kind == DefinitionKind::Part => {
                "Manufacturer part: opens its component".to_owned()
            }
            Item::Project { .. } => entry.description.clone(),
        };
        let response = kit::tree_row(
            ui,
            egui::Id::unique(("browser-catalog", group, entry.key())),
            &Row {
                depth: 1,
                icon: if entry.kind == DefinitionKind::Part {
                    "library"
                } else {
                    "component"
                },
                label: &label,
                secondary: &secondary,
                selected: is_open(entry, feed),
                modified: entry.draft,
                tip: &tip,
                ..Default::default()
            },
        );
        if response.clicked {
            *action = Some(Action::Open(item));
        }
    }
}

/// The open item's strip under the tree: what it is, and Place in
/// schematic and Favourite.
fn details(ui: &mut egui::Ui, engine: &Engine, found: &Found, feed: &Feed) -> Option<Action> {
    let (title, lines, item) = match &feed.folder {
        Some(folder) => {
            let key = EntryKey::Library {
                library: folder.pick.library,
                lineage: folder.pick.lineage,
            };
            let source = feed
                .sources
                .iter()
                .find(|s| s.library == Some(folder.pick.library))
                .map_or_else(|| folder.pick.library.to_string(), |s| s.name.clone());
            let mut lines = vec![format!("Read-only · {source} {}", folder.pick.revision)];
            match &folder.report {
                Some(report) => lines.push(summary(report)),
                None => lines.push("Reading…".into()),
            }
            let item = Item::Folder {
                pick: folder.pick.clone(),
                key,
            };
            (folder.pick.name.clone(), lines, Some(item))
        }
        None => {
            let lineage = feed.open?;
            let entry = found
                .entries
                .iter()
                .find(|e| e.source == Source::Project && e.lineage == lineage);
            let library = &engine.project().library;
            let name = entry.map_or_else(
                || {
                    library
                        .latest(lineage)
                        .and_then(|(_, r)| library.component(ComponentRef::new(lineage, r)))
                        .map_or_else(String::new, |c| c.meta.name.clone())
                },
                |e| e.name.clone(),
            );
            let lines = vec![match entry.and_then(|e| e.revision) {
                Some(r) => format!("This project · {r}"),
                None => "This project · never published".into(),
            }];
            (name, lines, entry.and_then(Item::of))
        }
    };
    let mut action = None;
    ui.add_space(6.0);
    ui.separator();
    // One line each, cut with an ellipsis (full text on hover), so a long
    // name never wraps the strip or widens the Browser.
    ui.add(egui::Label::new(egui::RichText::new(title).strong()).truncate());
    for line in lines {
        ui.add(egui::Label::new(egui::RichText::new(line).small().weak()).truncate());
    }
    let item = item?;
    kit::actions(ui, |ui| {
        if ui
            .add_enabled_ui(item.placeable(), |ui| {
                ui_kit::button(ui, "Place in schematic", Button::Normal)
            })
            .inner
            .on_hover_text("Switch to the Schematic and place it (one undo step)")
            .on_disabled_hover_text("Publish the component first")
            .clicked()
        {
            action = Some(Action::Place(item.clone()));
        }
        let favourite = feed.context.favourites.contains(&item.key());
        let label = if favourite { "★" } else { "☆" };
        if ui_kit::button(ui, label, Button::Normal)
            .on_hover_text(if favourite {
                "Remove from Favourites"
            } else {
                "Add to Favourites"
            })
            .clicked()
        {
            action = Some(Action::Favourite(item.key()));
        }
    });
    action
}

/// A folder item's one-line summary: terminals and packages.
fn summary(report: &sparksmith_core::catalog::ItemReport) -> String {
    let mut packages: Vec<&str> = report
        .implementations
        .iter()
        .map(|i| i.package_name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    packages.dedup();
    let terminals = match report.terminals {
        1 => "1 terminal".to_owned(),
        n => format!("{n} terminals"),
    };
    if packages.is_empty() {
        terminals
    } else {
        format!("{terminals} · {}", packages.join(", "))
    }
}

impl App {
    /// Per frame before the Browser draws: apply the catalog's last action,
    /// then publish its feed in the Component workspace (and withdraw it in
    /// the others).
    pub(super) fn catalog(&mut self, ctx: &egui::Context) {
        if let Some(action) = take_action(ctx) {
            self.catalog_action(action);
        }
        if self.workspace != Workspace::Part {
            publish(ctx, None);
            return;
        }
        let generation = self.sources.generation();
        let kept = ctx
            .data(|d| d.get_temp::<Feed>(feed_slot()))
            .filter(|f| f.generation == generation);
        let (context, sources) = match kept {
            Some(feed) => (feed.context, feed.sources),
            None => (
                Arc::new(self.sources.context()),
                Arc::new(self.sources.sources().iter().map(SourceRow::of).collect()),
            ),
        };
        publish(
            ctx,
            Some(Feed {
                generation,
                context,
                sources,
                loading: self.sources.loading(),
                open: self.part.lineage(),
                folder: self.part.folder_subject().map(|(pick, report)| OpenFolder {
                    pick: pick.clone(),
                    report: report.cloned(),
                }),
            }),
        );
    }

    fn catalog_action(&mut self, action: Action) {
        match action {
            Action::Open(Item::Project { lineage, key, .. }) => {
                self.workspace = Workspace::Part;
                self.part.open(lineage, &self.engine);
                self.sources.used(key);
            }
            Action::Open(Item::Folder { pick, .. }) => {
                // The part editor asks the app to read it
                // (`FolderAction::Preview`), which also marks it used.
                self.workspace = Workspace::Part;
                self.part.open_folder(pick);
            }
            Action::Place(_) if self.files.busy() => {
                self.say(Level::Info, "Wait for the file operation to finish");
            }
            Action::Place(Item::Project {
                component: Some(component),
                key,
                ..
            }) => {
                let position = self.schematic.place_point();
                match library_sources::place_project(&mut self.engine, component, position) {
                    Ok(library_sources::Placed::Placed {
                        designator,
                        instance,
                        position,
                    }) => {
                        self.sources.used(key);
                        self.workspace = Workspace::Schematic;
                        self.schematic
                            .reveal_placed(instance, position, self.engine.project());
                        self.say_with(
                            Level::Ok,
                            format!("Placed {designator}"),
                            vec![ToastAction::Undo],
                        );
                    }
                    Ok(library_sources::Placed::OtherRevision { message }) => {
                        self.say(Level::Error, message);
                    }
                    Err(error) => self.say(Level::Error, error),
                }
            }
            Action::Place(Item::Project {
                component: None, ..
            }) => self.say(Level::Info, "Publish the component before placing it"),
            Action::Place(Item::Folder { pick, key }) => {
                // The palette shows the other-revision choice and any
                // refusal, so the Schematic comes forward now.
                self.sources.used(key);
                self.workspace = Workspace::Schematic;
                self.sources.read_bundle(
                    pick,
                    BundlePurpose::Place(sparksmith_core::library::import::OtherRevision::default()),
                );
            }
            Action::Favourite(key) => self.sources.toggle_favourite(key),
            Action::ManageSources => self.sources.open_card(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparksmith_core::catalog::{SourceCatalog, source_catalog};
    use sparksmith_core::format::{EditorFile, FILE_VERSION};

    type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;
    const FOLDER: LibraryId = LibraryId(0xa2);

    fn labels(output: &egui::FullOutput) -> Vec<String> {
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

    /// A project of synthetic components, and a folder catalog holding the
    /// promoted closure of its first two.
    fn fixture() -> Fallible<(Engine, Arc<SourceCatalog>)> {
        let project = sparksmith_core::catalog::synthetic::project_with_parts(6, &sparksmith_core::NewBoard::fixture())?;
        let mut records = Vec::new();
        let fresh: Vec<sparksmith_core::ids::SourceLineageId> = (1..=256u128)
            .map(|n| sparksmith_core::ids::SourceLineageId((n << 64) | n))
            .collect();
        let mut held = Vec::new();
        for (at, component) in project
            .library
            .latest_components()
            .iter()
            .take(2)
            .enumerate()
        {
            let root = sparksmith_core::ids::DefinitionRef {
                kind: DefinitionKind::Component,
                id: component.id,
                revision: component.revision,
            };
            let ids = &fresh[at * 64..(at + 1) * 64];
            let plan = sparksmith_core::library::import::export(
                &project.library,
                root,
                FOLDER,
                "Sample Parts",
                ids,
            )?;
            for record in plan.bundle.records {
                if !held.contains(&(record.lineage, record.revision)) {
                    held.push((record.lineage, record.revision));
                    records.push(record);
                }
            }
        }
        let catalog = Arc::new(source_catalog(FOLDER, "Sample Parts", &records)?);
        let engine = Engine::from_file(EditorFile {
            version: FILE_VERSION,
            project,
            drafts: Default::default(),
        })?;
        Ok((engine, catalog))
    }

    fn feed(catalog: &Arc<SourceCatalog>, favourites: &[EntryKey]) -> Feed {
        Feed {
            generation: 1,
            context: Arc::new(CatalogContext {
                favourites: favourites.iter().copied().collect(),
                recent: Vec::new(),
                sources: vec![Arc::clone(catalog)],
                without_starter_rows: false,
            }),
            sources: Arc::new(vec![
                SourceRow {
                    library: Some(FOLDER),
                    name: "Sample Parts".into(),
                    state: "ro".into(),
                    reason: None,
                },
                SourceRow {
                    library: None,
                    name: "Gone".into(),
                    state: "path missing".into(),
                    reason: Some("path missing".into()),
                },
            ]),
            loading: false,
            open: None,
            folder: None,
        }
    }

    fn frame(ctx: &egui::Context, engine: &Engine) -> (bool, Vec<String>) {
        let mut drew = false;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 4000.0),
                )),
                ..Default::default()
            },
            |ui| {
                kit::form(ui, |ui| drew = Catalog::default().show(ui, engine));
            },
        );
        output.textures_delta.clear();
        (drew, labels(&output))
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        sparksmith_ui::fonts::setup_bundled_fonts(&ctx);
        ctx
    }

    /// Without a feed (Schematic, PCB) the catalog draws nothing and the
    /// Browser keeps its design tree.
    #[test]
    fn the_catalog_draws_only_with_a_feed() -> Fallible<()> {
        let (engine, catalog) = fixture()?;
        let ctx = context();
        assert_eq!(frame(&ctx, &engine), (false, vec![]));
        publish(&ctx, Some(feed(&catalog, &[])));
        assert!(active(&ctx));
        let (drew, shown) = frame(&ctx, &engine);
        assert!(drew);
        for text in [
            "This project",
            "Sample Parts",
            "Gone",
            "path missing",
            "Library sources…",
        ] {
            assert!(shown.iter().any(|t| t == text), "{text}: {shown:?}");
        }
        publish(&ctx, None);
        assert!(!active(&ctx));
        Ok(())
    }

    /// The tree lists every project and folder item, and a search lists
    /// each group in `library.search` order (the S1 pattern).
    #[test]
    fn groups_keep_the_engines_search_order() -> Fallible<()> {
        let (engine, catalog) = fixture()?;
        let ctx = context();
        let feed = feed(&catalog, &[]);
        publish(&ctx, Some(feed.clone()));
        for text in ["", "EX-R", "res>=10k"] {
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique(SEARCH), text.to_owned()));
            let (_, shown) = frame(&ctx, &engine);
            let QueryResult::Search(report) = engine.query_with(
                Query::Search {
                    query: text.into(),
                    limit: Some(MAX_RESULTS),
                    include_drafts: true,
                },
                &feed.context,
            )?
            else {
                return Err("not a search".into());
            };
            let expected = |library: Option<LibraryId>| -> Vec<String> {
                report
                    .results
                    .iter()
                    .map(|h| &h.entry)
                    .filter(|e| match library {
                        None => e.source == Source::Project,
                        Some(l) => e.source == Source::Library(l),
                    })
                    .filter(|e| Item::of(e).is_some())
                    .map(|e| match (&e.manufacturer, &e.mpn) {
                        (Some(m), Some(mpn)) => format!("{m} {mpn}"),
                        _ => e.name.clone(),
                    })
                    .collect()
            };
            let project = expected(None);
            let folder = expected(Some(FOLDER));
            assert!(!folder.is_empty(), "{text}: the folder has matches");
            // Rows appear in the tree in group order, then search order.
            let at = |name: &str, from: usize| {
                shown
                    .iter()
                    .skip(from)
                    .position(|t| t == name)
                    .map(|p| p + from)
            };
            let mut cursor = at("This project", 0).ok_or("project header")?;
            for name in &project {
                cursor = at(name, cursor + 1).ok_or_else(|| format!("{text}: {name}"))?;
            }
            cursor = at("Sample Parts", cursor + 1).ok_or("folder header")?;
            for name in &folder {
                cursor = at(name, cursor + 1).ok_or_else(|| format!("{text}: {name}"))?;
            }
        }
        Ok(())
    }

    /// A project entry opens its lineage (a Part opens its component) and a
    /// folder entry opens read-only by its folder pick.
    #[test]
    fn items_open_project_lineages_and_folder_picks() -> Fallible<()> {
        let (engine, catalog) = fixture()?;
        let QueryResult::Search(report) = engine.query_with(
            Query::Search {
                query: String::new(),
                limit: Some(MAX_RESULTS),
                include_drafts: true,
            },
            &CatalogContext {
                sources: vec![Arc::clone(&catalog)],
                ..Default::default()
            },
        )?
        else {
            return Err("not a search".into());
        };
        let project_component = report
            .results
            .iter()
            .map(|h| &h.entry)
            .find(|e| e.source == Source::Project && e.kind == DefinitionKind::Component)
            .ok_or("component")?;
        assert_eq!(
            Item::of(project_component),
            Some(Item::Project {
                lineage: project_component.lineage,
                component: project_component.component(),
                key: EntryKey::Project {
                    lineage: project_component.lineage
                },
            })
        );
        let project_part = report
            .results
            .iter()
            .map(|h| &h.entry)
            .find(|e| e.source == Source::Project && e.kind == DefinitionKind::Part)
            .ok_or("part")?;
        let implements = project_part.implements.ok_or("implements")?;
        assert!(matches!(
            Item::of(project_part),
            Some(Item::Project { lineage, component: Some(c), .. }) if lineage == implements.id && c == implements
        ));
        let folder = report
            .results
            .iter()
            .map(|h| &h.entry)
            .find(|e| e.source == Source::Library(FOLDER))
            .ok_or("folder entry")?;
        let Some(Item::Folder { pick, key }) = Item::of(folder) else {
            return Err("not a folder item".into());
        };
        assert_eq!(Some(pick.lineage), folder.source_lineage);
        assert_eq!(key, folder.key());
        Ok(())
    }

    /// Favourites list what the context marks, and the open folder preview
    /// shows its summary with Place in schematic.
    #[test]
    fn favourites_and_the_open_preview_are_shown() -> Fallible<()> {
        let (engine, catalog) = fixture()?;
        let entry = catalog.entries.first().ok_or("entry")?;
        let ctx = context();
        let mut feed = feed(&catalog, &[entry.key()]);
        let pick = FolderPick::of(entry).ok_or("pick")?;
        feed.folder = Some(OpenFolder { pick, report: None });
        publish(&ctx, Some(feed));
        ctx.data_mut(|d| {
            d.insert_persisted(
                egui::Id::unique(("browser-open", "catalog-favourites")),
                true,
            );
        });
        let (_, shown) = frame(&ctx, &engine);
        assert!(shown.iter().any(|t| t == "Favourites"), "{shown:?}");
        assert!(
            shown.iter().filter(|t| **t == entry.name).count() >= 3,
            "folder row, favourite row and the strip title: {shown:?}"
        );
        assert!(shown.iter().any(|t| t == "Reading…"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "Place in schematic"), "{shown:?}");
        assert!(
            shown
                .iter()
                .any(|t| t.starts_with("Read-only · Sample Parts")),
            "{shown:?}"
        );
        Ok(())
    }

    /// The catalog in the Browser card at its narrowest and widest: every
    /// text (header, search, rows, the open item's strip and its actions)
    /// stays inside the card, cut with an ellipsis where it does not fit.
    #[test]
    fn the_catalog_fits_the_browser_at_every_width() -> Fallible<()> {
        let (engine, catalog) = fixture()?;
        let entry = catalog.entries.first().ok_or("entry")?;
        let pick = FolderPick::of(entry).ok_or("pick")?;
        for width in [
            super::super::chrome::MIN_BROWSER_WIDTH,
            super::super::chrome::BROWSER_WIDTH,
            super::super::chrome::MAX_BROWSER_WIDTH,
        ] {
            let ctx = context();
            let mut feed = feed(&catalog, &[entry.key()]);
            feed.folder = Some(OpenFolder {
                pick: pick.clone(),
                report: None,
            });
            Arc::make_mut(&mut feed.sources).push(SourceRow {
                library: None,
                name: "A library folder with a very long name indeed".into(),
                state: "path missing on this machine".into(),
                reason: Some("path missing".into()),
            });
            publish(&ctx, Some(feed));
            let card = egui::Rect::from_min_size(egui::pos2(7.0, 7.0), egui::vec2(width, 3000.0));
            let mut painted = Vec::new();
            for _ in 0..3 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1400.0, 3100.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        crate::shell::card(
                            ui.ctx(),
                            "browser-test",
                            card,
                            None,
                            |ui| kit::card_title(ui, "Browser", "6 components · 2 sheets"),
                            |ui| {
                                Catalog::default().show(ui, &engine);
                            },
                        );
                    },
                );
                output.textures_delta.clear();
                painted = texts(&output);
            }
            assert!(painted.len() > 8, "{width}: {painted:?}");
            for (text, rect) in &painted {
                assert!(
                    rect.left() >= card.left() - 0.5 && rect.right() <= card.right() + 0.5,
                    "{width}: {text:?} at {rect:?} leaves the card {card:?}"
                );
            }
        }
        Ok(())
    }

    /// Every painted text and its screen rectangle.
    fn texts(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) if !text.galley.is_empty() => out.push((
                    text.galley.job.text.clone(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                )),
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

    #[test]
    fn actions_are_taken_once() {
        let ctx = egui::Context::default();
        post(&ctx, Action::ManageSources);
        assert_eq!(take_action(&ctx), Some(Action::ManageSources));
        assert_eq!(take_action(&ctx), None);
    }
}
