//! The project Browser: one tree over the whole design. Rows come from the
//! project and drafts; clicking a row opens its workspace and selects it
//! there. The tree reads state only; the app applies the returned
//! [`Pick`] through the editors.
use crate::kit::{self, Row};
use pcbcraft_core::library::Definition;
use pcbcraft_core::{DefinitionId, Engine, Id, Query, QueryResult};
use std::collections::BTreeSet;

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

/// One filter result per document revision and text.
#[derive(Clone)]
struct Filtered {
    revision: u64,
    text: String,
    result: Result<Option<Vec<DefinitionId>>, String>,
}

/// The lineages a filter text keeps, ranked by the engine's
/// `library.search` (drafts included); `None` for an empty filter.
fn filtered(
    ctx: &egui::Context,
    engine: &Engine,
    text: &str,
) -> Result<Option<Vec<DefinitionId>>, String> {
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
        limit: Some(pcbcraft_core::catalog::MAX_RESULTS),
        include_drafts: true,
    }) {
        Ok(QueryResult::Search(report)) => Ok(Some(
            report.results.iter().map(|h| h.entry.lineage).collect(),
        )),
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
        )
    });
    result
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
    let open = folded(ui, key, default_open);
    let count = count.map(|c| c.to_string()).unwrap_or_default();
    let response = kit::tree_row(
        ui,
        egui::Id::new(("browser", key)),
        &Row {
            depth,
            fold: Some(open),
            icon: "folder",
            label,
            secondary: &count,
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

pub fn show(ui: &mut egui::Ui, engine: &Engine, lit: Lit) -> Option<Pick> {
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
    let components: Vec<&ComponentRow> = match &matched {
        Ok(Some(order)) => order
            .iter()
            .filter_map(|id| all.iter().find(|c| c.id == *id))
            .collect(),
        _ => all.iter().collect(),
    };
    if folder(
        ui,
        "components",
        1,
        "Components",
        Some(components.len()),
        lit.component.is_some(),
        true,
    ) {
        ui.add_space(2.0);
        let response = kit::text_field(
            ui,
            FILTER,
            &mut filter,
            "Filter: name, 10k, R>=1k, pkg:0603",
            matched.is_err(),
        );
        if response.changed() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new(FILTER), filter.clone()));
        }
        ui.add_space(2.0);
        match &matched {
            Err(error) => {
                kit::status(ui, error, kit::Tone::Error);
            }
            Ok(Some(order)) if order.is_empty() => {
                kit::note(ui, "No components match.");
            }
            _ => {}
        }
        for component in components {
            let secondary = match component.revision {
                Some(r) => format!("r{r}"),
                None => "draft".into(),
            };
            let response = kit::tree_row(
                ui,
                egui::Id::new(("browser-component", component.id.0)),
                &Row {
                    depth: 2,
                    icon: "component",
                    label: &component.name,
                    secondary: &secondary,
                    selected: lit.component == Some(component.id),
                    modified: component.draft,
                    tip: if component.draft {
                        "Unpublished draft changes"
                    } else {
                        "Published revision"
                    },
                    ..Default::default()
                },
            );
            if response.clicked {
                pick = Some(Pick::Component(component.id));
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
                let response = kit::tree_row(
                    ui,
                    egui::Id::new(("browser-instance", instance.id)),
                    &Row {
                        depth: 3,
                        icon: "component",
                        label: &instance.designator,
                        secondary: &name,
                        selected: lit.instance == Some(instance.id),
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
                kit::form(ui, |ui| show(ui, engine, Lit::default()));
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
        set("R>=10k pkg:0603");
        let shown = frame(&ctx, &engine, vec![]);
        let rows: Vec<&String> = shown.iter().filter(|t| t.starts_with("EX-")).collect();
        assert_eq!(
            rows,
            ["EX-R0603-10K0-F", "EX-R0603-1M00-J", "EX-R0603-22K0-D"]
        );
        set("V>=10k");
        let shown = frame(&ctx, &engine, vec![]);
        assert!(
            shown
                .iter()
                .any(|t| t == "V>=10k: 10 k has no unit of voltage")
        );
        assert_eq!(shown.iter().filter(|t| t.starts_with("EX-")).count(), 24);
        set("nothing-like-this");
        let shown = frame(&ctx, &engine, vec![]);
        assert!(shown.iter().any(|t| t == "No components match."));
        assert_eq!(shown.iter().filter(|t| t.starts_with("EX-")).count(), 0);
        Ok(())
    }
}
