//! The Schematic editor's Place palette: a searchable sheet over the
//! project's component lineages. Every result list comes from the engine's
//! `library.search` query and every preview from `library.inspect`; the
//! palette only returns what the user chose. The editor turns that into the
//! ordinary `component.place` operation.
use crate::kit::{self, Tone};
use egui::{Rect, Ui, pos2};
use pcbcraft_core::catalog::{
    self, CatalogContext, CatalogEntry, Hit, ItemReport, MAX_QUERY, SearchReport,
};
use pcbcraft_core::{ComponentRef, DefinitionId, Engine, Query, QueryResult};
use solidcraft_ui_egui::theme::Tokens;

/// Rows one search asks for.
const ROWS: usize = 40;
/// Recently placed lineages remembered for ranking (this session only).
const RECENT: usize = 16;
/// Key names are spelled out: the bundled fonts have no ⏎ or ⇧ glyphs.
const KEYS: &str = "↑↓ move   Enter place   Shift+Enter place & keep open   click pick   Esc close";

/// What the user chose in the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    /// Place this revision now; keep the palette open with Shift+Enter.
    Place {
        component: ComponentRef,
        keep_open: bool,
    },
    /// Arm the Place tool with this revision; clicking the sheet places it.
    Pick(ComponentRef),
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
}

impl PlacePalette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open with an empty query, ready for typing.
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.focus = true;
        self.highlighted = 0;
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
        }
        self.results = Some(Results {
            revision: engine.revision(),
            query: self.query.clone(),
            report,
        });
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
            .and_then(|h| h.entry.component())
    }

    /// The sheet at the top of `area`. Owns the navigation keys while open.
    pub fn show(&mut self, ui: &mut Ui, area: Rect, engine: &Engine) -> Option<PaletteAction> {
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
        }
        if keys.up {
            self.highlighted = self.highlighted.saturating_sub(1);
            self.scroll = true;
        }
        self.highlighted = self.highlighted.min(count.saturating_sub(1));
        let mut action = None;
        if keys.enter
            && let Some(component) = self.highlighted()
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
        kit::search_sheet(&ctx, "place-palette", top, width, |ui| {
            let invalid = matches!(self.results.as_ref().map(|r| &r.report), Some(Err(_)));
            let field = kit::search_field(
                ui,
                egui::Id::new("place-palette-query"),
                &mut self.query,
                "Find a part: 10k 0603, R>=10k, tol<=1%",
                invalid,
                focus,
                MAX_QUERY,
            );
            focused = field.has_focus();
            // Show the results of what was just typed in this frame.
            if field.changed() {
                self.refresh(engine);
            }
            match self.results.as_ref().map(|r| &r.report) {
                Some(Err(error)) => {
                    ui.add_space(t.space_4);
                    kit::status(ui, error, Tone::Error);
                    ui.add_space(t.space_4);
                }
                Some(Ok(report)) => {
                    let hidden: Vec<String> = report
                        .hidden
                        .iter()
                        .map(|h| format!("{} hidden ({})", h.count, h.reason))
                        .collect();
                    kit::search_chips(ui, &report.chips, &hidden.join(", "));
                    if report.results.is_empty() {
                        kit::note(ui, "No published component matches.");
                    } else {
                        kit::search_heading(ui, "In this project", report.total);
                        egui::ScrollArea::vertical()
                            .id_salt("place-palette-rows")
                            .max_height(list_height)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (i, hit) in report.results.iter().enumerate() {
                                    let response = row(ui, hit, &self.query, i == self.highlighted);
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
            kit::search_footer(ui, KEYS);
        });
        if focused {
            self.focus = false;
        }
        if let Some(i) = picked {
            self.highlighted = i;
            if let Some(component) = self.highlighted() {
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
            _ => {}
        }
        action
    }

    /// The highlighted result in the properties card, from `library.inspect`.
    pub fn preview(&mut self, ui: &mut Ui, engine: &Engine) {
        let Some(component) = self.highlighted() else {
            kit::section(ui, "palette-preview", "Part", true, None, |ui| {
                kit::note(
                    ui,
                    "Type to find a published component. Field values filter by \
                     meaning: R>=10k, tol<=1%, C:1n..100n. pkg: matches package \
                     names only.",
                );
            });
            return;
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
            return;
        };
        match report {
            Ok(report) => item_sections(ui, report),
            Err(error) => {
                kit::status(ui, error, Tone::Error);
            }
        }
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
    let mut parts: Vec<String> = entry
        .key_values()
        .into_iter()
        .take(4)
        .map(str::to_owned)
        .collect();
    if let Some(revision) = entry.revision {
        parts.push(format!("project {revision}"));
    }
    if entry.used > 0 {
        parts.push(format!("used {}×", entry.used));
    }
    parts.join(" · ")
}

fn row(ui: &mut Ui, hit: &Hit, query: &str, highlighted: bool) -> egui::Response {
    let entry = &hit.entry;
    let badges = badges(entry);
    let detail = detail(entry);
    kit::search_row(
        ui,
        egui::Id::new(("place-palette-row", entry.lineage.0)),
        &kit::SearchRow {
            icon: "component",
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
