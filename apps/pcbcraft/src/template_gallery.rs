//! Slice B4: the "New from template" gallery sheet. One searchable sheet
//! (the kit search sheet scaled up: a field, family chips, the results on
//! the left and a preview column on the right) serves two scopes:
//!
//! - **New component** (New component ▾ › From template…, the catalog's
//!   Templates folder): one `library.new_from_template`, five drafts in one
//!   undo step. The preview chooses the skeleton (Start as), the name and
//!   the designator prefix.
//! - **Package** (the Footprint tab's From template, the 3D tab's Change
//!   package…): one `library.package_from_template` replacing the open
//!   working set's package, footprint and generated model. The preview
//!   shows the core's `library.package_plan` (terminals before and after,
//!   pads) before anything changes; terminal mappings are never guessed.
//!
//! Rows come from `library.templates` (the project's rows, every library
//! folder's rows and each family's built-in custom entry). A row's values
//! are its family's typed [`PackageForm`]; **Edit values** unfolds them
//! (the family rows of the 3D tab Body section, handed in as [`FormRows`])
//! with the core's `range_problems` inline, and **Save as row…** keeps the
//! edited values as a new project row (`library.save_package_row`). A row
//! says whether its values are cited from an outline drawing or estimated,
//! and whether its lands are cited or template defaults.
//!
//! The preview's pads are the core's own `samples::template_lands`, the
//! one land generator `library.new_from_template` and
//! `library.package_from_template` build with (the row's cited land
//! overrides included), so the preview is exactly what Create or Apply
//! builds; the top view draws the row's envelope and lead points. The pads
//! are plain arithmetic (no sketch solve), rebuilt only when the values
//! change and shared by reference between frames.
//!
//! A folder row is carried inline (`RowSource::Bundle`), as
//! `library.place_from` carries folder items: the gallery asks its host to
//! read the row's bundle off the UI thread ([`TemplateGallery::take_reads`])
//! and sends the request once the host hands the bundle back
//! ([`TemplateGallery::bundle_read`]).
//!
//! Each submit waits for the engine's answer: a refusal comes back to the
//! sheet's refusal line (`refusal::Origin::Dialog`), success closes the
//! sheet with an Undo toast. A created component is offered to the host to
//! open explicitly ([`TemplateGallery::take_created`]).
//!
//! The Open component page and the Place palette list the same rows below
//! real components ([`find`], [`search_row`]). Choosing one opens this form
//! on that row; from the Place palette the primary creates and publishes the
//! component ([`publish_request`]) so the host can arm placement of it
//! ([`TemplateGallery::take_to_place`]).
//!
//! The same lists also show the on-demand generic parts a text reads as
//! ([`find_generic`], [`generic_row`]): the part itself when the project holds
//! it, else a "not yet created" row that needs no form.
use crate::kit::severity::Level;
use crate::kit::toast::{ToastAction, post};
use crate::kit::{self, Tone};
use crate::library_sources::FolderPick;
use crate::refusal::{self, Origin};
use crate::symbol_view::{self, View};
use egui::{Align, Layout, Rect, Sense, Stroke, Ui, pos2, vec2};
use pcbcraft_core::catalog::generic::GenericCandidate;
use pcbcraft_core::catalog::templates::{TemplateHit, TemplateReport};
use pcbcraft_core::catalog::{CatalogContext, MAX_QUERY, Source};
use pcbcraft_core::library::footprint_source::ParamValue;
use pcbcraft_core::library::samples;
use pcbcraft_core::library::symbol_style::SymbolKind;
use pcbcraft_core::library::templates::catalog::{
    self, FamilyKind, LandValues, NewFromTemplate, PackageFromTemplate, PackagePlan, RowCitation,
    RowFrom, RowSource, SavePackageRow, SkeletonHint, TerminalMatch,
};
use pcbcraft_core::library::templates::generic::GenericState;
use pcbcraft_core::library::{
    Bundle, Definition, MAX_DESIGNATOR_PREFIX, MAX_LABEL, PackageDefinition, PackageForm,
    PackageTerminal, Pad, PadKind, Provenance,
};
use pcbcraft_core::{
    ComponentRef, DefinitionId, Engine, LibraryCommand, Operation, PackageTerminalId, Query,
    QueryResult,
};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::sync::Arc;

/// The sheet's id (also its refusal origin).
pub const GALLERY: &str = "template-gallery";
/// Rows one search asks for (the core caps it at `MAX_TEMPLATE_HITS`).
pub const ROWS: usize = pcbcraft_core::catalog::templates::MAX_TEMPLATE_HITS;
/// Frames a submit waits for the engine's answer before the sheet is
/// usable again (the app may drop an operation while a file is busy).
const ANSWER_FRAMES: u64 = 3;
/// Folder bundles the sheet keeps after reading them.
const MAX_BUNDLES: usize = 16;
/// Aliases one saved row takes from the Save form.
const MAX_SAVE_ALIASES: usize = 16;
/// Key hints along the sheet's bottom edge.
const KEYS_NEW: &str = "↑↓ move   ⏎ create   ⇥ edit values   esc close";
const KEYS_PLACE: &str = "↑↓ move   ⏎ create and place   ⇥ edit values   esc close";
const KEYS_PACKAGE: &str = "↑↓ move   ⏎ apply   ⇥ edit values   esc close";
const KEYS_EDIT: &str = "esc back to the rows";

/// The 3D tab Body section's typed rows for one form (every family's,
/// including the later families' `family_rows`); returns whether a value
/// changed. Handed in by the opener, which owns those rows.
pub type FormRows = fn(&mut Ui, &mut PackageForm) -> bool;

/// What the sheet was opened for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// A new component from the row: `library.new_from_template`.
    NewComponent,
    /// The open working set's package: `library.package_from_template`.
    Package {
        /// The package draft.
        draft: DefinitionId,
        /// Pads the footprint holds now; a non-empty footprint is replaced
        /// in the same atomic step.
        pads: usize,
    },
}

/// The family chips over the results: All, one per family, This project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Chip {
    #[default]
    All,
    Family(FamilyKind),
    /// Rows the project holds (saved or published here).
    Project,
}

impl Chip {
    /// Whether `hit` shows under this chip.
    pub fn shows(self, hit: &TemplateHit) -> bool {
        match self {
            Self::All => true,
            Self::Family(family) => hit.family == family,
            Self::Project => hit.source == Source::Project && !hit.is_custom() && !hit.is_starter(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Family(family) => family_label(family),
            Self::Project => "This project",
        }
    }
}

/// Chips in shown order.
pub fn chips() -> Vec<Chip> {
    let mut out = vec![Chip::All];
    out.extend(FamilyKind::ALL.into_iter().map(Chip::Family));
    out.push(Chip::Project);
    out
}

/// A family's chip and row label, by the packages it covers.
pub fn family_label(family: FamilyKind) -> &'static str {
    match family {
        FamilyKind::Chip => "Chip",
        FamilyKind::GullWing => "SOIC/QFP",
        FamilyKind::NoLead => "QFN/DFN",
        FamilyKind::Header => "Headers",
        FamilyKind::Tabbed => "SOT/DPAK",
        FamilyKind::Molded => "SOD/SMA",
        FamilyKind::Grid => "BGA",
        FamilyKind::Tht => "THT",
        FamilyKind::Module => "Modules",
        FamilyKind::UsbC => "USB-C",
    }
}

/// A search list's group of template rows, below its real components.
pub const SEARCH_HEADING: &str = "From template";
/// What a template row's badge in a search list means.
const CREATE_TIP: &str = "not a component yet: choosing it creates one from this template";

/// Template rows a component search also found, and how many more match.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub hits: Vec<TemplateHit>,
    pub more: usize,
}

/// `library.templates` for the text of a component search. An empty text
/// lists nothing, and a text in the component search's own grammar that
/// `library.templates` refuses simply finds no template.
pub fn find(engine: &Engine, query: &str, limit: usize, context: &CatalogContext) -> Found {
    if query.trim().is_empty() {
        return Found::default();
    }
    let request = Query::Templates {
        query: query.to_owned(),
        limit: Some(limit),
    };
    match engine.query_with(request, context) {
        Ok(QueryResult::Templates(report)) => Found {
            hits: report.hits,
            more: report.more,
        },
        _ => Found::default(),
    }
}

/// `library.generic_parts` for the text of a component search.
pub fn find_generic(engine: &Engine, query: &str) -> Vec<GenericCandidate> {
    let request = Query::GenericParts {
        query: query.to_owned(),
    };
    match engine.query(request) {
        Ok(QueryResult::GenericParts(report)) => report.candidates,
        _ => Vec::new(),
    }
}

/// A text that reads as the part of a template row lists it once.
pub fn drop_covered(found: &mut Found, candidates: &[GenericCandidate]) {
    found.hits.retain(|hit| {
        !candidates
            .iter()
            .any(|c| c.covers_row.is_some() && c.covers_row == hit.starter)
    });
}

/// The operation that creates a generic part that is not yet created.
pub fn generic_request(candidate: &GenericCandidate) -> Option<Operation> {
    let request = candidate.would_create.clone()?;
    Some(Operation::Library(LibraryCommand::PublishFromTemplate(
        Box::new(request),
    )))
}

/// One generic part of a component search list: its name, its detail (or why
/// its family refuses it), badges saying how the text was read, and a chip
/// saying where it stands.
pub fn generic_row(
    ui: &mut Ui,
    id: egui::Id,
    candidate: &GenericCandidate,
    query: &str,
    highlighted: bool,
) -> egui::Response {
    let badges: Vec<kit::SearchBadge<'_>> = candidate
        .badges
        .iter()
        .map(|b| kit::SearchBadge {
            label: &b.label,
            present: true,
            tip: &b.tip,
        })
        .collect();
    let chip = match candidate.state {
        GenericState::Existing { .. } => None,
        GenericState::NotCreated => Some(kit::RowChip {
            label: "not yet created",
            warning: false,
        }),
        GenericState::Draft { .. } => Some(kit::RowChip {
            label: "draft",
            warning: true,
        }),
        GenericState::Refused { .. } => Some(kit::RowChip {
            label: "cannot be made",
            warning: true,
        }),
    };
    kit::search_row_marked(
        ui,
        id,
        &kit::SearchRow {
            icon: if candidate.component().is_some() {
                "component"
            } else {
                "new"
            },
            label: &candidate.name,
            query,
            detail: &candidate.detail,
            badges: &badges,
            highlighted,
        },
        kit::RowMarks { star: false, chip },
    )
}

/// One template row of a component search list: the row's name and detail,
/// with a Create badge that says it is not a component yet.
pub fn search_row(
    ui: &mut Ui,
    id: egui::Id,
    hit: &TemplateHit,
    query: &str,
    highlighted: bool,
    source: &str,
) -> egui::Response {
    let line = detail(hit, source);
    let badges = [kit::SearchBadge {
        label: "Create",
        present: true,
        tip: CREATE_TIP,
    }];
    kit::search_row(
        ui,
        id,
        &kit::SearchRow {
            icon: "new",
            label: &hit.name,
            query,
            detail: &line,
            badges: &badges,
            highlighted,
        },
    )
}

/// Where a row comes from, in its detail line.
pub fn source_label(
    hit: &TemplateHit,
    library: impl Fn(pcbcraft_core::LibraryId) -> String,
) -> String {
    match &hit.source {
        Source::Project if hit.is_starter() || hit.is_custom() => "Built-in".to_owned(),
        Source::Project => "This project".to_owned(),
        Source::Library(id) => library(*id),
    }
}

/// The designator prefix a skeleton starts with: the row's suggested
/// symbol's (C for a tantalum row), else a chip's kind names its letter, a
/// polarised 2-terminal is a diode, a connector `J`, else `U` (the core's
/// `catalog::default_prefix_for`, which `library.new_from_template` uses).
pub fn default_prefix(
    form: &PackageForm,
    skeleton: SkeletonHint,
    symbol: Option<SymbolKind>,
) -> &'static str {
    catalog::default_prefix_for(skeleton, form, symbol)
}

/// The Start as choices.
fn skeletons() -> Vec<(SkeletonHint, String)> {
    [
        (SkeletonHint::TwoTerminal { polarised: false }, "2-terminal"),
        (
            SkeletonHint::TwoTerminal { polarised: true },
            "2-terminal, polarised",
        ),
        (SkeletonHint::IcBox, "IC box"),
        (SkeletonHint::Connector, "Connector"),
        (SkeletonHint::Blank, "Blank"),
    ]
    .into_iter()
    .map(|(s, l)| (s, l.to_owned()))
    .collect()
}

/// The prefixes a 2-terminal part offers.
const TWO_TERMINAL_PREFIXES: [&str; 9] = ["R", "C", "L", "D", "LED", "F", "FB", "Y", "SW"];

/// A length in the preview's wording ("1.27", "0.5").
fn mm(v: f64) -> String {
    kit::number_text(v)
}

/// A package value by its `pkg_*` name.
fn value(form: &PackageForm, name: &str) -> Option<ParamValue> {
    form.values()
        .into_iter()
        .find(|v| v.name.0 == format!("pkg_{name}"))
        .map(|v| v.value)
}

fn length(form: &PackageForm, name: &str) -> Option<f64> {
    match value(form, name)? {
        ParamValue::Length(v) => Some(v),
        _ => None,
    }
}

/// Terminals a form lands: its leads plus an exposed pad.
pub fn terminal_count(form: &PackageForm) -> u64 {
    form.terminal_counts().into_iter().max().unwrap_or(0)
}

/// The preview's value rows: body, terminals and pitch, size code.
pub fn summary(hit: &TemplateHit, form: &PackageForm) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if let Some([l, w, h]) = form.envelope() {
        rows.push((
            "Body",
            format!("{} × {} × {} mm (L × W × H)", mm(l), mm(w), mm(h)),
        ));
    }
    let leads = form.lead_count();
    let terminals = terminal_count(form);
    let mut count = if terminals > leads {
        format!("{leads} + {}", terminals - leads)
    } else {
        leads.to_string()
    };
    if let Some(pitch) = length(form, "e") {
        count.push_str(&format!("  (pitch {} mm)", mm(pitch)));
    }
    rows.push(("Terminals", count));
    if let Some(code) = &hit.size_code {
        rows.push((
            "Size code",
            format!("{} imperial · {} metric", code.imperial, code.metric),
        ));
    }
    rows
}

/// How a row's values are sourced: "custom values", "cited" (the
/// citation records the drawing's revision and page), "cited
/// (unverified)" (it names only the document) or "estimated".
pub fn basis(hit: &TemplateHit) -> &'static str {
    match (hit.is_custom(), hit.cited, hit.citation_checked) {
        (true, _, _) => "custom values",
        (false, true, true) => "cited",
        (false, true, false) => "cited (unverified)",
        (false, false, _) => "estimated",
    }
}

/// What a row's badge means, for its hover.
fn badge_tip(hit: &TemplateHit) -> &'static str {
    match (hit.is_custom(), hit.cited, hit.citation_checked) {
        (true, _, _) => "values changed from the template",
        (false, true, true) => "values cited from a drawing, its revision and page recorded",
        (false, true, false) => {
            "values cited from a drawing; its revision and page are not recorded"
        }
        (false, false, _) => "estimated template defaults, not from a drawing",
    }
}

/// A row's second line: family, source, and cited or estimated values.
pub fn detail(hit: &TemplateHit, source: &str) -> String {
    let basis = basis(hit);
    let mut line = format!("{} · {source} · {basis}", family_label(hit.family));
    if let Some(code) = &hit.size_code {
        line.push_str(&format!(" · {} / {}", code.imperial, code.metric));
    }
    if !hit.aliases.is_empty() {
        line.push_str(" · ");
        line.push_str(&hit.aliases.join(", "));
    }
    line
}

/// A row citation as one line: document, revision and page.
pub fn citation_text(citation: &RowCitation) -> String {
    let mut text = citation.document.clone();
    if let Some(revision) = &citation.document_revision {
        text.push_str(&format!(" rev. {revision}"));
    }
    if let Some(page) = citation.page {
        text.push_str(&format!(", p. {page}"));
    }
    text
}

/// Where a hit's row comes from, when the engine can take it directly:
/// a project row by reference, a built-in starter row by name, the
/// family's custom entry by family. A folder row needs its bundle read
/// first (`None`).
pub fn direct_source(hit: &TemplateHit) -> Option<RowSource> {
    if hit.is_custom() {
        return Some(RowSource::Custom { family: hit.family });
    }
    if let Some(name) = &hit.starter {
        return Some(RowSource::Starter { name: name.clone() });
    }
    match (hit.row, &hit.source) {
        (Some(row), Source::Project) => Some(RowSource::Project { row }),
        _ => None,
    }
}

/// The folder record a folder row stands for: its source and the row's
/// own revision (`TemplateHit::source_row`).
pub fn folder_pick(hit: &TemplateHit) -> Option<FolderPick> {
    let (Some(row), Source::Library(library)) = (hit.source_row, &hit.source) else {
        return None;
    };
    Some(FolderPick {
        library: *library,
        lineage: row.lineage,
        revision: row.revision,
        name: hit.name.clone(),
    })
}

/// The values a request carries: the edited form, or the row's own (a
/// custom entry always carries them).
fn request_values(hit: &TemplateHit, edited: Option<&PackageForm>) -> Option<PackageForm> {
    match edited {
        Some(form) if *form != hit.values => Some(form.clone()),
        _ if hit.is_custom() => Some(hit.values.clone()),
        _ => None,
    }
}

/// What a primary asks the core to build.
fn template_request(
    row: RowSource,
    hit: &TemplateHit,
    edited: Option<&PackageForm>,
    skeleton: SkeletonHint,
    name: &str,
    prefix: &str,
) -> Box<NewFromTemplate> {
    let prefix = prefix.trim();
    Box::new(NewFromTemplate {
        row,
        values: request_values(hit, edited),
        skeleton,
        name: name.trim().to_owned(),
        designator_prefix: (!prefix.is_empty()).then(|| prefix.to_owned()),
        symbol: None,
        without_fields: false,
    })
}

/// The one `library.new_from_template` the primary sends.
pub fn new_request(
    row: RowSource,
    hit: &TemplateHit,
    edited: Option<&PackageForm>,
    skeleton: SkeletonHint,
    name: &str,
    prefix: &str,
) -> Operation {
    let request = template_request(row, hit, edited, skeleton, name, prefix);
    Operation::Library(LibraryCommand::NewFromTemplate(request))
}

/// The one `library.publish_from_template` that makes a placeable component.
pub fn publish_request(
    row: RowSource,
    hit: &TemplateHit,
    edited: Option<&PackageForm>,
    skeleton: SkeletonHint,
    name: &str,
    prefix: &str,
) -> Operation {
    let request = template_request(row, hit, edited, skeleton, name, prefix);
    Operation::Library(LibraryCommand::PublishFromTemplate(request))
}

/// The request for a row that needs no choices, with the form's defaults.
/// `None` for a family's custom entry, whose values are still to enter, and
/// a folder row, whose bundle is still to read: those take the form.
pub fn ready_request(hit: &TemplateHit) -> Option<Operation> {
    if hit.is_custom() {
        return None;
    }
    let row = direct_source(hit)?;
    let prefix = default_prefix(&hit.values, hit.skeleton, hit.symbol);
    Some(publish_request(
        row,
        hit,
        None,
        hit.skeleton,
        &hit.name,
        prefix,
    ))
}

/// The `library.package_from_template` request for the package scope.
pub fn package_request(
    draft: DefinitionId,
    row: RowSource,
    hit: &TemplateHit,
    edited: Option<&PackageForm>,
    replace: bool,
) -> PackageFromTemplate {
    PackageFromTemplate {
        draft,
        row,
        values: request_values(hit, edited),
        replace,
        // The gallery always matches terminals by number; position is an
        // explicit request option for MCP and CLI callers.
        terminals: TerminalMatch::ByNumber,
        renumber: Vec::new(),
    }
}

/// The `library.save_package_row` of edited values. The saved row is the
/// author's own: no citation, so it reads as estimated until one is added.
pub fn save_request(values: PackageForm, name: &str, aliases: &str) -> Operation {
    let aliases = aliases
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .take(MAX_SAVE_ALIASES)
        .map(str::to_owned)
        .collect();
    Operation::Library(LibraryCommand::SavePackageRow(Box::new(SavePackageRow {
        from: RowFrom::Values { values },
        name: name.trim().to_owned(),
        aliases,
        provenance: Provenance::default(),
    })))
}

/// A package with `form`'s envelope and one terminal per lead (plus the
/// exposed pad), numbered from 1: what a template builds against.
pub fn preview_package(form: &PackageForm) -> PackageDefinition {
    let count = terminal_count(form).min(u64::from(u32::MAX - 1));
    let count = usize::try_from(count).unwrap_or(0);
    let terminals = samples::form_designations(form, count)
        .into_iter()
        .zip(1u32..)
        .map(|(designation, n)| PackageTerminal {
            id: PackageTerminalId(n),
            designation,
        })
        .collect::<Vec<_>>();
    PackageDefinition {
        next_local: u32::try_from(terminals.len())
            .unwrap_or(u32::MAX - 1)
            .saturating_add(1),
        terminals,
        form: form.clone(),
        envelope: form.envelope().unwrap_or([1.0, 1.0, 1.0]),
        ..PackageDefinition::default()
    }
}

/// The pads Create or Apply builds for `form` with the row's cited land
/// overrides `lands` (the core's `samples::template_lands`), or why it
/// does not.
pub fn preview_pads(
    family: FamilyKind,
    form: &PackageForm,
    lands: Option<&LandValues>,
) -> Result<Vec<Pad>, String> {
    let package = preview_package(form);
    let terminals: Vec<PackageTerminalId> = package.terminals.iter().map(|t| t.id).collect();
    samples::template_lands(form, &terminals, lands)
        .ok_or_else(|| format!("These {} values give no lands", family_label(family)))
}

/// The component drafts the engine holds.
fn component_drafts(engine: &Engine) -> Vec<DefinitionId> {
    engine
        .drafts()
        .items
        .iter()
        .filter(|d| matches!(d.definition, Definition::Component(_)))
        .map(|d| d.id)
        .collect()
}

/// The component lineages the project has published.
pub fn published_components(engine: &Engine) -> Vec<DefinitionId> {
    engine
        .project()
        .library
        .components
        .iter()
        .map(|c| c.id)
        .collect()
}

/// The latest revision of a component lineage not in `before`.
pub fn new_component(engine: &Engine, before: &[DefinitionId]) -> Option<ComponentRef> {
    engine
        .project()
        .library
        .components
        .iter()
        .rfind(|c| !before.contains(&c.id))
        .map(|c| ComponentRef::new(c.id, c.revision))
}

/// The search results at one document revision and context.
#[derive(Clone, Debug)]
struct Results {
    revision: u64,
    context: u64,
    query: String,
    report: Result<TemplateReport, String>,
}

/// The preview pads for one family, form and land overrides.
#[derive(Clone, Debug)]
struct PadPreview {
    family: FamilyKind,
    form: PackageForm,
    lands: Option<LandValues>,
    pads: Arc<Result<Vec<Pad>, String>>,
}

/// The package plan for one request at one revision.
#[derive(Clone, Debug)]
struct PlanPreview {
    revision: u64,
    request: PackageFromTemplate,
    plan: Result<PackagePlan, String>,
}

/// What a submit was.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Sent {
    Create {
        name: String,
    },
    /// Created and published for the host to arm placement.
    CreateToPlace,
    Package,
    SaveRow {
        name: String,
    },
}

/// A submitted operation waiting for the engine's answer.
#[derive(Clone, Debug)]
struct Submitted {
    sent: Sent,
    revision: u64,
    frame: u64,
    drafts: Vec<DefinitionId>,
    components: Vec<DefinitionId>,
}

/// What the primary does once a folder row's bundle arrives.
#[derive(Clone, Debug)]
struct Waiting {
    pick: FolderPick,
}

/// The open Save as row… form.
#[derive(Clone, Debug, Default)]
struct SaveForm {
    name: String,
    aliases: String,
}

/// The open sheet.
#[derive(Clone, Debug)]
struct Open {
    scope: Scope,
    /// The Place palette opened it: the primary continues to placement.
    to_place: bool,
    form_rows: Option<FormRows>,
    query: String,
    chip: Chip,
    results: Option<Results>,
    highlighted: usize,
    /// The highlighted row's identity, so a new search keeps it.
    kept: Option<(FamilyKind, String)>,
    /// Edited values of the highlighted row (Edit values is open).
    edited: Option<PackageForm>,
    save: Option<SaveForm>,
    skeleton: Option<SkeletonHint>,
    name: String,
    /// The author typed the name: a new row no longer renames it.
    named: bool,
    prefix: Option<String>,
    focus: bool,
    scroll: bool,
    pads: Option<PadPreview>,
    plan: Option<PlanPreview>,
    submitted: Option<Submitted>,
    waiting: Option<Waiting>,
    refusal: Option<String>,
}

impl Open {
    fn new(scope: Scope, form_rows: Option<FormRows>, query: String) -> Self {
        Self {
            scope,
            to_place: false,
            form_rows,
            query,
            chip: Chip::All,
            results: None,
            highlighted: 0,
            kept: None,
            edited: None,
            save: None,
            skeleton: None,
            name: String::new(),
            named: false,
            prefix: None,
            focus: true,
            scroll: false,
            pads: None,
            plan: None,
            submitted: None,
            waiting: None,
            refusal: None,
        }
    }

    /// The hits the open chip shows, in report order.
    fn shown(&self) -> Vec<&TemplateHit> {
        match self.results.as_ref().map(|r| &r.report) {
            Some(Ok(report)) => report.hits.iter().filter(|h| self.chip.shows(h)).collect(),
            _ => Vec::new(),
        }
    }

    fn hit(&self) -> Option<TemplateHit> {
        self.shown().get(self.highlighted).map(|h| (*h).clone())
    }

    /// The form the preview shows: the edited one, or the row's.
    fn form(&self, hit: &TemplateHit) -> PackageForm {
        self.edited.clone().unwrap_or_else(|| hit.values.clone())
    }

    /// A new highlighted row: its own values, skeleton and name.
    fn moved(&mut self) {
        self.edited = None;
        self.save = None;
        self.skeleton = None;
        self.prefix = None;
        self.refusal = None;
        self.waiting = None;
        self.scroll = true;
        let hit = self.hit();
        self.kept = hit.as_ref().map(|h| (h.family, h.name.clone()));
        if !self.named {
            self.name = hit.map(|h| h.name).unwrap_or_default();
        }
    }
}

/// The sheet's state; closed by default.
#[derive(Default)]
pub struct TemplateGallery {
    open: Option<Open>,
    /// Library folder catalogs and favourites, and their generation.
    context: CatalogContext,
    context_generation: u64,
    /// Folder rows to read for the host.
    reads: Vec<FolderPick>,
    /// Folder bundles read, most recent last.
    bundles: Vec<(FolderPick, Bundle)>,
    /// The component a create made, for the host to open.
    created: Option<DefinitionId>,
    /// The component a create for placement published.
    to_place: Option<ComponentRef>,
}

impl TemplateGallery {
    /// Open for a new component (New component ▾ › From template…).
    pub fn open_new(&mut self, form_rows: Option<FormRows>) {
        self.open = Some(Open::new(Scope::NewComponent, form_rows, String::new()));
    }

    /// Open on the package draft `draft` of the open working set, whose
    /// footprint holds `pads` pads (the Footprint tab's From template, the
    /// 3D tab's Change package…). `family` picks the first chip and
    /// `query` prefills the search.
    pub fn open_package(
        &mut self,
        draft: DefinitionId,
        pads: usize,
        family: Option<FamilyKind>,
        query: String,
        form_rows: Option<FormRows>,
    ) {
        let mut open = Open::new(Scope::Package { draft, pads }, form_rows, query);
        open.chip = family.map_or(Chip::All, Chip::Family);
        self.open = Some(open);
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The scope the open sheet serves.
    pub fn scope(&self) -> Option<&Scope> {
        self.open.as_ref().map(|o| &o.scope)
    }

    /// Library folder catalogs to search with the project, re-read when
    /// `generation` moves (as the Place palette takes them).
    pub fn set_context(&mut self, generation: u64, context: impl FnOnce() -> CatalogContext) {
        if generation != self.context_generation || self.context.sources.is_empty() {
            self.context = context();
            self.context_generation = generation;
        }
    }

    /// Folder rows the host reads off the UI thread
    /// (`library_folder::read_bundle`), then hands to [`Self::bundle_read`].
    pub fn take_reads(&mut self) -> Vec<FolderPick> {
        std::mem::take(&mut self.reads)
    }

    /// A folder row's bundle, read for `pick`. The waiting submit goes out
    /// on the next frame; a failed read shows on the refusal line.
    pub fn bundle_read(&mut self, pick: FolderPick, result: Result<Bundle, String>) {
        match result {
            Ok(bundle) => {
                self.bundles.retain(|(p, _)| *p != pick);
                if self.bundles.len() >= MAX_BUNDLES {
                    self.bundles.remove(0);
                }
                self.bundles.push((pick, bundle));
            }
            Err(error) => {
                if let Some(open) = &mut self.open
                    && open.waiting.as_ref().is_some_and(|w| w.pick == pick)
                {
                    open.waiting = None;
                    open.refusal = Some(error);
                }
            }
        }
    }

    /// Open for a new component on `hit`, a row the search `query` listed.
    /// With `to_place` the primary continues to placement.
    pub fn open_on(
        &mut self,
        hit: &TemplateHit,
        query: String,
        to_place: bool,
        form_rows: Option<FormRows>,
    ) {
        let mut open = Open::new(Scope::NewComponent, form_rows, query);
        open.to_place = to_place;
        open.kept = Some((hit.family, hit.name.clone()));
        open.name.clone_from(&hit.name);
        open.scroll = true;
        self.open = Some(open);
    }

    /// The component a create for placement published, once: the host arms
    /// placement of it.
    pub fn take_to_place(&mut self) -> Option<ComponentRef> {
        self.to_place.take()
    }

    /// The component draft the last create made, once: the host opens it
    /// explicitly in the Component workspace.
    pub fn take_created(&mut self) -> Option<DefinitionId> {
        self.created.take()
    }

    /// The row source of `hit`: direct, or a folder bundle already read.
    fn source_of(&self, hit: &TemplateHit) -> Option<RowSource> {
        direct_source(hit).or_else(|| {
            let pick = folder_pick(hit)?;
            self.bundles
                .iter()
                .find(|(p, _)| *p == pick)
                .map(|(_, bundle)| RowSource::Bundle {
                    bundle: Box::new(bundle.clone()),
                })
        })
    }

    /// Run `library.templates` when the text, the document or the folder
    /// context changed; keeps the highlighted row where it still exists.
    fn refresh(&mut self, engine: &Engine) {
        let Some(open) = &mut self.open else {
            return;
        };
        // New text starts at the new best match, never the old row.
        let retyped = field_changed(open);
        let fresh = !retyped
            && open.results.as_ref().is_some_and(|r| {
                r.revision == engine.revision() && r.context == self.context_generation
            });
        if fresh {
            return;
        }
        let report = match engine.query_with(
            Query::Templates {
                query: open.query.clone(),
                limit: Some(ROWS),
            },
            &self.context,
        ) {
            Ok(QueryResult::Templates(report)) => Ok(report),
            Ok(_) => Err("unexpected report".to_owned()),
            Err(error) => Err(refusal::shown_error(&error)),
        };
        open.results = Some(Results {
            revision: engine.revision(),
            context: self.context_generation,
            query: open.query.clone(),
            report,
        });
        let kept = open.kept.clone().filter(|_| !retyped);
        let index = kept.and_then(|(family, name)| {
            open.shown()
                .iter()
                .position(|h| h.family == family && h.name == name)
        });
        match index {
            Some(i) => open.highlighted = i,
            None => {
                open.highlighted = 0;
                open.moved();
            }
        }
    }

    /// Settle a submitted operation.
    fn settle(&mut self, ctx: &egui::Context, engine: &Engine) {
        let id = egui::Id::unique(GALLERY);
        let Some(open) = &mut self.open else {
            return;
        };
        let Some(sent) = &open.submitted else {
            return;
        };
        if let Some(why) = refusal::claim(ctx, Origin::Dialog(id)) {
            open.submitted = None;
            open.refusal = Some(why);
            return;
        }
        if engine.revision() != sent.revision {
            let sent = sent.clone();
            open.submitted = None;
            match sent.sent {
                Sent::Create { name } => {
                    self.created = component_drafts(engine)
                        .into_iter()
                        .find(|d| !sent.drafts.contains(d));
                    post(
                        ctx,
                        Level::Ok,
                        format!(
                            "Created {name}: component, symbol, package, footprint and binding"
                        ),
                        vec![ToastAction::Undo],
                    );
                    self.open = None;
                }
                Sent::CreateToPlace => {
                    self.to_place = new_component(engine, &sent.components);
                    self.open = None;
                }
                Sent::Package => {
                    post(
                        ctx,
                        Level::Ok,
                        "Package, footprint and model replaced",
                        vec![ToastAction::Undo],
                    );
                    self.open = None;
                }
                Sent::SaveRow { name } => {
                    post(
                        ctx,
                        Level::Ok,
                        format!("Saved {name} as a draft row; publish it to list it here"),
                        vec![ToastAction::Undo],
                    );
                    open.save = None;
                }
            }
            return;
        }
        if ctx.cumulative_frame_nr() > sent.frame.saturating_add(ANSWER_FRAMES) {
            open.submitted = None;
        }
    }

    /// Show the sheet when open; the operation to apply when the user
    /// creates, applies or saves a row.
    pub fn show(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        self.open.as_ref()?;
        self.settle(ctx, engine);
        self.refresh(engine);
        let open = self.open.as_mut()?;
        let id = egui::Id::unique(GALLERY);
        let query_id = id.with("query");
        // Keys: the rows own ↑↓⏎; while values are edited only Esc, which
        // folds them first.
        let mut keys = kit::SearchKeys::default();
        if open.edited.is_some() || open.save.is_some() {
            keys.escape =
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        } else {
            keys = kit::search_keys(ctx);
        }
        if keys.escape {
            if open.save.is_some() {
                open.save = None;
            } else if open.edited.is_some() {
                open.edited = None;
            } else {
                self.open = None;
                return None;
            }
        }
        let count = open.shown().len();
        if keys.down && count > 0 {
            open.highlighted = (open.highlighted + 1).min(count - 1);
            open.moved();
        }
        if keys.up && open.highlighted > 0 {
            open.highlighted -= 1;
            open.moved();
        }
        open.highlighted = open.highlighted.min(count.saturating_sub(1));
        let search_focused = ctx.memory(|m| m.has_focus(query_id));
        if open.edited.is_none()
            && open.form_rows.is_some()
            && search_focused
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab))
            && let Some(hit) = open.hit()
        {
            open.edited = Some(hit.values.clone());
        }
        let hit = open.hit();
        let source = hit.as_ref().and_then(|h| self.source_of(h));
        // A folder row's bundle arrived for the waiting primary: send it.
        let arrived = self
            .open
            .as_ref()
            .is_some_and(|o| o.waiting.is_some() && o.submitted.is_none())
            && source.is_some();
        if arrived && let Some(open) = self.open.as_mut() {
            open.waiting = None;
        }
        let mut primary = keys.enter || arrived;
        let mut save = false;
        let action = self.sheet(
            ctx,
            engine,
            hit.as_ref(),
            source.as_ref(),
            &mut primary,
            &mut save,
        );
        let open = self.open.as_mut()?;
        if action {
            self.open = None;
            return None;
        }
        let hit = hit?;
        if save {
            let form = open.form(&hit);
            let name = open
                .save
                .as_ref()
                .map(|s| s.name.clone())
                .unwrap_or_default();
            let aliases = open
                .save
                .as_ref()
                .map(|s| s.aliases.clone())
                .unwrap_or_default();
            if !name.trim().is_empty() && form.range_problems().is_empty() {
                return self.submit(
                    ctx,
                    engine,
                    Sent::SaveRow {
                        name: name.trim().to_owned(),
                    },
                    save_request(form, &name, &aliases),
                );
            }
        }
        if !primary || self.blocked(&hit).is_some() {
            return None;
        }
        let open = self.open.as_mut()?;
        let Some(source) = source else {
            // A folder row: read its bundle first; the request goes out
            // once the host hands it back.
            if let Some(pick) = folder_pick(&hit) {
                if open.waiting.is_none() {
                    self.reads.push(pick.clone());
                }
                open.waiting = Some(Waiting { pick });
            }
            return None;
        };
        open.waiting = None;
        let edited = open.edited.clone();
        match open.scope.clone() {
            Scope::NewComponent => {
                let skeleton = open.skeleton.unwrap_or(hit.skeleton);
                let prefix = open.prefix.clone().unwrap_or_else(|| {
                    default_prefix(&hit.values, skeleton, hit.symbol).to_owned()
                });
                let name = open.name.trim().to_owned();
                if open.to_place {
                    let operation =
                        publish_request(source, &hit, edited.as_ref(), skeleton, &name, &prefix);
                    return self.submit(ctx, engine, Sent::CreateToPlace, operation);
                }
                let operation =
                    new_request(source, &hit, edited.as_ref(), skeleton, &name, &prefix);
                self.submit(ctx, engine, Sent::Create { name }, operation)
            }
            Scope::Package { draft, pads } => {
                let request = package_request(draft, source, &hit, edited.as_ref(), pads > 0);
                let operation =
                    Operation::Library(LibraryCommand::PackageFromTemplate(Box::new(request)));
                self.submit(ctx, engine, Sent::Package, operation)
            }
        }
    }

    /// Record and return a submit.
    fn submit(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        sent: Sent,
        operation: Operation,
    ) -> Option<Operation> {
        let open = self.open.as_mut()?;
        refusal::mark(ctx, Origin::Dialog(egui::Id::unique(GALLERY)));
        open.refusal = None;
        open.submitted = Some(Submitted {
            sent,
            revision: engine.revision(),
            frame: ctx.cumulative_frame_nr(),
            drafts: component_drafts(engine),
            components: published_components(engine),
        });
        Some(operation)
    }

    /// Why the primary cannot act on `hit` now, if it cannot.
    fn blocked(&self, hit: &TemplateHit) -> Option<String> {
        let open = self.open.as_ref()?;
        if open.submitted.is_some() {
            return Some("Waiting for the engine…".into());
        }
        if open.waiting.is_some() {
            return Some("Reading the library row…".into());
        }
        let problems = open.form(hit).range_problems();
        if let Some(first) = problems.first() {
            return Some(first.clone());
        }
        if open.scope == Scope::NewComponent && open.name.trim().is_empty() {
            return Some("Name the component".into());
        }
        None
    }

    /// The package plan of the highlighted row (package scope), cached by
    /// revision and request.
    fn plan(
        &mut self,
        engine: &Engine,
        request: Option<PackageFromTemplate>,
    ) -> Option<Result<PackagePlan, String>> {
        let open = self.open.as_mut()?;
        let request = request?;
        let fresh = open
            .plan
            .as_ref()
            .is_some_and(|p| p.revision == engine.revision() && p.request == request);
        if !fresh {
            let plan = match engine.query(Query::PackagePlan {
                request: Box::new(request.clone()),
            }) {
                Ok(QueryResult::PackagePlan(plan)) => Ok(plan),
                Ok(_) => Err("unexpected report".to_owned()),
                Err(error) => Err(refusal::shown_error(&error)),
            };
            open.plan = Some(PlanPreview {
                revision: engine.revision(),
                request,
                plan,
            });
        }
        open.plan.as_ref().map(|p| p.plan.clone())
    }

    /// The pads of the preview, rebuilt only when the values change.
    fn pads(
        &mut self,
        family: FamilyKind,
        form: &PackageForm,
        lands: Option<&LandValues>,
    ) -> Option<Arc<Result<Vec<Pad>, String>>> {
        let open = self.open.as_mut()?;
        let fresh = open
            .pads
            .as_ref()
            .is_some_and(|p| p.family == family && p.form == *form && p.lands.as_ref() == lands);
        if !fresh {
            open.pads = Some(PadPreview {
                family,
                form: form.clone(),
                lands: lands.cloned(),
                pads: Arc::new(preview_pads(family, form, lands)),
            });
        }
        open.pads.as_ref().map(|p| Arc::clone(&p.pads))
    }

    /// Draw the sheet. Returns whether the user closed it; sets `primary`
    /// and `save` when those were clicked.
    fn sheet(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        hit: Option<&TemplateHit>,
        source: Option<&RowSource>,
        primary: &mut bool,
        save: &mut bool,
    ) -> bool {
        let t = Tokens::get();
        let area = ctx.content_rect();
        let width = (t.form_card_width * 3.0)
            .min(area.width() - t.space_16)
            .max(t.form_card_width);
        let top = pos2(area.center().x - width / 2.0, area.top() + t.space_24 * 2.0);
        let list_height = (area.height() - t.space_24 * 9.0).max(kit::search_row_height() * 4.0);
        let form = hit.map(|h| {
            self.open
                .as_ref()
                .map_or_else(|| h.values.clone(), |o| o.form(h))
        });
        let pads = match (hit, &form) {
            // Edited values are a custom package: the row's land
            // overrides no longer apply (as the core builds it).
            (Some(h), Some(f)) => {
                let lands = h.lands.as_ref().filter(|_| *f == h.values);
                self.pads(h.family, f, lands)
            }
            _ => None,
        };
        let plan_request = match (self.open.as_ref().map(|o| &o.scope), hit, source) {
            (Some(Scope::Package { draft, pads }), Some(h), Some(s)) => Some(package_request(
                *draft,
                s.clone(),
                h,
                self.open.as_ref().and_then(|o| o.edited.as_ref()),
                *pads > 0,
            )),
            _ => None,
        };
        let plan = self.plan(engine, plan_request);
        let blocked = hit.and_then(|h| self.blocked(h));
        let names: Vec<(pcbcraft_core::LibraryId, String)> = self
            .context
            .sources
            .iter()
            .map(|s| (s.library, s.name.clone()))
            .collect();
        let Some(open) = self.open.as_mut() else {
            return true;
        };
        let footer = if open.edited.is_some() || open.save.is_some() {
            KEYS_EDIT
        } else if open.to_place {
            KEYS_PLACE
        } else if matches!(open.scope, Scope::NewComponent) {
            KEYS_NEW
        } else {
            KEYS_PACKAGE
        };
        let query_id = egui::Id::unique(GALLERY).with("query");
        let mut closed = false;
        let focus = open.focus;
        let mut focused = false;
        let mut picked = None;
        let mut moved_chip = false;
        let scroll = std::mem::take(&mut open.scroll);
        let source_name = |h: &TemplateHit| {
            source_label(h, |id| {
                names
                    .iter()
                    .find(|(l, _)| *l == id)
                    .map_or_else(|| "Library".to_owned(), |(_, n)| n.clone())
            })
        };
        kit::search_sheet(ctx, GALLERY, top, width, footer, |ui| {
            let invalid = matches!(open.results.as_ref().map(|r| &r.report), Some(Err(_)));
            let field = kit::search_field(
                ui,
                query_id,
                &mut open.query,
                "Find a package: soic pins:14, qfn body:5x5 pitch:0.5, pkg:0402",
                invalid,
                focus,
                MAX_QUERY,
            );
            focused = field.has_focus();
            ui.add_space(t.space_4);
            ui.horizontal_wrapped(|ui| {
                for chip in chips() {
                    let style = if chip == open.chip {
                        Button::SmallSelected
                    } else {
                        Button::Small
                    };
                    if ui_kit::button(ui, chip.label(), style).clicked() && chip != open.chip {
                        open.chip = chip;
                        moved_chip = true;
                    }
                }
            });
            ui.add_space(t.space_4);
            let hits: Vec<TemplateHit> = open.shown().into_iter().cloned().collect();
            let columns = ui.available_width();
            let left = (columns * 0.48).max(0.0);
            let right = (columns - left - t.space_12).max(0.0);
            ui.allocate_ui_with_layout(
                vec2(columns, list_height),
                Layout::left_to_right(Align::Min),
                |ui| {
                    ui.allocate_ui_with_layout(
                        vec2(left, list_height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(left);
                            match open.results.as_ref().map(|r| &r.report) {
                                Some(Err(error)) => kit::search_message(ui, error, true),
                                Some(Ok(report)) if hits.is_empty() => kit::search_message(
                                    ui,
                                    if report.hits.is_empty() {
                                        "No template matches"
                                    } else {
                                        "No template of this family matches"
                                    },
                                    false,
                                ),
                                Some(Ok(report)) => {
                                    let more = if report.more > 0 {
                                        format!("{} more: narrow the search", report.more)
                                    } else {
                                        String::new()
                                    };
                                    kit::search_heading(ui, open.chip.label(), hits.len());
                                    if !more.is_empty() {
                                        kit::note(ui, &more);
                                    }
                                    egui::ScrollArea::vertical()
                                        .id_salt(("template-gallery-rows", open.chip.label()))
                                        .max_height(list_height)
                                        .auto_shrink([false, true])
                                        .show(ui, |ui| {
                                            for (i, h) in hits.iter().enumerate() {
                                                let line = detail(h, &source_name(h));
                                                let badges = [kit::SearchBadge {
                                                    label: if h.is_custom() {
                                                        "custom"
                                                    } else if h.cited && h.citation_checked {
                                                        "cited"
                                                    } else if h.cited {
                                                        "cited?"
                                                    } else {
                                                        "est."
                                                    },
                                                    present: h.cited || h.is_custom(),
                                                    tip: badge_tip(h),
                                                }];
                                                let row = kit::SearchRow {
                                                    icon: "component",
                                                    label: &h.name,
                                                    query: &open.query,
                                                    detail: &line,
                                                    badges: &badges,
                                                    highlighted: i == open.highlighted,
                                                };
                                                let response = kit::search_row(
                                                    ui,
                                                    egui::Id::unique(GALLERY).with(("row", i)),
                                                    &row,
                                                );
                                                if scroll && i == open.highlighted {
                                                    response.scroll_to_me(None);
                                                }
                                                if response.clicked() {
                                                    picked = Some(i);
                                                }
                                                if response.double_clicked() {
                                                    *primary = true;
                                                }
                                            }
                                        });
                                }
                                None => {}
                            }
                        },
                    );
                    ui.add_space(t.space_12);
                    ui.allocate_ui_with_layout(
                        vec2(right, list_height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(right);
                            egui::ScrollArea::vertical()
                                .id_salt("template-gallery-preview")
                                .max_height(list_height)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    if let (Some(h), Some(f)) = (hit, form.as_ref()) {
                                        if preview(
                                            ui,
                                            open,
                                            h,
                                            f,
                                            pads.as_deref(),
                                            plan.as_ref(),
                                            blocked.as_deref(),
                                            primary,
                                            save,
                                        ) {
                                            closed = true;
                                        }
                                    } else {
                                        kit::note(ui, "Pick a row to see its package");
                                    }
                                });
                        },
                    );
                },
            );
        });
        if focused {
            open.focus = false;
        }
        if field_changed(open) || moved_chip {
            open.highlighted = 0;
            open.moved();
        }
        if let Some(i) = picked
            && i != open.highlighted
        {
            open.highlighted = i;
            open.moved();
        }
        closed
    }
}

/// Whether the text moved since the last search (the next refresh runs
/// it); the highlight then starts at the best match.
fn field_changed(open: &Open) -> bool {
    open.results.as_ref().is_some_and(|r| r.query != open.query)
}

/// The preview column: title, citation, pads and top view, values, the
/// scope's choices and its primary. Returns whether Cancel was clicked.
#[allow(clippy::too_many_arguments)]
fn preview(
    ui: &mut Ui,
    open: &mut Open,
    hit: &TemplateHit,
    form: &PackageForm,
    pads: Option<&Result<Vec<Pad>, String>>,
    plan: Option<&Result<PackagePlan, String>>,
    blocked: Option<&str>,
    primary: &mut bool,
    save: &mut bool,
) -> bool {
    let t = Tokens::get();
    let edited = open.edited.as_ref().is_some_and(|e| *e != hit.values);
    kit::status(
        ui,
        &if edited {
            format!("{} (custom values)", hit.name)
        } else {
            hit.name.clone()
        },
        Tone::Note,
    );
    kit::note(
        ui,
        &match (hit.is_custom(), hit.cited, edited) {
            (_, _, true) => "Custom values: not from a drawing".to_owned(),
            (true, _, _) => format!(
                "{} custom entry: enter the values",
                family_label(hit.family)
            ),
            (false, true, _) => match &hit.citation {
                Some(citation) if citation.checked() => {
                    format!("Values cited from {}", citation_text(citation))
                }
                Some(citation) => format!(
                    "Values cited from {} (unverified: no drawing revision or page recorded)",
                    citation_text(citation)
                ),
                None => "Values cited from a public outline drawing".to_owned(),
            },
            (false, false, _) => "Values estimated: no outline drawing cited".to_owned(),
        },
    );
    ui.add_space(t.space_4);
    let side = ((ui.available_width() - t.space_8) / 2.0).max(t.space_24);
    let height = (side * 0.75).min(t.space_24 * 6.0);
    ui.horizontal(|ui| {
        pads_view(ui, vec2(side, height), pads);
        top_view(ui, vec2(side, height), form);
    });
    ui.add_space(t.space_4);
    for (label, value) in summary(hit, form) {
        kit::value_row(ui, label, &value);
    }
    kit::value_row(
        ui,
        "Lands",
        if hit.lands_cited && !edited {
            "cited land pattern"
        } else {
            "estimated template defaults"
        },
    );
    if let Some(Ok(pads)) = pads {
        let lands = pads
            .iter()
            .filter(|p| matches!(p.kind, PadKind::Smd | PadKind::PlatedThrough))
            .count();
        kit::value_row(ui, "Pads", &lands.to_string());
    }
    // Edit values: the family's typed rows, prefilled, with the core's
    // range problems inline.
    if let Some(rows) = open.form_rows {
        match &mut open.edited {
            None => {
                if ui_kit::button(ui, "Edit values", Button::Ghost)
                    .on_hover_text("Change this row's values for a custom package (⇥)")
                    .clicked()
                {
                    open.edited = Some(hit.values.clone());
                }
            }
            Some(values) => {
                kit::section(ui, (GALLERY, "values"), "Values", true, None, |ui| {
                    rows(ui, values);
                    for problem in values.range_problems() {
                        kit::refusal(ui, &problem);
                    }
                });
                ui.horizontal(|ui| {
                    if ui_kit::button(ui, "Use the row's values", Button::Ghost).clicked() {
                        open.edited = None;
                        open.save = None;
                    }
                    if open.save.is_none()
                        && ui_kit::button(ui, "Save as row…", Button::Ghost)
                            .on_hover_text("Keep these values as a template row of this project")
                            .clicked()
                    {
                        open.save = Some(SaveForm {
                            name: format!("{} custom", hit.name),
                            aliases: String::new(),
                        });
                    }
                });
            }
        }
    }
    if let Some(form) = &mut open.save {
        kit::section(ui, (GALLERY, "save"), "Save as row", true, None, |ui| {
            bounded_row(ui, "Name", &mut form.name, MAX_LABEL);
            bounded_row(ui, "Aliases", &mut form.aliases, MAX_LABEL);
            kit::note(
                ui,
                "Aliases are search text, comma separated; the row reads as estimated until a drawing is cited",
            );
            kit::actions(ui, |ui| {
                if kit::primary(ui, "Save row", !form.name.trim().is_empty()).clicked() {
                    *save = true;
                }
            });
        });
    }
    kit::rule(ui);
    let mut cancel = false;
    match open.scope.clone() {
        Scope::NewComponent => {
            let mut skeleton = open.skeleton.unwrap_or(hit.skeleton);
            if kit::choice_row(ui, "Start as", &mut skeleton, &skeletons()) {
                open.skeleton = Some(skeleton);
                open.prefix = None;
            }
            let mut prefix = open
                .prefix
                .clone()
                .unwrap_or_else(|| default_prefix(&hit.values, skeleton, hit.symbol).to_owned());
            let changed = if matches!(skeleton, SkeletonHint::TwoTerminal { .. }) {
                let options: Vec<(String, String)> = TWO_TERMINAL_PREFIXES
                    .iter()
                    .map(|p| ((*p).to_owned(), (*p).to_owned()))
                    .collect();
                kit::choice_row(ui, "Designator", &mut prefix, &options)
            } else {
                bounded_row(ui, "Designator", &mut prefix, MAX_DESIGNATOR_PREFIX).changed()
            };
            // The symbol the build draws, as the core chooses it.
            if let Some(kind) =
                samples::choose_symbol(skeleton, &hit.values, &prefix, None, hit.symbol)
            {
                kit::value_row(ui, "Symbol", kind.label());
            }
            if changed {
                open.prefix = Some(prefix);
            }
            if bounded_row(ui, "Name", &mut open.name, MAX_LABEL).changed() {
                open.named = true;
            }
        }
        Scope::Package { pads, .. } => {
            match plan {
                Some(Ok(plan)) => {
                    kit::value_row(
                        ui,
                        "Terminals",
                        &format!("{} → {}", plan.terminals_before, plan.terminals_after),
                    );
                    kit::value_row(ui, "Pads after", &plan.pads_after.to_string());
                    kit::value_row(
                        ui,
                        "Matched",
                        &format!(
                            "{}: {} kept, {} new",
                            plan.terminals.strategy.label(),
                            plan.terminals.kept,
                            plan.terminals.added
                        ),
                    );
                    if plan.footprint_created {
                        kit::note(
                            ui,
                            if plan.footprint_bound.is_some() {
                                "Creates the footprint draft and lists it in the binding"
                            } else {
                                "Creates the footprint draft"
                            },
                        );
                    }
                    if plan.terminals_before != plan.terminals_after {
                        kit::note(
                            ui,
                            "The terminal count changes: map the new terminals afterwards; nothing is guessed",
                        );
                    }
                    for finding in &plan.findings {
                        kit::note(ui, &finding.message);
                    }
                }
                Some(Err(error)) => {
                    kit::refusal(ui, error);
                }
                None => {
                    if folder_pick(hit).is_some() {
                        kit::note(ui, "The change is previewed once the library row is read");
                    }
                }
            }
            if pads > 0 {
                kit::note(
                    ui,
                    &format!(
                        "Replaces the package, the footprint's {pads} pads and the model; one undo step"
                    ),
                );
            }
        }
    }
    if let Some(why) = &open.refusal {
        kit::refusal(ui, why);
    }
    let label = match &open.scope {
        Scope::NewComponent if open.to_place => "Create and place",
        Scope::NewComponent => "Create component",
        Scope::Package { pads: 0, .. } => "Use package",
        Scope::Package { .. } => "Replace package",
    };
    kit::actions(ui, |ui| {
        let response = kit::primary(ui, label, blocked.is_none());
        let response = match blocked {
            Some(why) => response.on_disabled_hover_text(why),
            None => response,
        };
        if response.clicked() {
            *primary = true;
        }
        if ui_kit::button(ui, "Cancel", Button::Normal).clicked() {
            cancel = true;
        }
    });
    if let Some(why) = blocked {
        kit::note(ui, why);
    }
    cancel
}

/// The pads the template would build, fitted into `size`.
fn pads_view(ui: &mut Ui, size: egui::Vec2, pads: Option<&Result<Vec<Pad>, String>>) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    frame(ui, rect, "Pads");
    match pads {
        Some(Ok(pads)) if !pads.is_empty() => {
            let bounds = pads
                .iter()
                .map(Pad::bounds)
                .fold(None, |acc: Option<[f64; 4]>, b| {
                    Some(match acc {
                        None => [b.min[0], b.min[1], b.max[0], b.max[1]],
                        Some(a) => [
                            a[0].min(b.min[0]),
                            a[1].min(b.min[1]),
                            a[2].max(b.max[0]),
                            a[3].max(b.max[1]),
                        ],
                    })
                });
            let Some([x0, y0, x1, y1]) = bounds else {
                return;
            };
            let view = fit(rect.shrink(t.space_16), [x0, y0], [x1, y1]);
            let painter = ui.painter().with_clip_rect(rect);
            for pad in pads {
                symbol_view::paint_shape(
                    &painter,
                    view,
                    &pad.land(0.05),
                    Some(t.accent.gamma_multiply(0.35)),
                    Some(Stroke::new(1.0, t.accent)),
                );
            }
        }
        Some(Err(why)) => center_note(ui, rect, why),
        _ => center_note(ui, rect, "No pads"),
    }
}

/// The body envelope from above with its lead points, pin 1 marked.
fn top_view(ui: &mut Ui, size: egui::Vec2, form: &PackageForm) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    frame(ui, rect, "Top view");
    let Some([l, w, _]) = form.envelope() else {
        center_note(ui, rect, "No typed body");
        return;
    };
    let leads = form.lead_points();
    let mut min = [-l / 2.0, -w / 2.0];
    let mut max = [l / 2.0, w / 2.0];
    for p in &leads {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    let view = fit(rect.shrink(t.space_16), min, max);
    let painter = ui.painter().with_clip_rect(rect);
    let body = Rect::from_two_pos(
        view.screen([-l / 2.0, -w / 2.0]),
        view.screen([l / 2.0, w / 2.0]),
    );
    painter.rect(
        body,
        2.0,
        t.form_dim.gamma_multiply(0.25),
        Stroke::new(1.0, t.form_dim),
        egui::StrokeKind::Inside,
    );
    for (i, p) in leads.iter().enumerate() {
        let at = view.screen([p[0], p[1]]);
        let radius = if i == 0 { 3.0 } else { 2.0 };
        painter.circle_filled(at, radius, if i == 0 { t.accent } else { t.form_ink });
    }
}

/// A preview tile's frame and caption.
fn frame(ui: &Ui, rect: Rect, caption: &str) {
    let t = Tokens::get();
    t.container_rect(
        ui.painter(),
        rect,
        t.control_radius,
        t.dialog_footer,
        t.dialog_rule,
    );
    ui.painter().text(
        rect.left_top() + vec2(t.space_6, t.space_4),
        egui::Align2::LEFT_TOP,
        caption,
        egui::FontId::proportional(t.font_small),
        t.form_dim,
    );
}

fn center_note(ui: &Ui, rect: Rect, text: &str) {
    let t = Tokens::get();
    let galley = ui.painter().layout(
        text.to_owned(),
        egui::FontId::proportional(t.font_small),
        t.form_dim,
        (rect.width() - t.space_12).max(0.0),
    );
    let at = rect.center() - galley.size() / 2.0;
    ui.painter().galley(at, galley, t.form_dim);
}

/// A view that fits the millimetre box `min`..`max` into `rect`, centred.
fn fit(rect: Rect, min: [f64; 2], max: [f64; 2]) -> View {
    let w = (max[0] - min[0]).max(0.1) as f32;
    let h = (max[1] - min[1]).max(0.1) as f32;
    let scale = (rect.width() / w).min(rect.height() / h).max(0.01);
    let cx = ((min[0] + max[0]) / 2.0) as f32;
    let cy = ((min[1] + max[1]) / 2.0) as f32;
    View {
        origin: pos2(rect.center().x - cx * scale, rect.center().y + cy * scale),
        scale,
    }
}

/// A text row whose value is cut to `limit` bytes on a character boundary.
fn bounded_row(ui: &mut Ui, label: &str, value: &mut String, limit: usize) -> egui::Response {
    let response = kit::text_row(ui, label, value);
    if value.len() > limit {
        let mut end = limit;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refusal::Refused;
    use pcbcraft_core::library::{ChipForm, DefinitionMeta, LeadSides, NoLeadForm, SourceRevision};
    use pcbcraft_core::{LibraryId, PackageRowRef, Revision, SourceLineageId};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    use pcbcraft_core::library::ChipKind;

    fn chip(kind: ChipKind) -> PackageForm {
        PackageForm::Chip(Box::new(ChipForm::new(kind, [1.6, 0.8, 0.45])))
    }

    fn qfn16() -> PackageForm {
        PackageForm::NoLead(Box::new(NoLeadForm {
            sides: LeadSides::Four,
            terminals_per_side: [4, 4],
            pitch: 0.5,
            body_length: 3.0,
            body_width: 3.0,
            body_height: 0.9,
            terminal_length: 0.4,
            terminal_width: 0.25,
            exposed_pad: Some([1.7, 1.7]),
        }))
    }

    fn hit(family: FamilyKind, name: &str, values: PackageForm) -> TemplateHit {
        let skeleton = FamilyKind::default_skeleton(&values);
        TemplateHit {
            family,
            row: None,
            source: Source::Project,
            name: name.into(),
            aliases: Vec::new(),
            values,
            size_code: None,
            cited: false,
            lands_cited: false,
            source_row: None,
            skeleton,
            symbol: None,
            citation: None,
            shape: None,
            starter: None,
            lands: None,
            citation_checked: false,
            tier: pcbcraft_core::catalog::MatchTier::Substring,
        }
    }

    fn project_row(family: FamilyKind, name: &str, values: PackageForm, id: u64) -> TemplateHit {
        TemplateHit {
            row: Some(PackageRowRef::new(DefinitionId(id), Revision(1))),
            cited: true,
            ..hit(family, name, values)
        }
    }

    /// Hand the sheet a search result directly: which rows
    /// `library.templates` returns is the core's concern (slice B1).
    fn results(gallery: &mut TemplateGallery, engine: &Engine, hits: Vec<TemplateHit>) {
        let context = gallery.context_generation;
        if let Some(open) = gallery.open.as_mut() {
            open.results = Some(Results {
                revision: engine.revision(),
                context,
                query: open.query.clone(),
                report: Ok(TemplateReport {
                    hits,
                    more: 0,
                    chips: Vec::new(),
                }),
            });
            open.highlighted = 0;
            open.moved();
        }
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

    fn frame(
        ctx: &egui::Context,
        gallery: &mut TemplateGallery,
        engine: &Engine,
        events: Vec<egui::Event>,
    ) -> Option<Operation> {
        let mut out = None;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 900.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| out = gallery.show(ui.ctx(), engine));
        output.textures_delta.clear();
        out
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        ctx
    }

    /// Every family has a chip; a chip shows its family's rows, and This
    /// project only rows the project holds (never custom entries).
    #[test]
    fn chips_filter_by_family_and_by_source() {
        let all = chips();
        for family in FamilyKind::ALL {
            assert!(all.contains(&Chip::Family(family)), "{family:?}");
            assert!(!family_label(family).is_empty());
        }
        let custom = hit(FamilyKind::Chip, "Chip (custom)", chip(ChipKind::Resistor));
        let row = project_row(FamilyKind::NoLead, "QFN-16 3×3", qfn16(), 7);
        assert!(Chip::All.shows(&custom) && Chip::All.shows(&row));
        assert!(Chip::Family(FamilyKind::Chip).shows(&custom));
        assert!(!Chip::Family(FamilyKind::Chip).shows(&row));
        assert!(Chip::Project.shows(&row));
        assert!(
            !Chip::Project.shows(&custom),
            "a custom entry is no project row"
        );
    }

    /// A project row goes by reference, a custom entry by family with its
    /// values, and a folder row names the folder record the host reads by
    /// its own source revision.
    #[test]
    fn row_sources_are_explicit() {
        let custom = hit(FamilyKind::Chip, "Chip (custom)", chip(ChipKind::Resistor));
        assert_eq!(
            direct_source(&custom),
            Some(RowSource::Custom {
                family: FamilyKind::Chip
            })
        );
        let row = project_row(FamilyKind::NoLead, "QFN-16", qfn16(), 7);
        assert_eq!(
            direct_source(&row),
            Some(RowSource::Project {
                row: PackageRowRef::new(DefinitionId(7), Revision(1))
            })
        );
        let library = LibraryId(0xabc);
        let lineage = SourceLineageId(0xdef);
        let folder = TemplateHit {
            source: Source::Library(library),
            row: None,
            source_row: Some(SourceRevision {
                lineage,
                revision: Revision(1),
            }),
            ..project_row(FamilyKind::NoLead, "QFN-16", qfn16(), 3)
        };
        assert!(!folder.is_custom());
        assert_eq!(
            direct_source(&folder),
            None,
            "a folder row needs its bundle"
        );
        assert_eq!(
            folder_pick(&folder),
            Some(FolderPick {
                library,
                lineage,
                revision: Revision(1),
                name: "QFN-16".into(),
            })
        );
        assert_eq!(folder_pick(&row), None, "a project row is no folder item");
    }

    /// Edited values travel only when they differ from the row's; a custom
    /// entry always carries its values; the prefix is trimmed and an empty
    /// one is left to the skeleton.
    #[test]
    fn requests_carry_exactly_what_changed() -> TestResult {
        let row = project_row(FamilyKind::Chip, "R0603", chip(ChipKind::Resistor), 4);
        let source = direct_source(&row).ok_or("source")?;
        let unchanged = new_request(
            source.clone(),
            &row,
            Some(&row.values),
            SkeletonHint::TwoTerminal { polarised: false },
            " R0603 resistor ",
            " R ",
        );
        let Operation::Library(LibraryCommand::NewFromTemplate(request)) = unchanged else {
            return Err("not new_from_template".into());
        };
        assert_eq!(request.values, None);
        assert_eq!(request.name, "R0603 resistor");
        assert_eq!(request.designator_prefix.as_deref(), Some("R"));
        let mut edited = row.values.clone();
        if let PackageForm::Chip(c) = &mut edited {
            c.height = 0.55;
        }
        let changed = new_request(source, &row, Some(&edited), SkeletonHint::Blank, "R", "  ");
        let Operation::Library(LibraryCommand::NewFromTemplate(request)) = changed else {
            return Err("not new_from_template".into());
        };
        assert_eq!(request.values, Some(edited));
        assert_eq!(request.designator_prefix, None);
        let custom = hit(FamilyKind::NoLead, "QFN (custom)", qfn16());
        let plan = package_request(
            DefinitionId(9),
            RowSource::Custom {
                family: FamilyKind::NoLead,
            },
            &custom,
            None,
            true,
        );
        assert_eq!(plan.values, Some(qfn16()), "a custom entry carries values");
        assert!(plan.replace);
        let Operation::Library(LibraryCommand::SavePackageRow(save)) =
            save_request(qfn16(), " QFN-16 mine ", "QFN16, , VQFN-16 ")
        else {
            return Err("not save_package_row".into());
        };
        assert_eq!(save.name, "QFN-16 mine");
        assert_eq!(save.aliases, vec!["QFN16".to_owned(), "VQFN-16".to_owned()]);
        assert_eq!(save.from, RowFrom::Values { values: qfn16() });
        Ok(())
    }

    /// The preview builds the family's template on a package of the row's
    /// form: a chip lands two pads, a QFN-16 with exposed pad sixteen
    /// perimeter pads plus the exposed pad, one per terminal.
    #[test]
    fn previews_build_one_pad_per_terminal() -> Result<(), String> {
        let chip_pads = preview_pads(FamilyKind::Chip, &chip(ChipKind::Capacitor), None)?;
        let landing = |pads: &[Pad]| {
            pads.iter()
                .filter(|p| matches!(p.kind, PadKind::Smd | PadKind::PlatedThrough))
                .count()
        };
        assert_eq!(landing(&chip_pads), 2);
        let qfn = qfn16();
        let package = preview_package(&qfn);
        assert_eq!(package.terminals.len() as u64, terminal_count(&qfn));
        assert_eq!(
            package.terminals.last().map(|t| t.designation.as_str()),
            Some("EP")
        );
        let qfn_pads = preview_pads(FamilyKind::NoLead, &qfn, None)?;
        assert_eq!(landing(&qfn_pads), package.terminals.len());
        Ok(())
    }

    #[test]
    fn skeletons_and_prefixes_follow_the_family_only() {
        let resistor = chip(ChipKind::Resistor);
        let skeleton = FamilyKind::default_skeleton(&resistor);
        assert_eq!(skeleton, SkeletonHint::TwoTerminal { polarised: false });
        assert_eq!(default_prefix(&resistor, skeleton, None), "R");
        assert_eq!(
            default_prefix(&chip(ChipKind::Capacitor), skeleton, None),
            "C"
        );
        assert_eq!(FamilyKind::default_skeleton(&qfn16()), SkeletonHint::IcBox);
        assert_eq!(default_prefix(&qfn16(), SkeletonHint::IcBox, None), "U");
        assert_eq!(default_prefix(&qfn16(), SkeletonHint::Connector, None), "J");
        let summary = summary(&hit(FamilyKind::NoLead, "QFN", qfn16()), &qfn16());
        let terminals = summary
            .iter()
            .find(|(l, _)| *l == "Terminals")
            .map(|(_, v)| v.as_str());
        assert_eq!(terminals, Some("16 + 1  (pitch 0.5 mm)"));
    }

    /// A query `library.templates` refuses (a component-only filter) shows
    /// as the sheet's message and sends nothing; the plain search lists
    /// every family's custom entry.
    #[test]
    fn a_refused_search_is_a_message() -> TestResult {
        let ctx = context();
        let engine = Engine::demo()?;
        let mut gallery = TemplateGallery::default();
        gallery.open_new(None);
        assert!(frame(&ctx, &mut gallery, &engine, vec![]).is_none());
        let open = gallery.open.as_ref().ok_or("open")?;
        let Some(Ok(report)) = open.results.as_ref().map(|r| &r.report) else {
            return Err("the empty search lists entries".into());
        };
        for family in FamilyKind::ALL {
            assert!(
                report
                    .hits
                    .iter()
                    .any(|h| h.family == family && h.is_custom()),
                "{family:?} has a custom entry"
            );
        }
        if let Some(open) = gallery.open.as_mut() {
            open.query = "kind:component".into();
        }
        assert!(frame(&ctx, &mut gallery, &engine, vec![]).is_none());
        let open = gallery.open.as_ref().ok_or("open")?;
        assert!(
            matches!(open.results.as_ref().map(|r| &r.report), Some(Err(_))),
            "kind: is refused"
        );
        assert!(frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Enter)]).is_none());
        assert!(gallery.is_open());
        frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Escape)]);
        assert!(!gallery.is_open(), "Esc closes");
        Ok(())
    }

    /// Run 3, finding 1: typing on from "resistor" to "resistor 0603" moves
    /// the highlight to the new top row, the one Enter creates, even though
    /// the old top row (an 0201, "0603 metric") still matches.
    #[test]
    fn new_text_highlights_the_new_best_match() -> TestResult {
        let ctx = context();
        let engine = Engine::demo()?;
        for (first, then, expected) in [
            (
                "resistor",
                "resistor 0603",
                "Chip resistor 0603 (1608 metric)",
            ),
            ("header 1x", "header 1x4", "Pin header 1×4 2.54 mm"),
        ] {
            let mut gallery = TemplateGallery::default();
            gallery.open_new(None);
            frame(&ctx, &mut gallery, &engine, vec![]);
            for query in [first, then] {
                gallery.open.as_mut().ok_or("open")?.query = query.into();
                frame(&ctx, &mut gallery, &engine, vec![]);
                frame(&ctx, &mut gallery, &engine, vec![]);
            }
            let open = gallery.open.as_ref().ok_or("open")?;
            assert_eq!(open.highlighted, 0, "{then}");
            assert_eq!(open.hit().ok_or("a hit")?.name, expected);
            let Some(Operation::Library(LibraryCommand::NewFromTemplate(request))) =
                frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Enter)])
            else {
                return Err("Enter creates".into());
            };
            assert_eq!(request.name, expected);
        }
        Ok(())
    }

    /// New component scope: ↓ moves to the project row, ⏎ sends one
    /// `library.new_from_template` by reference with the row's skeleton,
    /// name and prefix. A refusal comes back to the sheet's refusal line
    /// and leaves it open and usable; an applied create closes it and
    /// offers the new component to open.
    #[test]
    fn create_sends_one_command_and_settles() -> TestResult {
        let ctx = context();
        let mut engine = Engine::demo()?;
        let mut gallery = TemplateGallery::default();
        gallery.open_new(None);
        frame(&ctx, &mut gallery, &engine, vec![]);
        results(
            &mut gallery,
            &engine,
            vec![
                hit(FamilyKind::Chip, "Chip (custom)", chip(ChipKind::Resistor)),
                project_row(FamilyKind::Chip, "C0603", chip(ChipKind::Capacitor), 41),
            ],
        );
        frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::ArrowDown)]);
        let operation =
            frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Enter)]).ok_or("sent")?;
        let Operation::Library(LibraryCommand::NewFromTemplate(request)) = &operation else {
            return Err("not new_from_template".into());
        };
        assert_eq!(
            request.row,
            RowSource::Project {
                row: PackageRowRef::new(DefinitionId(41), Revision(1))
            }
        );
        assert_eq!(request.name, "C0603");
        assert_eq!(request.designator_prefix.as_deref(), Some("C"));
        assert_eq!(
            request.skeleton,
            SkeletonHint::TwoTerminal { polarised: false }
        );
        // Waiting: a second ⏎ sends nothing.
        assert!(frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Enter)]).is_none());
        let before = engine.to_file();
        let error = engine
            .apply(engine.revision(), operation)
            .err()
            .ok_or("slice B1 handles it; this checkout refuses")?;
        assert_eq!(engine.to_file(), before, "a refusal changes nothing");
        crate::refusal::hand_back(
            &ctx,
            Refused {
                origin: Origin::Dialog(egui::Id::unique(GALLERY)),
                error,
            },
            false,
        );
        frame(&ctx, &mut gallery, &engine, vec![]);
        frame(&ctx, &mut gallery, &engine, vec![]);
        let open = gallery.open.as_ref().ok_or("still open")?;
        assert!(open.refusal.is_some(), "the refusal shows on the sheet");
        assert!(open.submitted.is_none(), "usable again");
        // Applied: the engine moved on with a new component draft.
        let sent = frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Enter)]);
        assert!(sent.is_some());
        let outcome = engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta {
                    name: "C0603".into(),
                    description: String::new(),
                    provenance: String::new(),
                },
                definition: Definition::Component(Default::default()),
            }),
        )?;
        let pcbcraft_core::Outcome::DraftCreated { id } = outcome else {
            return Err("no draft".into());
        };
        frame(&ctx, &mut gallery, &engine, vec![]);
        assert!(!gallery.is_open(), "an applied create closes the sheet");
        assert_eq!(gallery.take_created(), Some(id));
        assert_eq!(gallery.take_created(), None, "offered once");
        Ok(())
    }

    /// Package scope opens on the package's family and replaces a footprint
    /// with pads in the same command.
    #[test]
    fn package_scope_replaces_in_one_command() -> TestResult {
        let ctx = context();
        let engine = Engine::demo()?;
        let mut gallery = TemplateGallery::default();
        gallery.open_package(
            DefinitionId(5),
            4,
            Some(FamilyKind::NoLead),
            String::new(),
            None,
        );
        frame(&ctx, &mut gallery, &engine, vec![]);
        results(
            &mut gallery,
            &engine,
            vec![
                project_row(FamilyKind::Chip, "R0603", chip(ChipKind::Resistor), 2),
                project_row(FamilyKind::NoLead, "QFN-16 3×3", qfn16(), 3),
            ],
        );
        let open = gallery.open.as_ref().ok_or("open")?;
        assert_eq!(open.chip, Chip::Family(FamilyKind::NoLead));
        assert_eq!(open.shown().len(), 1, "the chip filters the rows");
        let operation =
            frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Enter)]).ok_or("sent")?;
        let Operation::Library(LibraryCommand::PackageFromTemplate(request)) = operation else {
            return Err("not package_from_template".into());
        };
        assert_eq!(request.draft, DefinitionId(5));
        assert!(request.replace, "4 pads are replaced atomically");
        assert_eq!(
            request.row,
            RowSource::Project {
                row: PackageRowRef::new(DefinitionId(3), Revision(1))
            }
        );
        Ok(())
    }

    /// Edited values with a range problem block the primary (the core's
    /// `range_problems`, shown inline); Esc folds the values before it
    /// closes the sheet.
    #[test]
    fn invalid_values_block_and_esc_folds_first() -> TestResult {
        let ctx = context();
        let engine = Engine::demo()?;
        let mut gallery = TemplateGallery::default();
        gallery.open_new(None);
        frame(&ctx, &mut gallery, &engine, vec![]);
        results(
            &mut gallery,
            &engine,
            vec![project_row(
                FamilyKind::Chip,
                "R0603",
                chip(ChipKind::Resistor),
                2,
            )],
        );
        let mut bad = chip(ChipKind::Resistor);
        if let PackageForm::Chip(c) = &mut bad {
            c.width = -1.0;
        }
        assert!(!bad.range_problems().is_empty());
        if let Some(open) = gallery.open.as_mut() {
            open.edited = Some(bad);
        }
        let hit = gallery.open.as_ref().and_then(Open::hit).ok_or("hit")?;
        assert!(gallery.blocked(&hit).is_some());
        // Values are open: ⏎ belongs to the fields, Esc folds them.
        assert!(frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Enter)]).is_none());
        frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Escape)]);
        let open = gallery.open.as_ref().ok_or("still open")?;
        assert!(open.edited.is_none(), "Esc folded the values");
        assert!(gallery.blocked(&hit).is_none());
        frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Escape)]);
        assert!(!gallery.is_open());
        Ok(())
    }

    /// A folder row is read by the host first; once its bundle arrives the
    /// request goes out inline (`RowSource::Bundle`) without another ⏎.
    #[test]
    fn a_folder_row_is_sent_inline_once_read() -> TestResult {
        let ctx = context();
        let engine = Engine::demo()?;
        let library = LibraryId(0x51);
        let lineage = SourceLineageId(0x52);
        let mut gallery = TemplateGallery::default();
        gallery.open_new(None);
        frame(&ctx, &mut gallery, &engine, vec![]);
        let folder = TemplateHit {
            source: Source::Library(library),
            row: None,
            source_row: Some(SourceRevision {
                lineage,
                revision: Revision(1),
            }),
            ..project_row(FamilyKind::NoLead, "QFN-16 3×3", qfn16(), 8)
        };
        results(&mut gallery, &engine, vec![folder]);
        assert!(frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Enter)]).is_none());
        let reads = gallery.take_reads();
        let pick = reads.first().cloned().ok_or("a read")?;
        assert_eq!(pick.lineage, lineage);
        assert!(frame(&ctx, &mut gallery, &engine, vec![key(egui::Key::Enter)]).is_none());
        assert!(gallery.take_reads().is_empty(), "read once");
        let bundle = Bundle {
            library,
            name: "Pcbcraft Packages".into(),
            root: SourceRevision {
                lineage,
                revision: Revision(1),
            },
            records: Vec::new(),
            assets: Vec::new(),
        };
        gallery.bundle_read(pick, Ok(bundle.clone()));
        let operation = frame(&ctx, &mut gallery, &engine, vec![]).ok_or("sent on arrival")?;
        let Operation::Library(LibraryCommand::NewFromTemplate(request)) = operation else {
            return Err("not new_from_template".into());
        };
        assert_eq!(
            request.row,
            RowSource::Bundle {
                bundle: Box::new(bundle)
            }
        );
        Ok(())
    }

    /// The gallery as the one-hour run saw it (SOIC-14, a 1380-point
    /// window): the preview's labels are whole, and every text cut short,
    /// a result row's detail among them, shows in full on hover. With
    /// `PCBCRAFT_TEXT_PICTURES=<dir>` it also writes the sheet's picture.
    #[test]
    fn the_gallery_text_is_whole_or_shows_in_full_on_hover() -> TestResult {
        let engine = Engine::demo()?;
        let mut gallery = TemplateGallery::default();
        gallery.open_new(None);
        if let Some(open) = gallery.open.as_mut() {
            open.query = "soic pins:14".into();
        }
        let screen = crate::kit::text::tests::Screen::new(vec2(1380.0, 900.0));
        let run = |gallery: &mut TemplateGallery, events: Vec<egui::Event>| {
            screen.run(events, |ui| {
                gallery.show(ui.ctx(), &engine);
            })
        };
        for _ in 0..4 {
            run(&mut gallery, vec![]);
        }
        let rest = run(&mut gallery, vec![]);
        for label in ["Body", "Terminals"] {
            assert!(
                crate::kit::text::tests::shows_in_full(&rest, label),
                "{label} is whole"
            );
        }
        screen.picture("template-gallery", &rest, 1.0)?;
        let (_, missing) = crate::kit::text::tests::cuts_without_hover(&screen.ctx, |events| {
            run(&mut gallery, events)
        });
        assert!(missing.is_empty(), "cut without a hover: {missing:?}");
        Ok(())
    }
}
