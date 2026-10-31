//! The project Browser: one tree over the whole design. Rows come from the
//! project and drafts; clicking a row opens its workspace and selects it
//! there. The tree reads state only; the app applies the returned
//! [`Pick`] through the editors.
use crate::kit::{self, Row};
use pcbcraft_core::catalog::{MAX_QUERY, MAX_RESULTS, UpdateAvailable};
use pcbcraft_core::library::Definition;
use pcbcraft_core::{DefinitionId, Engine, Id, Query, QueryResult};
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
}

/// A row the user picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Component(DefinitionId),
    Instance(Id),
    Net(Id),
    Placement(Id),
    Track(Id),
    Board,
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
    let key = egui::Id::new((FILTER, "result"));
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
                egui::Id::new((FILTER, "last-good")),
                (engine.revision(), matches.clone()),
            );
        }
    });
    result
}

/// The rows of the last filter text that parsed, at this revision.
fn last_good(ctx: &egui::Context, engine: &Engine) -> Option<Matches> {
    ctx.data(|d| d.get_temp::<(u64, Option<Matches>)>(egui::Id::new((FILTER, "last-good"))))
        .filter(|(revision, _)| *revision == engine.revision())
        .and_then(|(_, matches)| matches)
}

/// Fold state, persisted in egui memory per row key.
fn folded(ui: &egui::Ui, key: &str, default_open: bool) -> bool {
    ui.ctx()
        .data_mut(|d| d.get_persisted::<bool>(egui::Id::new(("browser-open", key))))
        .unwrap_or(default_open)
}

fn set_open(ui: &egui::Ui, key: &str, open: bool) {
    ui.ctx()
        .data_mut(|d| d.insert_persisted(egui::Id::new(("browser-open", key)), open));
}

/// A folder row; returns whether it is open after this frame.
fn folder(
    ui: &mut egui::Ui,
    key: &str,
    depth: usize,
    label: &str,
    count: Option<usize>,
    branch: bool,
    default_open: bool,
) -> bool {
    let count = count.map(|c| c.to_string()).unwrap_or_default();
    folder_labelled(ui, key, depth, label, &count, branch, default_open)
}

/// A folder row with its secondary text; returns whether it is open after
/// this frame.
fn folder_labelled(
    ui: &mut egui::Ui,
    key: &str,
    depth: usize,
    label: &str,
    count: &str,
    branch: bool,
    default_open: bool,
) -> bool {
    let open = folded(ui, key, default_open);
    let response = kit::tree_row(
        ui,
        egui::Id::new(("browser", key)),
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
    folded(ui, key, default_open)
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

pub fn show(
    ui: &mut egui::Ui,
    engine: &Engine,
    lit: Lit,
    updates: &[UpdateAvailable],
) -> Option<Pick> {
    let project = engine.project();
    let mut pick = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    if !folder(ui, "project", 0, &project.name, None, false, true) {
        return pick;
    }

    let all = components(engine);
    let mut filter = ui
        .ctx()
        .data(|d| d.get_temp::<String>(egui::Id::new(FILTER)))
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
    if folder_labelled(
        ui,
        "components",
        1,
        "Components",
        &count,
        lit.component.is_some(),
        true,
    ) {
        ui.add_space(2.0);
        let field = kit::filter_field(
            ui,
            egui::Id::new((FILTER, "field")),
            &mut filter,
            "Filter: name, 10k, res>=1k, pkg:0603",
            matched.is_err(),
            MAX_QUERY,
        );
        if field.changed {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new(FILTER), filter.clone()));
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
            let response = kit::tree_row(
                ui,
                egui::Id::new(("browser-component", component.id.0)),
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
                    tip: match &update_tip {
                        Some(tip) => tip,
                        None if component.draft => "Unpublished draft changes",
                        None => "Published revision",
                    },
                    ..Default::default()
                },
            );
            if response.clicked {
                pick = Some(Pick::Component(component.id));
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
                    egui::Id::new(("browser-part", part.id.0)),
                    &Row {
                        depth: 3,
                        icon: "library",
                        label: &label,
                        secondary: &secondary,
                        tip: "Manufacturer part",
                        ..Default::default()
                    },
                );
                if response.clicked {
                    pick = Some(Pick::Component(component.id));
                }
            }
        }
    }

    let schematic_lit = lit.instance.is_some() || lit.net.is_some();
    if folder(ui, "schematic", 1, "Schematic", None, schematic_lit, true) {
        if folder(
            ui,
            "instances",
            2,
            "Instances",
            Some(project.instances.len()),
            lit.instance.is_some(),
            true,
        ) {
            for instance in &project.instances {
                let name = project
                    .library
                    .component(instance.component)
                    .map_or_else(String::new, |c| c.meta.name.clone());
                // A chosen Part that disagrees with the instance is marked.
                let findings = pcbcraft_core::instances::part_findings(&project.library, instance);
                let stale = stale_text(&project.library, instance.component, updates);
                let stale_tip = stale.as_ref().map(|text| {
                    format!(
                        "Pinned to {}; {text}. Select it to review the update.",
                        instance.component.revision
                    )
                });
                let tip = findings
                    .first()
                    .map(|f| f.message.as_str())
                    .or(stale_tip.as_deref())
                    .unwrap_or("");
                let secondary = match (&stale, findings.is_empty()) {
                    (_, false) => format!("{name} · part mismatch"),
                    (Some(text), true) => format!("{name} · {text}"),
                    (None, true) => name,
                };
                let response = kit::tree_row(
                    ui,
                    egui::Id::new(("browser-instance", instance.id)),
                    &Row {
                        depth: 3,
                        icon: if !findings.is_empty() {
                            "status_warning"
                        } else if stale.is_some() {
                            "update"
                        } else {
                            "component"
                        },
                        label: &instance.designator,
                        secondary: &secondary,
                        selected: lit.instance == Some(instance.id),
                        tip,
                        ..Default::default()
                    },
                );
                if response.clicked {
                    pick = Some(Pick::Instance(instance.id));
                }
            }
        }
        if folder(
            ui,
            "nets",
            2,
            "Nets",
            Some(project.nets.len()),
            lit.net.is_some(),
            false,
        ) {
            for net in &project.nets {
                let members = format!("{} pins", net.terminals.len());
                let response = kit::tree_row(
                    ui,
                    egui::Id::new(("browser-net", net.id)),
                    &Row {
                        depth: 3,
                        icon: "line",
                        label: &net.name,
                        secondary: &members,
                        selected: lit.net == Some(net.id),
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
    if folder(ui, "pcb", 1, "PCB", None, pcb_lit, true) {
        let size = format!(
            "{} × {} mm",
            kit::number_text(project.board_size[0]),
            kit::number_text(project.board_size[1])
        );
        if kit::tree_row(
            ui,
            egui::Id::new("browser-board"),
            &Row {
                depth: 2,
                icon: "plane",
                label: "Board",
                secondary: &size,
                selected: lit.board,
                ..Default::default()
            },
        )
        .clicked
        {
            pick = Some(Pick::Board);
        }
        if folder(
            ui,
            "placements",
            2,
            "Placements",
            Some(project.placements.len()),
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
                if kit::tree_row(
                    ui,
                    egui::Id::new(("browser-placement", placement.instance)),
                    &Row {
                        depth: 3,
                        icon: "component",
                        label,
                        secondary: side,
                        selected: lit.placement == Some(placement.instance),
                        dim: placement.locked,
                        tip: if placement.locked { "Locked" } else { "" },
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
            "tracks",
            2,
            "Tracks",
            Some(project.tracks.len()),
            lit.track.is_some(),
            lit.track.is_some(),
        ) {
            for track in &project.tracks {
                let label = format!("Track {}", track.id);
                if kit::tree_row(
                    ui,
                    egui::Id::new(("browser-track", track.id)),
                    &Row {
                        depth: 3,
                        icon: "line",
                        label: &label,
                        secondary: project.net_name(track.net),
                        selected: lit.track == Some(track.id),
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
            egui::Id::new(("browser-source", at)),
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
        egui::Id::new("browser-sources-manage"),
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
                kit::form(ui, |ui| show(ui, engine, Lit::default(), &[]));
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
            ctx.data_mut(|d| d.insert_temp(egui::Id::new(FILTER), text.to_owned()));
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
        let text = || ctx.data(|d| d.get_temp::<String>(egui::Id::new(FILTER)));
        let field = egui::Id::new((FILTER, "field"));
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
}
