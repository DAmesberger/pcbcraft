//! The one plugin ABI, version 1: exports, imports, the description
//! format, the invoke envelope, the `generate-part` capability's wire types
//! and the bounds both sides rely on (`docs/design/extension-abi-design.md`
//! §2.6 and §3.5).
//!
//! A plugin describes itself once ([`Description`]): its id and version,
//! and its capabilities, each declaring its [`Effect`]. A generator is a
//! plugin with the capability [`GENERATE_PART`], of effect
//! [`Effect::None`]: a pure function whose result is hashed.
//!
//! Generation output is keyed by generation keys (pad `"1"`, terminal
//! `"A"`, unit `"A"`, graphic `"body"`), never by Sparksmith ids; the host
//! maps keys to persistent ids. Lengths are integer nanometres (`_nm`, or a
//! bare [`Point`]), angles integer millidegrees (`_mdeg`). No floats cross
//! the boundary.
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// The ABI version this module describes.
pub const ABI: u32 = 1;

/// The module's exports: these four functions and its linear memory as
/// `memory`.
pub mod exports {
    /// `() -> u32`: returns [`super::ABI`].
    pub const ABI: &str = "sparksmith_abi";
    /// `(len: u32) -> u32`: a buffer of `len` bytes for a request (or a
    /// reply the host hands back from `call`), or 0 when `len` is 0 or
    /// exceeds [`super::limits::REQUEST_BYTES`].
    pub const ALLOC: &str = "sparksmith_alloc";
    /// `() -> u64`: `(ptr << 32) | len` of the canonical [`super::Description`].
    pub const DESCRIBE: &str = "sparksmith_describe";
    /// `(ptr: u32, len: u32) -> u64`: takes the canonical [`super::Invoke`]
    /// in the buffer from `ALLOC` and returns `(ptr << 32) | len` of the
    /// response: one status byte ([`super::STATUS_OK`] or
    /// [`super::STATUS_ERR`]) then the capability's result (a
    /// [`super::Generation`] for [`super::GENERATE_PART`]) or an
    /// [`super::ErrorBody`].
    pub const INVOKE: &str = "sparksmith_invoke";
    /// The exported linear memory.
    pub const MEMORY: &str = "memory";
}

/// The host functions a module may import, all from module
/// [`imports::MODULE`] and all optional. Anything else is refused before
/// the module runs, and a plugin whose capabilities are all of effect
/// [`Effect::None`] may import nothing at all.
pub mod imports {
    /// The import module name.
    pub const MODULE: &str = "sparksmith";
    /// `(ptr: u32, len: u32) -> u64`: one envelope `{entry, params}` in;
    /// `(ptr << 32) | len` of a status byte and the reply or error out, in a
    /// buffer the host took from the module's `sparksmith_alloc`. During an
    /// invoke of effect [`super::Effect::None`] it is refused, always, with
    /// [`super::REFUSED_PURE`].
    pub const CALL: &str = "call";
    /// `(level: u32, ptr: u32, len: u32)`: a line for the run's log. Dropped
    /// in an invoke of effect [`super::Effect::None`].
    pub const LOG: &str = "log";
    /// `() -> u32`: 1 when the run should stop. Always 0 in an invoke of
    /// effect [`super::Effect::None`].
    pub const CANCELLED: &str = "cancelled";
    /// The three, in order.
    pub const ALL: [&str; 3] = [CALL, LOG, CANCELLED];
}

/// The capability that makes a part: effect [`Effect::None`], parameters
/// in, a [`Generation`] out, hashed.
pub const GENERATE_PART: &str = "generate-part";

/// The error code `call` answers in an invoke of effect [`Effect::None`].
pub const REFUSED_PURE: &str = "plugin.pure";

/// What `call` answers in an invoke of effect [`Effect::None`], always:
/// [`STATUS_ERR`] and this body, canonical. The same bytes on every host
/// and in the SDK's native build, so a module that calls during a pure
/// invoke still gives one result.
pub fn pure_refusal() -> ErrorBody {
    ErrorBody {
        errors: alloc::vec![Problem {
            code: REFUSED_PURE.into(),
            field: None,
            message: "A capability of effect none calls nothing: its result depends on its request alone.".into(),
        }],
    }
}

/// Response status byte: a [`Generation`] follows.
pub const STATUS_OK: u8 = 0;
/// Response status byte: an [`ErrorBody`] follows.
pub const STATUS_ERR: u8 = 1;

/// Bounds of the wire data that both sides enforce. Execution bounds (fuel,
/// memory, time) belong to the host; see `sparksmith_generators::Limits`.
pub mod limits {
    // Set from GP0's measurements (generators/README.md): the chip
    // generator's requests are at most 258 bytes, its description 14 KiB
    // (24 presets) and its responses at most 2.8 KiB; a 1 024-pad grid
    // array is projected at about 0.3 MiB of response.
    /// Largest request a module accepts (lists of pin names included).
    pub const REQUEST_BYTES: usize = 64 * 1024;
    /// Largest description a host reads.
    pub const DESCRIPTION_BYTES: usize = 256 * 1024;
    /// Largest response (status byte included) a host reads.
    pub const RESPONSE_BYTES: usize = 1024 * 1024;
    /// Most characters in any one text value.
    pub const TEXT_CHARS: usize = 1024;
    /// Most pads in a footprint.
    pub const PADS: usize = 1024;
    /// Most terminals in a component or package.
    pub const TERMINALS: usize = 1024;
    /// Most graphics in one symbol unit or one footprint layer list.
    pub const GRAPHICS: usize = 4096;
    /// Most points in one polyline or polygon.
    pub const POINTS: usize = 1024;
    /// Most symbol units.
    pub const UNITS: usize = 64;
    /// Largest absolute coordinate or size, 1 m.
    pub const COORDINATE_NM: i64 = 1_000_000_000;
    /// Most values a model gives its part file.
    pub const MODEL_VALUES: usize = 256;
    /// Most an arc's start and end may differ in their distance from its
    /// centre: each is rounded to the nanometre once.
    pub const ARC_RADIUS_NM: i64 = 1;
}

/// A point in nm.
pub type Point = [i64; 2];
/// A point in nm with height.
pub type Point3 = [i64; 3];

/// What a request asks a generator to produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// A complete part: component, symbol, package, footprint and binding.
    Component,
    /// Package and footprint only; an authored component binds to it.
    Package,
}

// ---------------------------------------------------------------- describe

/// What a plugin offers, in the one description format: read by the host,
/// UI, MCP and the AI without running anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Description {
    /// Always [`ABI`].
    pub abi: u32,
    /// The plugin's stable id, e.g. `chip`: with the parameters, what
    /// identifies a part it makes.
    pub id: String,
    /// The plugin's version within its `id`: a whole number from 1, raised
    /// whenever a release changes what it makes. A higher version of the
    /// same `id` is newer.
    pub version: u32,
    /// A display name.
    pub title: String,
    /// The plugin's licence (SPDX).
    pub licence: String,
    /// Whether its results are hashed: required, and then checked by the
    /// host (no float instructions), when any capability has effect
    /// [`Effect::None`].
    pub deterministic: bool,
    pub capabilities: Vec<Capability>,
    /// Catalog modules the plugin's `read` and `edit` capabilities call;
    /// empty for a plugin whose capabilities are all of effect
    /// [`Effect::None`].
    pub needs: Vec<Need>,
}

impl Description {
    /// The [`GENERATE_PART`] capability, if the plugin makes parts.
    pub fn part(&self) -> Option<&Capability> {
        self.capabilities.iter().find(|c| c.name == GENERATE_PART)
    }

    /// The part capability's parameters (none when it makes no parts).
    pub fn params(&self) -> &[Param] {
        self.part().map_or(&[], |c| c.params.as_slice())
    }

    /// The part capability's presets.
    pub fn presets(&self) -> &[Preset] {
        self.part().map_or(&[], |c| c.presets.as_slice())
    }

    /// The part capability's rules.
    pub fn rules(&self) -> &[Rule] {
        self.part().map_or(&[], |c| c.rules.as_slice())
    }

    /// The scopes the part capability makes.
    pub fn scopes(&self) -> &[Scope] {
        self.part()
            .and_then(|c| c.part.as_ref())
            .map_or(&[], |p| p.scopes.as_slice())
    }

    /// The part capability's own facts (families, scopes, grammar).
    pub fn offer(&self) -> Option<&PartOffer> {
        self.part().and_then(|c| c.part.as_ref())
    }

    /// Whether every capability is of effect [`Effect::None`].
    pub fn pure(&self) -> bool {
        self.capabilities.iter().all(|c| c.effect == Effect::None)
    }
}

/// What a capability may do while it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Nothing: `call` is refused, `log` dropped, `cancelled` answers 0;
    /// the result is canonical bytes, hashed.
    None,
    /// Queries on a frozen fork of the document (not served yet).
    Read,
    /// Queries and operations on a private fork (not served yet).
    Edit,
}

/// One thing the plugin can be asked to do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    /// Its name within the plugin, e.g. [`GENERATE_PART`].
    pub name: String,
    pub effect: Effect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub params: Vec<Param>,
    /// Cross-field rules, stated in plain words and enforced by the module.
    pub rules: Vec<Rule>,
    pub presets: Vec<Preset>,
    /// For [`GENERATE_PART`]: the families, scopes and search phrases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<PartOffer>,
}

/// What a part capability makes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartOffer {
    /// Package families it covers.
    pub families: Vec<String>,
    /// The scopes it can produce.
    pub scopes: Vec<Scope>,
    pub grammar: Vec<Grammar>,
    /// Standard fields an instance of the part carries (outside the part key).
    pub instance_fields: Vec<InstanceField>,
}

/// A catalog module a plugin's `read` or `edit` capabilities call: the
/// host refuses the plugin, naming both versions, when it cannot meet it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Need {
    pub module: String,
    pub major: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub minor: u32,
    /// What the plugin does with it: [`Effect::Read`] or [`Effect::Edit`].
    pub class: Effect,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// One parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(rename = "type")]
    pub ty: ParamType,
    pub default: ParamValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// A project setting the host fills this parameter from when a part is
    /// created (e.g. `project.land.chip_gap_min`), then freezes into the
    /// parameters. The module never reads project state itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// A parameter's type and range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ParamType {
    /// A length in nm.
    Length {
        min_nm: i64,
        max_nm: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_nm: Option<i64>,
    },
    /// An angle in millidegrees.
    Angle {
        min_mdeg: i64,
        max_mdeg: i64,
    },
    /// A whole number.
    Count {
        min: i64,
        max: i64,
    },
    /// One of a fixed set of keys.
    Enum {
        choices: Vec<Choice>,
    },
    Bool,
    /// Free text of at most `max_chars` characters.
    Text {
        max_chars: u32,
    },
    /// A decimal string of a standard quantity kind, e.g. `voltage` in `V`.
    Quantity {
        kind: String,
        unit: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<String>,
    },
    /// A bounded list of one type.
    List {
        of: Box<ParamType>,
        max_len: u32,
    },
}

/// One choice of an [`ParamType::Enum`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub key: String,
    pub label: String,
}

/// A parameter value: an integer (lengths, angles, counts), a boolean, text
/// (enum keys, free text, decimal quantities) or a list. Written as the
/// bare JSON value; read by its JSON kind (by hand rather than serde's
/// `untagged`, whose buffer has float paths a module must not contain).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ParamValue {
    Int(i64),
    Bool(bool),
    Text(String),
    List(Vec<ParamValue>),
}

impl<'de> Deserialize<'de> for ParamValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Kind;
        impl<'de> serde::de::Visitor<'de> for Kind {
            type Value = ParamValue;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("an integer, a boolean, text or a list")
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<ParamValue, E> {
                Ok(ParamValue::Int(v))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<ParamValue, E> {
                i64::try_from(v)
                    .map(ParamValue::Int)
                    .map_err(|_| E::custom("an integer beyond the range of a parameter"))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<ParamValue, E> {
                Ok(ParamValue::Bool(v))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ParamValue, E> {
                Ok(ParamValue::Text(String::from(v)))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<ParamValue, E> {
                Ok(ParamValue::Text(v))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<ParamValue, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(ParamValue::List(items))
            }
        }
        deserializer.deserialize_any(Kind)
    }
}

/// Complete parameters, keyed by [`Param::key`].
pub type Params = BTreeMap<String, ParamValue>;

/// A cross-field rule in plain words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub text: String,
    pub fields: Vec<String>,
}

/// A named parameter set (a starter row), with its source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub key: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub params: Params,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<Citation>,
    pub basis: Basis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Where a value comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    pub document: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
}

/// How far a value is backed by its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Cited,
    CitedUnverified,
    Estimated,
}

/// Search phrases for one scope, as data; the host owns the matcher.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grammar {
    pub scope: Scope,
    pub patterns: Vec<String>,
    pub examples: Vec<String>,
}

/// A standard field instances carry, when the parameters match `when`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceField {
    pub standard_field: String,
    pub when: Params,
}

// ---------------------------------------------------------------- generate

/// A [`GENERATE_PART`] request. `params` are complete: the host fills
/// every default from the [`Description`] before calling and before
/// hashing. Its canonical bytes are what the params hash covers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Always [`ABI`].
    pub abi: u32,
    pub scope: Scope,
    pub params: Params,
}

impl Request {
    /// The invoke envelope that asks for this part.
    pub fn invoke(&self) -> Invoke {
        Invoke {
            abi: self.abi,
            capability: GENERATE_PART.into(),
            params: self.params.clone(),
            scope: Some(self.scope),
        }
    }
}

/// What `sparksmith_invoke` reads: a capability by name, its parameters,
/// and for [`GENERATE_PART`] the scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invoke {
    /// Always [`ABI`].
    pub abi: u32,
    pub capability: String,
    pub params: Params,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
}

impl Invoke {
    /// The part request this invoke carries, if it asks for
    /// [`GENERATE_PART`] with a scope.
    pub fn request(&self) -> Option<Request> {
        (self.capability == GENERATE_PART).then_some(())?;
        Some(Request {
            abi: self.abi,
            scope: self.scope?,
            params: self.params.clone(),
        })
    }
}

/// A problem in plain words: a parameter the module refuses, or a host
/// refusal. `message` is a sentence a person can act on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Problem {
    /// A stable dotted code, e.g. `param.out_of_range` or `generator.out_of_fuel`.
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub message: String,
}

/// The body after [`STATUS_ERR`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    pub errors: Vec<Problem>,
}

/// The body after [`STATUS_OK`]: a complete part body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    /// Present in [`Scope::Component`] only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<Component>,
    /// Present in [`Scope::Component`] only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Symbol>,
    pub package: Package,
    pub footprint: Footprint,
    /// Package terminal to component terminal; empty in [`Scope::Package`].
    pub binding: Vec<BindingEntry>,
    /// The 3D model. Required: a part never exists without its model, so
    /// the host refuses a generation without one (slice GP2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    /// Derived field values, e.g. rated power from the size code.
    pub fields: Vec<FieldValue>,
    /// Non-fatal remarks for the person reviewing the part.
    pub notes: Vec<String>,
}

/// The electrical component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub terminals: Vec<Terminal>,
    /// Designator prefix, e.g. `R`.
    pub prefix: String,
    /// Suggested component name.
    pub name: String,
    /// Standard field keys instances carry, e.g. `resistance`.
    pub fields: Vec<String>,
}

/// An electrical terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub key: String,
    pub name: String,
    pub direction: Direction,
}

/// A terminal's electrical direction (Sparksmith's pin function vocabulary).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Input,
    Output,
    Bidirectional,
    Passive,
    PowerInput,
    PowerOutput,
}

/// The schematic symbol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Symbol {
    pub units: Vec<Unit>,
}

/// One symbol unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub key: String,
    pub pins: Vec<SymbolPin>,
    pub graphics: Vec<Graphic>,
}

/// A symbol pin: its connection point `at`, from which it reaches `length_nm`
/// towards the body in direction `rot_mdeg` (0 = +x, 90 000 = +y).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolPin {
    /// The component terminal key.
    pub terminal: String,
    pub at: Point,
    pub length_nm: i64,
    pub rot_mdeg: i64,
    pub show_name: bool,
    pub show_number: bool,
}

/// A drawn primitive with its stroke and fill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graphic {
    /// Its generation key: unique within its symbol unit, or within the
    /// footprint's silkscreen and assembly drawings together. The host
    /// keeps the drawing's id for the key across regenerations, so a key
    /// should name the drawing (`body`, `outline-top-1`), not its place
    /// in the list.
    pub key: String,
    pub shape: Primitive,
    pub stroke_nm: i64,
    pub fill: Fill,
}

/// Fill of a closed primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    None,
    /// The symbol body colour.
    Body,
    Solid,
}

/// Geometry in nm and millidegrees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Primitive {
    Line {
        a: Point,
        b: Point,
    },
    Rect {
        min: Point,
        max: Point,
    },
    Polyline {
        points: Vec<Point>,
    },
    Polygon {
        points: Vec<Point>,
    },
    Circle {
        center: Point,
        radius_nm: i64,
    },
    /// An arc about `center` from `start` to `end`, counter-clockwise
    /// unless `clockwise`; `start == end` is a full circle. The points are
    /// exact: the host does no trigonometry. Both lie on one circle, their
    /// distances from `center` agreeing within [`limits::ARC_RADIUS_NM`]
    /// (the module rounds each to the nanometre; the SDK's `draw::arc`
    /// computes them in integers).
    Arc {
        center: Point,
        start: Point,
        end: Point,
        clockwise: bool,
    },
    Text {
        at: Point,
        height_nm: i64,
        content: TextContent,
        h_align: HAlign,
        v_align: VAlign,
    },
}

/// What a text shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TextContent {
    /// The instance's designator, e.g. `R1`.
    Designator,
    /// The component's name.
    ComponentName,
    Literal(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    Top,
    Middle,
    Bottom,
}

/// The physical package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// Package terminal keys.
    pub terminals: Vec<String>,
    pub geometry: PackageGeometry,
}

/// The package's mechanical facts, generic over families.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageGeometry {
    /// Body length (x), width (y) and height (z), centred on the origin in
    /// x and y, resting on the seating plane.
    pub envelope_nm: Point3,
    /// Seating plane height above the board (standoff).
    pub seating_nm: i64,
    /// Where each terminal touches the board.
    pub leads: Vec<Lead>,
    /// The package terminal that is pin 1.
    pub pin1: String,
}

/// A lead's contact point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lead {
    /// The package terminal key.
    pub key: String,
    pub contact: Point3,
}

/// The land pattern.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footprint {
    pub pads: Vec<Pad>,
    /// The courtyard outline, a closed polygon.
    pub courtyard: Vec<Point>,
    pub silk: Vec<Graphic>,
    pub assembly: Vec<Graphic>,
}

/// A pad.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    /// The pad number shown, also its generation key.
    pub key: String,
    /// The package terminal it lands.
    pub terminal: String,
    pub shape: PadShape,
    pub at: Point,
    pub rot_mdeg: i64,
    pub side: Side,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hole: Option<Hole>,
    pub mask: Opening,
    pub paste: Opening,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PadShape {
    Rect { size: Point },
    RoundRect { size: Point, radius_nm: i64 },
    Circle { diameter_nm: i64 },
    Oval { size: Point },
    Polygon { points: Vec<Point> },
}

/// The copper side of a surface pad, or the side a through-hole pad is
/// placed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Top,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hole {
    pub shape: HoleShape,
    pub plated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum HoleShape {
    Round { diameter_nm: i64 },
    Slot { size: Point },
}

/// A mask or paste opening relative to the pad.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Opening {
    /// The project's rule applies.
    Default,
    /// Grown (or, negative, shrunk) by this much on every side.
    Expansion(i64),
    /// No opening.
    None,
}

/// Package terminal to component terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingEntry {
    pub package_terminal: String,
    pub terminal: String,
}

/// The 3D model description (slice GP2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Model {
    /// A `.solidcraft` part file of the generator's bundle, by the
    /// lower-case hex SHA-256 of its bytes, with values for some of its
    /// parameters: integers, nanometres for a length parameter,
    /// millidegrees for an angle, the count itself for a unitless one (the
    /// part file's parameter decides). The host evaluates the file at the
    /// values through Solidcraft, as for any part file.
    PartFile {
        asset: String,
        values: BTreeMap<String, ParamValue>,
    },
}

/// A derived field value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldValue {
    pub field: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<Citation>,
    pub basis: Basis,
}
