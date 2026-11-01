//! Synthetic fixtures built through public commands only.
use crate::completeness::{CheckTarget, DefinitionReport, InstanceReport};
use crate::functions::{Direction, TerminalFunctions};
use crate::instances::NewInstance;
use crate::library::{
    BindingDefinition, ComponentDefinition, Definition, DefinitionMeta, DraftEdit,
    FootprintDefinition, PackageDefinition, PackageTerminal, Pad, SymbolDefinition, Terminal,
    TerminalClass, TerminalMap, Unit,
};
use crate::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, DefinitionRef, Endpoint, Engine, Error,
    FootprintRef, FunctionId, Id, InstanceCommand, LibraryCommand, Operation, Outcome, PackageRef,
    PackageTerminalId, PadId, PhysicalChoice, Point, Query, QueryResult, Result, Revision,
    SymbolRef, TerminalId, UnitId,
};

/// A draft unit as core draws it in its own frame, the Component editor's
/// view: the one drawing [`crate::schematic::PlacedUnit::hit`] picks.
pub fn drawn_unit(
    engine: &Engine,
    f: &Fixture,
    unit: UnitId,
) -> Result<crate::schematic::PlacedUnit> {
    let (
        Definition::Symbol(symbol),
        Definition::Component(component),
        Definition::Binding(binding),
        Definition::Package(package),
    ) = (
        body(engine, f.symbol)?,
        body(engine, f.component)?,
        body(engine, f.binding)?,
        body(engine, f.package)?,
    )
    else {
        return Err(Error::Invalid("unexpected draft kinds".into()));
    };
    let name = engine
        .drafts()
        .get(f.component)
        .map(|d| d.meta.name.clone())
        .unwrap_or_default();
    crate::schematic::definition_unit(
        &engine.project().library,
        crate::schematic::DefinitionUnit {
            symbol: &symbol,
            unit,
            component: &component,
            component_name: &name,
            physical: Some((&binding, &package)),
        },
    )
}

pub fn apply(engine: &mut Engine, operation: Operation) -> Result<Outcome> {
    engine.apply(engine.revision(), operation)
}

/// Leave one undone step on the redo stack, so a failed operation that
/// committed or cleared history would be visible.
pub fn prime_history(engine: &mut Engine) -> Result<()> {
    let instance = engine
        .project()
        .instances
        .first()
        .map(|i| i.id)
        .ok_or_else(|| Error::NotFound("no instance".into()))?;
    apply(
        engine,
        Operation::Instance(InstanceCommand::SetDesignator {
            instance,
            designator: "PRIMED".into(),
        }),
    )?;
    engine.undo()?;
    assert!(engine.can_redo());
    Ok(())
}

/// The state a failed operation must leave unchanged.
pub struct Snapshot {
    document: crate::Document,
    revision: u64,
    undo: bool,
    redo: bool,
}

impl Snapshot {
    pub fn of(engine: &Engine) -> Self {
        Self {
            document: engine.document().clone(),
            revision: engine.revision(),
            undo: engine.can_undo(),
            redo: engine.can_redo(),
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Document, revision and undo/redo history are unchanged.
    pub fn assert_kept(&self, engine: &Engine, what: &str) {
        assert_eq!(engine.document(), &self.document, "{what}");
        assert_eq!(engine.revision(), self.revision, "{what}");
        assert_eq!(engine.can_undo(), self.undo, "{what}: undo history");
        assert_eq!(engine.can_redo(), self.redo, "{what}: redo history");
    }
}

pub fn ep(instance: Id, terminal: u32) -> Endpoint {
    Endpoint {
        instance,
        terminal: TerminalId(terminal),
    }
}

pub fn meta(name: &str) -> DefinitionMeta {
    DefinitionMeta {
        name: name.into(),
        description: String::new(),
        provenance: "Original synthetic test fixture".into(),
    }
}

pub fn new_draft(engine: &mut Engine, kind: DefinitionKind) -> Result<DefinitionId> {
    match apply(
        engine,
        Operation::Library(LibraryCommand::NewDraft {
            meta: DefinitionMeta::default(),
            definition: Definition::empty(kind),
        }),
    )? {
        Outcome::DraftCreated { id } => Ok(id),
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

pub fn edit(engine: &mut Engine, edits: Vec<DraftEdit>) -> Result<()> {
    apply(engine, Operation::Library(LibraryCommand::Edit { edits })).map(|_| ())
}

pub fn publish(engine: &mut Engine, drafts: Vec<DefinitionId>) -> Result<Vec<DefinitionRef>> {
    match apply(
        engine,
        Operation::Library(LibraryCommand::Publish { drafts }),
    )? {
        Outcome::Published { revisions } => Ok(revisions),
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

pub fn draft_from(engine: &mut Engine, revision: DefinitionRef) -> Result<()> {
    apply(
        engine,
        Operation::Library(LibraryCommand::DraftFrom { revision }),
    )
    .map(|_| ())
}

pub fn body(engine: &Engine, id: DefinitionId) -> Result<Definition> {
    engine
        .drafts()
        .get(id)
        .map(|d| d.definition.clone())
        .ok_or_else(|| Error::NotFound(format!("no draft {id}")))
}

/// Replace one draft body, keeping its envelope.
pub fn rewrite(
    engine: &mut Engine,
    id: DefinitionId,
    f: impl FnOnce(&mut Definition),
) -> Result<()> {
    let draft = engine
        .drafts()
        .get(id)
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("no draft {id}")))?;
    let mut definition = draft.definition;
    f(&mut definition);
    edit(
        engine,
        vec![DraftEdit {
            id,
            meta: draft.meta,
            definition,
        }],
    )
}

pub fn inspect(engine: &Engine, instance: Id) -> Result<InstanceReport> {
    match engine.query(Query::InspectInstance { instance })? {
        QueryResult::Instance(report) => Ok(*report),
        other => Err(Error::Invalid(format!("unexpected report {other:?}"))),
    }
}

pub fn check(engine: &Engine, target: CheckTarget) -> Result<DefinitionReport> {
    match engine.query(Query::CheckDefinition { target })? {
        QueryResult::Definition(report) => Ok(report),
        other => Err(Error::Invalid(format!("unexpected report {other:?}"))),
    }
}

/// Finding codes with the objects they name, for exact comparisons.
pub fn codes(findings: &[crate::Finding]) -> Vec<(String, Vec<crate::ObjectRef>)> {
    let mut all: Vec<_> = findings
        .iter()
        .map(|f| (f.code.clone(), f.objects.clone()))
        .collect();
    all.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    all
}

/// The body every fixture unit's pins point into.
const FIXTURE_BODY: crate::geometry::Bounds = crate::geometry::Bounds {
    min: [-3.0, -3.0],
    max: [3.0, 3.0],
};

/// A unit: ID, label and (terminal, pin connection end) pairs.
pub type UnitSpec<'a> = (u32, &'a str, Vec<(u32, Point)>);

/// A synthetic component: terminals, units with pins, package
/// designations and an explicit mapping (package terminal → terminal).
pub struct Spec<'a> {
    pub name: &'a str,
    /// (terminal id, name, class). Function IDs 0 are placeholders that
    /// [`bodies`] allocates after every other local ID.
    pub terminals: Vec<(u32, &'a str, TerminalClass)>,
    /// (unit id, label, pins as (terminal, position))
    pub units: Vec<UnitSpec<'a>>,
    /// (package terminal id, designation)
    pub package: Vec<(u32, &'a str)>,
    /// (package terminal, electrical terminal)
    pub mapping: Vec<(u32, u32)>,
    /// Publish a binding and footprint.
    pub physical: bool,
}

/// The lineages of one published fixture.
#[derive(Clone, Copy, Debug)]
pub struct Fixture {
    pub component: DefinitionId,
    pub symbol: DefinitionId,
    pub package: DefinitionId,
    pub footprint: DefinitionId,
    pub binding: DefinitionId,
}

impl Fixture {
    pub fn r1(&self) -> ComponentRef {
        ComponentRef::new(self.component, Revision::FIRST)
    }
    pub fn physical(&self) -> PhysicalChoice {
        PhysicalChoice {
            binding: self.binding,
            footprint: self.footprint,
        }
    }
    pub fn all(&self) -> Vec<DefinitionId> {
        vec![
            self.component,
            self.symbol,
            self.package,
            self.footprint,
            self.binding,
        ]
    }
}

fn next_local(ids: impl Iterator<Item = u32>) -> u32 {
    ids.max().unwrap_or(0).saturating_add(1)
}

/// One fixed function of `direction`, with a placeholder ID.
pub fn fixed(direction: Direction) -> TerminalClass {
    TerminalClass::Electrical(TerminalFunctions::fixed(FunctionId(0), direction))
}
pub fn passive() -> TerminalClass {
    fixed(Direction::Passive)
}
pub fn input() -> TerminalClass {
    fixed(Direction::Input)
}
/// A fixed push-pull output.
pub fn output() -> TerminalClass {
    fixed(Direction::Output)
}
pub fn power_in() -> TerminalClass {
    fixed(Direction::PowerInput)
}
pub fn power_out() -> TerminalClass {
    fixed(Direction::PowerOutput)
}

/// Terminals of a spec with placeholder function IDs replaced by fresh IDs
/// above every declared local ID, and the resulting `next_local`.
pub fn terminals(spec: &[(u32, &str, TerminalClass)]) -> (Vec<Terminal>, u32) {
    let declared = spec.iter().flat_map(|(id, _, class)| {
        let functions = match class {
            TerminalClass::Electrical(f) => f.functions.iter().map(|f| f.id.0).collect(),
            _ => Vec::new(),
        };
        std::iter::once(*id).chain(functions)
    });
    let mut next = next_local(declared);
    let terminals = spec
        .iter()
        .map(|(id, name, class)| {
            let mut class = class.clone();
            if let TerminalClass::Electrical(f) = &mut class {
                for function in &mut f.functions {
                    if function.id.0 == 0 {
                        function.id = FunctionId(next);
                        next += 1;
                    }
                }
            }
            Terminal {
                id: TerminalId(*id),
                name: (*name).into(),
                class,
            }
        })
        .collect();
    (terminals, next)
}

/// Bodies for a spec, pinning each sibling's first revision.
pub fn bodies(spec: &Spec<'_>, f: &Fixture) -> Vec<DraftEdit> {
    let first = Revision::FIRST;
    let (terminals, next) = terminals(&spec.terminals);
    let component = ComponentDefinition {
        next_local: next,
        designator_prefix: "U".into(),
        terminals,
        resources: vec![],
        constraints: vec![],
        representations: vec![SymbolRef::new(f.symbol, first)],
        implementations: if spec.physical {
            vec![BindingRef::new(f.binding, first)]
        } else {
            vec![]
        },
        fields: vec![],
        pin_rules: Vec::new(),
        marking: None,
        review: None,
    };
    let symbol = SymbolDefinition {
        component: f.component,
        next_local: next_local(spec.units.iter().map(|u| u.0)),
        pin_text_height: crate::library::graphics::PIN_TEXT_HEIGHT_MM,
        units: spec
            .units
            .iter()
            .map(|(id, label, unit_pins)| Unit {
                id: UnitId(*id),
                label: (*label).into(),
                graphics: vec![],
                pins: unit_pins
                    .iter()
                    .map(|(terminal, position)| {
                        crate::library::graphics::pin_toward(
                            TerminalId(*terminal),
                            *position,
                            FIXTURE_BODY,
                        )
                    })
                    .collect(),
            })
            .collect(),
    };
    let package = PackageDefinition {
        next_local: next_local(spec.package.iter().map(|p| p.0)),
        terminals: spec
            .package
            .iter()
            .map(|(id, designation)| PackageTerminal {
                id: PackageTerminalId(*id),
                designation: (*designation).into(),
            })
            .collect(),
        envelope: [4.0, 3.0, 1.0],
        ..PackageDefinition::default()
    };
    let footprint = FootprintDefinition {
        package: PackageRef::new(f.package, first),
        next_local: next_local(spec.package.iter().map(|p| p.0)),
        pads: spec
            .package
            .iter()
            .enumerate()
            .map(|(i, (id, _))| {
                Pad::smd_rect(
                    PadId(*id),
                    PackageTerminalId(*id),
                    id.to_string(),
                    [i as f64 * 1.5 - 1.5, 0.0],
                    [0.6, 0.8],
                )
            })
            .collect(),
        ..FootprintDefinition::default()
    };
    let binding = BindingDefinition {
        component: f.component,
        package: PackageRef::new(f.package, first),
        mappings: spec
            .mapping
            .iter()
            .map(|(package_terminal, terminal)| TerminalMap {
                package_terminal: PackageTerminalId(*package_terminal),
                terminal: TerminalId(*terminal),
            })
            .collect(),
        footprints: vec![FootprintRef::new(f.footprint, first)],
        unavailable: vec![],
    };
    vec![
        DraftEdit {
            id: f.component,
            meta: meta(spec.name),
            definition: Definition::Component(component),
        },
        DraftEdit {
            id: f.symbol,
            meta: meta(&format!("{} symbol", spec.name)),
            definition: Definition::Symbol(symbol),
        },
        DraftEdit {
            id: f.package,
            meta: meta(&format!("{} package", spec.name)),
            definition: Definition::Package(package),
        },
        DraftEdit {
            id: f.footprint,
            meta: meta(&format!("{} footprint", spec.name)),
            definition: Definition::Footprint(footprint),
        },
        DraftEdit {
            id: f.binding,
            meta: meta(&format!("{} binding", spec.name)),
            definition: Definition::Binding(binding),
        },
    ]
}

/// Reserve five lineages and fill them from `spec`, without publishing.
pub fn draft(engine: &mut Engine, spec: &Spec<'_>) -> Result<Fixture> {
    let fixture = Fixture {
        component: new_draft(engine, DefinitionKind::Component)?,
        symbol: new_draft(engine, DefinitionKind::Symbol)?,
        package: new_draft(engine, DefinitionKind::Package)?,
        footprint: new_draft(engine, DefinitionKind::Footprint)?,
        binding: new_draft(engine, DefinitionKind::Binding)?,
    };
    edit(engine, bodies(spec, &fixture))?;
    Ok(fixture)
}

/// Draft and publish `spec`. Without a physical implementation only the
/// component and symbol are published.
pub fn component(engine: &mut Engine, spec: &Spec<'_>) -> Result<Fixture> {
    let fixture = draft(engine, spec)?;
    let ids = if spec.physical {
        fixture.all()
    } else {
        vec![fixture.component, fixture.symbol]
    };
    publish(engine, ids)?;
    Ok(fixture)
}

/// One terminal, one unit with its pin at the origin, one pad.
pub fn probe(name: &str, class: TerminalClass) -> Spec<'_> {
    Spec {
        name,
        terminals: vec![(1, "P", class)],
        units: vec![(1, "A", vec![(1, [0.0, 0.0])])],
        package: vec![(1, "1")],
        mapping: vec![(1, 1)],
        physical: true,
    }
}

pub fn place(
    engine: &mut Engine,
    fixture: &Fixture,
    designator: &str,
    unit: u32,
    position: Point,
) -> Result<(Id, Id)> {
    place_ref(engine, fixture, fixture.r1(), designator, unit, position)
}

pub fn place_ref(
    engine: &mut Engine,
    fixture: &Fixture,
    component: ComponentRef,
    designator: &str,
    unit: u32,
    position: Point,
) -> Result<(Id, Id)> {
    let physical = engine
        .project()
        .library
        .component(component)
        .is_some_and(|c| !c.definition.implementations.is_empty())
        .then(|| fixture.physical());
    match apply(
        engine,
        Operation::Instance(InstanceCommand::Place(NewInstance {
            component,
            designator: Some(designator.into()),
            properties: vec![],
            representation: fixture.symbol,
            unit: UnitId(unit),
            position,
            physical,
        })),
    )? {
        Outcome::InstancePlaced {
            instance,
            occurrence,
            ..
        } => Ok((instance, occurrence)),
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

pub fn connect(engine: &mut Engine, from: Endpoint, to: Endpoint) -> Result<()> {
    apply(
        engine,
        Operation::Schematic(crate::schematic::SchematicCommand::Connect { from, to }),
    )
    .map(|_| ())
}

pub fn wire_between(engine: &Engine, a: Endpoint, b: Endpoint) -> Result<Id> {
    engine
        .project()
        .wires
        .iter()
        .find(|w| (w.from == a && w.to == b) || (w.from == b && w.to == a))
        .map(|w| w.id)
        .ok_or_else(|| Error::NotFound("wire".into()))
}

pub fn by_designator(engine: &Engine, designator: &str) -> Result<Id> {
    engine
        .project()
        .instance_by_designator(designator)
        .map(|i| i.id)
        .ok_or_else(|| Error::NotFound(designator.into()))
}

pub fn net_of(engine: &Engine, terminal: Endpoint) -> Result<Id> {
    engine
        .project()
        .net_of(terminal)
        .map(|net| net.id)
        .ok_or_else(|| Error::NotFound("terminal has no net".into()))
}

/// The demo's library lineages, in reservation order.
pub fn demo_fixture(engine: &Engine) -> Result<Fixture> {
    let drafts = &engine.drafts().items;
    let id = |kind: DefinitionKind| {
        drafts
            .iter()
            .find(|d| d.definition.kind() == kind)
            .map(|d| d.id)
            .ok_or_else(|| Error::NotFound(format!("{kind:?} draft")))
    };
    Ok(Fixture {
        component: id(DefinitionKind::Component)?,
        symbol: id(DefinitionKind::Symbol)?,
        package: id(DefinitionKind::Package)?,
        footprint: id(DefinitionKind::Footprint)?,
        binding: id(DefinitionKind::Binding)?,
    })
}

/// Point every sibling reference of a fixture's drafts at the siblings'
/// pending revisions, so the five can be published together as a cascade.
pub fn retarget(engine: &mut Engine, f: &Fixture) -> Result<()> {
    let pending = |engine: &Engine, id: DefinitionId| {
        engine
            .drafts()
            .get(id)
            .and_then(|d| d.pending())
            .ok_or_else(|| Error::NotFound(format!("no draft {id}")))
    };
    let symbol = SymbolRef::new(f.symbol, pending(engine, f.symbol)?);
    let package = PackageRef::new(f.package, pending(engine, f.package)?);
    let footprint = FootprintRef::new(f.footprint, pending(engine, f.footprint)?);
    let binding = BindingRef::new(f.binding, pending(engine, f.binding)?);
    let mut edits = Vec::new();
    for id in f.all() {
        let draft = engine
            .drafts()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("no draft {id}")))?;
        let mut definition = draft.definition;
        match &mut definition {
            Definition::Component(c) => {
                for r in &mut c.representations {
                    if r.id == f.symbol {
                        *r = symbol;
                    }
                }
                for r in &mut c.implementations {
                    if r.id == f.binding {
                        *r = binding;
                    }
                }
            }
            Definition::Footprint(fp) => fp.package = package,
            Definition::Binding(b) => {
                b.package = package;
                for r in &mut b.footprints {
                    if r.id == f.footprint {
                        *r = footprint;
                    }
                }
            }
            Definition::Symbol(_)
            | Definition::Package(_)
            | Definition::QuantityKind(_)
            | Definition::Field(_)
            | Definition::Part(_)
            | Definition::Model(_)
            | Definition::PackageRow(_) => {}
        }
        edits.push(DraftEdit {
            id,
            meta: draft.meta,
            definition,
        });
    }
    edit(engine, edits)
}

/// Publish the fixture's five drafts as a cascade after `change` edits them,
/// returning the new component revision.
pub fn cascade(
    engine: &mut Engine,
    f: &Fixture,
    change: impl FnOnce(&mut Engine) -> Result<()>,
) -> Result<ComponentRef> {
    change(engine)?;
    retarget(engine, f)?;
    let revisions = publish(engine, f.all())?;
    revisions
        .into_iter()
        .find(|r| r.id == f.component)
        .map(|r| ComponentRef::new(r.id, r.revision))
        .ok_or_else(|| Error::NotFound("component revision".into()))
}

pub fn component_body(definition: &mut Definition) -> Result<&mut ComponentDefinition> {
    match definition {
        Definition::Component(c) => Ok(c),
        _ => Err(Error::Invalid("not a component".into())),
    }
}
pub fn symbol_body(definition: &mut Definition) -> Result<&mut SymbolDefinition> {
    match definition {
        Definition::Symbol(s) => Ok(s),
        _ => Err(Error::Invalid("not a symbol".into())),
    }
}
pub fn footprint_body(definition: &mut Definition) -> Result<&mut FootprintDefinition> {
    match definition {
        Definition::Footprint(f) => Ok(f),
        _ => Err(Error::Invalid("not a footprint".into())),
    }
}
pub fn binding_body(definition: &mut Definition) -> Result<&mut BindingDefinition> {
    match definition {
        Definition::Binding(b) => Ok(b),
        _ => Err(Error::Invalid("not a binding".into())),
    }
}
pub fn package_body(definition: &mut Definition) -> Result<&mut PackageDefinition> {
    match definition {
        Definition::Package(p) => Ok(p),
        _ => Err(Error::Invalid("not a package".into())),
    }
}

/// Edit a draft body of a known kind through `f`, failing on the wrong kind.
pub fn with<T>(
    engine: &mut Engine,
    id: DefinitionId,
    get: fn(&mut Definition) -> Result<&mut T>,
    f: impl FnOnce(&mut T),
) -> Result<()> {
    let mut failure = None;
    rewrite(engine, id, |definition| match get(definition) {
        Ok(body) => f(body),
        Err(error) => failure = Some(error),
    })?;
    failure.map_or(Ok(()), Err)
}

/// Draft and publish one definition of any kind, returning its revision.
pub fn publish_one(
    engine: &mut Engine,
    name: &str,
    definition: Definition,
) -> Result<DefinitionRef> {
    let id = new_draft(engine, definition.kind())?;
    edit(
        engine,
        vec![DraftEdit {
            id,
            meta: meta(name),
            definition,
        }],
    )?;
    publish(engine, vec![id])?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("nothing published".into()))
}

pub fn publish_kind(
    engine: &mut Engine,
    name: &str,
    kind: crate::fields::QuantityKind,
) -> Result<crate::QuantityKindRef> {
    let r = publish_one(engine, name, Definition::QuantityKind(kind))?;
    Ok(crate::QuantityKindRef::new(r.id, r.revision))
}

pub fn publish_field(
    engine: &mut Engine,
    name: &str,
    field: crate::fields::FieldDefinition,
) -> Result<crate::FieldRef> {
    let r = publish_one(engine, name, Definition::Field(field))?;
    Ok(crate::FieldRef::new(r.id, r.revision))
}

/// Draft `spec`, select `fields` on its component and publish it.
pub fn component_with_fields(
    engine: &mut Engine,
    spec: &Spec<'_>,
    fields: Vec<crate::fields::FieldSelection>,
) -> Result<Fixture> {
    let fixture = draft(engine, spec)?;
    with(engine, fixture.component, component_body, |c| {
        c.fields = fields
    })?;
    let ids = if spec.physical {
        fixture.all()
    } else {
        vec![fixture.component, fixture.symbol]
    };
    publish(engine, ids)?;
    Ok(fixture)
}

/// A field selection shown on the symbol, without a component default.
pub fn select(field: crate::FieldRef) -> crate::fields::FieldSelection {
    crate::fields::FieldSelection {
        field,
        default: None,
        show_on_symbol: true,
    }
}

pub fn set_properties(
    engine: &mut Engine,
    instance: Id,
    edits: Vec<crate::fields::PropertyEdit>,
) -> Result<Vec<crate::fields::EffectiveField>> {
    match apply(
        engine,
        Operation::Instance(InstanceCommand::SetProperties { instance, edits }),
    )? {
        Outcome::PropertiesSet { effective } => Ok(effective),
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

pub fn text(field: crate::FieldRef, text: &str) -> crate::fields::PropertyEdit {
    crate::fields::PropertyEdit {
        field: field.id,
        input: crate::fields::PropertyInput::Text(text.into()),
    }
}
