//! Pcbcraft's electronics document and its single typed command engine.
//!
//! This crate is pure and wasm-compatible: no files, threads, windows or
//! transports. `pcbcraft-io` reads and writes documents; transports (UI, CLI,
//! MCP) translate requests into [`Operation`]s applied by [`Engine`].
// First: its `declare_classes!` is in textual scope for every module below.
#[macro_use]
pub mod permission;

pub mod catalog;
pub mod commands;
pub mod completeness;
pub mod diagnostics;
pub mod erc;
pub mod fabrication;
pub mod fields;
pub mod firmware;
pub mod format;
pub mod functions;
pub mod geometry;
pub mod ids;
pub mod instances;
pub mod interface;
pub mod issues;
pub mod layout;
pub mod library;
pub mod materials;
pub mod model;
pub mod nets;
pub mod pins;
pub mod product;
pub mod quantity;
pub mod rules;
pub mod schematic;
pub mod scripting;
pub mod shared;
#[cfg(test)]
mod tests;
pub mod units;
pub mod updates;
pub mod validate;
pub use commands::{InstanceCommand, LibraryCommand, OffGridPin, Outcome};
use completeness::{CheckTarget, DefinitionReport, InstanceReport};
pub use diagnostics::{Diagnostic, Error, Finding, ObjectRef, Result, Stage};
pub use geometry::point_valid;
pub use ids::*;
use library::samples::{self, SampleRefs};
use library::{Definition, DefinitionMeta, Drafts};
pub use model::*;
pub use permission::Class;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub use shared::Shared;
use std::collections::VecDeque;
use std::sync::Arc;

/// Every document mutation. UI gestures, CLI requests and MCP tools all
/// produce one of these and apply it through [`Engine::apply`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "editor", content = "operation", deny_unknown_fields)]
pub enum Operation {
    Library(LibraryCommand),
    Instance(InstanceCommand),
    Schematic(schematic::SchematicCommand),
    Layout(layout::LayoutCommand),
    /// Design rules and net classes (`rules.*`, `net_class.*`).
    Rules(rules::RulesCommand),
    /// Copper fills and keepouts (`fill.*`, `keepout.*`).
    Fill(layout::fill::FillCommand),
    /// Board shape and edge constraints (`board.*`).
    Board(layout::outline::BoardCommand),
    /// Part dimensions (`layout.dimension_*`).
    Dimension(layout::dimensions::DimensionCommand),
    /// Pre-flight waivers for fabrication output (`fabrication.*`).
    Fabrication(fabrication::FabricationCommand),
    /// The board interface an assembly references (`interface.*`).
    Interface(interface::InterfaceCommand),
    /// The assembly link (`assembly.*`).
    Assembly(interface::assembly_link::AssemblyCommand),
    Update(Box<updates::ApplyUpdate>),
}

impl Operation {
    /// This operation's permission class, chosen beside its command enum.
    pub const fn class(&self) -> Class {
        match self {
            Self::Library(command) => command.class(),
            Self::Instance(command) => command.class(),
            Self::Schematic(command) => command.class(),
            Self::Layout(command) => command.class(),
            Self::Rules(command) => command.class(),
            Self::Fill(command) => command.class(),
            Self::Board(command) => command.class(),
            Self::Dimension(command) => command.class(),
            Self::Fabrication(command) => command.class(),
            Self::Interface(command) => command.class(),
            Self::Assembly(command) => command.class(),
            Self::Update(_) => Class::Edit,
        }
    }

    /// This operation's wire name; `update` for an applied update.
    pub const fn command(&self) -> &'static str {
        match self {
            Self::Library(command) => command.command(),
            Self::Instance(command) => command.command(),
            Self::Schematic(command) => command.command(),
            Self::Layout(command) => command.command(),
            Self::Rules(command) => command.command(),
            Self::Fill(command) => command.command(),
            Self::Board(command) => command.command(),
            Self::Dimension(command) => command.command(),
            Self::Fabrication(command) => command.command(),
            Self::Interface(command) => command.command(),
            Self::Assembly(command) => command.command(),
            Self::Update(_) => "update",
        }
    }

    /// This operation's title for people, from [`Self::titles`].
    pub fn title(&self) -> &'static str {
        let name = self.command();
        Self::titles()
            .find(|(wire, _)| *wire == name)
            .map_or("Change", |(_, title)| *title)
    }

    /// The area of the design this operation changes.
    pub const fn area(&self) -> Area {
        match self {
            Self::Library(_) => Area::Component,
            Self::Instance(InstanceCommand::SetTextPlacement { .. })
            | Self::Layout(_)
            | Self::Rules(_)
            | Self::Fill(_)
            | Self::Board(_)
            | Self::Dimension(_)
            | Self::Fabrication(_)
            | Self::Interface(_)
            | Self::Assembly(_) => Area::Pcb,
            Self::Schematic(
                schematic::SchematicCommand::SetProperties { .. }
                | schematic::SchematicCommand::RenameProject { .. },
            ) => Area::Project,
            Self::Instance(_) | Self::Schematic(_) | Self::Update(_) => Area::Schematic,
        }
    }

    /// The one component instance this operation concerns, if it names
    /// exactly one.
    pub fn instance(&self) -> Option<Id> {
        use layout::LayoutCommand as L;
        match self {
            Self::Layout(
                L::Place { instance, .. }
                | L::Move { instance, .. }
                | L::SetPlacementLock { instance, .. }
                | L::SetPart { instance, .. }
                | L::RemovePart { instance },
            )
            | Self::Instance(
                InstanceCommand::Remove { instance }
                | InstanceCommand::SetDesignator { instance, .. }
                | InstanceCommand::SetProperties { instance, .. }
                | InstanceCommand::SelectFunctions { instance, .. }
                | InstanceCommand::SetUnitUnused { instance, .. }
                | InstanceCommand::SetNoConnect { instance, .. }
                | InstanceCommand::SetPhysical { instance, .. }
                | InstanceCommand::SetPinConfig { instance, .. }
                | InstanceCommand::SetMarking { instance, .. }
                | InstanceCommand::SetTextPlacement { instance, .. },
            ) => Some(*instance),
            Self::Instance(InstanceCommand::AssignPart { instances, .. }) => match instances[..] {
                [one] => Some(one),
                _ => None,
            },
            _ => None,
        }
    }

    /// Every operation by wire name with its class, for discovery.
    pub fn classes() -> impl Iterator<Item = &'static (&'static str, Class)> {
        const UPDATE: &[(&str, Class)] = &[("update", Class::Edit)];
        [
            LibraryCommand::CLASSES,
            InstanceCommand::CLASSES,
            schematic::SchematicCommand::CLASSES,
            layout::LayoutCommand::CLASSES,
            rules::RulesCommand::CLASSES,
            layout::fill::FillCommand::CLASSES,
            layout::outline::BoardCommand::CLASSES,
            layout::dimensions::DimensionCommand::CLASSES,
            fabrication::FabricationCommand::CLASSES,
            interface::InterfaceCommand::CLASSES,
            interface::assembly_link::AssemblyCommand::CLASSES,
            UPDATE,
        ]
        .into_iter()
        .flatten()
    }

    /// Every operation by wire name with its title, in [`Self::classes`]'s
    /// order.
    pub fn titles() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
        const UPDATE: &[(&str, &str)] = &[("update", "Apply an update")];
        [
            LibraryCommand::TITLES,
            InstanceCommand::TITLES,
            schematic::SchematicCommand::TITLES,
            layout::LayoutCommand::TITLES,
            rules::RulesCommand::TITLES,
            layout::fill::FillCommand::TITLES,
            layout::outline::BoardCommand::TITLES,
            layout::dimensions::DimensionCommand::TITLES,
            fabrication::FabricationCommand::TITLES,
            interface::InterfaceCommand::TITLES,
            interface::assembly_link::AssemblyCommand::TITLES,
            UPDATE,
        ]
        .into_iter()
        .flatten()
    }
}

/// Every read-only report. Queries never change the document, its revision
/// or history.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "query", content = "params", deny_unknown_fields)]
pub enum Query {
    /// Exact pins, resolved choices, unit states and completeness.
    #[serde(rename = "component.inspect")]
    InspectInstance { instance: Id },
    /// Publish findings of exactly one draft or published revision.
    #[serde(rename = "component.check")]
    CheckDefinition { target: CheckTarget },
    /// `component.check` of every draft, in draft order, staged once.
    #[serde(rename = "component.check_drafts")]
    CheckDrafts {},
    /// Ranked component lineages at their latest published revision for a
    /// one-line query. The query takes free text, `pkg:`/`has:`/`missing:`
    /// filters and unit-aware field predicates such as `res>=10k`, `tol<=1%` or
    /// `C:1n..100n`. Field keys resolve against the project's field
    /// definitions.
    #[serde(rename = "library.search")]
    Search {
        query: String,
        /// At most 500; 50 when omitted.
        #[serde(default)]
        limit: Option<usize>,
        /// List lineages that exist only as drafts instead of counting them
        /// as hidden.
        #[serde(default)]
        include_drafts: bool,
    },
    /// The fields, package labels and completeness badges that the entries
    /// matching a query carry, with value counts.
    #[serde(rename = "library.facets")]
    Facets {
        query: String,
        #[serde(default)]
        include_drafts: bool,
    },
    /// One published component revision (the latest when `revision` is
    /// omitted) with its revisions, symbols, implementations and badges.
    #[serde(rename = "library.inspect")]
    InspectLineage {
        lineage: DefinitionId,
        #[serde(default)]
        revision: Option<Revision>,
    },
    /// The project BOM, with resolved instances grouped by Part and unresolved
    /// lines marked. Unresolved instances group by component revision,
    /// footprint and identical requirements.
    #[serde(rename = "project.bom")]
    Bom {},
    /// Groups of instances without a resolved Part, each with the
    /// `library.search` query that finds Parts for it. A group shares
    /// requirements and footprint; an instance with an unresolved Part is
    /// included.
    #[serde(rename = "library.resolve_groups")]
    ResolveGroups {},
    /// What `library.edit_footprint` would produce for these edits, without
    /// changing the document. Pads, graphics and findings, including blocking
    /// ones. Needs the host's footprint evaluator.
    #[serde(rename = "library.footprint_preview")]
    FootprintPreview {
        draft: DefinitionId,
        edits: Vec<library::footprint_source::FootprintEdit>,
    },
    /// Project lineages copied from a library source that now holds a newer
    /// revision, or whose recorded revision changed on disk. Needs the
    /// host's source catalogs (`CatalogContext::sources`).
    #[serde(rename = "library.updates_available")]
    UpdatesAvailable {},
    /// What `library.promote` would write to the library `library` and
    /// record in the project: the export bundle of `root` and its closure.
    /// New lineages take ids from `fresh`, in order (io draws them).
    #[serde(rename = "library.promote_preview")]
    PromotePreview {
        root: DefinitionRef,
        library: LibraryId,
        /// The library's display name, carried in the bundle.
        name: String,
        fresh: Vec<SourceLineageId>,
    },
    /// Every issue of the project and its drafts in `scope`, most severe first,
    /// with its severity, group and title. Each row has the one-line title core
    /// gives the finding (docs/sidebar-ia.md); uncapped, with same-cause
    /// findings merged into one row and every finding kept.
    #[serde(rename = "project.issues")]
    Issues { scope: issues::IssueScope },
    /// B1: template rows (project, sources and each family's custom entry)
    /// for a one-line query with `pins:`, `pitch:`, `body:` and `pkg:`
    /// filters. At most 500 hits; 50 when omitted.
    #[serde(rename = "library.templates")]
    Templates {
        query: String,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// On-demand generic parts a text such as "header 1x4" or "10k 0603" reads
    /// as, under the families' patterns. Each says how the text was read,
    /// whether the part already exists, and what `library.find_or_create` would
    /// make. Empty when it reads as none.
    #[serde(rename = "library.generic_parts")]
    GenericParts { query: String },
    /// The built-in standard fields as a bundle `library.import` takes
    /// (`library.import_standard_fields` does both in one step).
    #[serde(rename = "library.standard_fields")]
    StandardFields {},
    /// B1: what `library.package_from_template` would change.
    #[serde(rename = "library.package_plan")]
    PackagePlan {
        request: Box<library::templates::catalog::PackageFromTemplate>,
    },
    /// D1: 3D library models (and generated families) for a query, ranked
    /// by fit when `fits` names the package draft the chooser serves.
    #[serde(rename = "library.models")]
    Models {
        query: String,
        #[serde(default)]
        fits: Option<DefinitionId>,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// C3: the plan `library.pinout_apply` would apply: changes, findings
    /// and review items.
    #[serde(rename = "library.pinout_preview")]
    PinoutPreview {
        target: library::pinout::PinoutTarget,
        document: Box<library::pinout::PinoutDocument>,
    },
    /// C2: ranked complete assignments that enable peripherals on one
    /// instance, or the rules that block them. Never applies anything.
    #[serde(rename = "functions.suggest")]
    SuggestPins {
        instance: Id,
        request: functions::suggest::SuggestRequest,
    },
    /// Per terminal of one instance: pads, GPIO, active function, peripheral,
    /// role, mux code, pin config and net. Every other endpoint on the net is
    /// listed too (C2).
    #[serde(rename = "project.pin_report")]
    PinReport { instance: Id },
    /// C4: the firmware pin-configuration IR of one instance.
    #[serde(rename = "project.pin_config")]
    PinConfig { instance: Id },
    /// D4: one instance's resolved marking layout (none when unmarked).
    #[serde(rename = "instance.marking")]
    Marking { instance: Id },
    /// Every effective design rule with its profile value and source,
    /// and the net classes.
    #[serde(rename = "rules.effective")]
    RulesEffective {},
    /// The project's own profile snapshot, if any; hosts with a profile
    /// library (desktop, CLI, MCP) append its profiles. Pcbcraft ships none.
    #[serde(rename = "rules.fab_profiles")]
    RuleProfiles {},
    /// What `rules.set_fab_profile` with these arguments would change: rules
    /// and the board issues that would appear or go.
    #[serde(rename = "rules.preview_fab_profile")]
    PreviewProfile {
        profile: Box<rules::RuleProfile>,
        #[serde(default)]
        keep_overrides: bool,
    },
    /// Every net class with its resolved width, via and clearance, and
    /// every net with its class and how it got it (explicit or pattern).
    #[serde(rename = "net_class.list")]
    NetClasses {},
    /// The nets whose class `net_class.set_patterns` with `patterns` would
    /// change. Never applies anything.
    #[serde(rename = "net_class.preview_patterns")]
    PreviewNetClassPatterns {
        patterns: Vec<rules::NetClassPattern>,
    },
    /// The solved board outline, cutouts, holes and named edges.
    #[serde(rename = "board.shape")]
    BoardShape {},
    /// Per fill: whether its computed copper is stale.
    #[serde(rename = "fill.status")]
    FillStatus {},
    /// Every schematic sheet in page order with its place in the hierarchy
    /// (parent sheet symbol, depth) and what it holds.
    #[serde(rename = "schematic.sheets")]
    Sheets {},
    /// Where a net appears across sheets: its labels, sheet pins and placed
    /// terminals, in page order. Each with sheet and position, for jumping
    /// between a label's other occurrences.
    #[serde(rename = "schematic.net_places")]
    NetPlaces { net: Id },
    /// What `schematic.paste` needs to recreate these sheet objects. The
    /// symbols' instances, labels, drawings, the wires named and the wires with
    /// both ends on named objects. A wire end on anything else reads as a free
    /// end. Changes nothing, so a transport or the app can keep the result as
    /// its copy buffer.
    #[serde(rename = "schematic.copy")]
    SheetCopy {
        /// The sheet they are on; the first sheet when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<SheetId>,
        objects: Vec<schematic::ClipObject>,
    },
    /// Everything one sheet draws, resolved to shapes and text in sheet
    /// millimetres. Page frame and title block, labels, power ports, sheet
    /// symbols, furniture, wires, junction dots and units.
    #[serde(rename = "schematic.scene")]
    SheetScene { sheet: SheetId },
    /// Z0 (and, with a gap, the edge-coupled Zdiff) of a trace on a
    /// stackup layer or an explicit structure, with the model's accuracy.
    #[serde(rename = "impedance.calculate")]
    ImpedanceCalculate {
        request: layout::impedance::ImpedanceRequest,
    },
    /// Every layer's line structure and the width and pair gap each impedance
    /// target needs on each layer. Per net class with an impedance target, and
    /// why a target cannot be met on a layer.
    #[serde(rename = "impedance.table")]
    ImpedanceTable {},
    /// Differential pairs proposed from net names (`USB_D+`/`USB_D-`,
    /// `_P`/`_N`, `DP`/`DM`); `diff_pair.add` makes one.
    #[serde(rename = "diff_pair.suggest")]
    SuggestPairs {},
    /// Every routed, paired or grouped net's length with its target and
    /// deviation, and every pair's skew. Length counts tracks plus via barrels;
    /// pairs also report uncoupled length, and every length group its window.
    #[serde(rename = "layout.length_report")]
    LengthReport {},
    /// What `layout.route_step` would do, without applying it. The edit set
    /// (the step's copper and every shoved track and via), where the head ends
    /// and why it stopped short.
    #[serde(rename = "layout.route_preview")]
    RoutePreview {
        request: layout::router::RouteStepRequest,
    },
    /// What `layout.track_drag` would do, without applying it. The plan has
    /// the moved segment, its neighbours and every shoved track and via, the
    /// moved segment's midpoint (`end`) and why it stopped short.
    #[serde(rename = "layout.track_drag_preview")]
    TrackDragPreview {
        request: layout::router::drag::TrackDragRequest,
    },
    /// What `layout.tune_length` would build, without applying it.
    #[serde(rename = "layout.tune_preview")]
    TunePreview {
        request: layout::meander::TuneRequest,
    },
    /// The production check before fabrication export: the design and
    /// production findings under a fab profile. `profile` is a profile id such
    /// as `std-6-6` or a whole profile, and the project's rules when omitted;
    /// each finding has its waiver key and saved waiver.
    #[serde(rename = "fabrication.preflight")]
    Preflight {
        #[serde(default)]
        profile: Option<fabrication::ProfileChoice>,
    },
    /// The board interface's exported datums, rules and retired slots.
    /// Each datum has its id, slot, token, display name, role, kind,
    /// source, geometry (board millimetres), status and whether it changed
    /// since the last publish; the list has the last published revision.
    #[serde(rename = "interface.list")]
    InterfaceList {},
    /// The assembly link's state from the document: none, a
    /// broken reference (publishing is blocked) or linked, with
    /// the findings. `assembly.preview` reads the file.
    #[serde(rename = "assembly.status")]
    AssemblyStatus {},
}

// A new command chooses its permission class here.
declare_classes!(Query {
    InspectInstance "component.inspect" Read "Inspect an instance",
    CheckDefinition "component.check" Read "Check a definition",
    CheckDrafts "component.check_drafts" Read "Check all drafts",
    Search "library.search" Read "Search the library",
    GenericParts "library.generic_parts" Read "Find generic parts",
    Facets "library.facets" Read "List search filters",
    InspectLineage "library.inspect" Read "Inspect a lineage",
    Bom "project.bom" Read "Bill of materials",
    ResolveGroups "library.resolve_groups" Read "Group instances for purchasing",
    FootprintPreview "library.footprint_preview" Read "Preview a footprint",
    UpdatesAvailable "library.updates_available" Read "List library updates",
    PromotePreview "library.promote_preview" Read "Preview a promotion",
    Issues "project.issues" Read "List issues",
    Templates "library.templates" Read "List package rows",
    StandardFields "library.standard_fields" Read "List standard fields",
    PackagePlan "library.package_plan" Read "Plan a package from a row",
    Models "library.models" Read "Find 3D models",
    PinoutPreview "library.pinout_preview" Read "Preview a pinout import",
    SuggestPins "functions.suggest" Read "Suggest pin assignments",
    PinReport "project.pin_report" Read "Pin report",
    PinConfig "project.pin_config" Read "Firmware pin configuration",
    Marking "instance.marking" Read "Resolve an instance's marking",
    RulesEffective "rules.effective" Read "Effective rules",
    RuleProfiles "rules.fab_profiles" Read "List fab profiles",
    PreviewProfile "rules.preview_fab_profile" Read "Preview a fab profile",
    NetClasses "net_class.list" Read "List net classes",
    PreviewNetClassPatterns "net_class.preview_patterns" Read "Preview net class patterns",
    BoardShape "board.shape" Read "Board shape",
    FillStatus "fill.status" Read "Fill status",
    Sheets "schematic.sheets" Read "List sheets",
    NetPlaces "schematic.net_places" Read "Find a net on the sheets",
    SheetCopy "schematic.copy" Read "Copy schematic objects",
    SheetScene "schematic.scene" Read "Sheet scene",
    ImpedanceCalculate "impedance.calculate" Read "Calculate impedance",
    ImpedanceTable "impedance.table" Read "Impedance table",
    SuggestPairs "diff_pair.suggest" Read "Suggest differential pairs",
    LengthReport "layout.length_report" Read "Length report",
    RoutePreview "layout.route_preview" Read "Preview a route",
    TrackDragPreview "layout.track_drag_preview" Read "Preview a track drag",
    TunePreview "layout.tune_preview" Read "Preview length tuning",
    Preflight "fabrication.preflight" Read "Production check",
    InterfaceList "interface.list" Read "List the board interface",
    AssemblyStatus "assembly.status" Read "Check the assembly link",
});

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "report", content = "result", rename_all = "snake_case")]
pub enum QueryResult {
    Instance(Box<InstanceReport>),
    Definition(DefinitionReport),
    Drafts(Vec<DefinitionReport>),
    Search(catalog::SearchReport),
    Facets(catalog::FacetReport),
    Lineage(Box<catalog::ItemReport>),
    Bom(completeness::BomReport),
    ResolveGroups(completeness::ResolveReport),
    FootprintPreview(Box<library::footprint_edit::FootprintPreview>),
    UpdatesAvailable(Vec<catalog::UpdateAvailable>),
    Promote(Box<library::import::PromotePlan>),
    /// `library.standard_fields`: a bundle for `library.import`.
    StandardFields(Box<library::Bundle>),
    Issues(issues::IssueReport),
    Templates(catalog::templates::TemplateReport),
    GenericParts(catalog::generic::GenericReport),
    PackagePlan(library::templates::catalog::PackagePlan),
    Models(catalog::models::ModelReport),
    PinoutPlan(Box<library::pinout::PinoutPlan>),
    SuggestPins(functions::suggest::SuggestReport),
    PinReport(Box<pins::PinReport>),
    PinConfig(Box<firmware::PinConfigReport>),
    Marking(Option<library::marking::MarkingLayout>),
    Rules(Box<rules::RulesReport>),
    RuleProfiles(Vec<rules::RuleProfile>),
    ProfileImpact(Box<rules::ProfileImpact>),
    NetClasses(Box<rules::netclass::NetClassReport>),
    NetClassChanges(Vec<rules::netclass::ClassChange>),
    BoardShape(Box<model::SolvedBoard>),
    FillStatus(Vec<layout::fill::FillStatus>),
    Sheets(Vec<schematic::SheetRow>),
    NetPlaces(Box<schematic::NetPlaces>),
    SheetClip(Box<schematic::SheetClip>),
    SheetScene(Box<schematic::scene::SheetScene>),
    Impedance(Box<layout::impedance::ImpedanceResult>),
    ImpedanceTable(Box<layout::impedance::ImpedanceTable>),
    PairSuggestions(Vec<rules::signal::PairSuggestion>),
    LengthReport(Box<layout::length::LengthReport>),
    TunePreview(Box<layout::meander::TunePreview>),
    RoutePreview(Box<layout::router::RoutePlan>),
    Preflight(Box<fabrication::PreflightReport>),
    InterfaceList(Box<interface::InterfaceList>),
    AssemblyStatus(Box<interface::assembly_link::AssemblyStatus>),
}

/// The host engines a command may need. The core never interprets a
/// sketch; these do, on the candidate only.
#[derive(Clone, Copy, Default)]
pub struct Evaluators<'a> {
    pub footprint: Option<&'a dyn library::FootprintEvaluator>,
    pub board: Option<&'a dyn layout::outline::BoardShapeEvaluator>,
}

/// Undo depth. Entries share every section an edit did not write
/// ([`Shared`]), so a step costs the sections it changed, not a copy of the
/// whole project.
pub const MAX_UNDO: usize = 100;
/// Operations [`Engine::apply_all_hosted`] takes in one step.
pub const MAX_BATCH: usize = 256;

/// The area of the design an undo step changed, as the workspaces name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    Component,
    Schematic,
    Pcb,
    /// The project itself: its name and properties.
    Project,
}

/// What one undo step did, for people: its operation's title, the area
/// it changed and, when it concerns exactly one part, that part's
/// designator as it was. A batch of one kind of operation takes that
/// operation's title; a mixed batch is [`Step::MIXED`], in its first
/// operation's area.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Step {
    pub title: &'static str,
    pub area: Area,
    /// The one part the step concerns, by designator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

/// The registry phrases a step's part replaces: "Move a placed part" on R1
/// reads "Move R1".
const PART_NOUNS: [&str; 4] = [
    "a placed part",
    "a part on the board",
    "a mechanical part",
    "a component",
];

impl Step {
    /// The title of a batch of different operations.
    pub const MIXED: &'static str = "Several changes";

    /// `operation`'s step on `project`, the document it applies to.
    fn of(operation: &Operation, project: &Project) -> Self {
        Self {
            title: operation.title(),
            area: operation.area(),
            subject: operation
                .instance()
                .and_then(|id| project.instance(id))
                .map(|i| i.designator.clone()),
        }
    }

    /// The step for people: its title, naming its part when it has one
    /// ("Move R1", "Set instance properties (R1)").
    pub fn line(&self) -> String {
        let Some(name) = &self.subject else {
            return self.title.to_owned();
        };
        PART_NOUNS
            .iter()
            .find_map(|noun| self.title.strip_suffix(noun))
            .map_or_else(
                || format!("{} ({name})", self.title),
                |verb| format!("{verb}{name}"),
            )
    }
}

/// The name a project's first save gives it: a project still called
/// [`UNTITLED`] takes the file's name ("usb-led.pcbcraft" names it
/// "usb-led"); a name someone chose is kept (`None`). `project` is the
/// copy being written, cloned only when renamed. Once the write succeeds the caller applies the same
/// `project.rename` to the engine, its own undo step: undo never takes back
/// a file, and a failed write leaves the document unchanged.
pub fn name_by_file(project: &mut Arc<Project>, file: &std::path::Path) -> Option<String> {
    let stem = file.file_stem()?.to_str()?.trim();
    if project.name != UNTITLED || stem == UNTITLED {
        return None;
    }
    schematic::sheets::rename_project(Arc::make_mut(project), stem).ok()?;
    Some(project.name.clone())
}

/// The document, its revision and one shared undo history.
///
/// Every mutation is staged on a candidate and validated before it replaces
/// the document, so a failed operation leaves document, revision and history
/// unchanged.
#[derive(Default)]
pub struct Engine {
    document: Document,
    revision: u64,
    /// The documents before each step, with what the step did.
    undo: VecDeque<(Document, Step)>,
    /// The documents after each undone step, with what it did.
    redo: Vec<(Document, Step)>,
    /// The revision the latest undo step was applied at, until an undo,
    /// redo or new document ([`Engine::join_last_step`]).
    last_step: Option<u64>,
    /// `fabrication.waive` refuses a waiver without a reason (a host
    /// setting; off by default).
    waiver_reason_required: bool,
    /// The copper graph of this revision, built on first use and dropped
    /// with every new revision ([`Engine::copper_graph`]).
    copper: std::sync::OnceLock<Arc<layout::CopperGraph>>,
    /// What the last applied operation re-routed of the copper its moved
    /// parts dragged ([`Engine::rerouted`]).
    rerouted: layout::router::reroute::Rerouted,
}

impl Engine {
    /// What the last operation applied re-routed of the copper its moved
    /// parts dragged along, and what it left unrouted: empty when it moved
    /// no part, or the carried copper stayed clean.
    pub fn rerouted(&self) -> &layout::router::reroute::Rerouted {
        &self.rerouted
    }

    /// Whether `fabrication.waive` needs a reason in this engine. The app
    /// sets it from its settings; headless hosts keep the default (off).
    pub fn require_waiver_reason(&mut self, required: bool) {
        self.waiver_reason_required = required;
    }

    /// Whether `fabrication.waive` needs a reason in this engine.
    pub fn waiver_reason_required(&self) -> bool {
        self.waiver_reason_required
    }

    /// An engine for an already validated file, with empty history.
    pub fn from_file(file: format::EditorFile) -> Result<Self> {
        let mut engine = Self::default();
        engine.replace_document(file)?;
        Ok(engine)
    }

    /// The PCB realism showcase: a 4-layer board with an SOIC-8, a QFN-16,
    /// a 2x5 header, 0603/0805 passives and a polarised part, routed on top
    /// and bottom through tented and open vias, built through the public
    /// commands. The sample parts live in `library::samples`.
    ///
    /// Unconnected terminals are marked no-connect. The board's rules are
    /// `profile`. The undo history is cleared.
    pub fn showcase_with(profile: rules::RuleProfile) -> Result<Self> {
        Self::showcase_in(None, profile)
    }

    /// A test fixture, never in the product: [`Engine::showcase_with`] the
    /// test prototype profile, whose values the showcase was drawn for.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn showcase() -> Result<Self> {
        Self::showcase_in(None, rules::fixtures::prototype()?)
    }

    /// The showcase as the shipped file shows it: [`Engine::showcase`]
    /// with a Power net class over GND and VCC and a top GND pour; with a
    /// board evaluator, also a sketched board with rounded corners, a
    /// round cutout, a milled slot and J1 held to the left edge by board
    /// parameters (`layout::showcase::board_edits`). The pours are filled
    /// last, over the final shape. The undo history is cleared.
    pub fn showcase_hosted(
        evaluators: Evaluators<'_>,
        profile: rules::RuleProfile,
    ) -> Result<Self> {
        Self::showcase_in(Some(evaluators), profile)
    }

    /// A test fixture, never in the product: [`Engine::showcase_hosted`]
    /// on the test prototype profile.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn showcase_hosted_fixture(evaluators: Evaluators<'_>) -> Result<Self> {
        Self::showcase_in(Some(evaluators), rules::fixtures::prototype()?)
    }

    fn showcase_in(hosted: Option<Evaluators<'_>>, profile: rules::RuleProfile) -> Result<Self> {
        let mut engine = Self::default();
        engine.apply(
            engine.revision,
            Operation::Rules(rules::RulesCommand::SetProfile {
                profile: Box::new(profile),
                keep_overrides: false,
            }),
        )?;
        engine.apply(
            engine.revision,
            Operation::Schematic(schematic::SchematicCommand::RenameProject {
                name: layout::showcase::PROJECT_NAME.into(),
            }),
        )?;
        let mut refs = Vec::new();
        for part in samples::showcase_parts() {
            let lineages = engine.reserve_lineages()?;
            let mut part_edits =
                samples::packaged_symbol(part.name, part.form, part.prefix, part.symbol, lineages)?;
            samples::trim_heels(&mut part_edits, part.heel_trim_mm)?;
            if part.pin_one {
                samples::mark_pin_one(&mut part_edits);
            }
            engine.apply(
                engine.revision,
                Operation::Library(LibraryCommand::Edit { edits: part_edits }),
            )?;
            engine.apply(
                engine.revision,
                Operation::Library(LibraryCommand::Publish {
                    drafts: vec![
                        lineages.component,
                        lineages.symbol.id,
                        lineages.package.id,
                        lineages.footprint.id,
                        lineages.binding.id,
                    ],
                    close: false,
                }),
            )?;
            refs.push(lineages);
        }
        for (k, (designator, part)) in samples::SHOWCASE_INSTANCES.iter().enumerate() {
            let lineages = refs
                .get(*part)
                .ok_or_else(|| Error::Invalid("showcase part index".into()))?;
            let column = (k % 3) as f64;
            let row = (k / 3) as f64;
            engine.apply(
                engine.revision,
                Operation::Instance(InstanceCommand::Place(instances::NewInstance {
                    sheet: None,
                    component: ComponentRef::new(lineages.component, Revision::FIRST),
                    designator: Some((*designator).into()),
                    properties: vec![],
                    representation: lineages.symbol.id,
                    unit: UnitId(1),
                    position: units::PointNm::from_mm_f64_rounded([column * 40.0, -row * 40.0])?,
                    physical: Some(PhysicalChoice {
                        binding: lineages.binding.id,
                        footprint: lineages.footprint.id,
                    }),
                })),
            )?;
        }
        for (name, terminals) in samples::SHOWCASE_NETS {
            let mut ends = Vec::with_capacity(terminals.len());
            for (designator, terminal) in *terminals {
                ends.push(layout::showcase::endpoint(
                    engine.project(),
                    designator,
                    *terminal,
                )?);
            }
            // Wire each terminal to the next; where no wire can be drawn
            // without running over another pin, a pair of labels of the
            // net's name, each on its pin, joins them instead.
            let mut labelled = std::collections::BTreeSet::new();
            for pair in ends.windows(2) {
                let wired = engine.apply(
                    engine.revision,
                    Operation::Schematic(schematic::SchematicCommand::Connect {
                        from: pair[0].into(),
                        to: pair[1].into(),
                    }),
                );
                match wired {
                    Ok(_) => {}
                    Err(Error::Conflict(_)) => {
                        for end in [pair[0], pair[1]] {
                            if !labelled.insert(end) {
                                continue;
                            }
                            let position = schematic::pin_position(engine.project(), end)
                                .ok_or_else(|| Error::Invalid("showcase pin".into()))?;
                            engine.apply(
                                engine.revision,
                                Operation::Schematic(schematic::SchematicCommand::PlaceLabel {
                                    sheet: None,
                                    name: (*name).into(),
                                    kind: model::LabelKind::Local,
                                    position: units::PointNm::from_mm_f64_rounded(position)?,
                                    rotation: Default::default(),
                                    attach: Some(end.into()),
                                }),
                            )?;
                        }
                    }
                    Err(other) => return Err(other),
                }
            }
            let net = ends
                .first()
                .and_then(|end| engine.project().net_of(*end))
                .map(|net| net.id)
                .ok_or_else(|| Error::Invalid("showcase net".into()))?;
            engine.apply(
                engine.revision,
                Operation::Schematic(schematic::SchematicCommand::RenameNet {
                    net,
                    name: (*name).into(),
                }),
            )?;
        }
        let mut unconnected: std::collections::BTreeMap<Id, Vec<TerminalId>> = Default::default();
        for net in &engine.project().nets {
            if let [only] = net.terminals[..] {
                unconnected
                    .entry(only.instance)
                    .or_default()
                    .push(only.terminal);
            }
        }
        for (instance, terminals) in unconnected {
            engine.apply(
                engine.revision,
                Operation::Instance(InstanceCommand::SetNoConnect {
                    instance,
                    terminals,
                    no_connect: true,
                }),
            )?;
        }
        for command in layout::showcase::commands(engine.project())? {
            engine.apply(engine.revision, Operation::Layout(command))?;
        }
        for command in layout::showcase::fill_commands(engine.project())? {
            engine.apply(engine.revision, Operation::Fill(command))?;
        }
        let evaluators = hosted.unwrap_or_default();
        if hosted.is_some() {
            engine.showcase_design(evaluators)?;
        }
        // The pours' copper, where this build computes fills; a build
        // without fill computation keeps them unfilled (stale).
        match engine.apply_hosted(
            engine.revision,
            Operation::Fill(layout::fill::FillCommand::Refill {
                target: layout::fill::RefillTarget::All,
            }),
            evaluators,
        ) {
            Ok(_) | Err(Error::Unsupported(_)) => {}
            Err(e) => return Err(e),
        }
        engine.undo.clear();
        engine.redo.clear();
        Ok(engine)
    }

    /// The hosted showcase's rules and shape over the plain showcase; see
    /// [`Engine::showcase_hosted`].
    fn showcase_design(&mut self, evaluators: Evaluators<'_>) -> Result<()> {
        let class = match self.apply(
            self.revision,
            Operation::Rules(rules::RulesCommand::AddNetClass {
                class: layout::showcase::power_class(),
            }),
        )? {
            Outcome::NetClassAdded { class } => class,
            _ => return Err(Error::Invalid("showcase net class".into())),
        };
        let nets: Vec<Id> = self
            .project()
            .nets
            .iter()
            .filter(|n| layout::showcase::POWER_NETS.contains(&n.name.as_str()))
            .map(|n| n.id)
            .collect();
        self.apply(
            self.revision,
            Operation::Rules(rules::RulesCommand::AssignNets {
                nets,
                class: Some(class),
            }),
        )?;
        for command in layout::showcase::top_fill_commands(self.project())? {
            self.apply(self.revision, Operation::Fill(command))?;
        }
        if evaluators.board.is_none() {
            return Ok(());
        }
        self.apply_hosted(
            self.revision,
            Operation::Board(layout::outline::BoardCommand::StartSketch),
            evaluators,
        )?;
        for edits in layout::showcase::board_edits() {
            self.apply_hosted(
                self.revision,
                Operation::Board(layout::outline::BoardCommand::Edit { edits }),
                evaluators,
            )?;
        }
        for dimension in layout::showcase::j1_dimensions(self.project())? {
            self.apply_hosted(
                self.revision,
                Operation::Dimension(layout::dimensions::DimensionCommand::Add { dimension }),
                evaluators,
            )?;
        }
        Ok(())
    }

    /// A test fixture, never in the product: an empty project on the test
    /// prototype profile.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn profiled() -> Result<Self> {
        let mut engine = Self::default();
        std::sync::Arc::make_mut(&mut engine.document.project).rules = rules::fixtures::rules()?;
        Ok(engine)
    }

    /// A test fixture, never in the product: two connected two-terminal
    /// components, placed on the board, with a typed resistance field
    /// shown on the symbol, on the test prototype profile. The sample
    /// library is built through the public commands and its drafts stay
    /// open for editing. Other crates' tests reach it through the
    /// `test-fixtures` feature of their dev-dependency; a launch and File ›
    /// New open an empty project.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn demo() -> Result<Self> {
        let mut engine = Self::default();
        std::sync::Arc::make_mut(&mut engine.document.project).rules = rules::fixtures::rules()?;
        let refs = engine.reserve_lineages()?;
        let kind = engine.reserve(DefinitionKind::QuantityKind)?;
        let field = engine.reserve(DefinitionKind::Field)?;
        let mut edits = samples::two_terminal("Resistor", samples::Kind::RESISTOR, refs)?;
        for edit in &mut edits {
            if let Definition::Component(component) = &mut edit.definition {
                component.fields.push(fields::FieldSelection {
                    field: FieldRef::new(field, Revision::FIRST),
                    default: None,
                    show_on_symbol: true,
                });
            }
        }
        edits.extend(samples::resistance_field(
            QuantityKindRef::new(kind, Revision::FIRST),
            field,
        ));
        engine.apply(
            engine.revision,
            Operation::Library(LibraryCommand::Edit { edits }),
        )?;
        engine.apply(
            engine.revision,
            Operation::Library(LibraryCommand::Publish {
                drafts: vec![
                    refs.component,
                    refs.symbol.id,
                    refs.package.id,
                    refs.footprint.id,
                    refs.binding.id,
                    kind,
                    field,
                ],
                close: false,
            }),
        )?;
        let mut placed = Vec::new();
        for position in [[-12.0, 0.0], [12.0, 0.0]] {
            let position = units::PointNm::from_mm_f64_rounded(position)?;
            let outcome = engine.apply(
                engine.revision,
                Operation::Instance(InstanceCommand::Place(instances::NewInstance {
                    sheet: None,
                    component: ComponentRef::new(refs.component, Revision::FIRST),
                    designator: None,
                    properties: vec![fields::PropertyEdit {
                        field,
                        input: fields::PropertyInput::Text("10k".into()),
                    }],
                    representation: refs.symbol.id,
                    unit: UnitId(1),
                    position,
                    physical: Some(PhysicalChoice {
                        binding: refs.binding.id,
                        footprint: refs.footprint.id,
                    }),
                })),
            )?;
            if let Outcome::InstancePlaced { instance, .. } = outcome {
                placed.push(instance);
            }
        }
        let [first, second] = placed[..] else {
            return Err(Error::Invalid("demo placement failed".into()));
        };
        engine.apply(
            engine.revision,
            Operation::Schematic(schematic::SchematicCommand::Connect {
                from: Endpoint {
                    instance: first,
                    terminal: TerminalId(2),
                }
                .into(),
                to: Endpoint {
                    instance: second,
                    terminal: TerminalId(1),
                }
                .into(),
            }),
        )?;
        // On the board, where the tests that build on the demo expect them
        // (Update PCB would stage them beside it).
        for (instance, position) in [(first, [8.0, 8.0]), (second, [16.0, 8.0])] {
            engine.apply(
                engine.revision,
                Operation::Layout(layout::LayoutCommand::Place {
                    instance,
                    position,
                    rotation_deg: 0.0,
                    layer: Layer::Top,
                }),
            )?;
        }
        engine.undo.clear();
        engine.redo.clear();
        Ok(engine)
    }

    /// Reserve one empty draft lineage of `kind`.
    fn reserve(&mut self, kind: DefinitionKind) -> Result<DefinitionId> {
        match self.apply(
            self.revision,
            Operation::Library(LibraryCommand::NewDraft {
                meta: DefinitionMeta::default(),
                definition: Definition::empty(kind),
            }),
        )? {
            Outcome::DraftCreated { id } => Ok(id),
            _ => Err(Error::Invalid("draft reservation failed".into())),
        }
    }

    /// Reserve one empty draft lineage of each kind and return the pending
    /// first revisions they would pin of each other.
    fn reserve_lineages(&mut self) -> Result<SampleRefs> {
        let mut ids = Vec::new();
        for kind in [
            DefinitionKind::Component,
            DefinitionKind::Symbol,
            DefinitionKind::Package,
            DefinitionKind::Footprint,
            DefinitionKind::Binding,
        ] {
            ids.push(self.reserve(kind)?);
        }
        let [component, symbol, package, footprint, binding] = ids[..] else {
            return Err(Error::Invalid("draft reservation failed".into()));
        };
        Ok(SampleRefs {
            component,
            symbol: SymbolRef::new(symbol, Revision::FIRST),
            package: PackageRef::new(package, Revision::FIRST),
            footprint: FootprintRef::new(footprint, Revision::FIRST),
            binding: BindingRef::new(binding, Revision::FIRST),
        })
    }

    pub fn document(&self) -> &Document {
        &self.document
    }
    pub fn project(&self) -> &Project {
        &self.document.project
    }
    /// Shared handle; equal pointers mean an unchanged project.
    pub fn project_arc(&self) -> &Arc<Project> {
        &self.document.project
    }
    pub fn drafts(&self) -> &Drafts {
        &self.document.drafts
    }
    pub fn to_file(&self) -> format::EditorFile {
        format::EditorFile::from_document(&self.document)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// What [`Engine::undo`] would take back, if anything.
    pub fn undo_step(&self) -> Option<Step> {
        self.undo.back().map(|(_, step)| step.clone())
    }
    /// What [`Engine::redo`] would apply again, if anything.
    pub fn redo_step(&self) -> Option<Step> {
        self.redo.last().map(|(_, step)| step.clone())
    }

    /// Replace the whole document (open, new, recover). History is cleared;
    /// an invalid file leaves everything unchanged.
    pub fn replace_document(&mut self, file: format::EditorFile) -> Result<()> {
        file.validate()?;
        let revision = self.next_revision()?;
        self.document = file.into_document();
        self.set_revision(revision);
        self.undo.clear();
        self.redo.clear();
        self.last_step = None;
        Ok(())
    }

    /// Apply one operation at an expected revision as one undo step. A
    /// failure leaves document, revision and history unchanged.
    pub fn apply(&mut self, expected_revision: u64, operation: Operation) -> Result<Outcome> {
        self.apply_inner(expected_revision, operation, Evaluators::default())
    }

    /// [`apply`](Self::apply) with the host's footprint evaluator, which
    /// footprint-source edits need (the core never interprets a sketch).
    /// The evaluator runs on the candidate only; undo and redo restore
    /// snapshots and never call it.
    pub fn apply_with(
        &mut self,
        expected_revision: u64,
        operation: Operation,
        evaluator: &dyn library::FootprintEvaluator,
    ) -> Result<Outcome> {
        self.apply_inner(
            expected_revision,
            operation,
            Evaluators {
                footprint: Some(evaluator),
                board: None,
            },
        )
    }

    /// [`apply`](Self::apply) with every host evaluator given.
    pub fn apply_hosted(
        &mut self,
        expected_revision: u64,
        operation: Operation,
        evaluators: Evaluators<'_>,
    ) -> Result<Outcome> {
        self.apply_inner(expected_revision, operation, evaluators)
    }

    /// Apply `operations` in order at an expected revision as one undo step
    /// and one revision, each checked as [`apply`](Self::apply) checks it
    /// against the document the ones before it made. Any failure leaves
    /// document, revision and history unchanged, and names the operation
    /// that failed. At most [`MAX_BATCH`] operations, and at least one.
    pub fn apply_all_hosted(
        &mut self,
        expected_revision: u64,
        operations: Vec<Operation>,
        evaluators: Evaluators<'_>,
    ) -> Result<Vec<Outcome>> {
        self.check_revision(expected_revision)?;
        self.rerouted = Default::default();
        let count = operations.len();
        if count == 0 {
            return Err(Error::Invalid(
                "a batch needs at least one operation".into(),
            ));
        }
        if count > MAX_BATCH {
            return Err(Error::Limit(format!(
                "a batch takes at most {MAX_BATCH} operations, not {count}"
            )));
        }
        // One kind of operation names the step; a mix is several changes.
        let project = &self.document.project;
        let mut step = operations
            .first()
            .map(|o| Step::of(o, project))
            .unwrap_or(Step {
                title: Step::MIXED,
                area: Area::Schematic,
                subject: None,
            });
        if operations.iter().any(|o| o.title() != step.title) {
            step.title = Step::MIXED;
        }
        if operations
            .iter()
            .any(|o| Step::of(o, project).subject != step.subject)
        {
            step.subject = None;
        }
        // The steps run on a copy without history; only the end commits.
        let mut scratch = Engine {
            document: self.document.clone(),
            revision: self.revision,
            waiver_reason_required: self.waiver_reason_required,
            copper: self.copper.clone(),
            ..Engine::default()
        };
        let mut outcomes = Vec::with_capacity(count);
        let mut rerouted = layout::router::reroute::Rerouted::default();
        for (index, operation) in operations.into_iter().enumerate() {
            let at = scratch.revision;
            let outcome = scratch
                .apply_inner(at, operation, evaluators)
                .map_err(|error| error.in_batch(index, count))?;
            rerouted.extend(std::mem::take(&mut scratch.rerouted));
            outcomes.push(outcome);
        }
        if scratch.revision != self.revision {
            self.commit(scratch.document, step)?;
        }
        self.rerouted = rerouted;
        Ok(outcomes)
    }

    /// The preview of an assembly update from `read`, applied on a copy of
    /// this engine with the linked design winning every item both changed:
    /// the rows that would change, and, when both files changed since they
    /// last agreed, both sides' changes and the items to choose
    /// (`interface::assembly_link`). Changes nothing here.
    pub fn assembly_preview(
        &self,
        read: interface::assembly_link::AssemblyRead,
        evaluators: Evaluators<'_>,
    ) -> interface::assembly_link::AssemblyPreview {
        use interface::assembly_link as link;
        let project = self.project();
        let mut fork = self.fork();
        let revision = fork.revision();
        let hash = read.file_sha256;
        let conflicts = link::conflicts(project, &read);
        let resolve = link::linked_design_wins(&conflicts);
        match fork.apply_hosted(
            revision,
            Operation::Assembly(link::AssemblyCommand::RecordUpdate {
                read: read.clone(),
                resolve,
            }),
            evaluators,
        ) {
            Ok(_) => link::with_both_sides(
                project,
                &read,
                conflicts,
                link::diff(project, fork.project(), revision, hash),
            ),
            Err(error) => link::refused(project, revision, hash, &error),
        }
    }

    /// A copy of this engine, history included, to try requests on without
    /// touching this one (an MCP `dry_run`). Documents share their sections,
    /// so the copy costs little.
    pub fn fork(&self) -> Engine {
        Engine {
            document: self.document.clone(),
            revision: self.revision,
            undo: self.undo.clone(),
            redo: self.redo.clone(),
            last_step: self.last_step,
            waiver_reason_required: self.waiver_reason_required,
            copper: self.copper.clone(),
            rerouted: self.rerouted.clone(),
        }
    }

    fn apply_inner(
        &mut self,
        expected_revision: u64,
        operation: Operation,
        evaluators: Evaluators<'_>,
    ) -> Result<Outcome> {
        let evaluator = evaluators.footprint;
        self.check_revision(expected_revision)?;
        self.rerouted = Default::default();
        let step = Step::of(&operation, &self.document.project);
        let mut rerouted = layout::router::reroute::Rerouted::default();
        let mut candidate = self.document.clone();
        let reviewed = matches!(operation, Operation::Update(_));
        let merge_named = matches!(&operation, Operation::Update(u) if u.merge_named_nets);
        // Only the assembly link's own commands move the board-sketch
        // geometry its references own.
        let assembly_command = matches!(operation, Operation::Assembly(_));
        // Recording a publish is no undo step, like a save: the files it
        // records stay written (docs/architecture/board-interface.md).
        let unrecorded = matches!(
            operation,
            Operation::Interface(interface::InterfaceCommand::RecordPublish { .. })
        );
        // Parts the operation moves by hand: their attached copper is
        // carried with them, all at once.
        // Selected loose copper a group drag slides with its parts; the
        // carry moves it and re-routes it with the rest.
        let loose = match &operation {
            Operation::Layout(layout::LayoutCommand::MoveParts {
                copper: Some(copper),
                ..
            }) => Some(copper.clone()),
            _ => None,
        };
        let moved_parts: Vec<Id> = match &operation {
            Operation::Layout(layout::LayoutCommand::Move { instance, .. }) => vec![*instance],
            Operation::Layout(layout::LayoutCommand::MoveParts { moves, .. }) => {
                moves.iter().map(|m| m.instance).collect()
            }
            Operation::Layout(
                layout::LayoutCommand::Align { instances, .. }
                | layout::LayoutCommand::Distribute { instances, .. },
            ) => instances.clone(),
            _ => Vec::new(),
        };
        let outcome = match operation {
            Operation::Library(command) => {
                commands::apply_library(&mut candidate, command, evaluator)?
            }
            Operation::Instance(command) => {
                if command.restates(&candidate.project)? {
                    Outcome::None
                } else {
                    commands::apply_instance(Arc::make_mut(&mut candidate.project), command)?
                }
            }
            // The project's own name again: no change and no undo step.
            Operation::Schematic(schematic::SchematicCommand::RenameProject { name })
                if name.trim() == candidate.project.name =>
            {
                Outcome::None
            }
            Operation::Schematic(command) => {
                match schematic::apply(
                    Arc::make_mut(&mut candidate.project),
                    command,
                    EditPolicy::Direct,
                )? {
                    schematic::Applied::None => Outcome::None,
                    schematic::Applied::Unit(occurrence) => Outcome::UnitPlaced { occurrence },
                    schematic::Applied::Label(label) => Outcome::LabelPlaced { label },
                    schematic::Applied::Junction(junction) => Outcome::JunctionPlaced { junction },
                    schematic::Applied::Sheet(sheet) => Outcome::SheetAdded { sheet },
                    schematic::Applied::SheetSymbol { symbol, child } => {
                        Outcome::SheetSymbolPlaced { symbol, child }
                    }
                    schematic::Applied::SheetPins(pins) => Outcome::SheetPinsAdded { pins },
                    schematic::Applied::Drawing(drawing) => Outcome::DrawingAdded { drawing },
                    schematic::Applied::Annotated(changes) => Outcome::Annotated { changes },
                    schematic::Applied::Pasted(report) => Outcome::Pasted { report },
                }
            }
            // Restating the current fabrication changes nothing, so it
            // takes no revision or undo step.
            Operation::Layout(layout::LayoutCommand::SetFabrication(fabrication))
                if fabrication == candidate.project.fabrication =>
            {
                Outcome::None
            }
            Operation::Layout(layout::LayoutCommand::RouteStep { request }) => {
                let project = Arc::make_mut(&mut candidate.project);
                let plan = layout::route_step(project, &request)?;
                layout::validate(project)?;
                Outcome::RouteStepped {
                    end: plan.end,
                    end_layer: plan.end_layer,
                    reached: plan.reached,
                    detour_factor: plan.detour_factor,
                }
            }
            Operation::Layout(layout::LayoutCommand::TrackDrag { request }) => {
                let project = Arc::make_mut(&mut candidate.project);
                let original = project
                    .tracks
                    .iter()
                    .find(|t| t.id == request.track)
                    .cloned();
                let plan = layout::track_drag(project, &request)?;
                layout::validate(project)?;
                Outcome::TrackDragged {
                    offset: original
                        .as_ref()
                        .map_or(0.0, |t| layout::router::drag::dragged_by(t, &plan)),
                    reached: plan.reached,
                }
            }
            // Align and distribute ask the host's board evaluator which
            // parts their dimensions hold.
            Operation::Layout(layout::LayoutCommand::Align { instances, to }) => {
                let project = Arc::make_mut(&mut candidate.project);
                layout::arrange::align(project, &instances, to, evaluators.board)?;
                layout::validate(project)?;
                Outcome::None
            }
            Operation::Layout(layout::LayoutCommand::Distribute { instances, axis }) => {
                let project = Arc::make_mut(&mut candidate.project);
                layout::arrange::distribute(project, &instances, axis, evaluators.board)?;
                layout::validate(project)?;
                Outcome::None
            }
            // Parts named with the copper stay; the outcome names them.
            Operation::Layout(layout::LayoutCommand::DeleteObjects { objects }) => {
                let project = Arc::make_mut(&mut candidate.project);
                let parts = layout::several::delete(project, &objects)?;
                layout::validate(project)?;
                if parts.is_empty() {
                    Outcome::None
                } else {
                    Outcome::PartsKept { parts }
                }
            }
            Operation::Layout(command) => {
                let part = command.places_part();
                let before = candidate.project.library.components.len();
                let project = Arc::make_mut(&mut candidate.project);
                match layout::apply_staged(project, command)? {
                    Some(instance) if part => {
                        let found = project.instance(instance);
                        Outcome::PartPlaced {
                            instance,
                            designator: found.map(|i| i.designator.clone()).unwrap_or_default(),
                            component: found.map(|i| i.component).unwrap_or_default(),
                            created: project.library.components.len() > before,
                        }
                    }
                    Some(via) => Outcome::ViaAdded { via },
                    None => Outcome::None,
                }
            }
            Operation::Rules(command) => {
                if let rules::RulesCommand::AssignNets { nets, .. } = &command
                    && nets.iter().any(|n| candidate.project.net(*n).is_none())
                {
                    return Err(Error::NotFound(
                        "net_class.net_missing: a chosen net no longer exists".into(),
                    ));
                }
                // A pair or group names nets that exist (checked before
                // the nets follow-up, which would drop it).
                layout::length::check_command_nets(&candidate.project, &command)?;
                // A command that leaves the rules as they are (the same
                // profile again, an override to its current value) is not
                // an undo step.
                let mut next = candidate.project.rules.clone();
                let added = rules::apply_added(&mut next, command)?;
                if next != candidate.project.rules {
                    let project = Arc::make_mut(&mut candidate.project);
                    project.rules = next;
                    // A sketch board follows `board.outline_fit`.
                    layout::outline::refit(project, evaluators.board)?;
                }
                match added {
                    Some(rules::signal::Added::Class(class)) => Outcome::NetClassAdded { class },
                    Some(rules::signal::Added::Pair(pair)) => Outcome::DiffPairAdded { pair },
                    Some(rules::signal::Added::Group(group)) => Outcome::LengthGroupAdded { group },
                    None => Outcome::None,
                }
            }
            Operation::Fill(command) => {
                let project = Arc::make_mut(&mut candidate.project);
                let adds_keepout = matches!(command, layout::fill::FillCommand::AddKeepout { .. });
                match layout::fill::apply(project, command)? {
                    Some(keepout) if adds_keepout => Outcome::KeepoutAdded { keepout },
                    Some(fill) => Outcome::FillAdded { fill },
                    None => Outcome::None,
                }
            }
            Operation::Board(command) => {
                if let layout::outline::BoardCommand::Edit { edits } = &command {
                    interface::assembly_link::guard_edits(
                        &self.document.project,
                        edits,
                        evaluators.board,
                    )?;
                }
                let project = Arc::make_mut(&mut candidate.project);
                match layout::outline::apply(project, command, evaluators.board)? {
                    layout::outline::BoardApplied::Edited { created } => {
                        Outcome::BoardEdited { created }
                    }
                    layout::outline::BoardApplied::Nothing => Outcome::None,
                }
            }
            Operation::Dimension(command) => {
                let project = Arc::make_mut(&mut candidate.project);
                match layout::dimensions::apply(project, command, evaluators.board)? {
                    layout::dimensions::DimensionApplied::Added(dimension) => {
                        Outcome::DimensionAdded { dimension }
                    }
                    layout::dimensions::DimensionApplied::Nothing => Outcome::None,
                }
            }
            Operation::Fabrication(command) => {
                let mut next = candidate.project.production.clone();
                fabrication::preflight::apply(&mut next, command, self.waiver_reason_required)?;
                if next != candidate.project.production {
                    Arc::make_mut(&mut candidate.project).production = next;
                }
                Outcome::None
            }
            Operation::Interface(command) => {
                if let Some(table) = interface::apply::apply(&candidate.project, command)? {
                    Arc::make_mut(&mut candidate.project).interface = table.into();
                }
                Outcome::None
            }
            Operation::Assembly(command) => {
                let project = Arc::make_mut(&mut candidate.project);
                interface::assembly_link::apply(project, command, evaluators.board)?;
                Outcome::None
            }
            Operation::Update(update) => {
                candidate.project = Arc::new(updates::candidate(
                    &self.document.project,
                    &self.copper_graph(),
                    self.revision,
                    *update,
                    evaluators.board,
                )?);
                Outcome::None
            }
        };
        if !Arc::ptr_eq(&candidate.project, &self.document.project) {
            let project = Arc::make_mut(&mut candidate.project);
            schematic::tidy(project)?;
            let changes = nets::reconcile(project)?;
            // Explicit net-class assignments follow their nets through
            // merges and splits; a net that no longer exists takes its
            // assignment with it.
            // Pairs and length groups follow them too.
            let rules_ = &project.rules;
            if !rules_.assignments.is_empty()
                || !rules_.diff_pairs.is_empty()
                || !rules_.length_groups.is_empty()
            {
                let live: std::collections::BTreeSet<Id> =
                    project.nets.iter().map(|n| n.id).collect();
                rules::netclass::follow_nets(
                    &mut project.rules,
                    &changes.merged,
                    &changes.split,
                    &changes.created,
                    &live,
                );
                rules::signal::follow_nets(&mut project.rules, &changes.merged, &live);
            }
            // Dimensioned parts, fills and keepouts are solved after the
            // board (and a move that cannot land is refused); a solved
            // part, like a part moved by hand, carries the copper attached
            // to its pads.
            let mut placed =
                layout::dimensions::follow(&self.document.project, project, evaluators.board)?;
            for instance in moved_parts {
                if !placed.contains(&instance) {
                    placed.push(instance);
                }
            }
            if !placed.is_empty() {
                rerouted = layout::dimensions::carry(
                    &self.document.project,
                    project,
                    &self.copper_graph(),
                    &placed,
                    loose.as_ref(),
                )?;
            }
            // Board-following fills take the (possibly changed) outline.
            layout::fill::follow_board(project)?;
            if !reviewed {
                schematic::protect_routed_nets(&self.document.project, project)?;
            }
            // Every operation, from any surface: two named nets become one
            // only when a reviewed update asks for it.
            if !merge_named {
                nets::refuse_named_merges(&self.document.project, project)?;
            }
            // The interface's automatic datums follow the board: a new
            // hole takes a new slot, a deleted one retires its slot.
            if let Some(table) = interface::auto::follow(&self.document.project, project)? {
                project.interface = table.into();
            }
            // A board edit never moves what the linked design drives, and the
            // loops between the two files follow the edit (slice B5).
            if !assembly_command {
                interface::assembly_link::guard_driven(
                    &self.document.project,
                    project,
                    evaluators.board,
                )?;
            }
            interface::assembly_link::follow_cycles(
                &self.document.project,
                project,
                evaluators.board,
            )?;
            validate::edited_document(
                &self.document.project,
                &candidate.project,
                &candidate.drafts,
            )?;
        } else if candidate.drafts == self.document.drafts {
            return Ok(outcome);
        } else {
            validate::drafts_only(&candidate.project, &candidate.drafts)?;
        }
        if unrecorded {
            self.store_unrecorded(candidate)?;
        } else {
            self.commit(candidate, step)?;
        }
        self.rerouted = rerouted;
        Ok(outcome)
    }

    /// Answer a read-only query.
    pub fn query(&self, query: Query) -> Result<QueryResult> {
        self.query_with(query, &catalog::CatalogContext::default())
    }

    /// Answer a read-only query with the host's catalog context (favourite
    /// and recently used lineages), which ranks `library.search` results.
    pub fn query_with(
        &self,
        query: Query,
        context: &catalog::CatalogContext,
    ) -> Result<QueryResult> {
        self.query_using(query, context, None)
    }

    /// [`query_with`](Self::query_with) with the host's footprint
    /// evaluator, which `library.footprint_preview` needs.
    pub fn query_using(
        &self,
        query: Query,
        context: &catalog::CatalogContext,
        evaluator: Option<&dyn library::FootprintEvaluator>,
    ) -> Result<QueryResult> {
        let index = || {
            catalog::Index::with_sources(
                &self.document.project,
                &self.document.drafts,
                &context.sources,
            )
        };
        match query {
            Query::Search {
                query,
                limit,
                include_drafts,
            } => {
                let index = index()?;
                let parsed = index.parse(&query)?;
                Ok(QueryResult::Search(index.search(
                    &parsed,
                    context,
                    catalog::SearchOptions {
                        limit: limit.unwrap_or(catalog::DEFAULT_LIMIT),
                        include_drafts,
                    },
                )))
            }
            Query::Facets {
                query,
                include_drafts,
            } => {
                let index = index()?;
                let parsed = index.parse(&query)?;
                Ok(QueryResult::Facets(index.facets(&parsed, include_drafts)))
            }
            Query::InspectLineage { lineage, revision } => {
                Ok(QueryResult::Lineage(Box::new(catalog::inspect(
                    &self.document.project,
                    &self.document.drafts,
                    lineage,
                    revision,
                )?)))
            }
            Query::Bom {} => Ok(QueryResult::Bom(completeness::bom(&self.document.project)?)),
            Query::ResolveGroups {} => Ok(QueryResult::ResolveGroups(
                completeness::resolve_groups(&self.document.project)?,
            )),
            Query::InspectInstance { instance } => Ok(QueryResult::Instance(Box::new(
                completeness::instance(&self.document.project, instance)?,
            ))),
            Query::CheckDefinition { target } => Ok(QueryResult::Definition(
                completeness::definition(&self.document.project, &self.document.drafts, target)?,
            )),
            Query::CheckDrafts {} => Ok(QueryResult::Drafts(completeness::drafts(
                &self.document.project,
                &self.document.drafts,
            )?)),
            Query::FootprintPreview { draft, edits } => {
                Ok(QueryResult::FootprintPreview(Box::new(
                    commands::preview_footprint(&self.document, draft, &edits, evaluator)?,
                )))
            }
            Query::UpdatesAvailable {} => Ok(QueryResult::UpdatesAvailable(
                catalog::updates_available(&self.document.project.library, &context.sources)?,
            )),
            Query::StandardFields {} => Ok(QueryResult::StandardFields(Box::new(
                library::standard_fields::bundle()?,
            ))),
            Query::PromotePreview {
                root,
                library,
                name,
                fresh,
            } => Ok(QueryResult::Promote(Box::new(library::import::export(
                &self.document.project.library,
                root,
                library,
                &name,
                &fresh,
            )?))),
            Query::RulesEffective {} => Ok(QueryResult::Rules(Box::new(rules::report(
                &self.document.project.rules,
            )))),
            Query::RuleProfiles {} => Ok(QueryResult::RuleProfiles(
                self.document
                    .project
                    .rules
                    .profile
                    .iter()
                    .cloned()
                    .collect(),
            )),
            Query::PreviewProfile {
                profile,
                keep_overrides,
            } => {
                let current = &self.document.project;
                let (next, refusal) =
                    rules::preview_candidate(&current.rules, &profile, keep_overrides);
                let mut impact = rules::rule_changes(&current.rules, &next);
                if let Some(reason) = refusal {
                    impact.refusal = Some(reason);
                    return Ok(QueryResult::ProfileImpact(Box::new(impact)));
                }
                let mut candidate = current.as_ref().clone();
                candidate.rules = next;
                let scope = issues::IssueScope::All;
                let rows = |p: &Project| -> Result<Vec<(String, String)>> {
                    issues::report(p, &self.document.drafts, &scope)?
                        .rows
                        .into_iter()
                        .map(|r| {
                            let id = serde_json::to_string(&(&r.finding.code, &r.finding.objects))?;
                            Ok((id, r.title))
                        })
                        .collect()
                };
                rules::diff_issues(&mut impact, rows(current)?, rows(&candidate)?);
                Ok(QueryResult::ProfileImpact(Box::new(impact)))
            }
            Query::NetClasses {} => {
                let project = &self.document.project;
                Ok(QueryResult::NetClasses(Box::new(rules::netclass::report(
                    &project.rules,
                    project.nets.iter().map(|n| (n.id, n.name.as_str())),
                ))))
            }
            Query::PreviewNetClassPatterns { patterns } => {
                let project = &self.document.project;
                Ok(QueryResult::NetClassChanges(
                    rules::netclass::preview_patterns(
                        &project.rules,
                        patterns,
                        project.nets.iter().map(|n| (n.id, n.name.as_str())),
                    )?,
                ))
            }
            Query::BoardShape {} => Ok(QueryResult::BoardShape(Box::new(
                self.document.project.board().into_owned(),
            ))),
            Query::Preflight { profile } => {
                let profile = profile
                    .map(fabrication::ProfileChoice::into_profile)
                    .transpose()?;
                Ok(QueryResult::Preflight(Box::new(
                    fabrication::preflight::report(
                        &self.document.project,
                        &self.document.drafts,
                        profile.as_ref(),
                    )?,
                )))
            }
            Query::InterfaceList {} => Ok(QueryResult::InterfaceList(Box::new(
                interface::list::list(&self.document.project)?,
            ))),
            Query::AssemblyStatus {} => Ok(QueryResult::AssemblyStatus(Box::new(
                interface::assembly_link::status(
                    &self.document.project,
                    interface::assembly_link::FileCheck::NotChecked,
                ),
            ))),
            Query::FillStatus {} => Ok(QueryResult::FillStatus(layout::fill::status(
                &self.document.project,
            ))),
            Query::Sheets {} => Ok(QueryResult::Sheets(schematic::sheet_rows(
                &self.document.project,
            ))),
            Query::SheetCopy { sheet, objects } => Ok(QueryResult::SheetClip(Box::new(
                schematic::clipboard::copy(
                    &self.document.project,
                    sheet.unwrap_or_else(|| self.document.project.sheets.first()),
                    &objects,
                )?,
            ))),
            Query::NetPlaces { net } => Ok(QueryResult::NetPlaces(Box::new(
                schematic::net_places(&self.document.project, net)?,
            ))),
            Query::SheetScene { sheet } => {
                if self.document.project.sheet(sheet).is_none() {
                    return Err(Error::NotFound("Unknown sheet.".into()));
                }
                Ok(QueryResult::SheetScene(Box::new(
                    schematic::scene::sheet_scene(&self.document.project, sheet),
                )))
            }
            Query::ImpedanceCalculate { request } => Ok(QueryResult::Impedance(Box::new(
                layout::impedance::calculate(&self.document.project, &request)?,
            ))),
            Query::ImpedanceTable {} => Ok(QueryResult::ImpedanceTable(Box::new(
                layout::impedance::table(&self.document.project),
            ))),
            Query::SuggestPairs {} => {
                let project = &self.document.project;
                Ok(QueryResult::PairSuggestions(rules::signal::suggest(
                    &project.rules,
                    project.nets.iter().map(|n| (n.id, n.name.as_str())),
                )))
            }
            Query::LengthReport {} => Ok(QueryResult::LengthReport(Box::new(
                layout::length::report_with(&self.document.project, &self.copper_graph()),
            ))),
            Query::RoutePreview { request } => {
                request.check()?;
                let project = &self.document.project;
                let index = layout::router::RouteIndex::with_graph(project, self.copper_graph());
                Ok(QueryResult::RoutePreview(Box::new(layout::router::plan(
                    project, &index, &request,
                ))))
            }
            Query::TrackDragPreview { request } => {
                request.check()?;
                let project = &self.document.project;
                let index = layout::router::RouteIndex::with_graph(project, self.copper_graph());
                Ok(QueryResult::RoutePreview(Box::new(
                    layout::router::drag::drag_plan(project, &index, &request),
                )))
            }
            Query::TunePreview { request } => {
                let project = &self.document.project;
                let obstacles = layout::obstacles::Obstacles::of(project);
                Ok(QueryResult::TunePreview(Box::new(
                    layout::meander::plan(project, &obstacles, &self.copper_graph(), &request)?
                        .preview(),
                )))
            }
            Query::Issues { scope } => Ok(QueryResult::Issues(issues::report_on(
                &self.document.project,
                &self.document.drafts,
                &scope,
                &self.copper_graph(),
            )?)),
            Query::Templates { query, limit } => {
                Ok(QueryResult::Templates(catalog::templates::search(
                    &self.document.project,
                    &self.document.drafts,
                    context,
                    &query,
                    limit
                        .unwrap_or(catalog::DEFAULT_LIMIT)
                        .min(catalog::templates::MAX_TEMPLATE_HITS),
                )?))
            }
            Query::GenericParts { query } => Ok(QueryResult::GenericParts(
                catalog::generic::search(&self.document.project, &self.document.drafts, &query),
            )),
            Query::PackagePlan { request } => Ok(QueryResult::PackagePlan(
                commands::templates::package_plan(&self.document, &request, evaluator)?,
            )),
            Query::Models { query, fits, limit } => {
                Ok(QueryResult::Models(catalog::models::search(
                    &self.document.project,
                    &self.document.drafts,
                    context,
                    &query,
                    fits,
                    limit
                        .unwrap_or(catalog::DEFAULT_LIMIT)
                        .min(catalog::models::MAX_MODEL_HITS),
                )?))
            }
            Query::PinoutPreview { target, document } => Ok(QueryResult::PinoutPlan(Box::new(
                commands::pinout::preview(&self.document, &target, &document)?,
            ))),
            Query::SuggestPins { instance, request } => Ok(QueryResult::SuggestPins(
                pins::suggest(&self.document.project, instance, &request)?,
            )),
            Query::PinReport { instance } => Ok(QueryResult::PinReport(Box::new(
                pins::pin_report(&self.document.project, instance)?,
            ))),
            Query::PinConfig { instance } => Ok(QueryResult::PinConfig(Box::new(
                firmware::pin_config(&self.document.project, instance)?,
            ))),
            Query::Marking { instance } => Ok(QueryResult::Marking(commands::marking::layout(
                &self.document.project,
                instance,
            )?)),
        }
    }

    pub fn preview_update(&self, change: updates::Change) -> updates::UpdatePlan {
        self.preview_update_hosted(change, Evaluators::default())
    }

    /// [`Self::preview_update`] with the host's evaluators, which trace the
    /// 3D model faces part dimensions name in the new package revisions and
    /// solve the dimensioned parts, as applying the update does.
    pub fn preview_update_hosted(
        &self,
        change: updates::Change,
        evaluators: Evaluators<'_>,
    ) -> updates::UpdatePlan {
        updates::preview(
            &self.document.project,
            &self.copper_graph(),
            self.revision,
            change,
            evaluators.board,
        )
        .0
    }

    pub fn apply_update(
        &mut self,
        expected_revision: u64,
        change: updates::Change,
        remove_conflicting_tracks: bool,
    ) -> Result<()> {
        self.apply(
            expected_revision,
            Operation::Update(Box::new(updates::ApplyUpdate {
                expected_revision,
                change,
                remove_conflicting_tracks,
                merge_named_nets: false,
            })),
        )
        .map(|_| ())
    }

    /// Store fill copper a host computed off the UI thread
    /// (`layout::fill::compute_stale`) without an undo step: the copper is
    /// derivable, so it belongs to the revision it was computed for, not
    /// to the history. Refused when `expected_revision` is not current;
    /// results whose key no longer matches their fill are skipped.
    /// Returns how many fills were stored.
    pub fn store_fills(
        &mut self,
        expected_revision: u64,
        results: Vec<(Id, model::fill::FilledCopper)>,
    ) -> Result<usize> {
        self.check_revision(expected_revision)?;
        // Only the stored fills' keys (and those they depend on).
        let project = &self.document.project;
        let ids: Vec<Id> = results
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| project.fill(*id).is_some())
            .collect();
        let keys = layout::fill::inputs_keys_of(project, ids)?;
        let fresh: Vec<_> = results
            .into_iter()
            .filter(|(id, c)| keys.get(id) == Some(&c.key))
            .collect();
        if fresh.is_empty() {
            return Ok(0);
        }
        let mut project = (*self.document.project).clone();
        for (id, copper) in &fresh {
            if let Some(f) = project.fills.iter_mut().find(|f| f.id == *id) {
                f.computed = Some(copper.clone());
            }
        }
        layout::fill::validate(&project)?;
        let revision = self.next_revision()?;
        self.document.project = Arc::new(project);
        self.set_revision(revision);
        Ok(fresh.len())
    }

    /// Refused as stale unless `expected_revision` is the current one.
    /// Every document change, undo and redo included, is checked here, so a
    /// caller never changes a document that moved since it last looked.
    pub fn check_revision(&self, expected_revision: u64) -> Result<()> {
        if expected_revision != self.revision {
            return Err(Error::Stale {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        Ok(())
    }

    /// The person's own undo in the window that shows this document: at
    /// the current revision, which is what they see. Transports and agents
    /// use [`Engine::undo_at`].
    pub fn undo(&mut self) -> Result<()> {
        self.undo_at(self.revision)
    }

    /// The person's own redo; see [`Engine::undo`].
    pub fn redo(&mut self) -> Result<()> {
        self.redo_at(self.revision)
    }

    /// Restore the document before the latest step, at an expected
    /// revision: refused, with nothing changed, when the document moved
    /// since, so someone else's newer step is never undone.
    pub fn undo_at(&mut self, expected_revision: u64) -> Result<()> {
        self.check_revision(expected_revision)?;
        let revision = self.next_revision()?;
        let (mut previous, step) = self
            .undo
            .back()
            .cloned()
            .ok_or_else(|| Error::History("nothing to undo".into()))?;
        Self::carry_interface(&self.document, &mut previous)?;
        self.undo.pop_back();
        self.redo
            .push((std::mem::replace(&mut self.document, previous), step));
        self.set_revision(revision);
        self.last_step = None;
        Ok(())
    }

    /// Reapply the latest undone step at an expected revision, as for
    /// [`Engine::undo_at`].
    pub fn redo_at(&mut self, expected_revision: u64) -> Result<()> {
        self.check_revision(expected_revision)?;
        let revision = self.next_revision()?;
        let (mut next, step) = self
            .redo
            .last()
            .cloned()
            .ok_or_else(|| Error::History("nothing to redo".into()))?;
        Self::carry_interface(&self.document, &mut next)?;
        self.redo.pop();
        self.undo
            .push_back((std::mem::replace(&mut self.document, next), step));
        self.set_revision(revision);
        self.last_step = None;
        Ok(())
    }

    /// An undo or redo moves from `current` to `restored`: the interface's
    /// slot and datum counters keep their highest values, so a slot or id
    /// handed out once is never handed out again
    /// (`interface::auto::carry`).
    fn carry_interface(current: &Document, restored: &mut Document) -> Result<()> {
        if let Some(table) = interface::auto::carry(&current.project.interface, &restored.project)?
        {
            Arc::make_mut(&mut restored.project).interface = table.into();
        }
        Ok(())
    }

    /// Fold the latest undo step into the one before it, so one undo takes
    /// both back: a step that finishes the one before (placing the part a
    /// search or template has just created). `at` is the revision the
    /// latest step was applied at, which the earlier step made. Refused,
    /// with nothing changed, after an undo or redo, when another step came
    /// between, or with fewer than two steps.
    pub fn join_last_step(&mut self, at: u64) -> Result<()> {
        if self.last_step != Some(at) || self.undo.len() < 2 {
            return Err(Error::History(
                "the latest step does not follow the one before".into(),
            ));
        }
        // The document between the two steps goes; the one before both
        // stays the undo target, named by the latest step.
        if let Some((_, step)) = self.undo.pop_back()
            && let Some(earlier) = self.undo.back_mut()
        {
            earlier.1 = step;
        }
        self.last_step = None;
        Ok(())
    }

    /// The copper graph of the current revision: built once, on first use,
    /// and shared by the checks, the Issues query and the desktop.
    pub fn copper_graph(&self) -> Arc<layout::CopperGraph> {
        self.copper
            .get_or_init(|| Arc::new(layout::CopperGraph::of(&self.document.project)))
            .clone()
    }

    /// Move to `revision`: every derived result of the old one is dropped.
    fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
        self.copper = std::sync::OnceLock::new();
    }

    fn next_revision(&self) -> Result<u64> {
        self.revision
            .checked_add(1)
            .ok_or_else(|| Error::Limit("revision exhausted".into()))
    }

    /// Store `candidate` as a new revision without an undo step; undo and
    /// redo keep what it recorded (`interface::auto::carry`), and the
    /// steps around it stay as they were.
    fn store_unrecorded(&mut self, candidate: Document) -> Result<()> {
        let revision = self.next_revision()?;
        self.document = candidate;
        self.last_step = None;
        self.set_revision(revision);
        Ok(())
    }

    fn commit(&mut self, candidate: Document, step: Step) -> Result<()> {
        let revision = self.next_revision()?;
        if self.undo.len() >= MAX_UNDO {
            self.undo.pop_front();
        }
        self.undo
            .push_back((std::mem::replace(&mut self.document, candidate), step));
        self.redo.clear();
        self.last_step = Some(self.revision);
        self.set_revision(revision);
        Ok(())
    }
}

/// JSON schemas of the document types every transport shares.
pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "operation": schemars::schema_for!(Operation),
        "outcome": schemars::schema_for!(Outcome),
        "query": schemars::schema_for!(Query),
        "query_result": schemars::schema_for!(QueryResult),
        "project": schemars::schema_for!(Project),
        "update_plan": schemars::schema_for!(updates::UpdatePlan),
        "editor_file": schemars::schema_for!(format::EditorFile),
        "finding": schemars::schema_for!(Finding),
        "diagnostic": schemars::schema_for!(Diagnostic),
        "pinout": library::pinout::schema(),
        "pin_config": firmware::schema(),
    })
}
