//! The built-in standard fields: SI quantity kinds and the fields a passive
//! or a rating usually selects, as an ordinary bundle `library.import` takes
//! (`library.standard_fields`, `library.import_standard_fields`). Names, kinds
//! and identities are this file's own.
//!
//! The lineages and the library identity are fixed, so importing the bundle
//! twice reuses the first copies. A published record never changes: a change
//! to one here is a new revision of it, with the old revision kept, or an
//! import into a project that holds the old copy is `library.source_modified`.
//!
//! No temperature field: a unit here is a scale of the SI unit, and a Celsius
//! reading needs an offset.
use super::bundle::{self, Bundle, SourceRevision, to_record};
use super::package_form::ChipKind;
use super::{Definition, DefinitionMeta, Library};
use crate::diagnostics::{Error, Result};
use crate::fields::{
    Choice, FieldDefinition, FieldSelection, QuantityKind, ToleranceBasis, ValueType,
};
use crate::ids::{
    ChoiceId, DefinitionId, DefinitionKind, FieldRef, LibraryId, QuantityKindRef, Revision,
    SourceLineageId,
};
use crate::quantity::{UnitOfMeasure, UnitSet};

/// The source name imported copies show.
pub const NAME: &str = "Standard fields";

const LIBRARY: &str = "5f0d2c7a9e3b41688ac4e17db26f9035";

/// One quantity kind and the field that selects it.
struct Standard {
    kind_name: &'static str,
    kind_lineage: &'static str,
    field_name: &'static str,
    field_lineage: &'static str,
    /// SI exponents: length, mass, time, current, temperature, amount,
    /// luminosity.
    dimension: [i8; 7],
    symbol: &'static str,
    aliases: &'static [&'static str],
}

const STANDARD: [Standard; 7] = [
    Standard {
        kind_name: "Resistance",
        kind_lineage: "a1c37e5490d24b6f8e12c9037bd6f418",
        field_name: "Resistance",
        field_lineage: RESISTANCE_LINEAGE,
        dimension: [2, 1, -3, -2, 0, 0, 0],
        symbol: "Ω",
        aliases: &["ohm", "R"],
    },
    Standard {
        kind_name: "Capacitance",
        kind_lineage: "d4802f6be91a47c3a57d30c8ee16b295",
        field_name: "Capacitance",
        field_lineage: CAPACITANCE_LINEAGE,
        dimension: [-2, -1, 4, 2, 0, 0, 0],
        symbol: "F",
        aliases: &[],
    },
    Standard {
        kind_name: "Inductance",
        kind_lineage: "38f6b1d07ca54e92b6c1a2e4907d3f5b",
        field_name: "Inductance",
        field_lineage: INDUCTANCE_LINEAGE,
        dimension: [2, 1, -2, -2, 0, 0, 0],
        symbol: "H",
        aliases: &[],
    },
    Standard {
        kind_name: "Voltage",
        kind_lineage: VOLTAGE_KIND_LINEAGE,
        field_name: "Rated voltage",
        field_lineage: RATED_VOLTAGE_LINEAGE,
        dimension: [2, 1, -3, -1, 0, 0, 0],
        symbol: "V",
        aliases: &[],
    },
    Standard {
        kind_name: "Current",
        kind_lineage: "4c7ea92816d34fb09a65d8c3f1072eb6",
        field_name: "Rated current",
        field_lineage: RATED_CURRENT_LINEAGE,
        dimension: [0, 0, 0, 1, 0, 0, 0],
        symbol: "A",
        aliases: &[],
    },
    Standard {
        kind_name: "Power",
        kind_lineage: "b63a10f9d7e2458c8c21f4e7906ad3b1",
        field_name: "Rated power",
        field_lineage: "75d1e3c08fa2496ba2e83b6d4f09c17a",
        dimension: [2, 1, -3, 0, 0, 0, 0],
        symbol: "W",
        aliases: &[],
    },
    Standard {
        kind_name: "Frequency",
        kind_lineage: "2e98c4b7013d4fa6b7d05a96e1c38f42",
        field_name: "Frequency",
        field_lineage: "ad406f7b39e1425c91f7c2d85e06b3a9",
        dimension: [0, 0, -1, 0, 0, 0, 0],
        symbol: "Hz",
        aliases: &[],
    },
];

const TOLERANCE_LINEAGE: &str = "58c3f7e16a0d4b929e47b1d30ca2f685";
const VOLTAGE_KIND_LINEAGE: &str = "92d5c04e7ab1486d8f39e6b10c47da23";
const FORWARD_VOLTAGE_LINEAGE: &str = "0eeb669094bf434ab801ff44513db708";
const COLOUR_LINEAGE: &str = "b2387bbf96b94b0495552e4d79c1645a";
const RESISTANCE_LINEAGE: &str = "6be14d0ac2f7493d9b05e8a13c72d690";
const CAPACITANCE_LINEAGE: &str = "17ac95e3f06b4d28b3e4d172a9c05f86";
const INDUCTANCE_LINEAGE: &str = "c02e7a941db8436fa86f15d3b07e92c4";
const RATED_VOLTAGE_LINEAGE: &str = "e7304bc6185f4a9dbc72f09ad1e563b8";
const RATED_CURRENT_LINEAGE: &str = "0fb86d21e5c74a3899d4c7e06a13f25d";

/// The colours the `Colour` field offers: the emitted colours of indicator
/// LEDs. Choice IDs count from 1 and are never reused.
pub const LED_COLOURS: [&str; 8] = [
    "Red",
    "Orange",
    "Amber",
    "Yellow",
    "Yellow-green",
    "Green",
    "Blue",
    "White",
];

/// The built-in source's identity, as origins of imported copies record it.
pub fn library_id() -> Option<LibraryId> {
    LibraryId::parse(LIBRARY)
}

fn parsed(text: &str) -> Result<SourceLineageId> {
    SourceLineageId::parse(text)
        .ok_or_else(|| Error::Invalid(format!("the built-in lineage {text} is malformed")))
}

fn meta(name: &str, description: &str) -> DefinitionMeta {
    DefinitionMeta {
        name: name.to_owned(),
        description: description.to_owned(),
        provenance: "Built into Pcbcraft.".to_owned(),
    }
}

fn first(lineage: SourceLineageId) -> SourceRevision {
    SourceRevision {
        lineage,
        revision: Revision::FIRST,
    }
}

/// The selections a chip component starts with, all shown on the symbol
/// (so the instance card can set them), from the project's imported copies
/// of the standard fields: the passive's value, rating and tolerance, or an
/// LED's `Colour` and `Forward voltage`. `NotFound` when they were not
/// imported.
pub fn chip_fields(library: &Library, kind: ChipKind) -> Result<Vec<FieldSelection>> {
    let source = library_id()
        .ok_or_else(|| Error::Invalid("the built-in library identity is malformed".into()))?;
    let lineages: [&str; 3] = match kind {
        ChipKind::Resistor => [RESISTANCE_LINEAGE, RATED_VOLTAGE_LINEAGE, TOLERANCE_LINEAGE],
        ChipKind::Capacitor => [
            CAPACITANCE_LINEAGE,
            RATED_VOLTAGE_LINEAGE,
            TOLERANCE_LINEAGE,
        ],
        ChipKind::Inductor => [INDUCTANCE_LINEAGE, RATED_CURRENT_LINEAGE, TOLERANCE_LINEAGE],
        ChipKind::Led => [COLOUR_LINEAGE, FORWARD_VOLTAGE_LINEAGE, ""],
    };
    lineages
        .into_iter()
        .filter(|lineage| !lineage.is_empty())
        .map(|lineage| {
            let lineage = parsed(lineage)?;
            let local = library
                .origins
                .iter()
                .find(|o| o.origin.library == source && o.origin.lineage == lineage)
                .map(|o| o.local)
                .filter(|local| local.kind == DefinitionKind::Field)
                .ok_or_else(|| Error::NotFound("a standard field is not imported".into()))?;
            Ok(FieldSelection {
                field: FieldRef::new(local.id, local.revision),
                default: None,
                show_on_symbol: true,
            })
        })
        .collect()
}

/// The bundle of every standard kind and field: kinds first, then fields,
/// rooted at the first field. Equal on every call.
pub fn bundle() -> Result<Bundle> {
    let unreferenced = |what: &str| Error::Invalid(format!("a standard {what} names no lineage"));
    let mut records = Vec::with_capacity(2 * STANDARD.len() + 1);
    for s in &STANDARD {
        let kind = QuantityKind {
            units: UnitSet {
                dimension: s.dimension,
                units: vec![UnitOfMeasure {
                    symbol: s.symbol.into(),
                    aliases: s.aliases.iter().map(|a| (*a).into()).collect(),
                    scale: 1.0,
                    si_prefixes: true,
                }],
            },
            default_unit: s.symbol.into(),
        };
        records.push(to_record(
            parsed(s.kind_lineage)?,
            Revision::FIRST,
            meta(s.kind_name, "Built-in SI quantity kind."),
            Definition::QuantityKind(kind),
            &mut |_| Err(unreferenced("kind")),
            &mut |_, _| Err(unreferenced("kind")),
        )?);
    }
    for s in &STANDARD {
        // A placeholder the record's links resolve to the kind's lineage.
        let kind = DefinitionId(1);
        let lineage = parsed(s.kind_lineage)?;
        let field = FieldDefinition {
            next_local: 1,
            value_type: ValueType::Quantity {
                kind: QuantityKindRef::new(kind, Revision::FIRST),
                min: Some(0.0),
                max: None,
            },
            default: None,
            required_at: None,
        };
        records.push(to_record(
            parsed(s.field_lineage)?,
            Revision::FIRST,
            meta(s.field_name, "Built-in field."),
            Definition::Field(field),
            &mut |_| Ok(first(lineage)),
            &mut |_, _| Ok(lineage),
        )?);
    }
    records.push(to_record(
        parsed(TOLERANCE_LINEAGE)?,
        Revision::FIRST,
        meta(
            "Tolerance",
            "Built-in field: a fraction of the nominal value.",
        ),
        Definition::Field(FieldDefinition {
            next_local: 1,
            value_type: ValueType::Tolerance {
                basis: ToleranceBasis::Relative,
            },
            default: None,
            required_at: None,
        }),
        &mut |_| Err(unreferenced("tolerance")),
        &mut |_, _| Err(unreferenced("tolerance")),
    )?);
    let voltage = parsed(VOLTAGE_KIND_LINEAGE)?;
    records.push(to_record(
        parsed(FORWARD_VOLTAGE_LINEAGE)?,
        Revision::FIRST,
        meta(
            "Forward voltage",
            "Built-in field: an LED's or diode's forward voltage.",
        ),
        Definition::Field(FieldDefinition {
            next_local: 1,
            value_type: ValueType::Quantity {
                // A placeholder the record's links resolve to the kind's lineage.
                kind: QuantityKindRef::new(DefinitionId(1), Revision::FIRST),
                min: Some(0.0),
                max: None,
            },
            default: None,
            required_at: None,
        }),
        &mut |_| Ok(first(voltage)),
        &mut |_, _| Ok(voltage),
    )?);
    let choices: Vec<Choice> = (1u32..)
        .zip(LED_COLOURS)
        .map(|(id, label)| Choice {
            id: ChoiceId(id),
            label: label.to_owned(),
        })
        .collect();
    let next_local = u32::try_from(choices.len() + 1)
        .map_err(|_| Error::Invalid("too many built-in colours".into()))?;
    records.push(to_record(
        parsed(COLOUR_LINEAGE)?,
        Revision::FIRST,
        meta("Colour", "Built-in field: the colour an LED emits."),
        Definition::Field(FieldDefinition {
            next_local,
            value_type: ValueType::Enum { choices },
            default: None,
            required_at: None,
        }),
        &mut |_| Err(unreferenced("colour")),
        &mut |_, _| Err(unreferenced("colour")),
    )?);
    let library = library_id()
        .ok_or_else(|| Error::Invalid("the built-in library identity is malformed".into()))?;
    let root = first(parsed(
        STANDARD
            .first()
            .map(|s| s.field_lineage)
            .unwrap_or(TOLERANCE_LINEAGE),
    )?);
    let bundle = Bundle {
        library,
        name: NAME.to_owned(),
        root,
        records,
        assets: Vec::new(),
    };
    bundle::validate_bundle(&bundle)?;
    Ok(bundle)
}
