//! Pcbcraft's electronics document and its single typed command engine.
//!
//! This crate is pure and wasm-compatible: no files, threads, windows or
//! transports. `pcbcraft-io` reads and writes documents; transports (UI, CLI,
//! MCP) translate requests into [`Operation`]s applied by [`Engine`].
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
pub mod issues;
pub mod layout;
pub mod library;
pub mod materials;
pub mod model;
pub mod nets;
pub mod pins;
pub mod quantity;
pub mod rules;
pub mod schematic;
pub mod scripting;
#[cfg(test)]
mod tests;
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
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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
    /// Pre-flight waivers for fabrication output (`fabrication.*`).
    Fabrication(fabrication::FabricationCommand),
    Update(Box<updates::ApplyUpdate>),
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
    /// one-line query: free text, `pkg:`/`has:`/`missing:` filters and
    /// unit-aware field predicates such as `res>=10k`, `tol<=1%` or
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
    /// The project BOM: resolved instances grouped by Part, unresolved ones
    /// by component revision, footprint and identical requirements, each
    /// line marked when it is unresolved.
    #[serde(rename = "project.bom")]
    Bom {},
    /// Groups of instances without a Part (or with an unresolved one) that
    /// share requirements and footprint, each with the `library.search`
    /// query that finds Parts for it.
    #[serde(rename = "library.resolve_groups")]
    ResolveGroups {},
    /// What `library.edit_footprint` would produce for these edits (pads,
    /// graphics, findings, including blocking ones), without changing the
    /// document. Needs the host's footprint evaluator.
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
    /// Every issue of the project and its drafts in `scope`, with the
    /// severity, group and one-line title core gives each finding
    /// (docs/sidebar-ia.md): uncapped, most severe first, with same-cause
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
    /// C2: per terminal of one instance: pads, GPIO, active function,
    /// peripheral and role, mux code, pin config, net and every other
    /// endpoint on the net.
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
    /// The built-in rule profiles (hosts append the user library).
    #[serde(rename = "rules.profiles")]
    RuleProfiles {},
    /// What `rules.set_profile` with these arguments would change: rules
    /// and the board issues that would appear or go.
    #[serde(rename = "rules.preview_profile")]
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
    /// terminals with sheet and position, in page order (jump between a
    /// label's other occurrences).
    #[serde(rename = "schematic.net_places")]
    NetPlaces { net: Id },
    /// Everything one sheet draws, resolved to shapes and text in sheet
    /// millimetres: page frame and title block, labels, power ports, sheet
    /// symbols, furniture, wires, junction dots and units.
    #[serde(rename = "schematic.scene")]
    SheetScene { sheet: SheetId },
    /// Z0 (and, with a gap, the edge-coupled Zdiff) of a trace on a
    /// stackup layer or an explicit structure, with the model's accuracy.
    #[serde(rename = "impedance.calculate")]
    ImpedanceCalculate {
        request: layout::impedance::ImpedanceRequest,
    },
    /// Every layer's line structure and, for each net class with an
    /// impedance target, the width and pair gap it needs on each layer
    /// and why a target cannot be met there.
    #[serde(rename = "impedance.table")]
    ImpedanceTable {},
    /// Differential pairs proposed from net names (`USB_D+`/`USB_D-`,
    /// `_P`/`_N`, `DP`/`DM`); `diff_pair.add` makes one.
    #[serde(rename = "diff_pair.suggest")]
    SuggestPairs {},
    /// Every routed, paired or grouped net's length (tracks plus via
    /// barrels), its target and deviation; every pair's skew and
    /// uncoupled length; every length group's window.
    #[serde(rename = "layout.length_report")]
    LengthReport {},
    /// What `layout.route_step` would do, without applying it: the edit
    /// set (the step's copper and every shoved track and via), where the
    /// head ends and why it stopped short.
    #[serde(rename = "layout.route_preview")]
    RoutePreview {
        request: layout::router::RouteStepRequest,
    },
    /// What `layout.tune_length` would build, without applying it.
    #[serde(rename = "layout.tune_preview")]
    TunePreview {
        request: layout::meander::TuneRequest,
    },
    /// The production check before fabrication export: the design checked
    /// under `profile` (a profile id such as `std-6-6` or a whole profile;
    /// the project's rules when omitted) plus production findings, each
    /// with its waiver key and saved waiver.
    #[serde(rename = "fabrication.preflight")]
    Preflight {
        #[serde(default)]
        profile: Option<fabrication::ProfileChoice>,
    },
}

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
    Issues(issues::IssueReport),
    Templates(catalog::templates::TemplateReport),
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
    SheetScene(Box<schematic::scene::SheetScene>),
    Impedance(Box<layout::impedance::ImpedanceResult>),
    ImpedanceTable(Box<layout::impedance::ImpedanceTable>),
    PairSuggestions(Vec<rules::signal::PairSuggestion>),
    LengthReport(Box<layout::length::LengthReport>),
    TunePreview(Box<layout::meander::TunePreview>),
    RoutePreview(Box<layout::router::RoutePlan>),
    Preflight(Box<fabrication::PreflightReport>),
}

/// The host engines a command may need. The core never interprets a
/// sketch; these do, on the candidate only.
#[derive(Clone, Copy, Default)]
pub struct Evaluators<'a> {
    pub footprint: Option<&'a dyn library::FootprintEvaluator>,
    pub board: Option<&'a dyn layout::outline::BoardShapeEvaluator>,
}

/// Undo depth. Entries share unchanged data, so this bounds bookkeeping, not
/// copies of the whole project.
pub const MAX_UNDO: usize = 100;

/// The document, its revision and one shared undo history.
///
/// Every mutation is staged on a candidate and validated before it replaces
/// the document, so a failed operation leaves document, revision and history
/// unchanged.
#[derive(Default)]
pub struct Engine {
    document: Document,
    revision: u64,
    undo: VecDeque<Document>,
    redo: Vec<Document>,
}

impl Engine {
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
    /// Unconnected terminals are marked no-connect. The undo history is
    /// cleared, as for [`Engine::demo`].
    pub fn showcase() -> Result<Self> {
        Self::showcase_in(None)
    }

    /// The showcase as the shipped file shows it: [`Engine::showcase`]
    /// with a Power net class over GND and VCC and a top GND pour; with a
    /// board evaluator, also a sketched board with rounded corners, a
    /// round cutout, a milled slot and J1 held to the left edge by board
    /// parameters (`layout::showcase::board_edits`). The pours are filled
    /// last, over the final shape. The undo history is cleared.
    pub fn showcase_hosted(evaluators: Evaluators<'_>) -> Result<Self> {
        Self::showcase_in(Some(evaluators))
    }

    fn showcase_in(hosted: Option<Evaluators<'_>>) -> Result<Self> {
        let mut engine = Self::default();
        let mut refs = Vec::new();
        for part in samples::showcase_parts() {
            let lineages = engine.reserve_lineages()?;
            let mut part_edits =
                samples::packaged_symbol(part.name, part.form, part.prefix, part.symbol, lineages);
            samples::trim_heels(&mut part_edits, part.heel_trim_mm);
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
                    position: [column * 40.0, -row * 40.0],
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
            for pair in ends.windows(2) {
                engine.apply(
                    engine.revision,
                    Operation::Schematic(schematic::SchematicCommand::Connect {
                        from: pair[0].into(),
                        to: pair[1].into(),
                    }),
                )?;
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
        let Some(board) = evaluators.board else {
            return Ok(());
        };
        self.apply_hosted(
            self.revision,
            Operation::Board(layout::outline::BoardCommand::StartSketch),
            evaluators,
        )?;
        let model::board::BoardShape::Sketch(sketch) = &self.project().board_shape else {
            return Err(Error::Invalid("showcase board sketch".into()));
        };
        for edits in layout::showcase::board_edits(board, sketch)? {
            self.apply_hosted(
                self.revision,
                Operation::Board(layout::outline::BoardCommand::Edit { edits }),
                evaluators,
            )?;
        }
        let constraint = layout::showcase::j1_constraint(self.project())?;
        self.apply_hosted(
            self.revision,
            Operation::Board(layout::outline::BoardCommand::Constrain { constraint }),
            evaluators,
        )?;
        Ok(())
    }

    /// Two connected two-terminal components, placed on the board, with a
    /// typed resistance field shown on the symbol. The sample library is
    /// built through the public commands and its drafts stay open for
    /// editing.
    pub fn demo() -> Result<Self> {
        let mut engine = Self::default();
        let refs = engine.reserve_lineages()?;
        let kind = engine.reserve(DefinitionKind::QuantityKind)?;
        let field = engine.reserve(DefinitionKind::Field)?;
        let mut edits = samples::two_terminal("Resistor", samples::Kind::RESISTOR, refs);
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
        engine.apply(
            engine.revision,
            Operation::Layout(layout::LayoutCommand::Sync),
        )?;
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

    /// Replace the whole document (open, new, recover). History is cleared;
    /// an invalid file leaves everything unchanged.
    pub fn replace_document(&mut self, file: format::EditorFile) -> Result<()> {
        file.validate()?;
        let revision = self.next_revision()?;
        self.document = file.into_document();
        self.revision = revision;
        self.undo.clear();
        self.redo.clear();
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

    fn apply_inner(
        &mut self,
        expected_revision: u64,
        operation: Operation,
        evaluators: Evaluators<'_>,
    ) -> Result<Outcome> {
        let evaluator = evaluators.footprint;
        if expected_revision != self.revision {
            return Err(Error::Stale {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        let mut candidate = self.document.clone();
        let reviewed = matches!(operation, Operation::Update(_));
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
                    schematic::Applied::Sheet(sheet) => Outcome::SheetAdded { sheet },
                    schematic::Applied::SheetSymbol { symbol, child } => {
                        Outcome::SheetSymbolPlaced { symbol, child }
                    }
                    schematic::Applied::SheetPins(pins) => Outcome::SheetPinsAdded { pins },
                    schematic::Applied::Drawing(drawing) => Outcome::DrawingAdded { drawing },
                    schematic::Applied::Annotated(changes) => Outcome::Annotated { changes },
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
                    Arc::make_mut(&mut candidate.project).rules = next;
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
                let project = Arc::make_mut(&mut candidate.project);
                match layout::outline::apply(project, command, evaluators.board)? {
                    layout::outline::BoardApplied::Constraint(constraint) => {
                        Outcome::EdgeConstraintAdded { constraint }
                    }
                    layout::outline::BoardApplied::Edited { created } => {
                        Outcome::BoardEdited { created }
                    }
                    layout::outline::BoardApplied::Nothing => Outcome::None,
                }
            }
            Operation::Fabrication(command) => {
                let mut next = candidate.project.production.clone();
                fabrication::preflight::apply(&mut next, command)?;
                if next != candidate.project.production {
                    Arc::make_mut(&mut candidate.project).production = next;
                }
                Outcome::None
            }
            Operation::Update(update) => {
                candidate.project = Arc::new(updates::candidate(
                    &self.document.project,
                    self.revision,
                    *update,
                )?);
                Outcome::None
            }
        };
        if !Arc::ptr_eq(&candidate.project, &self.document.project) {
            let project = Arc::make_mut(&mut candidate.project);
            schematic::tidy(project);
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
            // Constrained parts, fills and keepouts follow their board
            // edges (and a move away from one is refused).
            layout::outline::follow(&self.document.project, project, evaluators.board)?;
            if !reviewed {
                schematic::protect_routed_nets(&self.document.project, project)?;
            }
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
        self.commit(candidate)?;
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
            Query::RuleProfiles {} => Ok(QueryResult::RuleProfiles(rules::builtin::all())),
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
            Query::FillStatus {} => Ok(QueryResult::FillStatus(layout::fill::status(
                &self.document.project,
            ))),
            Query::Sheets {} => Ok(QueryResult::Sheets(schematic::sheet_rows(
                &self.document.project,
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
                layout::length::report(&self.document.project),
            ))),
            Query::RoutePreview { request } => {
                request.check()?;
                let project = &self.document.project;
                let index = layout::router::RouteIndex::of(project);
                Ok(QueryResult::RoutePreview(Box::new(layout::router::plan(
                    project, &index, &request,
                ))))
            }
            Query::TunePreview { request } => {
                let project = &self.document.project;
                let obstacles = layout::obstacles::Obstacles::of(project);
                Ok(QueryResult::TunePreview(Box::new(
                    layout::meander::plan(project, &obstacles, &request)?.preview(),
                )))
            }
            Query::Issues { scope } => Ok(QueryResult::Issues(issues::report(
                &self.document.project,
                &self.document.drafts,
                &scope,
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
        updates::preview(&self.document.project, self.revision, change).0
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
        if expected_revision != self.revision {
            return Err(Error::Stale {
                expected: expected_revision,
                actual: self.revision,
            });
        }
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
        self.revision = revision;
        Ok(fresh.len())
    }

    pub fn undo(&mut self) -> Result<()> {
        let revision = self.next_revision()?;
        let previous = self
            .undo
            .pop_back()
            .ok_or_else(|| Error::History("nothing to undo".into()))?;
        self.redo
            .push(std::mem::replace(&mut self.document, previous));
        self.revision = revision;
        Ok(())
    }

    pub fn redo(&mut self) -> Result<()> {
        let revision = self.next_revision()?;
        let next = self
            .redo
            .pop()
            .ok_or_else(|| Error::History("nothing to redo".into()))?;
        self.undo
            .push_back(std::mem::replace(&mut self.document, next));
        self.revision = revision;
        Ok(())
    }

    fn next_revision(&self) -> Result<u64> {
        self.revision
            .checked_add(1)
            .ok_or_else(|| Error::Limit("revision exhausted".into()))
    }

    fn commit(&mut self, candidate: Document) -> Result<()> {
        let revision = self.next_revision()?;
        if self.undo.len() >= MAX_UNDO {
            self.undo.pop_front();
        }
        self.undo
            .push_back(std::mem::replace(&mut self.document, candidate));
        self.redo.clear();
        self.revision = revision;
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
