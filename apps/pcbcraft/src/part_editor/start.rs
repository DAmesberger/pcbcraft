//! Slice A1: the Component workspace start page, shown whenever the
//! workspace has no subject: a catalog search (the same `library.search`
//! as the Place palette, project and every ready library source), Recent,
//! New from a template, Create from datasheet… and Import pinout…. A search
//! also lists the template rows it finds, below the components; choosing one
//! opens the New from template form on it. A text that reads as an on-demand
//! generic part lists that part as well: choosing it opens the part the
//! project holds, or creates and publishes it and opens that, with no form.
use super::PartEditor;
use super::identity::Subject;
#[cfg(test)]
use super::{Request, Template};
use crate::kit;
use crate::library_sources::FolderPick;
use crate::refusal::{self, Origin};
use crate::template_gallery::{self, Found};
use egui::{RichText, Ui};
use pcbcraft_core::catalog::generic::GenericCandidate;
use pcbcraft_core::catalog::templates::TemplateHit;
use pcbcraft_core::catalog::{CatalogEntry, Source};
use pcbcraft_core::library::templates::generic::GenericState;
use pcbcraft_core::{DefinitionId, DefinitionKind, Engine, Operation, Query, QueryResult};
use solidcraft_ui_egui::ui_kit::{self, Button};
use solidcraft_ui_egui::{fonts, theme::Tokens};

/// Results shown before "Show all".
const SHOWN: usize = 8;
/// Results "Show all" lists.
const ALL: usize = 50;
/// The longest query, in bytes.
const MAX_QUERY: usize = 256;
/// Where a refused create from this page comes back.
const START_CREATE: &str = "component-start-create";

/// The page's state: the search text and its last answer.
#[derive(Default)]
pub struct StartPage {
    /// Focus the search field on the next frame.
    pub(super) focus: bool,
    query: String,
    all: bool,
    /// (query, engine revision, context generation, limit) → hits.
    results: Option<(Key, Answer)>,
    /// The template rows of the same search.
    templates: Found,
    /// The generic parts the same search reads as.
    generic: Vec<GenericCandidate>,
    /// A create chosen here, for the app to apply.
    outgoing: Option<Operation>,
    /// A create was sent: the revision and components it started from.
    sent: Option<(u64, Vec<DefinitionId>)>,
    /// Why the last generic part chosen was not created.
    notice: Option<String>,
}

impl StartPage {
    /// The create this page chose, once.
    pub(super) fn take_outgoing(&mut self) -> Option<Operation> {
        self.outgoing.take()
    }
}

/// What a start page row asks for.
enum Chosen {
    Subject(Subject),
    Template(Box<TemplateHit>),
    Generic(Box<GenericCandidate>),
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
        if self.settle_create(ui.ctx(), engine) {
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
                        let recent = self.start_recents(ui, engine).map(Chosen::Subject);
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
        if let Some(chosen) = open {
            match chosen {
                Chosen::Subject(subject) => self.open_subject(subject, engine),
                Chosen::Template(hit) => {
                    let query = self.start.query.clone();
                    self.gallery
                        .open_on(&hit, query, false, Some(super::model_body::form_rows));
                }
                Chosen::Generic(candidate) => self.choose_generic(ui.ctx(), engine, &candidate),
            }
        } else if new {
            self.open_template_gallery();
        }
        true
    }

    /// A generic part chosen: open it, create it first, or say why not.
    fn choose_generic(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        candidate: &GenericCandidate,
    ) {
        self.start.notice = None;
        match &candidate.state {
            GenericState::Existing { component } => self.open(component.id, engine),
            GenericState::Draft { id } => self.open(*id, engine),
            GenericState::Refused { reason } => self.start.notice = Some(reason.clone()),
            GenericState::NotCreated => {
                refusal::mark(ctx, Origin::Dialog(egui::Id::new(START_CREATE)));
                self.start.sent = Some((
                    engine.revision(),
                    template_gallery::published_components(engine),
                ));
                self.start.outgoing = template_gallery::generic_request(candidate);
            }
        }
    }

    /// The answer to a create sent from this page; `true` when it opened the
    /// part, so the page is not drawn.
    fn settle_create(&mut self, ctx: &egui::Context, engine: &Engine) -> bool {
        let Some((revision, before)) = &self.start.sent else {
            return false;
        };
        if let Some(why) = refusal::claim(ctx, Origin::Dialog(egui::Id::new(START_CREATE))) {
            self.start.sent = None;
            self.start.notice = Some(why);
        } else if engine.revision() != *revision {
            let created = template_gallery::new_component(engine, before);
            self.start.sent = None;
            if let Some(component) = created {
                self.open(component.id, engine);
                return true;
            }
        }
        false
    }

    /// Tests: start a new component from a sample `template` in new
    /// lineages; it opens once the app has created it. People start one
    /// from the New-from-template gallery.
    #[cfg(test)]
    pub(super) fn create_new(&mut self, template: Template) {
        self.identity.expect_new = true;
        self.request = Some(Request::Create(template));
    }

    /// The search field and its results; what a click chose.
    fn start_search(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Chosen> {
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
            let context = &self.identity.context.1;
            let answer = search(engine, &self.start.query, limit, context);
            self.start.generic = template_gallery::find_generic(engine, &self.start.query);
            self.start.templates =
                template_gallery::find(engine, &self.start.query, limit, context);
            template_gallery::drop_covered(&mut self.start.templates, &self.start.generic);
            self.start.notice = None;
            // A text this grammar reads need not be one the search can. A
            // part the project holds lists once, as that part.
            let generic = &self.start.generic;
            let held = |entry: &CatalogEntry| {
                entry.source == Source::Project
                    && generic
                        .iter()
                        .any(|c| c.component().is_some_and(|r| r.id == entry.lineage))
            };
            let answer = match answer {
                Ok((mut entries, total)) => {
                    let before = entries.len();
                    entries.retain(|e| !held(e));
                    let total = total.saturating_sub(before - entries.len());
                    Ok((entries, total))
                }
                Err(_) if !generic.is_empty() => Ok((Vec::new(), 0)),
                Err(error) => Err(error),
            };
            self.start.results = Some((key, answer));
        }
        let mut open = None;
        let mut show_all = false;
        let names = |entry: &CatalogEntry| match entry.source {
            Source::Project => "This project".to_owned(),
            Source::Library(library) => self.identity.library_name(library),
        };
        if let Some(notice) = &self.start.notice {
            kit::search_message(ui, notice, true);
        }
        match &self.start.results {
            Some((_, Ok((entries, total)))) => {
                let templates = &self.start.templates;
                let generic = &self.start.generic;
                let query = &self.start.query;
                let held = generic.iter().filter(|c| c.component().is_some());
                let held = held.count();
                let mut generic_row = |ui: &mut Ui, (i, candidate): (usize, &GenericCandidate)| {
                    let id = egui::Id::new("component-start-generic").with(i);
                    if template_gallery::generic_row(ui, id, candidate, query, false).clicked() {
                        open = Some(Chosen::Generic(Box::new(candidate.clone())));
                    }
                };
                if !entries.is_empty()
                    || held > 0
                    || (generic.is_empty() && templates.hits.is_empty())
                {
                    kit::search_heading(ui, "Components", *total + held);
                }
                if entries.is_empty() && templates.hits.is_empty() && generic.is_empty() {
                    kit::search_message(ui, "Nothing matches; try fewer words", false);
                }
                generic
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.component().is_some())
                    .for_each(|row| generic_row(ui, row));
                let mut own: Vec<&CatalogEntry> = Vec::with_capacity(entries.len());
                own.extend(entries.iter().filter(|e| e.source == Source::Project));
                let others: Vec<&CatalogEntry> = entries
                    .iter()
                    .filter(|e| e.source != Source::Project)
                    .collect();
                let mut chosen_entry = None;
                let mut entry_row = |ui: &mut Ui, entry: &CatalogEntry| {
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
                        chosen_entry = subject_of(entry).map(Chosen::Subject);
                    }
                };
                own.iter().for_each(|entry| entry_row(ui, entry));
                if generic.len() > held {
                    kit::search_heading(ui, "Generic parts", generic.len() - held);
                }
                generic
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.component().is_none())
                    .for_each(|row| generic_row(ui, row));
                if !templates.hits.is_empty() {
                    let count = templates.hits.len() + templates.more;
                    kit::search_heading(ui, template_gallery::SEARCH_HEADING, count);
                }
                for (i, hit) in templates.hits.iter().enumerate() {
                    let source =
                        template_gallery::source_label(hit, |l| self.identity.library_name(l));
                    let id = egui::Id::new("component-start-template").with(i);
                    let query = &self.start.query;
                    if template_gallery::search_row(ui, id, hit, query, false, &source).clicked() {
                        open = Some(Chosen::Template(Box::new(hit.clone())));
                    }
                }
                if templates.more > 0 {
                    if self.start.all {
                        kit::note(ui, &format!("{} more: narrow the search", templates.more));
                    } else if ui_kit::button(
                        ui,
                        format!("Show all {}", templates.hits.len() + templates.more),
                        Button::Ghost,
                    )
                    .clicked()
                    {
                        show_all = true;
                    }
                }
                others.iter().for_each(|entry| entry_row(ui, entry));
                if *total > entries.len()
                    && ui_kit::button(ui, format!("Show all {total}"), Button::Ghost).clicked()
                {
                    show_all = true;
                }
                open = open.or(chosen_entry);
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
            .on_hover_text("Draft a part from its datasheets with a model you configure: Claude or any OpenAI-compatible endpoint, local or hosted")
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

#[cfg(test)]
mod generic_tests {
    use super::*;
    use crate::kit::text::tests::Screen;
    use egui::vec2;
    use pcbcraft_core::Evaluators;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn apply(engine: &mut Engine, operation: Operation) -> TestResult {
        engine.apply_hosted(
            engine.revision(),
            operation,
            Evaluators {
                footprint: Some(&pcbcraft_solidcraft::footprint::SketchEvaluator),
                board: None,
            },
        )?;
        Ok(())
    }

    /// Open lists the generic part; choosing it creates, publishes and opens
    /// it with no form, and the same text then opens that part.
    #[test]
    fn open_creates_the_part_and_opens_it() -> TestResult {
        let mut engine = Engine::default();
        let mut editor = PartEditor::default();
        let screen = Screen::new(vec2(1000.0, 700.0));
        editor.start.query = "header 1x4".to_owned();
        let drawn = screen.run(vec![], |ui| {
            editor.canvas(ui, &engine);
        });
        screen.picture("open-page-generic", &drawn, 1.0)?;
        assert_eq!(editor.start.generic.len(), 1);
        assert!(
            !editor
                .start
                .templates
                .hits
                .iter()
                .any(|h| h.name == "Pin header 1×4 2.54 mm"),
            "the fixed row is the same part: listed once"
        );

        let candidate = editor.start.generic[0].clone();
        editor.choose_generic(&screen.ctx, &engine, &candidate);
        let operation = editor.start.take_outgoing().ok_or("one create")?;
        assert!(matches!(
            operation,
            Operation::Library(pcbcraft_core::LibraryCommand::PublishFromTemplate(_))
        ));
        apply(&mut engine, operation)?;
        screen.run(vec![], |ui| {
            editor.canvas(ui, &engine);
        });
        let component = engine
            .project()
            .library
            .latest_components()
            .first()
            .map(|c| c.id)
            .ok_or("published")?;
        assert_eq!(editor.lineage, Some(component), "it opened");

        let mut again = PartEditor::default();
        again.start.query = "header 1x4".to_owned();
        screen.run(vec![], |ui| {
            again.canvas(ui, &engine);
        });
        let found = again.start.generic.first().cloned().ok_or("the part")?;
        assert_eq!(found.component().map(|c| c.id), Some(component));
        again.choose_generic(&screen.ctx, &engine, &found);
        assert!(again.start.take_outgoing().is_none(), "nothing to create");
        assert_eq!(again.lineage, Some(component));
        Ok(())
    }

    /// A refused part says why on the page and sends nothing.
    #[test]
    fn open_says_why_a_part_is_refused() -> TestResult {
        let engine = Engine::default();
        let mut editor = PartEditor::default();
        let screen = Screen::new(vec2(1000.0, 700.0));
        editor.start.query = "header 1x9999".to_owned();
        screen.run(vec![], |ui| {
            editor.canvas(ui, &engine);
        });
        let candidate = editor.start.generic.first().cloned().ok_or("the row")?;
        editor.choose_generic(&screen.ctx, &engine, &candidate);
        assert!(editor.start.take_outgoing().is_none());
        assert!(
            editor
                .start
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("at most 512 pins"))
        );
        assert_eq!(editor.lineage, None);
        Ok(())
    }
}
