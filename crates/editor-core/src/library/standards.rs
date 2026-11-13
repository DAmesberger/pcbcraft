//! The standards and conventions Sparksmith follows, as data from the
//! shipped library (`docs/shipped-library.md`): the standard fields, the
//! value marking codes, the generic-part search grammar and the schematic
//! symbol drawings. The core holds none of them in code and reads no file:
//! `sparksmith-io` loads them and [`install`]s one [`Standards`] value for
//! the process at startup, before any engine runs. Everything that needs a
//! standard asks [`get`], which refuses when nothing was installed rather
//! than falling back to a value written here.
//!
//! The shipped library does not change while the program runs, so the
//! value is installed once; installing an equal value again is a no-op.
use super::marking::codes::MarkingCodes;
use super::standard_fields::StandardFields;
use super::symbol_style::SymbolDrawings;
use crate::diagnostics::{Error, Result};
use crate::quantity::{self, Decimal, UnitOfMeasure, UnitSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// The data-file format every standards file states.
pub const FORMAT: u32 = 1;

/// Refuse a file of another format version, naming what it is.
pub fn check_format(format: u32, what: &str) -> Result<()> {
    if format == FORMAT {
        Ok(())
    } else {
        Err(Error::Unsupported(format!(
            "{what} is format {format}; this build reads format {FORMAT}"
        )))
    }
}

/// Every standard the core uses, checked against each other.
#[derive(Clone, Debug, PartialEq)]
pub struct Standards {
    pub fields: StandardFields,
    pub marking: MarkingCodes,
    pub generic: GenericGrammar,
    pub symbols: SymbolDrawings,
}

impl Standards {
    /// The standards, after checking that the grammar's value slots and
    /// colour list name standard fields that have units and choices.
    pub fn new(
        fields: StandardFields,
        marking: MarkingCodes,
        generic: GenericGrammar,
        symbols: SymbolDrawings,
    ) -> Result<Self> {
        for slot in &generic.value_slots {
            if fields.units(&slot.field).is_none() {
                return Err(Error::Invalid(format!(
                    "the generic-part grammar's {} slot names {}, which is not a standard quantity field",
                    slot.slot, slot.field
                )));
            }
        }
        if fields.choices(&generic.colour_field).is_none() {
            return Err(Error::Invalid(format!(
                "the generic-part grammar's colour field {} is not a standard field of choices",
                generic.colour_field
            )));
        }
        Ok(Self {
            fields,
            marking,
            generic,
            symbols,
        })
    }
}

static INSTALLED: OnceLock<Standards> = OnceLock::new();

/// Install the process's standards. A second install of an equal value is
/// a no-op; a different one is refused, since engines already running
/// would otherwise see two sets.
pub fn install(standards: Standards) -> Result<()> {
    let installed = INSTALLED.get_or_init(|| standards.clone());
    if *installed == standards {
        Ok(())
    } else {
        Err(Error::Conflict(
            "other standards were already loaded in this process".into(),
        ))
    }
}

/// The installed standards. Test builds load the repository's shipped
/// library on first use; the product installs them at startup
/// (`sparksmith_io::standards`).
pub fn get() -> Result<&'static Standards> {
    #[cfg(any(test, feature = "test-fixtures"))]
    if INSTALLED.get().is_none() {
        install(fixture::shipped()?)?;
    }
    INSTALLED.get().ok_or_else(|| {
        Error::NotFound(
            "the shipped standards were not loaded: the program must load its shipped library at startup"
                .into(),
        )
    })
}

/// The families that build on demand.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GenericFamily {
    Header,
    Resistor,
    Capacitor,
    Inductor,
    Led,
    Diode,
}

/// One family's declared patterns, and example texts each building a part.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FamilyPatterns {
    pub family: GenericFamily,
    pub patterns: Vec<String>,
    pub examples: Vec<String>,
}

/// A value slot type (`{value:ohm}`): the standard field it fills, and the
/// suffixes it reads without a unit, so "10k" is a resistance and "100n" a
/// capacitance (`""` is a plain number).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ValueSlot {
    pub slot: String,
    pub field: String,
    pub bare: Vec<String>,
}

/// A unit a length slot reads, with its scale to metres.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LengthUnit {
    pub symbol: String,
    pub scale: f64,
}

/// The search grammar of generic parts built on demand
/// (`catalog::generic`): its vocabulary and defaults. The matcher that
/// reads it is code.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GenericGrammar {
    pub format: u32,
    #[serde(default)]
    pub about: String,
    pub families: Vec<FamilyPatterns>,
    pub value_slots: Vec<ValueSlot>,
    /// The standard field whose choices a `{colour:list}` slot reads.
    pub colour_field: String,
    pub length_units: Vec<LengthUnit>,
    /// A header's pitch when the text gives none, as an exact decimal.
    pub header_pitch_mm: String,
}

/// Families, patterns per family, value slots and bare suffixes a grammar
/// may declare.
const MAX_GRAMMAR_ITEMS: usize = 32;

impl GenericGrammar {
    /// Bounds, unique slot names, a parsable default pitch and a valid set
    /// of length units. The patterns themselves are checked by the matcher
    /// (`catalog::generic::check`).
    pub fn check(&self) -> Result<()> {
        check_format(self.format, "the generic-part grammar")?;
        let too_many = |what: &str| {
            Error::Limit(format!(
                "the generic-part grammar lists more than {MAX_GRAMMAR_ITEMS} {what}"
            ))
        };
        if self.families.len() > MAX_GRAMMAR_ITEMS {
            return Err(too_many("families"));
        }
        for family in &self.families {
            if family.patterns.is_empty() || family.patterns.len() > MAX_GRAMMAR_ITEMS {
                return Err(Error::Invalid(format!(
                    "each generic family needs 1–{MAX_GRAMMAR_ITEMS} patterns"
                )));
            }
            if family.examples.len() > MAX_GRAMMAR_ITEMS {
                return Err(too_many("examples of a family"));
            }
        }
        if self.value_slots.len() > MAX_GRAMMAR_ITEMS
            || self
                .value_slots
                .iter()
                .any(|s| s.bare.len() > MAX_GRAMMAR_ITEMS)
        {
            return Err(too_many("value slots or bare suffixes"));
        }
        let mut names: Vec<&str> = self.value_slots.iter().map(|s| s.slot.as_str()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        if names.len() != count
            || names
                .iter()
                .any(|n| matches!(*n, "" | "length" | "code" | "list"))
        {
            return Err(Error::Invalid(
                "the generic-part grammar's value slots need unique names other than length, code and list".into(),
            ));
        }
        let pitch: Decimal = self.header_pitch_mm.parse().map_err(|_| {
            Error::Invalid(format!(
                "the generic-part grammar's header pitch {} is not a number",
                self.header_pitch_mm
            ))
        })?;
        if pitch.is_zero() || pitch.is_negative() {
            return Err(Error::Invalid(
                "the generic-part grammar's header pitch must be positive".into(),
            ));
        }
        self.length_unit_set().check()
    }

    /// The units a length slot reads, the first being the unit a bare
    /// number is in.
    pub fn length_unit_set(&self) -> UnitSet {
        UnitSet {
            dimension: quantity::LENGTH,
            units: self
                .length_units
                .iter()
                .map(|u| UnitOfMeasure {
                    symbol: u.symbol.clone(),
                    aliases: Vec::new(),
                    scale: u.scale,
                    si_prefixes: false,
                })
                .collect(),
        }
    }
}

/// The repository's shipped library, compiled into test builds only.
#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixture {
    use super::*;
    use crate::library::marking::codes::{
        ColourCode, Eia96Code, PreferredSeries, VoltageLetterTables,
    };
    use crate::library::standard_fields::FieldRoles;
    use crate::library::symbol_style::{KindDrawing, SymbolKind, SymbolStyle};

    macro_rules! shipped {
        ($path:literal) => {
            include_str!(concat!("../../../../library/", $path))
        };
    }

    fn parse<T: serde::de::DeserializeOwned>(name: &str, text: &str) -> Result<T> {
        serde_json::from_str(text).map_err(|e| Error::Invalid(format!("library/{name}: {e}")))
    }

    /// The symbol drawing file of each drawn kind (the box kinds are laid
    /// out from their pins).
    const SYMBOLS: [(SymbolKind, &str); 20] = [
        (
            SymbolKind::Resistor,
            shipped!("standards/symbols/resistor.json"),
        ),
        (
            SymbolKind::Capacitor,
            shipped!("standards/symbols/capacitor.json"),
        ),
        (
            SymbolKind::PolarisedCapacitor,
            shipped!("standards/symbols/polarised_capacitor.json"),
        ),
        (
            SymbolKind::Inductor,
            shipped!("standards/symbols/inductor.json"),
        ),
        (
            SymbolKind::FerriteBead,
            shipped!("standards/symbols/ferrite_bead.json"),
        ),
        (SymbolKind::Diode, shipped!("standards/symbols/diode.json")),
        (SymbolKind::Zener, shipped!("standards/symbols/zener.json")),
        (
            SymbolKind::Schottky,
            shipped!("standards/symbols/schottky.json"),
        ),
        (SymbolKind::Led, shipped!("standards/symbols/led.json")),
        (
            SymbolKind::Crystal,
            shipped!("standards/symbols/crystal.json"),
        ),
        (SymbolKind::Fuse, shipped!("standards/symbols/fuse.json")),
        (
            SymbolKind::Switch,
            shipped!("standards/symbols/switch.json"),
        ),
        (
            SymbolKind::Pushbutton,
            shipped!("standards/symbols/pushbutton.json"),
        ),
        (
            SymbolKind::TestPoint,
            shipped!("standards/symbols/test_point.json"),
        ),
        (
            SymbolKind::NpnBjt,
            shipped!("standards/symbols/npn_bjt.json"),
        ),
        (
            SymbolKind::PnpBjt,
            shipped!("standards/symbols/pnp_bjt.json"),
        ),
        (
            SymbolKind::NMosfet,
            shipped!("standards/symbols/n_mosfet.json"),
        ),
        (
            SymbolKind::PMosfet,
            shipped!("standards/symbols/p_mosfet.json"),
        ),
        (
            SymbolKind::Ground,
            shipped!("standards/symbols/ground.json"),
        ),
        (SymbolKind::Power, shipped!("standards/symbols/power.json")),
    ];

    /// The standards of the repository's `library/`, parsed as
    /// `sparksmith-io` parses the installed folder.
    pub fn shipped() -> Result<Standards> {
        let fields = StandardFields::new(
            parse(
                "standard-fields/standard-fields.json",
                shipped!("standard-fields/standard-fields.json"),
            )?,
            parse::<FieldRoles>(
                "standard-fields/roles.json",
                shipped!("standard-fields/roles.json"),
            )?,
        )?;
        let marking = MarkingCodes::new(
            parse::<PreferredSeries>("standards/e96.json", shipped!("standards/e96.json"))?,
            parse::<Eia96Code>("standards/eia-96.json", shipped!("standards/eia-96.json"))?,
            parse::<VoltageLetterTables>(
                "standards/voltage-letters.json",
                shipped!("standards/voltage-letters.json"),
            )?,
            parse::<ColourCode>(
                "standards/colour-code.json",
                shipped!("standards/colour-code.json"),
            )?,
        )?;
        let generic: GenericGrammar = parse(
            "catalog/generic-families.json",
            shipped!("catalog/generic-families.json"),
        )?;
        generic.check()?;
        let style: SymbolStyle = parse(
            "standards/symbols/style.json",
            shipped!("standards/symbols/style.json"),
        )?;
        let kinds = SYMBOLS
            .iter()
            .map(|(kind, text)| parse::<KindDrawing>(kind.label(), text))
            .collect::<Result<Vec<_>>>()?;
        let symbols = SymbolDrawings::new(style, kinds)?;
        Standards::new(fields, marking, generic, symbols)
    }
}
