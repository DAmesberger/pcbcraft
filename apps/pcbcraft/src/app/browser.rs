//! The project Browser: one tree over the whole design. Rows come from the
//! project and drafts; clicking a row opens its workspace and selects it
//! there. A component row only selects on a click; a double-click opens it
//! for editing (as Solidcraft's Browser does). The tree reads state only;
//! the app applies the returned [`Pick`] through the editors.
use crate::issues::{Badge, Badges};
use crate::kit::{self, Row};
use pcbcraft_core::catalog::{MAX_QUERY, MAX_RESULTS, UpdateAvailable};
use pcbcraft_core::library::Definition;
use pcbcraft_core::schematic::SheetRow;
use pcbcraft_core::{DefinitionId, Engine, Id, Query, QueryResult, SheetId};
use std::collections::{BTreeMap, BTreeSet};

/// What the Browser highlights: the active workspace's selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lit {
    pub component: Option<DefinitionId>,
    pub instance: Option<Id>,
    pub placement: Option<Id>,
    pub net: Option<Id>,
    pub track: Option<Id>,
    pub board: bool,
    /// The schematic's active sheet.
    pub sheet: Option<SheetId>,
}

/// A row the user picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Select a component row; the workspace stays and nothing is drafted.
    Component(DefinitionId),
    /// Open a component in the Component workspace for editing.
    EditComponent(DefinitionId),
    Instance(Id),
    /// Select an instance and bring it into view on its sheet.
    Locate(Id),
    Net(Id),
    Placement(Id),
    Track(Id),
    Board,
    /// A schematic sheet: show it.
    Sheet(SheetId),
    /// Run a registry command, as the palette would (a folder's menu).
    Command(&'static str),
    /// Open the review that removes an instance, as its Delete… on the
    /// sheet does.
    Remove(Id),
}

/// What a Browser row's right-click menu is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuTarget {
    Component(DefinitionId),
    /// An instance row, with its component lineage.
    Instance(Id, DefinitionId),
    /// A folder row, by its key.
    Folder(&'static str),
}

/// The rows of a Browser row's menu: each picks what the row's click or
/// double-click picks, locates as Issues does, or runs what the same
/// object's menu on the sheet runs. A folder offers the commands that add
/// to it; a folder with none has no menu.
pub fn menu_rows(ctx: &egui::Context, target: MenuTarget) -> crate::context_menu::Rows<Pick> {
    use crate::context_menu::Entry;
    let command = |id: &'static str, label: &str| {
        let mut entry = Entry::command(ctx, id, label, true);
        entry.run = crate::context_menu::Run::Act(Pick::Command(id));
        Some(entry)
    };
    match target {
        MenuTarget::Component(id) => vec![Some(Entry::act(
            "Edit component",
            "component",
            Pick::EditComponent(id),
            true,
        ))],
        MenuTarget::Instance(id, component) => vec![
            Some(Entry::act("Select", "select", Pick::Instance(id), true)),
            Some(Entry::act("Locate", "zoom", Pick::Locate(id), true)),
            None,
            Some(Entry::act(
                "Edit component",
                "component",
                Pick::EditComponent(component),
                true,
            )),
            None,
            Some(
                Entry::act("Delete…", "delete", Pick::Remove(id), true)
                    .key_of(ctx, "schematic.delete"),
            ),
        ],
        MenuTarget::Folder(key) => match key {
            "components" => vec![command("component.new", "New component…")],
            "schematic" => vec![
                command("schematic.place", "Place part"),
                command("schematic.add-sheet", "Add sheet"),
            ],
            "sheets" => vec![command("schematic.add-sheet", "Add sheet")],
            "instances" => vec![
                command("schematic.place", "Place part"),
                command("schematic.annotate", "Annotate"),
            ],
            "nets" => vec![
                command("schematic.wire", "Wire"),
                command("schematic.label", "Label"),
            ],
            "pcb" => vec![
                command("pcb.update", "Update PCB"),
                command("pcb.production", "Production files…"),
            ],
            "placements" => vec![command("pcb.update", "Update PCB")],
            "tracks" => vec![command("pcb.route", "Route")],
            _ => Vec::new(),
        },
    }
}

/// One component lineage as the Browser lists it.
struct ComponentRow {
    id: DefinitionId,
    name: String,
    revision: Option<u32>,
    draft: bool,
}

fn components(engine: &Engine) -> Vec<ComponentRow> {
    let library = &engine.project().library;
    let drafts = engine.drafts();
    let mut rows: Vec<ComponentRow> = library
        .latest_components()
        .into_iter()
        .map(|c| ComponentRow {
            id: c.id,
            name: drafts
                .get(c.id)
                .map_or_else(|| c.meta.name.clone(), |d| d.meta.name.clone()),
            revision: Some(c.revision.0),
            draft: crate::part_editor::WorkingSet::find(drafts, Some(c.id))
                .filter(|set| set.ids.component == c.id)
                .is_some_and(|set| set.unpublished(engine)),
        })
        .collect();
    let published: BTreeSet<DefinitionId> = rows.iter().map(|r| r.id).collect();
    for draft in &drafts.items {
        if matches!(draft.definition, Definition::Component(_)) && !published.contains(&draft.id) {
            rows.push(ComponentRow {
                id: draft.id,
                name: draft.meta.name.clone(),
                revision: None,
                draft: true,
            });
        }
    }
    rows
}

/// The Components filter text, kept in egui memory.
const FILTER: &str = "browser-component-filter";

/// The lineages one filter text keeps, in rank order, and how many matched
/// in all (the list holds at most [`MAX_RESULTS`]).
#[derive(Clone, Debug, PartialEq)]
struct Matches {
    order: Vec<DefinitionId>,
    total: usize,
}

/// One filter result per document revision and text.
#[derive(Clone)]
struct Filtered {
    revision: u64,
    text: String,
    result: Result<Option<Matches>, String>,
}

/// The lineages a filter text keeps, ranked by the engine's
/// `library.search` (drafts included); `None` for an empty filter.
fn filtered(ctx: &egui::Context, engine: &Engine, text: &str) -> Result<Option<Matches>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let key = egui::Id::unique((FILTER, "result"));
    if let Some(cached) = ctx.data(|d| d.get_temp::<Filtered>(key))
        && cached.revision == engine.revision()
        && cached.text == text
    {
        return cached.result;
    }
    let result = match engine.query(Query::Search {
        query: text.into(),
        limit: Some(MAX_RESULTS),
        include_drafts: true,
    }) {
        Ok(QueryResult::Search(report)) => Ok(Some(Matches {
            order: report.results.iter().map(|h| h.entry.lineage).collect(),
            total: report.total,
        })),
        Ok(_) => Err("unexpected report".into()),
        Err(error) => Err(error.to_string()),
    };
    ctx.data_mut(|d| {
        d.insert_temp(
            key,
            Filtered {
                revision: engine.revision(),
                text: text.into(),
                result: result.clone(),
            },
        );
        // A text that fails to parse keeps the list of the last one that
        // did, so the rows do not jump while a predicate is typed.
        if let Ok(matches) = &result {
            d.insert_temp(
                egui::Id::unique((FILTER, "last-good")),
                (engine.revision(), matches.clone()),
            );
        }
    });
    result
}

/// The rows of the last filter text that parsed, at this revision.
fn last_good(ctx: &egui::Context, engine: &Engine) -> Option<Matches> {
    ctx.data(|d| d.get_temp::<(u64, Option<Matches>)>(egui::Id::unique((FILTER, "last-good"))))
        .filter(|(revision, _)| *revision == engine.revision())
        .and_then(|(_, matches)| matches)
}

/// Fold state, persisted in egui memory per row key.
fn folded(ui: &egui::Ui, key: &str, default_open: bool) -> bool {
    ui.ctx()
        .data_mut(|d| d.get_persisted::<bool>(egui::Id::unique(("browser-open", key))))
        .unwrap_or(default_open)
}

fn set_open(ui: &egui::Ui, key: &str, open: bool) {
    ui.ctx()
        .data_mut(|d| d.insert_persisted(egui::Id::unique(("browser-open", key)), open));
}

/// A folder row of the project tree; returns whether it is open after
/// this frame. A right-click opens its menu (when it has rows).
#[allow(clippy::too_many_arguments)]
fn folder(
    ui: &mut egui::Ui,
    menu: &mut crate::context_menu::State<MenuTarget>,
    key: &'static str,
    depth: usize,
    label: &str,
    count: &str,
    branch: bool,
    default_open: bool,
) -> bool {
    let (open, at) = folder_row(ui, key, depth, label, count, branch, default_open);
    if let Some(at) = at
        && !menu_rows(ui.ctx(), MenuTarget::Folder(key)).is_empty()
    {
        menu.open(at, MenuTarget::Folder(key));
    }
    open
}

/// The Nets folder's count: every net, and how many of them reach one pin
/// or none ("22 · 8 single-pin"), so it agrees with the sheet card's
/// "Connected nets" (the rest).
fn nets_count(nets: &[pcbcraft_core::Net]) -> String {
    let single = nets.iter().filter(|n| n.terminals.len() < 2).count();
    if single == 0 {
        nets.len().to_string()
    } else {
        format!("{} · {single} single-pin", nets.len())
    }
}

/// A folder row with its secondary text; returns whether it is open after
/// this frame.
pub(super) fn folder_labelled(
    ui: &mut egui::Ui,
    key: &str,
    depth: usize,
    label: &str,
    count: &str,
    branch: bool,
    default_open: bool,
) -> bool {
    folder_row(ui, key, depth, label, count, branch, default_open).0
}

/// A folder row: whether it is open after this frame, and where a
/// right-click asked for its menu.
fn folder_row(
    ui: &mut egui::Ui,
    key: &str,
    depth: usize,
    label: &str,
    count: &str,
    branch: bool,
    default_open: bool,
) -> (bool, Option<egui::Pos2>) {
    let open = folded(ui, key, default_open);
    let response = kit::tree_row(
        ui,
        egui::Id::unique(("browser", key)),
        &Row {
            depth,
            fold: Some(open),
            icon: "folder",
            label,
            secondary: count,
            branch,
            ..Default::default()
        },
    );
    if response.fold || response.clicked {
        set_open(ui, key, !open);
    }
    (folded(ui, key, default_open), response.menu)
}

/// The Browser's one-line summary for its title band.
pub fn summary(engine: &Engine) -> String {
    let project = engine.project();
    let count = project.instances.len();
    format!("{count} instance{}", if count == 1 { "" } else { "s" })
}

/// Whether an instance pins an older revision than the newest one it
/// could move to: a newer revision of its lineage in the project, or one a
/// library source offers. The text names that revision and where it is.
pub fn stale_text(
    library: &pcbcraft_core::library::Library,
    component: pcbcraft_core::ComponentRef,
    updates: &[UpdateAvailable],
) -> Option<String> {
    let in_project = library
        .latest(component.id)
        .map(|(_, r)| r)
        .filter(|r| *r > component.revision);
    let offered = updates
        .iter()
        .find(|u| u.local.id == component.id && u.latest > u.origin.revision);
    match (in_project, offered) {
        (Some(r), _) => Some(format!("{r} available")),
        (None, Some(u)) => Some(format!("{} in {}", u.latest, u.source)),
        (None, None) => None,
    }
}

/// The issue glyph and hover text of a row with `badge`, after `tip`.
fn issue_mark(badge: Option<&Badge>, tip: &str) -> (Option<kit::severity::Level>, String) {
    match badge {
        Some(badge) if tip.is_empty() => (Some(badge.level), badge.tip()),
        Some(badge) => (Some(badge.level), format!("{tip}\n{}", badge.tip())),
        None => (None, tip.to_owned()),
    }
}

/// A click on a component (or one of its Parts) selects it; a
/// double-click opens it for editing.
fn component_pick(response: &kit::RowResponse, id: DefinitionId) -> Option<Pick> {
    if response.double {
        Some(Pick::EditComponent(id))
    } else {
        response.clicked.then_some(Pick::Component(id))
    }
}

pub fn show(
    ui: &mut egui::Ui,
    engine: &Engine,
    lit: Lit,
    updates: &[UpdateAvailable],
    badges: &Badges,
    menu: &mut crate::context_menu::State<MenuTarget>,
) -> Option<Pick> {
    let pick = tree(ui, engine, lit, updates, badges, menu);
    // The open row menu; a row gone from the project closes it.
    let rows = match menu.target() {
        Some(MenuTarget::Component(id)) if !components(engine).iter().any(|c| c.id == *id) => {
            Vec::new()
        }
        Some(MenuTarget::Instance(id, _)) if engine.project().instance(*id).is_none() => Vec::new(),
        Some(target) => menu_rows(ui.ctx(), *target),
        None => Vec::new(),
    };
    match menu.show(ui.ctx(), &rows) {
        Some(crate::context_menu::Run::Act(chosen)) => Some(chosen),
        _ => pick,
    }
}

/// The tree's rows; a right-click on a component or instance row opens
/// its menu.
fn tree(
    ui: &mut egui::Ui,
    engine: &Engine,
    lit: Lit,
    updates: &[UpdateAvailable],
    badges: &Badges,
    menu: &mut crate::context_menu::State<MenuTarget>,
) -> Option<Pick> {
    let project = engine.project();
    let mut pick = None;
    // A2: the Component workspace's catalog replaces the
    // tree once it draws.
    if super::browser_catalog::Catalog::default().show(ui, engine) {
        return pick;
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    if !folder(ui, menu, "project", 0, &project.name, "", false, true) {
        return pick;
    }

    let all = components(engine);
    let mut filter = ui
        .ctx()
        .data(|d| d.get_temp::<String>(egui::Id::unique(FILTER)))
        .unwrap_or_default();
    let matched = filtered(ui.ctx(), engine, &filter);
    let shown_matches = match &matched {
        Ok(matches) => matches.clone(),
        Err(_) => last_good(ui.ctx(), engine),
    };
    let by_id: BTreeMap<DefinitionId, &ComponentRow> = all.iter().map(|c| (c.id, c)).collect();
    let components: Vec<&ComponentRow> = match &shown_matches {
        Some(matches) => matches
            .order
            .iter()
            .filter_map(|id| by_id.get(id).copied())
            .collect(),
        None => all.iter().collect(),
    };
    // While filtering, the count reads "shown of all" so an active filter
    // is visible from every workspace.
    let count = if shown_matches.is_some() {
        format!("{} of {}", components.len(), all.len())
    } else {
        all.len().to_string()
    };
    let (open, at) = folder_row(
        ui,
        "components",
        1,
        "Components",
        &count,
        lit.component.is_some(),
        true,
    );
    if let Some(at) = at {
        menu.open(at, MenuTarget::Folder("components"));
    }
    if open {
        ui.add_space(2.0);
        let field = kit::filter_field(
            ui,
            egui::Id::unique((FILTER, "field")),
            &mut filter,
            "Filter: name, 10k, res>=1k, pkg:0603",
            matched.is_err(),
            MAX_QUERY,
        );
        if field.changed {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::unique(FILTER), filter.clone()));
        }
        ui.add_space(2.0);
        match &matched {
            Err(error) => {
                kit::search_message(ui, error, true);
            }
            Ok(Some(matches)) if matches.order.is_empty() => {
                kit::search_message(ui, "No components match.", false);
            }
            Ok(Some(matches)) if matches.total > matches.order.len() => {
                kit::search_message(
                    ui,
                    &format!(
                        "Showing the best {} of {} matches; refine the filter.",
                        matches.order.len(),
                        matches.total
                    ),
                    false,
                );
            }
            _ => {}
        }
        for component in components {
            let update = updates
                .iter()
                .find(|u| u.local.id == component.id && u.latest > u.origin.revision);
            let secondary = match (component.revision, update) {
                (Some(r), Some(u)) => format!("r{r} · {} available", u.latest),
                (Some(r), None) => format!("r{r}"),
                (None, _) => "draft".into(),
            };
            let update_tip = update.map(|u| {
                format!(
                    "{} holds {}; this project has {}",
                    u.source, u.latest, u.local.revision
                )
            });
            let (issue, tip) = issue_mark(
                badges.components.get(&component.id),
                match &update_tip {
                    Some(tip) => tip,
                    None if component.draft => "Unpublished draft changes",
                    None => "Published revision",
                },
            );
            let response = kit::tree_row(
                ui,
                egui::Id::unique(("browser-component", component.id.0)),
                &Row {
                    depth: 2,
                    icon: if update.is_some() {
                        "update"
                    } else {
                        "component"
                    },
                    label: &component.name,
                    secondary: &secondary,
                    selected: lit.component == Some(component.id),
                    modified: component.draft,
                    issue,
                    tip: &tip,
                    ..Default::default()
                },
            );
            if let Some(row) = component_pick(&response, component.id) {
                pick = Some(row);
            }
            if let Some(at) = response.menu {
                menu.open(at, MenuTarget::Component(component.id));
            }
            // The component's manufacturer Parts, opened in its Parts editor.
            for part in project.library.latest_parts() {
                if part.definition.component.id != component.id {
                    continue;
                }
                let label = format!("{} {}", part.definition.manufacturer, part.definition.mpn);
                let secondary = part.revision.to_string();
                let response = kit::tree_row(
                    ui,
                    egui::Id::unique(("browser-part", part.id.0)),
                    &Row {
                        depth: 3,
                        icon: "library",
                        label: &label,
                        secondary: &secondary,
                        tip: "Manufacturer part",
                        ..Default::default()
                    },
                );
                if let Some(row) = component_pick(&response, component.id) {
                    pick = Some(row);
                }
            }
        }
    }

    let schematic_lit = lit.instance.is_some() || lit.net.is_some();
    if folder(
        ui,
        menu,
        "schematic",
        1,
        "Schematic",
        "",
        schematic_lit,
        true,
    ) {
        if let Some(sheet) = sheets(ui, engine, lit, menu) {
            pick = Some(Pick::Sheet(sheet));
        }
        if folder(
            ui,
            menu,
            "instances",
            2,
            "Instances",
            &project.instances.len().to_string(),
            lit.instance.is_some(),
            true,
        ) {
            for instance in &project.instances {
                let name = project
                    .library
                    .component(instance.component)
                    .map_or_else(String::new, |c| c.meta.name.clone());
                let stale = stale_text(&project.library, instance.component, updates);
                let stale_tip = stale.as_ref().map(|text| {
                    format!(
                        "Pinned to {}; {text}. Select it to review the update.",
                        instance.component.revision
                    )
                });
                let secondary = match &stale {
                    Some(text) => format!("{name} · {text}"),
                    None => name,
                };
                let (issue, tip) = issue_mark(
                    badges.instances.get(&instance.id),
                    stale_tip.as_deref().unwrap_or(""),
                );
                let response = kit::tree_row(
                    ui,
                    egui::Id::unique(("browser-instance", instance.id)),
                    &Row {
                        depth: 3,
                        icon: if stale.is_some() {
                            "update"
                        } else {
                            "component"
                        },
                        label: &instance.designator,
                        secondary: &secondary,
                        selected: lit.instance == Some(instance.id),
                        issue,
                        tip: &tip,
                        ..Default::default()
                    },
                );
                if response.clicked {
                    pick = Some(Pick::Instance(instance.id));
                }
                if let Some(at) = response.menu {
                    menu.open(at, MenuTarget::Instance(instance.id, instance.component.id));
                }
            }
        }
        if folder(
            ui,
            menu,
            "nets",
            2,
            "Nets",
            &nets_count(&project.nets),
            lit.net.is_some(),
            false,
        ) {
            // Connected nets first, single-pin nets last, each in file order.
            let mut nets: Vec<_> = project.nets.iter().collect();
            nets.sort_by_key(|n| n.terminals.len() < 2);
            for net in nets {
                let members = match net.terminals.len() {
                    1 => "1 pin".to_owned(),
                    n => format!("{n} pins"),
                };
                let (issue, tip) = issue_mark(badges.nets.get(&net.id), "");
                let response = kit::tree_row(
                    ui,
                    egui::Id::unique(("browser-net", net.id)),
                    &Row {
                        depth: 3,
                        icon: "line",
                        label: &net.name,
                        secondary: &members,
                        selected: lit.net == Some(net.id),
                        issue,
                        tip: &tip,
                        ..Default::default()
                    },
                );
                if response.clicked {
                    pick = Some(Pick::Net(net.id));
                }
            }
        }
    }

    let pcb_lit = lit.board || lit.track.is_some() || lit.placement.is_some();
    if folder(ui, menu, "pcb", 1, "PCB", "", pcb_lit, true) {
        let size = format!(
            "{} × {} mm",
            kit::number_text(project.board_size[0]),
            kit::number_text(project.board_size[1])
        );
        let (issue, tip) = issue_mark(badges.board.as_ref(), "");
        if kit::tree_row(
            ui,
            egui::Id::unique("browser-board"),
            &Row {
                depth: 2,
                icon: "plane",
                label: "Board",
                secondary: &size,
                selected: lit.board,
                issue,
                tip: &tip,
                ..Default::default()
            },
        )
        .clicked
        {
            pick = Some(Pick::Board);
        }
        if folder(
            ui,
            menu,
            "placements",
            2,
            "Placements",
            &project.placements.len().to_string(),
            lit.placement.is_some(),
            lit.placement.is_some(),
        ) {
            for placement in &project.placements {
                let label = project
                    .instance(placement.instance)
                    .map_or("Component", |i| i.designator.as_str());
                let side = match placement.layer {
                    pcbcraft_core::Layer::Top => "Top",
                    pcbcraft_core::Layer::Bottom => "Bottom",
                };
                let (issue, tip) = issue_mark(
                    badges.placements.get(&placement.instance),
                    if placement.locked { "Locked" } else { "" },
                );
                if kit::tree_row(
                    ui,
                    egui::Id::unique(("browser-placement", placement.instance)),
                    &Row {
                        depth: 3,
                        icon: "component",
                        label,
                        secondary: side,
                        selected: lit.placement == Some(placement.instance),
                        dim: placement.locked,
                        issue,
                        tip: &tip,
                        ..Default::default()
                    },
                )
                .clicked
                {
                    pick = Some(Pick::Placement(placement.instance));
                }
            }
        }
        if folder(
            ui,
            menu,
            "tracks",
            2,
            "Tracks",
            &project.tracks.len().to_string(),
            lit.track.is_some(),
            lit.track.is_some(),
        ) {
            for track in &project.tracks {
                let label = format!("Track {}", track.id);
                let (issue, tip) = issue_mark(badges.tracks.get(&track.id), "");
                if kit::tree_row(
                    ui,
                    egui::Id::unique(("browser-track", track.id)),
                    &Row {
                        depth: 3,
                        icon: "line",
                        label: &label,
                        secondary: project.net_name(track.net),
                        selected: lit.track == Some(track.id),
                        issue,
                        tip: &tip,
                        ..Default::default()
                    },
                )
                .clicked
                {
                    pick = Some(Pick::Track(track.id));
                }
            }
        }
    }
    pick
}

/// A library-sources row the user picked: both open the sources card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcePick {
    Source(usize),
    Manage,
}

/// The Library sources branch under the project (S3): every configured
/// folder, a disabled one dimmed with its reason instead of left out, and
/// a row to manage them.
pub fn show_sources(
    ui: &mut egui::Ui,
    sources: &[crate::library_sources::SourceView],
) -> Option<SourcePick> {
    let mut pick = None;
    // The catalog lists the sources with their items in the Component
    // workspace.
    if super::browser_catalog::active(ui.ctx()) {
        return pick;
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    let ready = sources.iter().filter(|s| s.ready()).count();
    let count = if ready == sources.len() {
        sources.len().to_string()
    } else {
        format!("{ready} of {}", sources.len())
    };
    if !folder_labelled(
        ui,
        "library-sources",
        0,
        "Library sources",
        &count,
        false,
        true,
    ) {
        return pick;
    }
    for (at, source) in sources.iter().enumerate() {
        let secondary = source.short();
        let reason = source.reason();
        let tip = match &reason {
            Some(why) => format!("{} · disabled: {why}", source.setting.path.display()),
            None => source.setting.path.display().to_string(),
        };
        let response = kit::tree_row(
            ui,
            egui::Id::unique(("browser-source", at)),
            &Row {
                depth: 1,
                icon: if reason.is_some() {
                    "status_warning"
                } else {
                    "library"
                },
                label: &source.name,
                secondary: &secondary,
                dim: reason.is_some(),
                tip: &tip,
                ..Default::default()
            },
        );
        if response.clicked {
            pick = Some(SourcePick::Source(at));
        }
    }
    if kit::tree_row(
        ui,
        egui::Id::unique("browser-sources-manage"),
        &Row {
            depth: 1,
            icon: "folder",
            label: "Manage sources…",
            dim: true,
            tip: "Add a library folder, turn sources on or off, rescan",
            ..Default::default()
        },
    )
    .clicked
    {
        pick = Some(SourcePick::Manage);
    }
    pick
}

/// The Sheets folder: every sheet in page order, nested under the sheet
/// whose sheet symbol uses it (`schematic.sheets`). Returns the picked one.
fn sheets(
    ui: &mut egui::Ui,
    engine: &Engine,
    lit: Lit,
    menu: &mut crate::context_menu::State<MenuTarget>,
) -> Option<SheetId> {
    let Ok(QueryResult::Sheets(rows)) = engine.query(Query::Sheets {}) else {
        return None;
    };
    if !folder(
        ui,
        menu,
        "sheets",
        2,
        "Sheets",
        &rows.len().to_string(),
        false,
        true,
    ) {
        return None;
    }
    let mut pick = None;
    for row in hierarchy(&rows) {
        let secondary = format!("page {}", row.page);
        let response = kit::tree_row(
            ui,
            egui::Id::unique(("browser-sheet", row.id.0)),
            &Row {
                depth: 3 + row.depth.min(8),
                icon: "sketch",
                label: &row.name,
                secondary: &secondary,
                selected: lit.sheet == Some(row.id),
                tip: "Show this sheet",
                ..Default::default()
            },
        );
        if response.clicked {
            pick = Some(row.id);
        }
    }
    pick
}

/// Sheets depth-first under each top-level sheet, siblings in page order.
fn hierarchy(rows: &[SheetRow]) -> Vec<&SheetRow> {
    let mut out: Vec<&SheetRow> = Vec::with_capacity(rows.len());
    let mut stack: Vec<&SheetRow> = rows.iter().filter(|r| r.parent.is_none()).rev().collect();
    while let Some(row) = stack.pop() {
        if out.iter().any(|r| r.id == row.id) {
            continue;
        }
        out.push(row);
        stack.extend(
            rows.iter()
                .filter(|r| r.parent.as_ref().map(|p| p.sheet) == Some(row.id))
                .rev(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn frame(ctx: &egui::Context, engine: &Engine, events: Vec<egui::Event>) -> Vec<String> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300.0, 1600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                kit::form(ui, |ui| {
                    show(
                        ui,
                        engine,
                        Lit::default(),
                        &[],
                        &Badges::default(),
                        &mut Default::default(),
                    )
                });
            },
        );
        output.textures_delta.clear();
        labels(&output)
    }

    #[test]
    fn the_components_filter_keeps_the_engines_matches_in_rank_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::from_file(pcbcraft_core::format::EditorFile {
            version: pcbcraft_core::format::FILE_VERSION,
            project: pcbcraft_core::catalog::synthetic::project(24)?,
            drafts: Default::default(),
        })?;
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let all = frame(&ctx, &engine, vec![]);
        assert_eq!(all.iter().filter(|t| t.starts_with("EX-")).count(), 24);

        let set = |text: &str| {
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique(FILTER), text.to_owned()));
        };
        set("res>=10k pkg:0603");
        let shown = frame(&ctx, &engine, vec![]);
        let rows: Vec<&String> = shown.iter().filter(|t| t.starts_with("EX-")).collect();
        assert_eq!(
            rows,
            ["EX-R0603-10K0-F", "EX-R0603-1M00-J", "EX-R0603-22K0-D"]
        );
        assert!(shown.iter().any(|t| t == "3 of 24"), "{shown:?}");
        // A text that does not parse shows its error and keeps the rows of
        // the last one that did; the count matches what is listed.
        set("V>=10k");
        let shown = frame(&ctx, &engine, vec![]);
        assert!(
            shown
                .iter()
                .any(|t| t == "V>=10k: 10 k has no unit of voltage")
        );
        assert_eq!(shown.iter().filter(|t| t.starts_with("EX-")).count(), 3);
        assert!(shown.iter().any(|t| t == "3 of 24"), "{shown:?}");
        set("nothing-like-this");
        let shown = frame(&ctx, &engine, vec![]);
        assert!(shown.iter().any(|t| t == "No components match."));
        assert_eq!(shown.iter().filter(|t| t.starts_with("EX-")).count(), 0);
        assert!(shown.iter().any(|t| t == "0 of 24"), "{shown:?}");
        Ok(())
    }

    /// Every configured source is listed; a disabled one keeps its row,
    /// dimmed with the reason, and Manage sources… is always offered.
    #[test]
    fn library_sources_are_listed_with_their_disabled_reason() {
        use crate::library_sources::{SourceView, Status};
        use pcbcraft_io::library_folder::Disabled;
        use pcbcraft_io::settings::SourceSetting;
        let view = |name: &str, status| SourceView {
            setting: SourceSetting {
                path: format!("/libs/{name}").into(),
                enabled: true,
                writable: false,
                name: None,
            },
            name: name.into(),
            status,
        };
        let sources = vec![
            view("Team parts", Status::Loading),
            view("Old", Status::Disabled(Disabled::UnsupportedFormat(7))),
            view("Gone", Status::Disabled(Disabled::PathMissing)),
            view(
                "Vendor",
                Status::Disabled(Disabled::UnknownFeatures(vec!["parametric-3d".into()])),
            ),
        ];
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let mut pick = None;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            kit::form(ui, |ui| pick = show_sources(ui, &sources));
        });
        output.textures_delta.clear();
        let shown = labels(&output);
        for text in [
            "Library sources",
            "0 of 4",
            "Team parts",
            "loading…",
            "Old",
            "unsupported format v7",
            "Vendor",
            "unknown feature",
            "Gone",
            "path missing",
            "Manage sources…",
        ] {
            assert!(shown.iter().any(|t| t == text), "{text}: {shown:?}");
        }
        assert_eq!(pick, None);
    }

    #[test]
    fn esc_and_the_clear_button_empty_the_filter() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::from_file(pcbcraft_core::format::EditorFile {
            version: pcbcraft_core::format::FILE_VERSION,
            project: pcbcraft_core::catalog::synthetic::project(24)?,
            drafts: Default::default(),
        })?;
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let text = || ctx.data(|d| d.get_temp::<String>(egui::Id::unique(FILTER)));
        let field = egui::Id::unique((FILTER, "field"));
        frame(&ctx, &engine, vec![]);
        ctx.memory_mut(|m| m.request_focus(field));
        frame(&ctx, &engine, vec![egui::Event::Text("hdr".into())]);
        assert_eq!(text().as_deref(), Some("hdr"));
        let shown = frame(&ctx, &engine, vec![]);
        assert!(shown.iter().any(|t| t == "8 of 24"), "{shown:?}");
        let esc = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &engine, vec![esc]);
        assert_eq!(text().as_deref(), Some(""));
        let shown = frame(&ctx, &engine, vec![]);
        assert!(shown.iter().any(|t| t == "24"), "{shown:?}");
        Ok(())
    }

    /// A right-click on a component or instance row opens its menu and
    /// selects nothing; a row picks what the row's double-click picks, or
    /// locates.
    #[test]
    fn a_rows_right_click_menu_picks() -> Result<(), Box<dyn std::error::Error>> {
        use crate::context_menu::{Run, State};
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let r1 = engine.project().instances[0].clone();
        let mut menu = State::default();
        let run = |events: Vec<egui::Event>, menu: &mut State<MenuTarget>| {
            let mut pick = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 1200.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    pick = kit::form(ui, |ui| {
                        show(ui, &engine, Lit::default(), &[], &Badges::default(), menu)
                    });
                },
            );
            output.textures_delta.clear();
            (crate::kit::text::tests::painted(&output), pick)
        };
        let (painted, _) = run(vec![], &mut menu);
        let row = painted
            .iter()
            .find(|(t, ..)| *t == r1.designator)
            .map(|(_, rect, ..)| rect.center())
            .ok_or("the instance row")?;
        let button = |at, which, pressed| egui::Event::PointerButton {
            pos: at,
            button: which,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        use egui::PointerButton::{Primary, Secondary};
        run(vec![egui::Event::PointerMoved(row)], &mut menu);
        assert_eq!(run(vec![button(row, Secondary, true)], &mut menu).1, None);
        assert_eq!(run(vec![button(row, Secondary, false)], &mut menu).1, None);
        assert_eq!(
            menu.target(),
            Some(&MenuTarget::Instance(r1.id, r1.component.id))
        );
        let (painted, _) = run(vec![], &mut menu);
        let locate = painted
            .iter()
            .find(|(t, ..)| t == "Locate")
            .map(|(_, rect, ..)| rect.center())
            .ok_or("the Locate row")?;
        run(vec![egui::Event::PointerMoved(locate)], &mut menu);
        run(vec![button(locate, Primary, true)], &mut menu);
        assert_eq!(
            run(vec![button(locate, Primary, false)], &mut menu).1,
            Some(Pick::Locate(r1.id))
        );
        assert!(!menu.is_open());
        // A component's row edits it.
        let edit = menu_rows(&ctx, MenuTarget::Component(r1.component.id));
        let edit = edit.into_iter().flatten().next().ok_or("Edit component")?;
        assert_eq!(edit.label, "Edit component");
        assert_eq!(edit.run, Run::Act(Pick::EditComponent(r1.component.id)));
        // Run 3, finding 21: an instance row offers what its part's menu on
        // the sheet does: Edit component and Delete….
        let rows: Vec<_> = menu_rows(&ctx, MenuTarget::Instance(r1.id, r1.component.id))
            .into_iter()
            .flatten()
            .map(|e| (e.label, e.run))
            .collect();
        assert!(rows.contains(&(
            "Edit component".into(),
            Run::Act(Pick::EditComponent(r1.component.id))
        )));
        assert!(rows.contains(&("Delete…".into(), Run::Act(Pick::Remove(r1.id)))));
        // A folder's right-click opens a menu of the commands that add to it.
        let (painted, _) = run(vec![], &mut menu);
        let instances = painted
            .iter()
            .find(|(t, ..)| t == "Instances")
            .map(|(_, rect, ..)| rect.center())
            .ok_or("the Instances folder")?;
        run(vec![egui::Event::PointerMoved(instances)], &mut menu);
        run(vec![button(instances, Secondary, true)], &mut menu);
        run(vec![button(instances, Secondary, false)], &mut menu);
        assert_eq!(menu.target(), Some(&MenuTarget::Folder("instances")));
        let (painted, _) = run(vec![], &mut menu);
        let place = painted
            .iter()
            .find(|(t, ..)| t == "Place part")
            .map(|(_, rect, ..)| rect.center())
            .ok_or("the Place part row")?;
        run(vec![egui::Event::PointerMoved(place)], &mut menu);
        run(vec![button(place, Primary, true)], &mut menu);
        assert_eq!(
            run(vec![button(place, Primary, false)], &mut menu).1,
            Some(Pick::Command("schematic.place"))
        );
        // Every folder with a menu names registry commands that exist.
        let registry = crate::commands::Registry::standard();
        for key in [
            "components",
            "schematic",
            "sheets",
            "instances",
            "nets",
            "pcb",
            "placements",
            "tracks",
        ] {
            let rows = menu_rows(&ctx, MenuTarget::Folder(key));
            assert!(!rows.is_empty(), "{key}");
            for entry in rows.into_iter().flatten() {
                let Run::Act(Pick::Command(id)) = entry.run else {
                    return Err(format!("{key}: {entry:?}").into());
                };
                assert!(registry.get(id).is_some(), "{key}: {id}");
            }
        }
        Ok(())
    }

    /// Precision run finding 3, as in Solidcraft's Browser: a click on a
    /// component row selects it; only a double-click opens it for editing.
    /// The Nets folder counts every net and says how many reach one pin,
    /// and lists those last, so its count agrees with the sheet card's
    /// "Connected nets" (run 3, finding 20).
    #[test]
    fn the_nets_folder_counts_single_pin_nets_and_lists_them_last()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let first = engine.project().instances[0].clone();
        // An unwired resistor: each of its pins is a net of its own.
        engine.apply(
            engine.revision(),
            pcbcraft_core::Operation::Instance(pcbcraft_core::InstanceCommand::Place(
                pcbcraft_core::instances::NewInstance {
                    sheet: None,
                    component: first.component,
                    designator: Some("R3".into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: pcbcraft_core::UnitId(1),
                    position: crate::schematic_editor::pt([30.0, 0.0]),
                    physical: first.physical,
                },
            )),
        )?;
        let nets = &engine.project().nets;
        let single: Vec<&str> = nets
            .iter()
            .filter(|n| n.terminals.len() < 2)
            .map(|n| n.name.as_str())
            .collect();
        let connected: Vec<&str> = nets
            .iter()
            .filter(|n| n.terminals.len() >= 2)
            .map(|n| n.name.as_str())
            .collect();
        assert!(!single.is_empty() && !connected.is_empty(), "{nets:?}");
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        ctx.data_mut(|d| d.insert_persisted(egui::Id::unique(("browser-open", "nets")), true));
        let shown = frame(&ctx, &engine, vec![]);
        let count = format!("{} · {} single-pin", nets.len(), single.len());
        assert!(shown.contains(&count), "{count}: {shown:?}");
        let at = shown.iter().position(|t| t == "Nets").ok_or("no Nets")?;
        let listed: Vec<&str> = shown
            .iter()
            .skip(at)
            .map(String::as_str)
            .filter(|t| single.contains(t) || connected.contains(t))
            .take(nets.len())
            .collect();
        let expected: Vec<&str> = connected.iter().chain(&single).copied().collect();
        assert_eq!(listed, expected);
        Ok(())
    }

    #[test]
    fn a_click_selects_a_component_and_a_double_click_edits_it() {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let id = DefinitionId(7);
        let at = egui::pos2(60.0, 12.0);
        let mut picks = Vec::new();
        let mut time = 0.0;
        let mut row = |events: Vec<egui::Event>, picks: &mut Vec<Option<Pick>>| {
            time += 0.05;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 200.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let response = kit::tree_row(
                        ui,
                        egui::Id::unique("component"),
                        &Row {
                            label: "Resistor",
                            ..Default::default()
                        },
                    );
                    picks.push(component_pick(&response, id));
                },
            );
            output.textures_delta.clear();
        };
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        row(vec![egui::Event::PointerMoved(at)], &mut picks);
        row(vec![button(true)], &mut picks);
        row(vec![button(false)], &mut picks);
        row(vec![button(true)], &mut picks);
        row(vec![button(false)], &mut picks);
        let picked: Vec<Pick> = picks.into_iter().flatten().collect();
        assert_eq!(
            picked,
            vec![Pick::Component(id), Pick::EditComponent(id)],
            "click selects, the second click of a double-click edits"
        );
    }
}
