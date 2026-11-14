//! The standards and conventions Sparksmith follows, as data from the
//! shipped library (`docs/shipped-library.md`): the standard fields, the
//! value marking codes, the generic-part search grammar and the schematic
//! symbol drawings. The core holds none of them in code and reads no file:
//! `sparksmith-io` loads them into a [`Context`], the host's explicit data
//! for an engine, which the host hands to each engine it makes
//! ([`crate::Engine::set_context`]). Nothing is installed for the process:
//! two engines in one process may carry different standards.
//!
//! While an engine applies an operation or answers a query, its context is
//! the current one on that thread ([`Context::enter`]); everything that
//! needs a standard asks [`get`], which refuses when no context is current
//! rather than falling back to a value written here. A host that draws or
//! builds outside the engine (a symbol preview, a 3D build on a worker
//! thread) enters the context it holds first.
use super::marking::codes::MarkingCodes;
use super::standard_fields::StandardFields;
use super::symbol_style::SymbolDrawings;
use crate::diagnostics::{Error, Result};
use crate::materials::MaterialLibrary;
use crate::quantity::{self, Decimal, UnitOfMeasure, UnitSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::sync::Arc;

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

/// The host's data an engine works with: the standards and, for 3D, the
/// material library. Explicit: the host builds one from the shipped
/// library and hands it to each engine and session; nothing in the process
/// holds one for everybody.
#[derive(Debug)]
pub struct Context {
    standards: Arc<Standards>,
    /// The material library, or why loading it failed: a host without 3D
    /// still runs every request that does not build a model. Built models
    /// keep references into it, so it lives as long as the program: a host
    /// makes one context at startup, not one per request.
    materials: std::result::Result<&'static MaterialLibrary, String>,
}

thread_local! {
    /// The contexts entered on this thread, innermost last.
    static CURRENT: RefCell<Vec<Arc<Context>>> = const { RefCell::new(Vec::new()) };
}

/// A context entered on this thread ([`Context::enter`]); leaving the
/// scope restores the one before.
#[must_use = "the context is current only while the scope lives"]
pub struct Scope {
    /// Not `Send`: a scope belongs to the thread that entered it.
    _thread: std::marker::PhantomData<*const ()>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            current.borrow_mut().pop();
        });
    }
}

impl Context {
    /// A context with `standards` and no material library yet.
    pub fn new(standards: Standards) -> Self {
        Self {
            standards: Arc::new(standards),
            materials: Err(
                "the material library was not loaded: the program loads the shipped library's materials/ folder before it builds a 3D model"
                    .into(),
            ),
        }
    }

    /// This context with the material library, or why it failed to load.
    /// The library is kept for the rest of the program (see `materials`).
    #[must_use]
    pub fn with_materials(
        mut self,
        materials: std::result::Result<MaterialLibrary, String>,
    ) -> Self {
        self.materials = materials.map(|library| &*Box::leak(Box::new(library)));
        self
    }

    pub fn standards(&self) -> &Arc<Standards> {
        &self.standards
    }

    /// The material library, or why it is not there.
    pub fn materials(&self) -> Result<&'static MaterialLibrary> {
        self.materials
            .as_ref()
            .copied()
            .map_err(|why| Error::Invalid(why.clone()))
    }

    /// Make this context current on this thread until the scope ends.
    pub fn enter(self: &Arc<Self>) -> Scope {
        CURRENT.with(|current| current.borrow_mut().push(Arc::clone(self)));
        Scope {
            _thread: std::marker::PhantomData,
        }
    }

    /// The context current on this thread: what a worker thread enters to
    /// go on with its parent's.
    pub fn current() -> Result<Arc<Self>> {
        CURRENT
            .with(|current| current.borrow().last().cloned())
            .ok_or_else(|| {
                Error::NotFound(
                    "no context: the program hands its engine the shipped standards and materials before it uses them"
                        .into(),
                )
            })
    }

    /// The repository's shipped standards and materials as test builds
    /// compile them in, made once per test process: a fixture, never in
    /// the product.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn fixture() -> Result<Arc<Self>> {
        static FIXTURE: std::sync::OnceLock<std::result::Result<Arc<Context>, String>> =
            std::sync::OnceLock::new();
        FIXTURE
            .get_or_init(|| {
                let materials = fixture::materials().map_err(|e| e.to_string());
                fixture::shipped()
                    .map(|standards| Arc::new(Context::new(standards).with_materials(materials)))
                    .map_err(|e| e.to_string())
            })
            .clone()
            .map_err(Error::Invalid)
    }
}

/// `work` with this thread's current context entered, for another thread
/// to run: a worker reads the same standards and materials as the thread
/// that started it.
pub fn carried<T>(work: impl FnOnce() -> T + Send) -> impl FnOnce() -> T + Send {
    let context = Context::current().ok();
    move || {
        let _context = context.as_ref().map(|c| c.enter());
        work()
    }
}

/// The standards of the current context ([`Context::current`]).
pub fn get() -> Result<Arc<Standards>> {
    Context::current().map(|context| Arc::clone(&context.standards))
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
    /// The repository's shipped material library.
    pub fn materials() -> Result<MaterialLibrary> {
        MaterialLibrary::load(|name| match name {
            "package-materials.json" => Ok(shipped!("materials/package-materials.json").into()),
            "board-looks.json" => Ok(shipped!("materials/board-looks.json").into()),
            "finishes.json" => Ok(shipped!("materials/finishes.json").into()),
            "appearance.json" => Ok(shipped!("materials/appearance.json").into()),
            other => Err(Error::NotFound(format!(
                "library/materials/{other} is not a fixture file"
            ))),
        })
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, Query, QueryResult};

    /// A resistor the shipped grammar's own example names.
    fn example() -> Result<String> {
        Context::fixture()?
            .standards()
            .generic
            .families
            .iter()
            .find(|family| family.family == GenericFamily::Resistor)
            .and_then(|family| family.examples.first().cloned())
            .ok_or_else(|| Error::NotFound("a resistor example".into()))
    }

    fn resistors(engine: &Engine) -> Result<usize> {
        match engine.query(Query::GenericParts { query: example()? })? {
            QueryResult::GenericParts(report) => Ok(report.candidates.len()),
            _ => Err(Error::Invalid("not a generic-parts report".into())),
        }
    }

    /// Two engines in one process, handed different standards, answer the
    /// same query each by its own.
    #[test]
    fn two_engines_with_different_standards_answer_differently() -> Result<()> {
        let shipped = Context::fixture()?;
        let mut fewer = (**shipped.standards()).clone();
        fewer
            .generic
            .families
            .retain(|family| family.family != GenericFamily::Resistor);
        let fewer = Arc::new(Context::new(fewer));
        let mut a = Engine::default();
        a.set_context(Arc::clone(&shipped));
        let mut b = Engine::default();
        b.set_context(fewer);
        assert!(resistors(&a)? > 0);
        assert_eq!(resistors(&b)?, 0);
        // And again, so neither changed what the other reads.
        assert!(resistors(&a)? > 0);
        Ok(())
    }

    /// Nothing is installed for the process: outside an engine's call and
    /// with no context entered, a standard is refused, not made up.
    #[test]
    fn without_a_context_a_standard_is_refused() -> Result<()> {
        assert!(get().is_err());
        let context = Context::fixture()?;
        {
            let _scope = context.enter();
            assert!(get().is_ok());
        }
        assert!(get().is_err());
        Ok(())
    }

    #[test]
    fn a_worker_carries_its_parents_context() -> Result<()> {
        let context = Context::fixture()?;
        let _scope = context.enter();
        let worker = std::thread::spawn(carried(|| get().is_ok()));
        assert_eq!(worker.join().ok(), Some(true));
        let bare = std::thread::spawn(|| get().is_ok());
        assert_eq!(bare.join().ok(), Some(false));
        Ok(())
    }
}
