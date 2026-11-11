//! ABI version 1: exports, wire types and the bounds both sides rely on.
//!
//! Everything is keyed by generation keys (pad `"1"`, terminal `"A"`, unit
//! `"A"`), never by Pcbcraft ids; the host maps keys to persistent ids.
//! Lengths are integer nanometres (`_nm`, or a bare [`Point`]), angles
//! integer millidegrees (`_mdeg`). No floats cross the boundary.
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// The ABI version this module describes.
pub const ABI: u32 = 1;

/// The module's exports. A module exports exactly these functions plus its
/// linear memory as `memory`, and imports nothing.
pub mod exports {
    /// `() -> u32`: returns [`super::ABI`].
    pub const ABI: &str = "pcbcraft_gen_abi";
    /// `(len: u32) -> u32`: a buffer of `len` bytes for the request, or 0
    /// when `len` exceeds [`super::limits::REQUEST_BYTES`].
    pub const ALLOC: &str = "pcbcraft_gen_alloc";
    /// `() -> u64`: `(ptr << 32) | len` of the canonical [`super::Description`].
    pub const DESCRIBE: &str = "pcbcraft_gen_describe";
    /// `(ptr: u32, len: u32) -> u64`: takes the canonical [`super::Request`]
    /// in the buffer from `ALLOC` and returns `(ptr << 32) | len` of the
    /// response: one status byte ([`super::STATUS_OK`] or
    /// [`super::STATUS_ERR`]) then the canonical [`super::Generation`] or
    /// [`super::ErrorBody`].
    pub const GENERATE: &str = "pcbcraft_gen_generate";
    /// The exported linear memory.
    pub const MEMORY: &str = "memory";
}

/// Response status byte: a [`Generation`] follows.
pub const STATUS_OK: u8 = 0;
/// Response status byte: an [`ErrorBody`] follows.
pub const STATUS_ERR: u8 = 1;

/// Bounds of the wire data that both sides enforce. Execution bounds (fuel,
/// memory, time) belong to the host; see `pcbcraft_generators::Limits`.
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

/// What a generator offers: read by the host, UI, MCP and the AI without
/// running anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Description {
    /// Always [`ABI`].
    pub abi: u32,
    /// A short stable name for the generator lineage, e.g. `chip`.
    pub id_hint: String,
    /// A display name.
    pub name: String,
    /// Package families it covers.
    pub families: Vec<String>,
    /// The scopes it can produce.
    pub scopes: Vec<Scope>,
    pub params: Vec<Param>,
    /// Cross-field rules, stated in plain words and enforced by the module.
    pub rules: Vec<Rule>,
    pub presets: Vec<Preset>,
    pub grammar: Vec<Grammar>,
    /// Standard fields an instance of the part carries (outside the part key).
    pub instance_fields: Vec<InstanceField>,
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
/// (enum keys, free text, decimal quantities) or a list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Int(i64),
    Bool(bool),
    Text(String),
    List(Vec<ParamValue>),
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

/// A generate request. `params` are complete: the host fills every default
/// from the [`Description`] before calling and before hashing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Always [`ABI`].
    pub abi: u32,
    pub scope: Scope,
    pub params: Params,
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
    /// The 3D model description. Not produced in ABI 1 until the 3D script
    /// lands (slice GP2); the host builds no model when it is absent.
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

/// A terminal's electrical direction (Pcbcraft's pin function vocabulary).
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
    /// Counter-clockwise from `start_mdeg` by `sweep_mdeg` (negative: clockwise).
    Arc {
        center: Point,
        radius_nm: i64,
        start_mdeg: i64,
        sweep_mdeg: i64,
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
    /// A `.solidcraft` part file shipped with the generator revision, by
    /// SHA-256, with its parameter values.
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
