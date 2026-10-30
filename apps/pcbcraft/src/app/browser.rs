//! The project Browser: one tree over the whole design. Rows come from the
//! project and drafts; clicking a row opens its workspace and selects it
//! there. The tree reads state only; the app applies the returned
//! [`Pick`] through the editors.
use crate::kit::{self, Row};
use crate::shell::Workspace;
use pcbcraft_core::library::Definition;
use pcbcraft_core::{DefinitionId, Engine, Id};
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
    icon: &str,
    label: &str,
    count: Option<usize>,
    branch: bool,
    default_open: bool,
    pick: &mut Option<Pick>,
    on_click: Option<Pick>,
) -> bool {
    let open = folded(ui, key, default_open);
    let count = count.map(|c| c.to_string()).unwrap_or_default();
    let response = kit::tree_row(
        ui,
        egui::Id::new(("browser", key)),
        &Row {
            depth,
            fold: Some(open),
            icon,
            label,
            secondary: &count,
            branch,
            ..Default::default()
        },
    );
    if response.fold || (response.clicked && on_click.is_none()) {
        set_open(ui, key, !open);
    } else if response.clicked {
        *pick = on_click;
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
    if !folder(
        ui,
        "project",
        0,
        "folder",
        &project.name,
        None,
        false,
        true,
        &mut pick,
        None,
    ) {
        return pick;
    }

    let components = components(engine);
    if folder(
        ui,
        "components",
        1,
        "folder",
        "Components",
        Some(components.len()),
        lit.component.is_some(),
        true,
        &mut pick,
        None,
    ) {
        for component in &components {
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
    if folder(
        ui,
        "schematic",
        1,
        "folder",
        "Schematic",
        None,
        schematic_lit,
        true,
        &mut pick,
        None,
    ) {
        if folder(
            ui,
            "instances",
            2,
            "folder",
            "Instances",
            Some(project.instances.len()),
            lit.instance.is_some(),
            true,
            &mut pick,
            None,
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
            "folder",
            "Nets",
            Some(project.nets.len()),
            lit.net.is_some(),
            false,
            &mut pick,
            None,
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
    if folder(
        ui, "pcb", 1, "folder", "PCB", None, pcb_lit, true, &mut pick, None,
    ) {
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
            "folder",
            "Placements",
            Some(project.placements.len()),
            lit.placement.is_some(),
            lit.placement.is_some(),
            &mut pick,
            None,
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
            "folder",
            "Tracks",
            Some(project.tracks.len()),
            lit.track.is_some(),
            lit.track.is_some(),
            &mut pick,
            None,
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
