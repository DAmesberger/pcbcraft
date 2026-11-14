//! A KiCad board's footprints built into a new project through the one
//! command engine: every footprint shape that differs becomes a
//! project-local component set (component, symbol, package, footprint and
//! binding), published; every footprint is placed on a schematic sheet and
//! on the board where KiCad has it. Nothing is written to a document
//! directly: each step is a typed [`Operation`] applied by [`Engine`], and
//! any refused step fails the whole import (the engine is dropped).
use crate::board::{self, Board, Footprint};
use crate::report::{Case, Report};
use crate::translate::{self, DefPad, Mapped};
use sparksmith_core::functions::{Direction, TerminalFunctions};
use sparksmith_core::geometry::Bounds;
use sparksmith_core::layout::LayoutCommand;
use sparksmith_core::layout::fill::FillCommand;
use sparksmith_core::layout::outline::{BoardCommand, BoardEdit};
use sparksmith_core::library::footprint_source::{SketchEdit, SketchRef};
use sparksmith_core::library::samples::named_block_symbol;
use sparksmith_core::library::{
    BindingDefinition, ComponentDefinition, Definition, DefinitionMeta, DraftEdit,
    FootprintDefinition, ModelPlacement, PackageDefinition, PackageForm, PackageModel,
    PackageTerminal, Pad, PadDefaults, PadKind, Terminal, TerminalClass, TerminalMap,
    designator_prefix_valid,
};
use sparksmith_core::materials::MaterialId;
use sparksmith_core::model::board::BoardShape;
use sparksmith_core::model::fill::PadFillOverride;
use sparksmith_core::rules::{
    NetClassId, NetClassPattern, NetClassSpec, RuleKey, RuleProfile, RuleValue, RulesCommand,
};
use sparksmith_core::units::{Nm, PointNm};
use sparksmith_core::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, FootprintRef, FunctionId, Id,
    InstanceCommand, LibraryCommand, Operation, Outcome, PackageRef, PackageTerminalId, PadId,
    PhysicalChoice, Revision, StackupPresets, SymbolRef, TerminalId, UnitId,
};
use sparksmith_core::{Engine, Evaluators, NewBoard, instances::NewInstance};
use std::collections::BTreeMap;

/// Why an import failed. Messages carry no file content.
#[derive(Debug)]
pub enum ImportError {
    Board(board::BoardError),
    /// The engine refused a step: its command and the core's reason.
    Refused {
        step: &'static str,
        reason: String,
    },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Board(e) => e.fmt(f),
            Self::Refused { step, reason } => write!(f, "kicad.refused: {step}: {reason}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<board::BoardError> for ImportError {
    fn from(e: board::BoardError) -> Self {
        Self::Board(e)
    }
}

/// One imported pad, for difference reports: its class and board box.
#[derive(Clone, Debug, PartialEq)]
pub struct PadRecord {
    pub class: String,
    pub world: Bounds,
    /// Round custom-pad primitives on the board (centre, radius, mm).
    pub arcs: Vec<([f64; 2], f64)>,
}

/// The new project, its report and the import map of pads.
pub struct Imported {
    pub engine: Engine,
    /// Where our board origin lies in KiCad's frame, and KiCad's auxiliary
    /// (drill and place) origin, nm: our output is KiCad's plot about the
    /// auxiliary origin moved by `(aux.x − origin.x, origin.y − aux.y)`.
    pub origin_nm: [i64; 2],
    pub aux_origin_nm: Option<[i64; 2]>,
    pub report: Report,
    pub pads: Vec<PadRecord>,
}

impl Imported {
    /// What moves a KiCad plot about the auxiliary origin onto our output,
    /// nm (`kicad-cli … --use-drill-file-origin`).
    pub fn plot_shift_nm(&self) -> [i64; 2] {
        let aux = self.aux_origin_nm.unwrap_or_default();
        [aux[0] - self.origin_nm[0], self.origin_nm[1] - aux[1]]
    }
}

/// The provenance every imported definition records (L1).
const PROVENANCE: &str = "Imported from a KiCad board's embedded footprint copy. Project-only: \
     it may come from KiCad's CC-BY-SA libraries, so never publish it to a shared library.";

/// Schematic spacing of the imported instances, mm.
const SHEET_PITCH_MM: f64 = 25.4;

/// What an import builds on: shipped data and host engines the caller
/// loads (the importer reads no files).
#[derive(Clone, Copy)]
pub struct Setup<'a> {
    /// The new project's board.
    pub board: &'a NewBoard,
    /// The stackup family a stackup the file does not state comes from.
    pub presets: &'a StackupPresets,
    /// The fab profile the board's rules start from; the file's own
    /// settings (mask margin, via tenting) are set over it. Without one the
    /// board has no rules, so its outline, tracks and fills are not
    /// imported.
    pub profile: Option<&'a RuleProfile>,
    /// The host's sketch engines: with a board evaluator the outline is
    /// imported as a board sketch, else as the rectangle around it.
    pub evaluators: Evaluators<'a>,
    /// The board's project file (`.kicad_pro`), when there is one: its
    /// rules become the project's own rule set.
    pub project_file: Option<&'a [u8]>,
    /// The project's schematic files, when there are any: drawn as the
    /// schematic; without them each pin gets a label of its board net.
    pub schematic: Option<crate::schematic::SchematicFiles<'a>>,
}

/// Import a KiCad board file into a new project.
pub fn import(bytes: &[u8], setup: &Setup<'_>) -> Result<Imported, ImportError> {
    let mut report = Report::default();
    let board = board::read(bytes, &mut report)?;
    report.format_version = board.version;
    report.counts.copper_layers = board.copper.len();
    report.counts.footprints = board.footprints.len();
    report.counts.footprints_back = board.footprints.iter().filter(|f| f.back).count();
    report.add_n(Case::BoardObjects, board.other.values().sum());
    let mut engine = Engine::new(setup.board);
    // The host's context, current on the importing thread, is the
    // imported project's.
    if let Ok(context) = sparksmith_core::library::standards::Context::current() {
        engine.set_context(context);
    }
    let mut session = Session {
        engine,
        evaluators: setup.evaluators,
    };
    session.rules(&board, setup.profile, &mut report)?;
    let stackup = match crate::shape::stackup(&board, setup.presets, &mut report) {
        Some(stackup) => Some(stackup),
        None if board.copper.len() != setup.board.stackup.copper.len() => Some(
            setup
                .presets
                .preset(board.copper.len())
                .map_err(|e| ImportError::Refused {
                    step: "layout.set_stackup",
                    reason: e.to_string(),
                })?,
        ),
        None => None,
    };
    if let Some(stackup) = stackup {
        session.apply(
            "layout.set_stackup",
            Operation::Layout(LayoutCommand::SetStackup(stackup)),
        )?;
    }
    session.outline(&board, &mut report)?;
    // The schematic, read first: each reference's KiCad symbol shapes its
    // part's symbol.
    let schematic = match setup.schematic.map(crate::schematic::read) {
        Some(Ok(schematic)) => Some(schematic),
        Some(Err(_)) => {
            report.add(Case::SchematicUnreadable);
            None
        }
        None => None,
    };
    let by_reference = schematic
        .as_ref()
        .map(crate::sheets::symbols_by_reference)
        .unwrap_or_default();
    // Each footprint's pads in its own definition frame, with its KiCad
    // symbol; equal definitions are shared.
    let mut definitions: Vec<(Vec<Pad>, Option<crate::schematic::LibSymbol>)> = Vec::new();
    let mut by_key: BTreeMap<String, usize> = BTreeMap::new();
    let mut uses: Vec<usize> = Vec::with_capacity(board.footprints.len());
    let mut per_footprint: Vec<Vec<PadRecord>> = Vec::with_capacity(board.footprints.len());
    let mut terminal_nets: Vec<Vec<(PackageTerminalId, u32)>> =
        Vec::with_capacity(board.footprints.len());
    let mut zone_overrides: Vec<Vec<(PadId, PadFillOverride)>> =
        Vec::with_capacity(board.footprints.len());
    for fp in &board.footprints {
        let mut records = Vec::new();
        let mut nets = Vec::new();
        let mut overrides = Vec::new();
        let pads = footprint_pads(
            &board,
            fp,
            &mut report,
            &mut records,
            &mut nets,
            &mut overrides,
        );
        per_footprint.push(records);
        terminal_nets.push(nets);
        zone_overrides.push(overrides);
        let symbol = by_reference
            .get(&fp.reference.to_ascii_uppercase())
            .cloned();
        let mut key = serde_json::to_string(&pads).map_err(|e| ImportError::Refused {
            step: "footprint key",
            reason: e.to_string(),
        })?;
        if let Some(symbol) = &symbol {
            key.push_str(&format!("{symbol:?}"));
        }
        let next = definitions.len();
        let index = *by_key.entry(key).or_insert(next);
        if index == next {
            definitions.push((pads, symbol));
        }
        uses.push(index);
        report.add_n(Case::FootprintFabGraphics, fp.fab_graphics);
        report.add_n(Case::FootprintDrawings, fp.drawings);
        report.add_n(Case::Models, fp.models);
        if fp.net_tie {
            report.add(Case::NetTie);
        }
        report.add_n(
            Case::PadZoneOverrides,
            fp.pads.iter().filter(|p| !p.zone.is_empty()).count(),
        );
        if !fp.zone.is_empty() {
            report.add(Case::FootprintZoneOverrides);
        }
    }
    report.counts.definitions = definitions.len();
    report.add_n(Case::ProjectOnly, definitions.len());
    let mut sets: Vec<Set> = Vec::with_capacity(definitions.len());
    for (k, (pads, symbol)) in definitions.into_iter().enumerate() {
        // A footprint without electrical pads (a fiducial, a mounting
        // hole) becomes a part without terminals.
        let prefix = board
            .footprints
            .iter()
            .zip(&uses)
            .find(|(_, u)| **u == k)
            .map(|(fp, _)| prefix_of(&fp.reference))
            .unwrap_or_else(|| "U".to_owned());
        sets.push(session.component_set(k, pads, &prefix, symbol.as_ref(), &mut report)?);
    }
    // The schematic's sheets, and where each reference's unit goes.
    let plan = match schematic {
        Some(schematic) => {
            let plan = crate::sheets::sheets(&mut session, &schematic, &mut report)?;
            Some((schematic, plan))
        }
        None => None,
    };
    let mut labelled = std::collections::BTreeSet::new();
    let mut unlabelled_places = Vec::new();
    let mut first_units: Vec<Option<crate::sheets::UnitPlace>> = Vec::new();
    let mut later_units: Vec<Vec<crate::sheets::UnitPlace>> = Vec::new();
    // Instances on the sheet, then on the board, in batches.
    let mut taken = std::collections::BTreeSet::new();
    let mut place = Vec::with_capacity(board.footprints.len());
    let mut placed: Vec<&Footprint> = Vec::with_capacity(board.footprints.len());
    let mut placed_nets = Vec::with_capacity(board.footprints.len());
    let mut placed_overrides = Vec::with_capacity(board.footprints.len());
    let mut records = Vec::new();
    for (((fp, (index, recs)), nets), overrides) in board
        .footprints
        .iter()
        .zip(uses.iter().zip(per_footprint))
        .zip(terminal_nets)
        .zip(zone_overrides)
    {
        let set = sets.get(*index).ok_or(ImportError::Refused {
            step: "component.place",
            reason: "a footprint without its component".into(),
        })?;
        let n = placed.len();
        placed.push(fp);
        placed_nets.push(nets);
        placed_overrides.push(overrides);
        records.extend(recs);
        let designator = if sparksmith_core::instances::designator_valid(&fp.reference)
            && taken.insert(fp.reference.to_ascii_uppercase())
        {
            Some(fp.reference.clone())
        } else {
            report.add_detail(Case::Designator, &fp.reference, "", "");
            None
        };
        if fp.board_only {
            // KiCad's "not in schematic": on the board only.
            report.add(Case::BoardOnly);
            place.push(Operation::Instance(InstanceCommand::PlaceBoardOnly {
                component: ComponentRef::new(set.component, Revision::FIRST),
                representation: set.symbol,
                physical: PhysicalChoice {
                    binding: set.binding,
                    footprint: set.footprint,
                },
                designator,
            }));
            continue;
        }
        let column = (n % 16) as f64;
        let row = (n / 16) as f64;
        let units = plan
            .as_ref()
            .and_then(|(_, p)| p.places.get(&fp.reference.to_ascii_uppercase()))
            .filter(|u| !u.is_empty());
        if plan.is_some() && units.is_none() {
            report.add(Case::FootprintWithoutSymbol);
            unlabelled_places.push(n);
        }
        // The first unit with the instance; the others after.
        let first = units.and_then(|u| u.first()).copied();
        later_units.push(units.map(|u| u[1..].to_vec()).unwrap_or_default());
        first_units.push(first);
        let (sheet, position, unit) = match first {
            Some(u) => (Some(u.sheet), u.at, UnitId(u.unit.max(1))),
            None => (
                None,
                [column * SHEET_PITCH_MM, -row * SHEET_PITCH_MM],
                UnitId(1),
            ),
        };
        place.push(Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet,
            component: ComponentRef::new(set.component, Revision::FIRST),
            designator,
            properties: vec![],
            representation: set.symbol,
            unit,
            position: PointNm::from_mm_f64_rounded(position).map_err(|e| ImportError::Refused {
                step: "component.place",
                reason: e.to_string(),
            })?,
            physical: Some(PhysicalChoice {
                binding: set.binding,
                footprint: set.footprint,
            }),
        })));
    }
    let mut instances: Vec<Id> = Vec::with_capacity(place.len());
    let mut first_occurrences: Vec<Option<Id>> = Vec::with_capacity(place.len());
    for outcome in session.batches("component.place", place)? {
        match outcome {
            Outcome::InstancePlaced {
                instance,
                occurrence,
                ..
            } => {
                instances.push(instance);
                first_occurrences.push(Some(occurrence));
            }
            Outcome::PartPlaced { instance, .. } => {
                instances.push(instance);
                first_occurrences.push(None);
            }
            _ => {
                return Err(ImportError::Refused {
                    step: "component.place",
                    reason: "no instance placed".into(),
                });
            }
        }
    }
    // Every unit turned as KiCad turns it; the other units placed.
    if plan.is_some() {
        crate::sheets::units(
            &mut session,
            &instances,
            &first_occurrences,
            &first_units,
            &later_units,
            &mut report,
        );
    }
    let mut layout = Vec::with_capacity(instances.len());
    for (fp, instance) in placed.iter().zip(&instances) {
        let (position, rotation_deg, layer) = translate::placement(&board, fp);
        layout.push(Operation::Layout(LayoutCommand::Place {
            instance: *instance,
            position,
            rotation_deg,
            layer,
        }));
    }
    session.batches("layout.place", layout)?;
    // Zone connection overrides: the footprint's for the part, each pad's
    // own over it.
    let mut overrides = Vec::new();
    for ((fp, instance), pads) in placed.iter().zip(&instances).zip(&placed_overrides) {
        let part = crate::zones::pad_override(&fp.zone, None);
        if !part.is_empty() {
            overrides.push(Operation::Fill(FillCommand::SetPadOverride {
                instance: *instance,
                pad: None,
                settings: part,
            }));
        }
        for (pad, settings) in pads {
            overrides.push(Operation::Fill(FillCommand::SetPadOverride {
                instance: *instance,
                pad: Some(*pad),
                settings: *settings,
            }));
        }
    }
    session.batches("fill.set_pad_override", overrides)?;
    if let Some((schematic, plan)) = &plan {
        crate::sheets::wire(&mut session, schematic, plan, &mut report)?;
        for n in unlabelled_places {
            if let Some(id) = instances.get(n) {
                labelled.insert(*id);
            }
        }
    }
    let nets = crate::routing::connect(
        &mut session,
        &board,
        &placed_nets,
        &instances,
        plan.is_some().then_some(&labelled),
        &mut report,
    )?;
    crate::routing::copper(&mut session, &board, &nets, &mut report)?;
    session.project_rules(setup.project_file, &board, &nets, &mut report)?;
    crate::zones::fills(&mut session, &board, &nets, &mut report)?;
    report.counts.pads = board.footprints.iter().map(|f| f.pads.len()).sum();
    report.counts.pads_imported = records.len();
    report.add_n(Case::PadsExact, records.len());
    Ok(Imported {
        engine: session.engine,
        origin_nm: board.origin,
        aux_origin_nm: board.aux_origin,
        report,
        pads: records,
    })
}

/// The designator prefix of a reference: its leading capital letters.
fn prefix_of(reference: &str) -> String {
    let letters: String = reference
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if designator_prefix_valid(&letters) {
        letters
    } else {
        "U".to_owned()
    }
}

/// A footprint's pads in its definition frame, numbered, with their
/// package terminals: one per distinct pad number (pads sharing a number
/// share it), and one each for unnumbered plated pads.
fn footprint_pads(
    board: &Board,
    fp: &Footprint,
    report: &mut Report,
    records: &mut Vec<PadRecord>,
    nets: &mut Vec<(PackageTerminalId, u32)>,
    overrides: &mut Vec<(PadId, PadFillOverride)>,
) -> Vec<Pad> {
    let mut pads = Vec::with_capacity(fp.pads.len());
    let mut terminals: BTreeMap<String, PackageTerminalId> = BTreeMap::new();
    let mut count = 0usize;
    let mut paste_only = Vec::new();
    for kpad in &fp.pads {
        let DefPad {
            mut pad,
            class,
            world,
            arcs,
        } = match translate::pad(board, fp, kpad, report) {
            Mapped::Pad(p) => *p,
            Mapped::PasteOnly(p) => {
                paste_only.push((p.pad, fp.reference.as_str(), kpad.number.as_str()));
                continue;
            }
            Mapped::Refused(case) => {
                if !matches!(
                    case,
                    Case::PadUnknownToken
                        | Case::CustomPadPrimitive
                        | Case::CustomPadNotOneOutline
                        | Case::PadShapeUnsupported
                ) {
                    report.add_detail(case, &fp.reference, &kpad.number, "");
                }
                continue;
            }
        };
        pad.id = PadId(u32::try_from(pads.len() + 1).unwrap_or(u32::MAX));
        // A footprint's "thermal for through-hole pads only" depends on
        // the pad, so it lands on each pad.
        let through_hole = pad.kind == PadKind::PlatedThrough;
        let mut own = crate::zones::pad_override(&kpad.zone, Some(through_hole));
        if fp.zone.connect == Some(3) && own.connection.is_none() {
            own.connection = crate::zones::pad_override(&fp.zone, Some(through_hole)).connection;
        }
        if !own.is_empty() {
            overrides.push((pad.id, own));
        }
        if translate::needs_terminal(&pad) {
            let key = if pad.number.trim().is_empty() {
                // Unnumbered plated pads land their own terminal each.
                format!("\u{0}{}", pads.len())
            } else {
                pad.number.clone()
            };
            let id = *terminals.entry(key).or_insert_with(|| {
                let id = translate::terminal_id(count);
                count += 1;
                id
            });
            pad.terminal = Some(id);
            if let Some((net, _)) = &kpad.net {
                nets.push((id, *net));
            }
        }
        records.push(PadRecord { class, world, arcs });
        pads.push(pad);
    }
    for (window, reference, number) in paste_only {
        if !translate::add_paste_window(&mut pads, &window) {
            report.add_detail(Case::PadWithoutCopper, reference, number, "");
        } else {
            report.add(Case::PasteWindow);
        }
    }
    if !pads.iter().any(|p| p.terminal.is_some()) {
        report.add(Case::NoTerminals);
    }
    pads
}

/// The drafts of one imported footprint, published.
struct Set {
    component: DefinitionId,
    symbol: DefinitionId,
    footprint: DefinitionId,
    binding: DefinitionId,
}

pub(crate) struct Session<'a> {
    pub(crate) engine: Engine,
    pub(crate) evaluators: Evaluators<'a>,
}

/// Board parameters a rectangle sketch starts with (`board.start_sketch`).
const RECTANGLE_PARAMETERS: [&str; 2] = ["width", "height"];

impl Session<'_> {
    pub(crate) fn apply(
        &mut self,
        step: &'static str,
        op: Operation,
    ) -> Result<Outcome, ImportError> {
        self.engine
            .apply_hosted(self.engine.revision(), op, self.evaluators)
            .map_err(|e| ImportError::Refused {
                step,
                reason: e.to_string(),
            })
    }

    /// The fab profile, with the file's board-wide settings over it: the
    /// mask margin vias and inherited pads take, and whether vias are
    /// tented.
    fn rules(
        &mut self,
        board: &Board,
        profile: Option<&RuleProfile>,
        report: &mut Report,
    ) -> Result<(), ImportError> {
        let Some(profile) = profile else {
            report.add(Case::NoRules);
            return Ok(());
        };
        self.apply(
            "rules.set_fab_profile",
            Operation::Rules(RulesCommand::SetProfile {
                profile: Box::new(profile.clone()),
                keep_overrides: false,
            }),
        )?;
        let mut overrides = vec![(
            RuleKey::MaskExpansion,
            RuleValue::Length(board.setup.mask_margin as f64 / 1e6),
        )];
        if let Some(open) = board.setup.vias_on_mask {
            overrides.push((RuleKey::TentVias, RuleValue::Flag(!open)));
        }
        self.set_overrides(overrides, report);
        Ok(())
    }

    /// The project file's rules as the project's own rule set, "From the
    /// KiCad project", over the fab profile: the default class's values
    /// and the board minimums as overrides (limits looser than the
    /// profile's are not taken), every other class as a net class with its
    /// name patterns and explicit assignments. Set after the tracks and
    /// vias, which go in as the file has them (a check finding, not a
    /// refusal, is the place for any the project's rules flag), and before
    /// the fills, which they shape.
    pub(crate) fn project_rules(
        &mut self,
        project_file: Option<&[u8]>,
        board: &Board,
        nets: &BTreeMap<u32, Id>,
        report: &mut Report,
    ) -> Result<(), ImportError> {
        if self.engine.project().rules.require().is_err() {
            return Ok(());
        }
        let rules = match project_file.map(crate::project::read) {
            Some(Some(rules)) => rules,
            Some(None) => {
                report.add(Case::ProjectUnreadable);
                return Ok(());
            }
            None => {
                report.add(Case::NoProject);
                return Ok(());
            }
        };
        report.add(Case::ProjectRules);
        let profile = self.engine.project().rules.effective();
        self.set_overrides(crate::project::overrides(&rules, &profile, report), report);
        // Every other class, by name.
        let mut classes: BTreeMap<String, NetClassId> = BTreeMap::new();
        for class in &rules.classes {
            let spec = NetClassSpec {
                name: class.name.clone(),
                track_width: class.track_width,
                clearance: class.clearance,
                via_drill: class.via_drill,
                via_diameter: class.via_diameter,
                diff_pair_width: class.diff_pair_width,
                diff_pair_gap: class.diff_pair_gap,
                ..NetClassSpec::default()
            };
            match self.apply(
                "net_class.add",
                Operation::Rules(RulesCommand::AddNetClass { class: spec }),
            ) {
                Ok(Outcome::NetClassAdded { class: id }) => {
                    report.add(Case::NetClass);
                    classes.insert(class.name.clone(), id);
                }
                _ => report.add(Case::NetClassRefused),
            }
        }
        // Patterns that name an imported class and are plain wildcards
        // (`*`, `?`); the default class's need none.
        let default = rules.default.as_ref().map(|c| c.name.as_str());
        let mut patterns = Vec::new();
        for (pattern, class) in &rules.patterns {
            if Some(class.as_str()) == default {
                continue;
            }
            let plain = !pattern.trim().is_empty()
                && pattern.len() <= sparksmith_core::rules::MAX_PROFILE_TEXT
                && !pattern.contains(['[', ']', '(', ')', '{', '}', '^', '$', '|', '\\', '+']);
            match classes.get(class) {
                Some(id) if plain => patterns.push(NetClassPattern {
                    pattern: pattern.clone(),
                    class: *id,
                }),
                _ => report.add(Case::NetClassPatternRefused),
            }
        }
        // KiCad's class of each net (explicit, then the first matching
        // pattern), assigned explicitly where our patterns, on our net
        // names, would not give it.
        let kicad_class = |name: &str| -> Option<&str> {
            rules
                .assignments
                .iter()
                .find(|(net, _)| net == name)
                .map(|(_, c)| c.as_str())
                .or_else(|| {
                    rules
                        .patterns
                        .iter()
                        .find(|(p, _)| sparksmith_core::rules::glob_matches(p, name))
                        .map(|(_, c)| c.as_str())
                })
        };
        let mut assign: BTreeMap<NetClassId, Vec<Id>> = BTreeMap::new();
        for (number, net) in nets {
            let kicad_name = board.items.nets.get(number).map_or("", String::as_str);
            let Some(class) = kicad_class(kicad_name).filter(|c| Some(*c) != default) else {
                continue;
            };
            let Some(id) = classes.get(class) else {
                report.add(Case::NetClassPatternRefused);
                continue;
            };
            let ours = self.engine.project().net_name(*net).to_owned();
            let by_pattern = patterns
                .iter()
                .find(|p| sparksmith_core::rules::glob_matches(&p.pattern, &ours))
                .map(|p| p.class);
            if by_pattern != Some(*id) {
                assign.entry(*id).or_default().push(*net);
            }
        }
        if !patterns.is_empty() {
            self.apply(
                "net_class.set_patterns",
                Operation::Rules(RulesCommand::SetPatterns { patterns }),
            )?;
        }
        for (class, nets) in assign {
            report.add_n(Case::NetClassAssigned, nets.len());
            self.apply(
                "net_class.assign",
                Operation::Rules(RulesCommand::AssignNets {
                    nets,
                    class: Some(class),
                }),
            )?;
        }
        self.apply(
            "rules.set_project_source",
            Operation::Rules(RulesCommand::SetProjectSource {
                source: Some(crate::project::SOURCE.to_owned()),
            }),
        )?;
        Ok(())
    }

    fn set_overrides(&mut self, overrides: Vec<(RuleKey, RuleValue)>, report: &mut Report) {
        for (key, value) in overrides {
            if self
                .apply(
                    "rules.set_override",
                    Operation::Rules(RulesCommand::SetOverride { key, value }),
                )
                .is_err()
            {
                report.add(Case::RuleRefused);
            }
        }
    }

    /// The board outline: a board sketch of the `Edge.Cuts` drawings (with
    /// a board evaluator), else the rectangle from the origin around them.
    fn outline(&mut self, board: &Board, report: &mut Report) -> Result<(), ImportError> {
        let Some(bounds) = crate::shape::outline_bounds(board) else {
            report.add(Case::OutlineMissing);
            return Ok(());
        };
        if self.engine.project().rules.require().is_err() {
            report.add(Case::OutlineNotImported);
            return Ok(());
        }
        let curves = crate::shape::outline_edits(board, report);
        if self.evaluators.board.is_none() {
            // The box's corner is the origin, up to its fitted arcs.
            if bounds.min[0] < -1e-6 || bounds.min[1] < -1e-6 {
                report.add(Case::OutlineNotImported);
                return Ok(());
            }
            self.apply(
                "board.set_rectangle",
                Operation::Board(BoardCommand::SetRectangle { size: bounds.max }),
            )?;
            report.add(Case::OutlineRectangle);
            return Ok(());
        }
        self.apply(
            "board.start_sketch",
            Operation::Board(BoardCommand::StartSketch),
        )?;
        let BoardShape::Sketch(sketch) = &self.engine.project().board_shape else {
            return Err(ImportError::Refused {
                step: "board.start_sketch",
                reason: "no board sketch".into(),
            });
        };
        // The starting rectangle goes: its lines, then its parameters.
        let mut edits = vec![BoardEdit::Sketch {
            edit: SketchEdit::Delete {
                ids: sketch
                    .edges
                    .iter()
                    .map(|e| SketchRef::Curve(e.curve.clone()))
                    .collect(),
            },
        }];
        edits.extend(
            sketch
                .parameters
                .iter()
                .filter(|p| RECTANGLE_PARAMETERS.contains(&p.name.0.as_str()))
                .map(|p| BoardEdit::RemoveParameter {
                    name: p.name.clone(),
                }),
        );
        edits.extend(curves.into_iter().map(|edit| BoardEdit::Sketch { edit }));
        let max = sparksmith_core::layout::outline::MAX_BOARD_EDITS;
        for chunk in edits.chunks(max) {
            self.apply(
                "board.edit",
                Operation::Board(BoardCommand::Edit {
                    edits: chunk.to_vec(),
                }),
            )?;
        }
        match &self.engine.project().board_shape {
            BoardShape::Sketch(sketch) if sketch.fault.is_none() => {
                report.add(Case::OutlineSketch);
                report.counts.cutouts = sketch.solved.cutouts.len();
            }
            _ => report.add(Case::OutlineNotClosed),
        }
        Ok(())
    }

    /// Apply `ops` in batches of at most [`sparksmith_core::MAX_BATCH`].
    pub(crate) fn batches(
        &mut self,
        step: &'static str,
        ops: Vec<Operation>,
    ) -> Result<Vec<Outcome>, ImportError> {
        let mut out = Vec::with_capacity(ops.len());
        let mut ops = ops.into_iter().peekable();
        while ops.peek().is_some() {
            let batch: Vec<Operation> = ops.by_ref().take(sparksmith_core::MAX_BATCH).collect();
            let outcomes = self
                .engine
                .apply_all_hosted(self.engine.revision(), batch, self.evaluators)
                .map_err(|e| ImportError::Refused {
                    step,
                    reason: e.to_string(),
                })?;
            out.extend(outcomes);
        }
        Ok(out)
    }

    fn component_set(
        &mut self,
        k: usize,
        pads: Vec<Pad>,
        prefix: &str,
        kicad_symbol: Option<&crate::schematic::LibSymbol>,
        report: &mut Report,
    ) -> Result<Set, ImportError> {
        let kinds = [
            DefinitionKind::Component,
            DefinitionKind::Symbol,
            DefinitionKind::Package,
            DefinitionKind::Footprint,
            DefinitionKind::Binding,
        ];
        let reserve = kinds
            .iter()
            .map(|kind| {
                Operation::Library(LibraryCommand::NewDraft {
                    meta: DefinitionMeta::default(),
                    definition: Definition::empty(*kind),
                })
            })
            .collect();
        let mut ids = Vec::with_capacity(5);
        for outcome in self.batches("library.new_draft", reserve)? {
            match outcome {
                Outcome::DraftCreated { id } => ids.push(id),
                _ => {
                    return Err(ImportError::Refused {
                        step: "library.new_draft",
                        reason: "no draft created".into(),
                    });
                }
            }
        }
        let [component, symbol, package, footprint, binding] = ids[..] else {
            return Err(ImportError::Refused {
                step: "library.new_draft",
                reason: "five drafts expected".into(),
            });
        };
        let mut unpinned = 0;
        let edits = drafts(
            k,
            pads,
            prefix,
            kicad_symbol,
            &mut unpinned,
            [component, symbol, package, footprint, binding],
        )
        .map_err(|e| ImportError::Refused {
            step: "library.edit",
            reason: e.to_string(),
        })?;
        report.add_n(Case::TerminalWithoutPin, unpinned);
        self.batches(
            "library.publish",
            vec![
                Operation::Library(LibraryCommand::Edit { edits }),
                Operation::Library(LibraryCommand::Publish {
                    drafts: vec![component, symbol, package, footprint, binding],
                    close: true,
                }),
            ],
        )?;
        Ok(Set {
            component,
            symbol,
            footprint,
            binding,
        })
    }
}

/// The five definition bodies of one imported footprint.
fn drafts(
    k: usize,
    pads: Vec<Pad>,
    prefix: &str,
    kicad_symbol: Option<&crate::schematic::LibSymbol>,
    unpinned: &mut usize,
    [component, symbol, package, footprint, binding]: [DefinitionId; 5],
) -> sparksmith_core::Result<Vec<DraftEdit>> {
    // Package terminals in id order, designated by their first pad's
    // number (an unnumbered plated pad is "MP" and its terminal number).
    let mut designations: BTreeMap<PackageTerminalId, String> = BTreeMap::new();
    for pad in &pads {
        if let Some(t) = pad.terminal {
            designations.entry(t).or_insert_with(|| {
                if pad.number.trim().is_empty() {
                    format!("MP{}", t.0)
                } else {
                    pad.number.clone()
                }
            });
        }
    }
    let count = u32::try_from(designations.len()).unwrap_or(u32::MAX);
    let terminals: Vec<TerminalId> = (1..=count).map(TerminalId).collect();
    let named: Vec<(TerminalId, &str)> = terminals
        .iter()
        .zip(designations.values())
        .map(|(t, n)| (*t, n.as_str()))
        .collect();
    let symbol_body = match kicad_symbol {
        Some(lib) => crate::sheets::kicad_symbol(component, lib, &named, unpinned)?,
        None => named_block_symbol(component, &named)?,
    };
    let bounds = Bounds::of(pads.iter().flat_map(|p| {
        let b = p.bounds();
        [b.min, b.max]
    }))
    .unwrap_or(Bounds::point([0.0, 0.0]));
    let size = bounds.size();
    let envelope = [
        Nm::from_mm_f64_rounded(size[0].max(0.1))?,
        Nm::from_mm_f64_rounded(size[1].max(0.1))?,
        Nm::from_mm_f64_rounded(1.0)?,
    ];
    let next_local = u32::try_from(pads.len() + 1).unwrap_or(u32::MAX);
    let name = format!("KiCad footprint {}", k + 1);
    let meta = |what: &str| DefinitionMeta {
        name: format!("{name} {what}"),
        description: String::new(),
        provenance: PROVENANCE.into(),
    };
    Ok(vec![
        DraftEdit {
            id: component,
            meta: meta("component"),
            definition: Definition::Component(ComponentDefinition {
                pin_rules: Vec::new(),
                marking: None,
                review: None,
                generic: None,
                next_local: 2 * count + 1,
                designator_prefix: prefix.into(),
                terminals: named
                    .iter()
                    .map(|(id, name)| Terminal {
                        id: *id,
                        name: (*name).to_owned(),
                        class: TerminalClass::Electrical(TerminalFunctions::fixed(
                            FunctionId(id.0 + count),
                            Direction::Passive,
                        )),
                    })
                    .collect(),
                resources: vec![],
                constraints: vec![],
                representations: vec![SymbolRef::new(symbol, Revision::FIRST)],
                implementations: vec![BindingRef::new(binding, Revision::FIRST)],
                fields: vec![],
            }),
        },
        DraftEdit {
            id: symbol,
            meta: meta("symbol"),
            definition: Definition::Symbol(symbol_body),
        },
        DraftEdit {
            id: package,
            meta: meta("package"),
            definition: Definition::Package(PackageDefinition {
                next_local: count + 1,
                terminals: designations
                    .iter()
                    .map(|(id, designation)| PackageTerminal {
                        id: *id,
                        designation: designation.clone(),
                    })
                    .collect(),
                form: PackageForm::Generic,
                envelope_nm: envelope,
                model: PackageModel::Envelope {
                    material: MaterialId::EpoxyMould,
                },
                placement: ModelPlacement::default(),
                mechanical: None,
                datums: Vec::new(),
            }),
        },
        DraftEdit {
            id: footprint,
            meta: meta("footprint"),
            definition: Definition::Footprint(FootprintDefinition {
                package: PackageRef::new(package, Revision::FIRST),
                next_local,
                pads,
                defaults: PadDefaults::default(),
                ..FootprintDefinition::default()
            }),
        },
        DraftEdit {
            id: binding,
            meta: meta("binding"),
            definition: Definition::Binding(BindingDefinition {
                component,
                package: PackageRef::new(package, Revision::FIRST),
                mappings: designations
                    .keys()
                    .zip(&terminals)
                    .map(|(p, t)| TerminalMap {
                        package_terminal: *p,
                        terminal: *t,
                    })
                    .collect(),
                footprints: vec![FootprintRef::new(footprint, Revision::FIRST)],
                unavailable: vec![],
            }),
        },
    ])
}
