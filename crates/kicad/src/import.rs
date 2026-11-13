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
use sparksmith_core::library::samples::named_block_symbol;
use sparksmith_core::library::{
    BindingDefinition, ComponentDefinition, Definition, DefinitionMeta, DraftEdit,
    FootprintDefinition, ModelPlacement, PackageDefinition, PackageForm, PackageModel,
    PackageTerminal, Pad, PadDefaults, PadKind, Terminal, TerminalClass, TerminalMap,
    designator_prefix_valid,
};
use sparksmith_core::materials::MaterialId;
use sparksmith_core::units::{Nm, PointNm};
use sparksmith_core::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, FootprintRef, FunctionId, Id,
    InstanceCommand, LibraryCommand, Operation, Outcome, PackageRef, PackageTerminalId, PadId,
    PhysicalChoice, Revision, Stackup, SymbolRef, TerminalId, UnitId,
};
use sparksmith_core::{Engine, instances::NewInstance};
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
    /// Sharp land corners on the board, with the mask or paste offset
    /// there (mm).
    pub corners: Vec<([f64; 2], f64)>,
}

/// The new project, its report and the import map of pads.
pub struct Imported {
    pub engine: Engine,
    pub report: Report,
    pub pads: Vec<PadRecord>,
}

/// The provenance every imported definition records (L1).
const PROVENANCE: &str = "Imported from a KiCad board's embedded footprint copy. Project-only: \
     it may come from KiCad's CC-BY-SA libraries, so never publish it to a shared library.";

/// Schematic spacing of the imported instances, mm.
const SHEET_PITCH_MM: f64 = 25.4;

/// Import the footprints of a KiCad board file.
pub fn import(bytes: &[u8]) -> Result<Imported, ImportError> {
    let mut report = Report::default();
    let board = board::read(bytes, &mut report)?;
    report.format_version = board.version;
    report.counts.copper_layers = board.copper.len();
    report.counts.footprints = board.footprints.len();
    report.counts.footprints_back = board.footprints.iter().filter(|f| f.back).count();
    report.add_n(Case::BoardObjects, board.other.values().sum());
    let mut session = Session {
        engine: Engine::default(),
    };
    if board.copper.len() != 2 {
        let stackup = Stackup::preset(board.copper.len()).map_err(|e| ImportError::Refused {
            step: "layout.set_stackup",
            reason: e.to_string(),
        })?;
        session.apply(
            "layout.set_stackup",
            Operation::Layout(LayoutCommand::SetStackup(stackup)),
        )?;
    }
    // Each footprint's pads in its own definition frame; equal definitions
    // are shared.
    let mut definitions: Vec<Vec<Pad>> = Vec::new();
    let mut by_key: BTreeMap<String, usize> = BTreeMap::new();
    let mut uses: Vec<usize> = Vec::with_capacity(board.footprints.len());
    let mut per_footprint: Vec<Vec<PadRecord>> = Vec::with_capacity(board.footprints.len());
    for fp in &board.footprints {
        let mut records = Vec::new();
        let pads = footprint_pads(&board, fp, &mut report, &mut records);
        per_footprint.push(records);
        let key = serde_json::to_string(&pads).map_err(|e| ImportError::Refused {
            step: "footprint key",
            reason: e.to_string(),
        })?;
        let next = definitions.len();
        let index = *by_key.entry(key).or_insert(next);
        if index == next {
            definitions.push(pads);
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
            fp.pads.iter().filter(|p| p.zone_overrides).count(),
        );
    }
    report.counts.definitions = definitions.len();
    report.add_n(Case::ProjectOnly, definitions.len());
    let mut sets: Vec<Option<Set>> = Vec::with_capacity(definitions.len());
    for (k, pads) in definitions.into_iter().enumerate() {
        // The core needs at least one electrical terminal unless a part
        // file models the part (docs/mechanical-parts.md): a footprint of
        // bare holes only is refused and listed.
        if !pads.iter().any(|p| p.terminal.is_some()) {
            sets.push(None);
            continue;
        }
        let prefix = board
            .footprints
            .iter()
            .zip(&uses)
            .find(|(_, u)| **u == k)
            .map(|(fp, _)| prefix_of(&fp.reference))
            .unwrap_or_else(|| "U".to_owned());
        sets.push(Some(session.component_set(k, pads, &prefix)?));
    }
    // Instances on the sheet, then on the board, in batches.
    let mut taken = std::collections::BTreeSet::new();
    let mut place = Vec::with_capacity(board.footprints.len());
    let mut placed: Vec<&Footprint> = Vec::with_capacity(board.footprints.len());
    let mut records = Vec::new();
    for (fp, (index, recs)) in board.footprints.iter().zip(uses.iter().zip(per_footprint)) {
        let Some(set) = sets.get(*index).ok_or(ImportError::Refused {
            step: "component.place",
            reason: "a footprint without its component".into(),
        })?
        else {
            report.add_detail(Case::FootprintWithoutTerminals, &fp.reference, "", "");
            continue;
        };
        let n = placed.len();
        placed.push(fp);
        records.extend(recs);
        let designator = if sparksmith_core::instances::designator_valid(&fp.reference)
            && taken.insert(fp.reference.to_ascii_uppercase())
        {
            Some(fp.reference.clone())
        } else {
            report.add_detail(Case::Designator, &fp.reference, "", "");
            None
        };
        let column = (n % 16) as f64;
        let row = (n / 16) as f64;
        place.push(Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: ComponentRef::new(set.component, Revision::FIRST),
            designator,
            properties: vec![],
            representation: set.symbol,
            unit: UnitId(1),
            position: PointNm::from_mm_f64_rounded([
                column * SHEET_PITCH_MM,
                -row * SHEET_PITCH_MM,
            ])
            .map_err(|e| ImportError::Refused {
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
    for outcome in session.batches("component.place", place)? {
        match outcome {
            Outcome::InstancePlaced { instance, .. } => instances.push(instance),
            _ => {
                return Err(ImportError::Refused {
                    step: "component.place",
                    reason: "no instance placed".into(),
                });
            }
        }
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
    report.counts.pads = board.footprints.iter().map(|f| f.pads.len()).sum();
    report.counts.pads_imported = records.len();
    report.add_n(Case::PadsExact, records.len());
    Ok(Imported {
        engine: session.engine,
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
            corners,
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
        }
        records.push(PadRecord {
            class,
            world,
            arcs,
            corners,
        });
        pads.push(pad);
    }
    for (window, reference, number) in paste_only {
        if !translate::add_paste_window(&mut pads, &window) {
            report.add_detail(Case::PadWithoutCopper, reference, number, "");
        } else {
            report.add(Case::PasteWindow);
        }
    }
    // The core needs an electrical terminal on every part (unless a part
    // file models it): a footprint whose copper pads land none (a
    // fiducial, a mounting pad) lands one on its first copper pad.
    if !pads.iter().any(|p| p.terminal.is_some())
        && let Some(pad) = pads.iter_mut().find(|p| {
            matches!(
                p.kind,
                PadKind::Mechanical | PadKind::Fiducial | PadKind::Smd
            )
        })
    {
        pad.kind = PadKind::Mechanical;
        pad.terminal = Some(translate::terminal_id(0));
        report.add_detail(Case::TerminalAdded, &fp.reference, &pad.number, "");
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

struct Session {
    engine: Engine,
}

impl Session {
    fn apply(&mut self, step: &'static str, op: Operation) -> Result<Outcome, ImportError> {
        self.engine
            .apply(self.engine.revision(), op)
            .map_err(|e| ImportError::Refused {
                step,
                reason: e.to_string(),
            })
    }

    /// Apply `ops` in batches of at most [`sparksmith_core::MAX_BATCH`].
    fn batches(
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
                .apply_all_hosted(self.engine.revision(), batch, Default::default())
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
        let edits = drafts(
            k,
            pads,
            prefix,
            [component, symbol, package, footprint, binding],
        )
        .map_err(|e| ImportError::Refused {
            step: "library.edit",
            reason: e.to_string(),
        })?;
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
    let symbol_body = named_block_symbol(component, &named)?;
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
