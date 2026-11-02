//! Slice A1: the Component workspace start page, shown whenever the
//! workspace has no subject: a catalog search (the same `library.search`
//! as the Place palette, project and every ready library source), Recent,
//! New from a template, Create from datasheet… and Import pinout….
use super::PartEditor;
use super::identity::Subject;
#[cfg(test)]
use super::{Request, Template};
use crate::kit;
use crate::library_sources::FolderPick;
use egui::{RichText, Ui};
use pcbcraft_core::catalog::{CatalogEntry, Source};
use pcbcraft_core::{DefinitionKind, Engine, Query, QueryResult};
use solidcraft_ui_egui::ui_kit::{self, Button};
use solidcraft_ui_egui::{fonts, theme::Tokens};

/// Results shown before "Show all".
const SHOWN: usize = 8;
/// Results "Show all" lists.
const ALL: usize = 50;
/// The longest query, in bytes.
const MAX_QUERY: usize = 256;

/// The page's state: the search text and its last answer.
#[derive(Default)]
pub struct StartPage {
    /// Focus the search field on the next frame.
    pub(super) focus: bool,
    query: String,
    all: bool,
    /// (query, engine revision, context generation, limit) → hits.
    results: Option<(Key, Answer)>,
}

type Key = (String, u64, u64, usize);
/// The hits shown and how many matched, or why the search failed.
type Answer = Result<(Vec<CatalogEntry>, usize), String>;

impl PartEditor {
    /// Draw the start page when the workspace has no subject; `true` when
    /// it drew.
    pub(super) fn start_page(&mut self, ui: &mut Ui, engine: &Engine) -> bool {
        if self.lineage.is_some() || self.is_read_only() {
            return false;
        }
        let t = Tokens::get();
        let width = ui.available_width().min(t.form_card_width * 2.0);
        let side = ((ui.available_width() - width) / 2.0).max(0.0);
        let (open, new) = egui::ScrollArea::vertical()
            .id_salt("component-start")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(side);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.add_space(t.space_24);
                        ui.label(
                            RichText::new("Open a component")
                                .font(fonts::semibold(t.font_heading))
                                .color(t.text),
                        );
                        kit::note(
                            ui,
                            "Search this project and every library source, or start a new one",
                        );
                        ui.add_space(t.space_8);
                        let found = self.start_search(ui, engine);
                        let recent = self.start_recents(ui, engine);
                        ui.add_space(t.space_16);
                        let new = start_new(ui, &mut self.assist);
                        self.assist.ui(ui);
                        (found.or(recent), new)
                    })
                    .inner
                })
                .inner
            })
            .inner;
        if let Some(subject) = open {
            self.open_subject(subject, engine);
        } else if new {
            self.open_template_gallery();
        }
        true
    }

    /// Tests: start a new component from a sample `template` in new
    /// lineages; it opens once the app has created it. People start one
    /// from the New-from-template gallery.
    #[cfg(test)]
    pub(super) fn create_new(&mut self, template: Template) {
        self.identity.expect_new = true;
        self.request = Some(Request::Create(template));
    }

    /// The search field and its results; the subject a click opens.
    fn start_search(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Subject> {
        let focus = std::mem::take(&mut self.start.focus);
        kit::search_field(
            ui,
            egui::Id::new("component-start-search"),
            &mut self.start.query,
            "Search: name, 10k, pkg:0603…",
            false,
            focus,
            MAX_QUERY,
        );
        let limit = if self.start.all { ALL } else { SHOWN };
        let key = (
            self.start.query.clone(),
            engine.revision(),
            self.identity.context.0,
            limit,
        );
        if self.start.results.as_ref().is_none_or(|(k, _)| *k != key) {
            let answer = search(engine, &self.start.query, limit, &self.identity.context.1);
            self.start.results = Some((key, answer));
        }
        let mut open = None;
        let mut show_all = false;
        let names = |entry: &CatalogEntry| match entry.source {
            Source::Project => "This project".to_owned(),
            Source::Library(library) => self.identity.library_name(library),
        };
        match &self.start.results {
            Some((_, Ok((entries, total)))) => {
                kit::search_heading(ui, "Components", *total);
                if entries.is_empty() {
                    kit::search_message(ui, "Nothing matches; try fewer words", false);
                }
                for entry in entries {
                    let revision = entry
                        .revision
                        .map_or_else(|| "draft only".to_owned(), |r| r.to_string());
                    let draft = if entry.draft { " · draft" } else { "" };
                    let detail = format!("{} · {revision}{draft}", names(entry));
                    let row = kit::SearchRow {
                        icon: "component",
                        label: &entry.name,
                        query: &self.start.query,
                        detail: &detail,
                        badges: &[],
                        highlighted: false,
                    };
                    let id = egui::Id::new(("component-start-hit", entry.key()));
                    if kit::search_row(ui, id, &row).clicked() {
                        open = subject_of(entry);
                    }
                }
                if *total > entries.len()
                    && ui_kit::button(ui, format!("Show all {total}"), Button::Ghost).clicked()
                {
                    show_all = true;
                }
            }
            Some((_, Err(error))) => kit::search_message(ui, error, true),
            None => {}
        }
        if show_all {
            self.start.all = true;
        }
        open
    }

    /// Recent subjects, newest first.
    fn start_recents(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Subject> {
        let recents = self.identity.recents();
        if recents.is_empty() {
            return None;
        }
        let rows: Vec<(Subject, String)> = recents
            .iter()
            .take(6)
            .map(|s| (s.clone(), self.subject_name(s, engine)))
            .collect();
        let t = Tokens::get();
        ui.add_space(t.space_16);
        kit::search_heading(ui, "Recent", rows.len());
        let mut open = None;
        for (subject, name) in rows {
            let detail = match &subject {
                Subject::Project(_) => "This project".to_owned(),
                Subject::Folder(pick) => format!("read-only · {}", pick.revision),
            };
            let row = kit::SearchRow {
                icon: "component",
                label: &name,
                query: "",
                detail: &detail,
                badges: &[],
                highlighted: false,
            };
            let id = egui::Id::new(("component-start-recent", format!("{subject:?}")));
            if kit::search_row(ui, id, &row).clicked() {
                open = Some(subject);
            }
        }
        open
    }
}

/// New from template… (the one searchable gallery of package rows and
/// family entries), Create from datasheet… (opens the assistant's form
/// below), and Import pinout….
fn start_new(ui: &mut Ui, assist: &mut super::datasheet_assist::DatasheetAssist) -> bool {
    kit::search_heading(ui, "New", 1);
    let mut chosen = false;
    ui.horizontal_wrapped(|ui| {
        chosen = ui_kit::button(ui, "New from template…", Button::Normal)
            .on_hover_text("Search package rows by name, size code, pins, pitch or body")
            .clicked();
        if ui_kit::button(ui, "Create from datasheet…", Button::Normal)
            .on_hover_text("Draft a part from its datasheets with Claude (needs ANTHROPIC_API_KEY)")
            .clicked()
        {
            assist.open = true;
        }
        ui.add_enabled_ui(false, |ui| {
            ui_kit::button(ui, "Import pinout…", Button::Normal)
        })
        .response
        .on_disabled_hover_text("Pinout import runs from the CLI or an agent for now");
    });
    chosen
}

/// [`start_new`] for the assistant's start-page test.
#[cfg(test)]
pub(super) fn start_new_for_tests(
    ui: &mut Ui,
    assist: &mut super::datasheet_assist::DatasheetAssist,
) -> bool {
    start_new(ui, assist)
}

/// What opening a catalog entry shows: a project lineage (a Part opens its
/// component), or a folder item read-only.
fn subject_of(entry: &CatalogEntry) -> Option<Subject> {
    match entry.source {
        Source::Project => match entry.kind {
            DefinitionKind::Component => Some(Subject::Project(entry.lineage)),
            DefinitionKind::Part => entry.implements.map(|c| Subject::Project(c.id)),
            _ => None,
        },
        Source::Library(_) => FolderPick::of(entry).map(Subject::Folder),
    }
}

/// `library.search` with drafts listed, as the catalog shows them.
fn search(
    engine: &Engine,
    query: &str,
    limit: usize,
    context: &pcbcraft_core::catalog::CatalogContext,
) -> Answer {
    match engine.query_with(
        Query::Search {
            query: query.to_owned(),
            limit: Some(limit),
            include_drafts: true,
        },
        context,
    ) {
        Ok(QueryResult::Search(report)) => Ok((
            report.results.into_iter().map(|h| h.entry).collect(),
            report.total,
        )),
        Ok(_) => Err("library.search returned an unexpected report".into()),
        Err(error) => Err(error.to_string()),
    }
}
