//! The Schematic editor's Place palette: a searchable sheet over the
//! project's component lineages. Every result list comes from the engine's
//! `library.search` query and every preview from `library.inspect`; the
//! palette only returns what the user chose. The editor turns that into the
//! ordinary `component.place` operation.
//!
//! In *assign mode* the same sheet chooses a manufacturer part for one or
//! more instances: it opens pre-filled with `kind:part` and the instances'
//! requirements, and Enter returns the Part, which the editor turns into
//! one `instance.assign_part`.
use crate::kit::{self, Tone};
use egui::{Rect, Ui, pos2};
use pcbcraft_core::catalog::{
    self, CatalogContext, CatalogEntry, Hit, ItemReport, MAX_QUERY, SearchReport,
};
use pcbcraft_core::{
    ComponentRef, DefinitionId, DefinitionKind, Engine, Id, PartRef, Query, QueryResult,
};
use solidcraft_ui_egui::theme::Tokens;

/// Rows one search asks for.
const ROWS: usize = 40;
/// Recently placed lineages remembered for ranking (this session only).
const RECENT: usize = 16;
/// Key names are spelled out: the bundled fonts have no ⏎ or ⇧ glyphs.
const KEYS: &str = "↑↓ move   Enter place   Shift+Enter place & keep open   click pick   Esc close";
/// The footer while nothing can be placed.
const KEYS_IDLE: &str = "↑↓ move   Esc close";
/// The footer of assign mode.
const KEYS_ASSIGN: &str = "↑↓ move   Enter assign   click assign   Esc close";
/// The footer of assign mode on a Part for a newer component revision.
const KEYS_UPDATE: &str = "↑↓ move   Enter review update & assign   Esc close";
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Esc: the palette closed without a choice.
    Close,
}

/// Results of one query text at one document revision.
struct Results {
    revision: u64,
    query: String,
    report: Result<SearchReport, String>,
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
    recent: Vec<DefinitionId>,
    /// Counts openings and query changes; it salts the result list so each
    /// starts scrolled to the top.
    generation: u64,
    /// Why the last Enter or click placed nothing, shown under the field.
    notice: Option<String>,
    mode: Mode,
    /// Assign mode: the component revisions of the target instances.
    targets: Vec<ComponentRef>,
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
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    #[cfg(test)]
    pub fn query(&self) -> &str {
        &self.query
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
    }

    /// Remember a placed or picked lineage for ranking.
    pub fn used(&mut self, lineage: DefinitionId) {
        self.recent.retain(|id| *id != lineage);
        self.recent.insert(0, lineage);
        self.recent.truncate(RECENT);
    }

    /// Run `library.search` when the text or the document changed.
    fn refresh(&mut self, engine: &Engine) {
        let fresh = self
            .results
            .as_ref()
            .is_some_and(|r| r.revision == engine.revision() && r.query == self.query);
        if fresh {
            return;
        }
        let context = CatalogContext {
            favourites: Default::default(),
            recent: self.recent.clone(),
        };
        let report = match engine.query_with(
            Query::Search {
                query: self.query.clone(),
                limit: Some(ROWS),
                include_drafts: false,
            },
            &context,
        ) {
            Ok(QueryResult::Search(report)) => Ok(report),
            Ok(_) => Err("unexpected report".into()),
            Err(error) => Err(error.to_string()),
        };
        // A new text starts at the best match; a document change keeps the
        // row as far as it still exists.
        if self.results.as_ref().is_none_or(|r| r.query != self.query) {
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
                    report
                        .results
                        .retain(|h| h.entry.implements.is_some_and(|c| lineages.contains(&c.id)));
                    report.total = report.results.len();
                }
                // Components first: Parts are assigned, not placed.
                Mode::Place => report
                    .results
                    .sort_by_key(|h| h.entry.kind == DefinitionKind::Part),
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
        self.results = Some(Results {
            revision: engine.revision(),
            query: self.query.clone(),
            report,
        });
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

    /// The highlighted result's exact revision.
    pub fn highlighted(&self) -> Option<ComponentRef> {
        self.hits()
            .get(self.highlighted)
            .filter(|h| h.entry.kind == DefinitionKind::Component)
            .and_then(|h| h.entry.component())
    }

    /// The highlighted result's exact Part revision.
    pub fn highlighted_part(&self) -> Option<PartRef> {
        self.hits()
            .get(self.highlighted)
            .and_then(|h| part_of(&h.entry))
    }

    /// Assign mode: the highlighted Part for the instances, or a notice.
    fn choose_part(&mut self) -> Option<PaletteAction> {
        let Mode::Assign { instances } = &self.mode else {
            return None;
        };
        let instances = instances.clone();
        let hit = self.hits().get(self.highlighted)?;
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
        let hit = self.hits().get(self.highlighted)?;
        let name = hit.entry.name.clone();
        if hit.entry.kind == DefinitionKind::Part {
            self.notice = Some(PART_NOT_PLACED.into());
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
        if !self.open {
            return None;
        }
        let ctx = ui.ctx().clone();
        let keys = kit::search_keys(&ctx);
        if keys.escape {
            self.close();
            return Some(PaletteAction::Close);
        }
        self.refresh(engine);
        let count = self.hits().len();
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
        let assigning = matches!(self.mode, Mode::Assign { .. });
        if keys.enter && assigning {
            action = self.choose_part();
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
        let highlighted_entry = self.hits().get(self.highlighted).map(|h| &h.entry);
        let footer = if assigning {
            match highlighted_entry {
                Some(e) if part_of(e).is_some() && self.newer_target(e).is_some() => KEYS_UPDATE,
                Some(e) if part_of(e).is_some() => KEYS_ASSIGN,
                _ => KEYS_IDLE,
            }
        } else if self.highlighted().is_some_and(placeable) {
            KEYS
        } else if highlighted_entry.is_some_and(|e| e.kind == DefinitionKind::Part) {
            KEYS_PART
        } else {
            KEYS_IDLE
        };
        let (hint, empty, heading) = if assigning {
            (
                "Find a manufacturer part: kind:part mpn:EX- res=10k",
                "No manufacturer part matches.",
                "Parts in this project",
            )
        } else {
            (
                "Find a part: 10k 0603, res>=10k, tol<=1%",
                "No component matches.",
                "In this project",
            )
        };
        kit::search_sheet(&ctx, "place-palette", top, width, footer, |ui| {
            let invalid = matches!(self.results.as_ref().map(|r| &r.report), Some(Err(_)));
            let field = kit::search_field(
                ui,
                egui::Id::new("place-palette-query"),
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
                    if report.results.is_empty() {
                        kit::search_message(ui, empty, false);
                    } else {
                        kit::search_heading(ui, heading, report.total);
                        egui::ScrollArea::vertical()
                            .id_salt(("place-palette-rows", generation))
                            .max_height(list_height)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (i, hit) in report.results.iter().enumerate() {
                                    let response = row(
                                        ui,
                                        hit,
                                        &self.query,
                                        i == self.highlighted,
                                        &self.targets,
                                    );
                                    if scroll && i == self.highlighted {
                                        response.scroll_to_me(None);
                                    }
                                    if response.clicked() {
                                        picked = Some(i);
                                    }
                                }
                            });
                    }
                }
                None => {}
            }
        });
        if focused {
            self.focus = false;
        }
        if let Some(i) = picked {
            self.highlighted = i;
            if assigning {
                action = self.choose_part();
            } else if let Some(component) = self.choose(placeable) {
                action = Some(PaletteAction::Pick(component));
            }
        }
        match action {
            Some(PaletteAction::Place {
                component,
                keep_open,
            }) => {
                self.used(component.id);
                if !keep_open {
                    self.close();
                }
            }
            Some(PaletteAction::Pick(component)) => {
                self.used(component.id);
                self.close();
            }
            Some(
                PaletteAction::Assign { part, .. } | PaletteAction::UpdateAndAssign { part, .. },
            ) => {
                self.used(part.id);
                self.close();
            }
            _ => {}
        }
        action
    }

    /// The highlighted result in the properties card, from `library.inspect`.
    /// In assign mode with no matching Part it offers New Part…, returned
    /// as the component lineage and the values to rate.
    pub fn preview(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
    ) -> Option<crate::schematic_editor::NewPartRequest> {
        if let Mode::Assign { instances } = &self.mode {
            let empty = self.hits().is_empty();
            return assign_preview(
                ui,
                engine,
                instances,
                self.highlighted_part(),
                &self.targets,
                empty,
            );
        }
        let Some(component) = self.highlighted() else {
            kit::section(ui, "palette-preview", "Part", true, None, |ui| {
                kit::note(
                    ui,
                    "Type to find a published component. Field values filter by \
                     meaning: res>=10k, tol<=1%, C:1n..100n. pkg: matches package \
                     names only.",
                );
            });
            return None;
        };
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
                Err(error) => Err(error.to_string()),
            };
            self.preview = Some((engine.revision(), component, report));
        }
        let Some((_, _, report)) = &self.preview else {
            return None;
        };
        match report {
            Ok(report) => item_sections(ui, report),
            Err(error) => {
                kit::status(ui, error, Tone::Error);
            }
        }
        None
    }
}

/// The badges a row shows, always all four.
fn badges(entry: &CatalogEntry) -> [kit::SearchBadge<'static>; 4] {
    catalog::Badge::ALL.map(|badge| kit::SearchBadge {
        label: badge.label(),
        present: entry.badges.get(badge),
    })
}

fn detail(entry: &CatalogEntry) -> String {
    let mut parts: Vec<String> = entry.manufacturer.iter().cloned().collect();
    parts.extend(entry.key_values().into_iter().take(4).map(str::to_owned));
    if let Some(revision) = entry.revision {
        parts.push(format!("project {revision}"));
    }
    if entry.used > 0 {
        parts.push(format!("used {}×", entry.used));
    }
    parts.join(" · ")
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

/// Assign mode's properties: who gets the Part, then the highlighted Part
/// as the project library holds it.
fn assign_preview(
    ui: &mut Ui,
    engine: &Engine,
    instances: &[Id],
    part: Option<PartRef>,
    targets: &[ComponentRef],
    empty: bool,
) -> Option<crate::schematic_editor::NewPartRequest> {
    let project = engine.project();
    let library = &project.library;
    kit::section(
        ui,
        "assign-target",
        "Assign to",
        true,
        Some(instances.len()),
        |ui| {
            let designators: Vec<&str> = instances
                .iter()
                .filter_map(|id| project.instance(*id))
                .map(|i| i.designator.as_str())
                .collect();
            kit::value_row(ui, "Instances", &designators.join(", "));
            kit::note(
                ui,
                "The Part must implement the instances' component revision and footprint and meet their values; otherwise nothing changes.",
            );
        },
    );
    let Some(r) = part else {
        let mut request = None;
        kit::section(ui, "assign-part", "Part", true, None, |ui| {
            kit::note(
                ui,
                "Results are manufacturer parts of the instances' component. Field values filter by meaning: res=10k, V>=16, tol<=1%. mpn: matches part numbers only.",
            );
            let first = instances.first().and_then(|id| project.instance(*id));
            if empty && let Some(instance) = first {
                kit::status(
                    ui,
                    "No Part of this component meets these values yet.",
                    Tone::Note,
                );
                kit::actions(ui, |ui| {
                    if kit::primary(ui, "New Part for these values…", true)
                        .on_hover_text(
                            "Open the component's Parts editor with a draft rated at these values",
                        )
                        .clicked()
                    {
                        let values = pcbcraft_core::instances::requirements(library, instance)
                            .map(|r| crate::resolve_parts::requirement_values(&r))
                            .unwrap_or_default();
                        request = Some((instance.component.id, values));
                    }
                });
            }
        });
        return request;
    };
    let Some(published) = library.part(r) else {
        kit::status(
            ui,
            "This Part revision is not in the project library.",
            Tone::Error,
        );
        return None;
    };
    let part = &published.definition;
    kit::section(ui, "assign-part", "Part", true, None, |ui| {
        kit::value_row(ui, "Maker", &part.manufacturer);
        kit::value_row(ui, "MPN", &part.mpn);
        kit::value_row(ui, "Revision", &r.revision.to_string());
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
        if targets
            .iter()
            .all(|t| t.id == part.component.id && t.revision < part.component.revision)
            && !targets.is_empty()
        {
            let designators: Vec<&str> = instances
                .iter()
                .filter_map(|id| project.instance(*id))
                .map(|i| i.designator.as_str())
                .collect();
            kit::status(
                ui,
                &format!(
                    "This Part is for {component}. Enter reviews updating {} to {} and assigning it, as one step.",
                    designators.join(", "),
                    part.component.revision
                ),
                Tone::Warning,
            );
        }
    });
    if !part.specs.is_empty() {
        kit::section(
            ui,
            "assign-specs",
            "Rated values",
            true,
            Some(part.specs.len()),
            |ui| {
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
            },
        );
    }
    if !part.offers.is_empty() || !part.references.is_empty() {
        kit::section(ui, "assign-sources", "Sources", false, None, |ui| {
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
        });
    }
    None
}

fn row(
    ui: &mut Ui,
    hit: &Hit,
    query: &str,
    highlighted: bool,
    targets: &[ComponentRef],
) -> egui::Response {
    let entry = &hit.entry;
    let badges = badges(entry);
    let mut detail = detail(entry);
    if let Some(newer) = newer_than(entry, targets) {
        detail = format!("needs update to {} · {detail}", newer.revision);
    }
    kit::search_row(
        ui,
        egui::Id::new(("place-palette-row", entry.lineage.0)),
        &kit::SearchRow {
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
        },
    )
}

/// The preview sections of one inspected revision.
fn item_sections(ui: &mut Ui, report: &ItemReport) {
    let entry = &report.entry;
    kit::section(ui, "palette-part", "Part", true, None, |ui| {
        kit::value_row(ui, "Name", &entry.name);
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
        kit::value_row(ui, "Revision", &revision);
        kit::value_row(ui, "Source", "This project");
        kit::value_row(
            ui,
            "Used",
            &match entry.used {
                0 => "Not placed yet".to_owned(),
                1 => "1 instance".to_owned(),
                n => format!("{n} instances"),
            },
        );
        let present: Vec<&str> = catalog::Badge::ALL
            .iter()
            .filter(|b| entry.badges.get(**b))
            .map(|b| b.label())
            .collect();
        let missing: Vec<&str> = catalog::Badge::ALL
            .iter()
            .filter(|b| !entry.badges.get(**b))
            .map(|b| b.label())
            .collect();
        kit::value_row(ui, "Has", &present.join(" · "));
        if !missing.is_empty() {
            kit::value_row(ui, "Missing", &missing.join(" · "));
        }
        if entry.draft {
            kit::status(
                ui,
                "This lineage has unpublished draft changes; the published revision is placed.",
                Tone::Note,
            );
        }
        if !entry.description.is_empty() {
            kit::note(ui, &entry.description);
        }
    });
    if !entry.fields.is_empty() {
        kit::section(
            ui,
            "palette-fields",
            "Fields",
            true,
            Some(entry.fields.len()),
            |ui| {
                for field in &entry.fields {
                    kit::value_row(ui, &field.label, &field.text);
                }
            },
        );
    }
    kit::section(ui, "palette-physical", "Physical", true, None, |ui| {
        for symbol in &report.symbols {
            kit::value_row(
                ui,
                "Symbol",
                &format!(
                    "{} · {} · {} unit{}",
                    symbol.name,
                    symbol.symbol.revision,
                    symbol.units.len(),
                    if symbol.units.len() == 1 { "" } else { "s" }
                ),
            );
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
            kit::note(ui, "Schematic only: no physical implementation.");
        }
        kit::value_row(ui, "Terminals", &report.terminals.to_string());
    });
    if !report.provenance.is_empty() {
        kit::note(ui, &report.provenance);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::format::{EditorFile, FILE_VERSION};

    /// Assign mode: the palette opens with the given query, Enter on a Part
    /// row returns the Part for the instances and closes, and a component
    /// row keeps the palette open with a notice. The result list is set
    /// directly: which entries `library.search` returns is the catalog's
    /// concern, not the palette's.
    #[test]
    fn assign_mode_returns_the_highlighted_part_for_the_instances()
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
        let instances = vec![41, 42];
        let mut palette = PlacePalette::default();
        palette.open_assign("kind:part res=10k".into(), instances.clone());
        assert_eq!(palette.query(), "kind:part res=10k");
        assert_eq!(
            palette.mode(),
            &Mode::Assign {
                instances: instances.clone()
            }
        );
        palette.results = Some(Results {
            revision: engine.revision(),
            query: palette.query.clone(),
            report: Ok(report),
        });
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
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(run(&mut palette, vec![]), None);
        assert_eq!(palette.highlighted_part(), Some(part));
        assert_eq!(palette.highlighted(), None, "a Part is not placeable");
        // The component row is not a Part: a notice, nothing assigned.
        run(&mut palette, vec![key(egui::Key::ArrowDown)]);
        assert_eq!(run(&mut palette, vec![key(egui::Key::Enter)]), None);
        assert!(palette.is_open());
        assert!(palette.notice.is_some());
        run(&mut palette, vec![key(egui::Key::ArrowUp)]);
        let action = run(&mut palette, vec![key(egui::Key::Enter)]);
        assert_eq!(action, Some(PaletteAction::Assign { instances, part }));
        assert!(!palette.is_open());
        // Opening for placement returns to Place mode with an empty query.
        palette.open();
        assert_eq!(palette.mode(), &Mode::Place);
        assert_eq!(palette.query(), "");
        Ok(())
    }

    /// A Part for a newer revision of the instances' component is flagged
    /// on its row and Enter asks for the reviewed update-and-assign, never
    /// a refused plain assignment. In Place mode, Enter on a Part row
    /// explains that Parts are assigned, not placed.
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
            detail(&hit.entry).starts_with("ExampleCo"),
            "the maker leads"
        );
        let mut palette = PlacePalette::default();
        palette.open_assign("kind:part".into(), vec![41]);
        palette.results = Some(Results {
            revision: engine.revision(),
            query: palette.query.clone(),
            report: Ok(report.clone()),
        });
        palette.targets = vec![current];
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
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
        assert_eq!(
            run(&mut palette, vec![key(egui::Key::Enter)]),
            Some(PaletteAction::UpdateAndAssign {
                instances: vec![41],
                part,
                target: newer
            })
        );
        // Place mode: a Part row is not placed.
        palette.open();
        palette.results = Some(Results {
            revision: engine.revision(),
            query: palette.query.clone(),
            report: Ok(report),
        });
        assert_eq!(run(&mut palette, vec![key(egui::Key::Enter)]), None);
        assert!(palette.is_open());
        assert_eq!(palette.notice.as_deref(), Some(PART_NOT_PLACED));
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
        assert_eq!(detail(entry), "10 kΩ · ±1% · 0603 · project r1");
        let shown: Vec<(&str, bool)> = badges(entry).iter().map(|b| (b.label, b.present)).collect();
        assert_eq!(
            shown,
            [("sym", true), ("fp", true), ("3D", true), ("MPN", false)]
        );
        Ok(())
    }
}
