//! Original synthetic catalog data for tests, benchmarks and trying the
//! Place palette. Every name is an invented `EX-` identifier; values follow
//! the generic preferred-number habit of passive parts and come from no
//! vendor, EDA library or taxonomy. Nothing here is verified against a real
//! component.
use super::{CatalogEntry, Index, Vocabulary, entries};
use crate::diagnostics::{Error, Result};
use crate::fields::{
    FieldDefinition, FieldSelection, FieldValue, QuantityKind, RatedSpec, SpecMeaning,
    ToleranceBasis, ValueType,
};
use crate::ids::{
    BindingRef, ComponentRef, DefinitionId, FieldRef, FootprintRef, PackageRef, QuantityKindRef,
    Revision, SymbolRef,
};
use crate::library::samples::{self, Kind, SampleRefs};
use crate::library::{
    Definition, DefinitionMeta, DistributorOffer, DraftEdit, Library, PartDefinition, Provenance,
    Published, Reference, ReferenceKind,
};
use crate::model::Project;
use crate::quantity::{Decimal, UnitOfMeasure, UnitSet};

/// Parts one synthetic project holds; each takes three lineages.
pub const MAX_PARTS: usize = 1000;

const PROVENANCE: &str = "Synthetic catalog fixture; invented values, not a real component.";

/// Resistances (Ω) with their invented code.
const RESISTORS: &[(f64, &str)] = &[
    (10e3, "10K0"),
    (4.7e3, "4K70"),
    (22e3, "22K0"),
    (470.0, "470R"),
    (1e6, "1M00"),
    (100e3, "100K"),
    (0.47, "R470"),
    (2.2e3, "2K20"),
];
/// Relative tolerances with their invented code letter.
const TOLERANCES: &[(f64, &str)] = &[(0.01, "F"), (0.05, "J"), (0.005, "D")];
/// Capacitances (F) with their invented code.
const CAPACITORS: &[(f64, &str)] = &[
    (100e-9, "100N"),
    (10e-6, "10U0"),
    (4.7e-9, "4N70"),
    (1e-6, "1U00"),
    (22e-12, "22P0"),
];
const VOLTAGES: &[f64] = &[16.0, 50.0, 25.0, 6.3];
/// Size labels: plain text, as package names are.
const SIZES: &[&str] = &["0603", "0402"];
/// Invented 0402 geometry: envelope (mm), pad centres and land size. The
/// 0603 parts keep the shared sample's geometry.
const SMALL: ([f64; 3], [[f64; 2]; 2], [f64; 2]) =
    ([1.0, 0.5, 0.35], [[-0.5, 0.0], [0.5, 0.0]], [0.5, 0.6]);
const HEADER: &str = "TH 2.54";

struct Allocator(u64);

impl Allocator {
    fn next(&mut self) -> Result<DefinitionId> {
        let id = self.0;
        self.0 = id
            .checked_add(1)
            .ok_or_else(|| Error::Limit("synthetic identity space exhausted".into()))?;
        Ok(DefinitionId(id))
    }
}

fn meta(name: &str, description: &str) -> DefinitionMeta {
    DefinitionMeta {
        name: name.into(),
        description: description.into(),
        provenance: PROVENANCE.into(),
    }
}

fn first<T>(id: DefinitionId, meta: DefinitionMeta, definition: T) -> Published<T> {
    Published {
        id,
        revision: Revision::FIRST,
        meta,
        definition,
    }
}

fn push(library: &mut Library, edit: DraftEdit) {
    let DraftEdit {
        id,
        meta,
        definition,
    } = edit;
    match definition {
        Definition::Component(d) => library.components.push(first(id, meta, d)),
        Definition::Symbol(d) => library.symbols.push(first(id, meta, d)),
        Definition::Package(d) => library.packages.push(first(id, meta, d)),
        Definition::Footprint(d) => library.footprints.push(first(id, meta, d)),
        Definition::Binding(d) => library.bindings.push(first(id, meta, d)),
        Definition::QuantityKind(d) => library.quantity_kinds.push(first(id, meta, d)),
        Definition::Field(d) => library.fields.push(first(id, meta, d)),
        Definition::Part(d) => library.parts.push(first(id, meta, *d)),
        Definition::Model(d) => library.models.push(first(id, meta, *d)),
        Definition::PackageRow(d) => library.package_rows.push(first(id, meta, *d)),
    }
}

fn kind(dimension: [i8; 7], symbol: &str, aliases: &[&str]) -> QuantityKind {
    QuantityKind {
        units: UnitSet {
            dimension,
            units: vec![UnitOfMeasure {
                symbol: symbol.into(),
                aliases: aliases.iter().map(|a| (*a).into()).collect(),
                scale: 1.0,
                si_prefixes: true,
            }],
        },
        default_unit: symbol.into(),
    }
}

/// The fields every synthetic part may select.
struct Fields {
    resistance: DefinitionId,
    capacitance: DefinitionId,
    tolerance: DefinitionId,
    voltage: DefinitionId,
}

fn fields(library: &mut Library, ids: &mut Allocator) -> Result<Fields> {
    let mut quantity_field = |library: &mut Library,
                              name: &str,
                              dimension: [i8; 7],
                              symbol: &str,
                              aliases: &[&str],
                              field: &str|
     -> Result<DefinitionId> {
        let kind_id = ids.next()?;
        let field_id = ids.next()?;
        library.quantity_kinds.push(first(
            kind_id,
            meta(name, ""),
            kind(dimension, symbol, aliases),
        ));
        library.fields.push(first(
            field_id,
            meta(field, ""),
            FieldDefinition {
                next_local: 1,
                value_type: ValueType::Quantity {
                    kind: QuantityKindRef::new(kind_id, Revision::FIRST),
                    min: Some(0.0),
                    max: None,
                },
                default: None,
                required_at: None,
            },
        ));
        Ok(field_id)
    };
    let resistance = quantity_field(
        library,
        "Resistance",
        [2, 1, -3, -2, 0, 0, 0],
        "Ω",
        &["ohm", "R"],
        "Resistance",
    )?;
    let capacitance = quantity_field(
        library,
        "Capacitance",
        [-2, -1, 4, 2, 0, 0, 0],
        "F",
        &[],
        "Capacitance",
    )?;
    let voltage = quantity_field(
        library,
        "Voltage",
        [2, 1, -3, -1, 0, 0, 0],
        "V",
        &[],
        "Max voltage",
    )?;
    let tolerance = ids.next()?;
    library.fields.push(first(
        tolerance,
        meta("Tolerance", ""),
        FieldDefinition {
            next_local: 1,
            value_type: ValueType::Tolerance {
                basis: ToleranceBasis::Relative,
            },
            default: None,
            required_at: None,
        },
    ));
    Ok(Fields {
        resistance,
        capacitance,
        tolerance,
        voltage,
    })
}

/// The three part families, each with its own designator prefix and body.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Resistor,
    Capacitor,
    Header,
}

impl Family {
    fn kind(self) -> Kind<'static> {
        match self {
            Self::Resistor => Kind::RESISTOR,
            Self::Capacitor => Kind::CAPACITOR,
            Self::Header => Kind::CONNECTOR,
        }
    }

    fn edits(self, name: &str, refs: SampleRefs) -> Result<Vec<DraftEdit>> {
        match self {
            Self::Header => samples::two_terminal_header(name, self.kind(), refs),
            _ => samples::two_terminal(name, self.kind(), refs),
        }
    }
}

/// A package and its footprint, shared by every part of one family and
/// size. Resistors and capacitors of one size code are different packages
/// (different bodies) under the same name.
fn shared_package(
    library: &mut Library,
    ids: &mut Allocator,
    label: &str,
    family: Family,
) -> Result<(PackageRef, FootprintRef)> {
    let package = PackageRef::new(ids.next()?, Revision::FIRST);
    let footprint = FootprintRef::new(ids.next()?, Revision::FIRST);
    let refs = SampleRefs {
        component: DefinitionId(0),
        symbol: SymbolRef::default(),
        package,
        footprint,
        binding: BindingRef::default(),
    };
    let edits = family.edits(label, refs)?;
    let small = label == "0402";
    let mut body = None;
    for mut edit in edits {
        match &mut edit.definition {
            Definition::Package(package) => {
                if small {
                    // A typed form other than a chip derives its envelope;
                    // setting it by hand would publish a stale one.
                    package.resize_envelope(SMALL.0)?;
                }
                body = Some(package.clone());
                edit.meta = meta(label, "Synthetic two-terminal package");
                push(library, edit);
            }
            Definition::Footprint(footprint) => {
                if small {
                    for (pad, position) in footprint.pads.iter_mut().zip(SMALL.1) {
                        pad.position = crate::units::PointNm::from_mm_f64_rounded(position)?;
                        pad.shape
                            .set_size(crate::units::PointNm::from_mm_f64_rounded(SMALL.2)?);
                    }
                    // The drawings follow the smaller lands and body.
                    if let Some(package) = &body {
                        samples::redraw(footprint, package);
                    }
                }
                edit.meta = meta(&format!("{label} lands"), "Synthetic two-land footprint");
                push(library, edit);
            }
            _ => {}
        }
    }
    Ok((package, footprint))
}

/// A synthetic part: its name, description, family, package size (an
/// index into [`SIZES`]; headers have none) and field defaults.
struct Part {
    name: String,
    description: String,
    family: Family,
    size: usize,
    values: Vec<(DefinitionId, FieldValue, bool)>,
}

fn part(index: usize, fields: &Fields) -> Part {
    let round = index / 3;
    let generation = |len: usize| {
        let lap = round / len.max(1);
        if lap == 0 {
            String::new()
        } else {
            format!("-{}", lap + 1)
        }
    };
    // Finite constants always have a decimal form.
    let quantity = |si: f64| FieldValue::Quantity {
        si: Decimal::from_f64(si).unwrap_or_default(),
        display_unit: None,
    };
    let cycle = |len: usize| round % len.max(1);
    match index % 3 {
        0 => {
            let (ohms, code) = RESISTORS[cycle(RESISTORS.len())];
            let (tolerance, letter) = TOLERANCES[round % TOLERANCES.len()];
            let size = round % SIZES.len();
            Part {
                name: format!(
                    "EX-R{}-{code}-{letter}{}",
                    SIZES[size],
                    generation(RESISTORS.len())
                ),
                description: format!("Synthetic thick-film resistor, {} package", SIZES[size]),
                family: Family::Resistor,
                size,
                values: vec![
                    (fields.resistance, quantity(ohms), true),
                    (
                        fields.tolerance,
                        FieldValue::Tolerance {
                            minus: Decimal::from_f64(tolerance).unwrap_or_default(),
                            plus: Decimal::from_f64(tolerance).unwrap_or_default(),
                        },
                        false,
                    ),
                ],
            }
        }
        1 => {
            let (farads, code) = CAPACITORS[cycle(CAPACITORS.len())];
            let volts = VOLTAGES[round % VOLTAGES.len()];
            let size = (round + 1) % SIZES.len();
            Part {
                name: format!(
                    "EX-C{}-{code}-{}V{}",
                    SIZES[size],
                    crate::quantity::format_number(volts),
                    generation(CAPACITORS.len())
                ),
                description: format!("Synthetic ceramic capacitor, {} package", SIZES[size]),
                family: Family::Capacitor,
                size,
                values: vec![
                    (fields.capacitance, quantity(farads), true),
                    (fields.voltage, quantity(volts), false),
                ],
            }
        }
        _ => Part {
            name: format!("EX-HDR-1X2-{:03}", round + 1),
            description: "Synthetic two-pin header, 2.54 mm pitch".into(),
            family: Family::Header,
            size: 0,
            values: vec![],
        },
    }
}

/// The invented manufacturer of every synthetic Part.
pub const MANUFACTURER: &str = "ExampleCo";

/// A project whose library holds `parts` published synthetic components
/// (resistors, capacitors and headers in turn) with typed field defaults.
pub fn project(parts: usize) -> Result<Project> {
    build(parts, false)
}

/// [`project`] plus two generic components without values, a 0603
/// resistor and a 0402 capacitor, and exact `ExampleCo` Parts that pin
/// them: `EX-R0603-10K0-F`, `EX-R0603-22K0-F` (±1 %), `EX-C0402-100N-16V`
/// and `EX-C0402-100N-25V` (rated voltage as a minimum rating).
pub fn project_with_parts(parts: usize) -> Result<Project> {
    build(parts, true)
}

fn build(parts: usize, exact: bool) -> Result<Project> {
    if parts > MAX_PARTS {
        return Err(Error::Limit(format!(
            "a synthetic project holds at most {MAX_PARTS} parts"
        )));
    }
    let mut ids = Allocator(1);
    let mut library = Library::default();
    let fields = fields(&mut library, &mut ids)?;
    let mut resistors = Vec::new();
    let mut capacitors = Vec::new();
    for size in SIZES {
        resistors.push(shared_package(
            &mut library,
            &mut ids,
            size,
            Family::Resistor,
        )?);
        capacitors.push(shared_package(
            &mut library,
            &mut ids,
            size,
            Family::Capacitor,
        )?);
    }
    let header = shared_package(&mut library, &mut ids, HEADER, Family::Header)?;
    for index in 0..parts {
        let part = part(index, &fields);
        let sized = |packages: &[(PackageRef, FootprintRef)]| {
            packages.get(part.size).copied().unwrap_or(header)
        };
        let (package, footprint) = match part.family {
            Family::Resistor => sized(&resistors),
            Family::Capacitor => sized(&capacitors),
            Family::Header => header,
        };
        let refs = SampleRefs {
            component: ids.next()?,
            symbol: SymbolRef::new(ids.next()?, Revision::FIRST),
            package,
            footprint,
            binding: BindingRef::new(ids.next()?, Revision::FIRST),
        };
        for mut edit in part.family.edits(&part.name, refs)? {
            match &mut edit.definition {
                Definition::Component(component) => {
                    component.fields = part
                        .values
                        .iter()
                        .map(|(field, value, shown)| FieldSelection {
                            field: FieldRef::new(*field, Revision::FIRST),
                            default: Some(value.clone()),
                            show_on_symbol: *shown,
                        })
                        .collect();
                    edit.meta = meta(&part.name, &part.description);
                }
                Definition::Symbol(_) | Definition::Binding(_) => {
                    edit.meta.provenance = PROVENANCE.into();
                }
                _ => continue,
            }
            push(&mut library, edit);
        }
    }
    if exact {
        let small = |packages: &[(PackageRef, FootprintRef)], i: usize| {
            packages.get(i).copied().unwrap_or(header)
        };
        exact_parts(
            &mut library,
            &mut ids,
            &fields,
            small(&resistors, 0),
            small(&capacitors, 1),
        )?;
    }
    Ok(Project {
        name: "Synthetic parts".into(),
        next_id: ids.0,
        library,
        ..Project::default()
    })
}

/// A generic component of `family` selecting `selected` without defaults
/// (the first shown on the symbol), with one binding to `package`.
fn generic(
    library: &mut Library,
    ids: &mut Allocator,
    family: Family,
    name: &str,
    (package, footprint): (PackageRef, FootprintRef),
    selected: &[DefinitionId],
) -> Result<(ComponentRef, BindingRef)> {
    let refs = SampleRefs {
        component: ids.next()?,
        symbol: SymbolRef::new(ids.next()?, Revision::FIRST),
        package,
        footprint,
        binding: BindingRef::new(ids.next()?, Revision::FIRST),
    };
    for mut edit in family.edits(name, refs)? {
        match &mut edit.definition {
            Definition::Component(component) => {
                component.fields = selected
                    .iter()
                    .enumerate()
                    .map(|(i, field)| FieldSelection {
                        field: FieldRef::new(*field, Revision::FIRST),
                        default: None,
                        show_on_symbol: i == 0,
                    })
                    .collect();
                edit.meta = meta(name, "Synthetic generic part without a value");
            }
            Definition::Symbol(_) | Definition::Binding(_) => {
                edit.meta.provenance = PROVENANCE.into();
            }
            _ => continue,
        }
        push(library, edit);
    }
    Ok((
        ComponentRef::new(refs.component, Revision::FIRST),
        refs.binding,
    ))
}

/// One synthetic Part: invented MPN, a datasheet record without a copy and
/// one invented distributor identity.
fn exact_part(
    mpn: &str,
    (component, binding): (ComponentRef, BindingRef),
    specs: Vec<(DefinitionId, FieldValue, SpecMeaning)>,
) -> PartDefinition {
    PartDefinition {
        next_local: 3,
        manufacturer: MANUFACTURER.into(),
        mpn: mpn.into(),
        component,
        binding,
        specs: specs
            .into_iter()
            .map(|(field, value, meaning)| RatedSpec {
                field: FieldRef::new(field, Revision::FIRST),
                value,
                meaning,
            })
            .collect(),
        references: vec![Reference {
            id: 1,
            kind: ReferenceKind::Datasheet,
            title: format!("{mpn} synthetic datasheet"),
            url: Some(format!(
                "https://example.com/exampleco/{}",
                mpn.to_lowercase()
            )),
            document_revision: Some("A".into()),
            asset: None,
            licence: None,
            provenance: Provenance::default(),
        }],
        offers: vec![DistributorOffer {
            id: 2,
            distributor: "Example Distribution".into(),
            sku: format!("EXD-{mpn}"),
            url: None,
            packaging: Some("cut tape".into()),
        }],
        provenance: Provenance::default(),
        top_mark: None,
    }
}

fn exact_parts(
    library: &mut Library,
    ids: &mut Allocator,
    fields: &Fields,
    resistor: (PackageRef, FootprintRef),
    capacitor: (PackageRef, FootprintRef),
) -> Result<()> {
    let quantity = |si: f64| -> Result<FieldValue> {
        Ok(FieldValue::Quantity {
            si: Decimal::from_f64(si)?,
            display_unit: None,
        })
    };
    let r = generic(
        library,
        ids,
        Family::Resistor,
        "EX-R0603 generic resistor",
        resistor,
        &[fields.resistance, fields.tolerance],
    )?;
    let c = generic(
        library,
        ids,
        Family::Capacitor,
        "EX-C0402 generic capacitor",
        capacitor,
        &[fields.capacitance, fields.voltage],
    )?;
    let one_percent = FieldValue::Tolerance {
        minus: Decimal::from_f64(0.01)?,
        plus: Decimal::from_f64(0.01)?,
    };

    let parts = [
        exact_part(
            "EX-R0603-10K0-F",
            r,
            vec![
                (
                    fields.resistance,
                    quantity(10e3)?,
                    SpecMeaning::ExactNominal,
                ),
                (
                    fields.tolerance,
                    one_percent.clone(),
                    SpecMeaning::MaximumTolerance,
                ),
            ],
        ),
        exact_part(
            "EX-R0603-22K0-F",
            r,
            vec![
                (
                    fields.resistance,
                    quantity(22e3)?,
                    SpecMeaning::ExactNominal,
                ),
                (fields.tolerance, one_percent, SpecMeaning::MaximumTolerance),
            ],
        ),
        exact_part(
            "EX-C0402-100N-16V",
            c,
            vec![
                (
                    fields.capacitance,
                    quantity(100e-9)?,
                    SpecMeaning::ExactNominal,
                ),
                (fields.voltage, quantity(16.0)?, SpecMeaning::MinimumRating),
            ],
        ),
        exact_part(
            "EX-C0402-100N-25V",
            c,
            vec![
                (
                    fields.capacitance,
                    quantity(100e-9)?,
                    SpecMeaning::ExactNominal,
                ),
                (fields.voltage, quantity(25.0)?, SpecMeaning::MinimumRating),
            ],
        ),
    ];
    for part in parts {
        let id = ids.next()?;
        let name = part.mpn.clone();
        library.parts.push(first(
            id,
            meta(&name, "Synthetic exact part by an invented manufacturer"),
            part,
        ));
    }
    Ok(())
}

/// An index of `count` synthetic entries for benchmarks: the entries of a
/// small synthetic project repeated with distinct lineages and names.
pub fn index(count: usize) -> Result<Index> {
    let project = project(48)?;
    let base = entries(&project, &Default::default());
    if base.is_empty() {
        return Err(Error::Invalid("synthetic project has no entries".into()));
    }
    let mut out: Vec<CatalogEntry> = Vec::with_capacity(count);
    for k in 0..count {
        let mut entry = base[k % base.len()].clone();
        let lap = k / base.len();
        if lap > 0 {
            entry.lineage = DefinitionId(1_000_000 + k as u64);
            entry.name = format!("{}-{lap:04}", entry.name);
        }
        entry.used = usize::from(k % 97 == 0);
        out.push(entry);
    }
    Index::new(out, Vocabulary::of(&project.library))
}
