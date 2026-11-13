//! The document aggregate: data and lookups only. Rules over this data live
//! in `instances`, `nets`, `schematic` and `layout`.
use crate::fields::InstanceFields;
use crate::functions::FunctionSelections;
pub use crate::geometry::Point;
use crate::ids::{ComponentRef, DefinitionId, Endpoint, Id, PartRef, SheetId, TerminalId, UnitId};
use crate::library::{Drafts, Library, Rotation};
use crate::shared::Shared;
use crate::units::{Nm, PointNm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

pub mod assembly_link;
pub mod board;
pub mod dimension;
pub mod fill;
pub mod interface;
pub mod production;
pub mod sheet;
mod stackup;
mod via;
pub use board::{BoardShape, SolvedBoard};
pub use dimension::PartDimension;
pub use fill::{Fill, Keepout};
pub use sheet::{
    Label, LabelKind, Paper, PaperOrientation, PortShape, PowerStyle, ProjectProperties, Sheet,
    SheetDrawing, SheetItem, SheetPin, SheetSide, SheetSymbol, Sheets,
};
pub use stackup::{
    COPPER_UM_RANGE, CopperFoil, CopperLayer, DIELECTRIC_MM_RANGE, DK_RANGE, Dielectric,
    DielectricKind, GlassDk, GlassStep, GlassStyle, MASK_UM_RANGE, MAX_COPPER_LAYERS,
    MIN_COPPER_LAYERS, OUNCE_UM, PresetDielectric, Stackup, StackupPresets, Unsaved,
};
pub use via::{Via, ViaSpan};

/// The physical implementation an instance uses, as lineages resolved
/// through its pinned component revision.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PhysicalChoice {
    pub binding: DefinitionId,
    pub footprint: DefinitionId,
}

/// One use of a component in the project. It pins one exact component
/// revision; every other choice is a lineage that revision offers.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ComponentInstance {
    pub id: Id,
    pub component: ComponentRef,
    pub designator: String,
    /// Field overrides by field lineage; a missing field is inherited.
    pub properties: InstanceFields,
    /// A symbol lineage listed by the pinned component.
    pub representation: DefinitionId,
    pub physical: Option<PhysicalChoice>,
    /// Units deliberately left off the sheet.
    pub unused_units: BTreeSet<UnitId>,
    /// Project decisions that these terminals stay unconnected.
    pub no_connect: BTreeSet<TerminalId>,
    /// Explicit function selections by terminal. A missing terminal uses its
    /// fixed function or design default.
    pub functions: FunctionSelections,
    /// The exact manufacturer part, when one is chosen (S2). It pins the
    /// same component revision and the binding of `physical`; rules in
    /// `instances`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<PartRef>,
    /// C2: per-terminal pin configuration (pull, speed, initial level,
    /// firmware name). Never changes function ERC directly;
    /// pin rules may read it through `option(…)`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub pin_config: crate::functions::PinConfigs,
    /// D4: the instance's marking choices.
    #[serde(
        default,
        skip_serializing_if = "crate::library::marking::InstanceMarking::is_default"
    )]
    pub marking: crate::library::marking::InstanceMarking,
    /// Footprint texts this instance prints elsewhere, by graphic
    /// (`instance.set_text_placement`). A text not listed prints where its
    /// footprint does.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub text_placements: std::collections::BTreeMap<crate::ids::GraphicId, TextPlacement>,
}

/// Where one instance prints a text of its footprint, relative to where the
/// footprint prints it. It belongs to the part: it turns and flips with the
/// placement.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TextPlacement {
    /// Added to the text's position, in the footprint's own axes, mm.
    pub offset: Point,
    /// Quarter turns added to the text's rotation.
    #[serde(default)]
    pub turn: crate::library::graphics::Rotation,
}

/// One unit of one instance drawn on the sheet. Each (instance, unit) has at
/// most one occurrence.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SymbolOccurrence {
    pub id: Id,
    /// The sheet it is drawn on.
    #[serde(default, skip_serializing_if = "SheetId::is_first")]
    pub sheet: SheetId,
    pub instance: Id,
    pub unit: UnitId,
    pub position: PointNm,
    /// Quarter turns counter-clockwise about the unit origin, applied after
    /// `mirror`.
    pub rotation: Rotation,
    /// Reflected across the unit's local x-axis (y → −y) before rotating.
    /// A left-to-right flip on the sheet is `mirror` with the rotation
    /// taken from 180°.
    pub mirror: bool,
}

/// A wire: one path from its `from` end through its `corners` to its `to`
/// end, each pair of neighbouring vertices joined by a straight segment.
/// Crossing another wire never connects, and neither does a corner lying on
/// one. A wire joins another where an end of one lies on the other
/// (`schematic::tidy` makes that a [`Junction`]), or through a shared
/// terminal, label or sheet pin; a connection point lying on a wire's path
/// splits the wire there.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    pub id: Id,
    /// The sheet it is drawn on. Stored, because a wire with two free ends
    /// has no object to take its sheet from.
    #[serde(default, skip_serializing_if = "SheetId::is_first")]
    pub sheet: SheetId,
    pub from: WireEnd,
    pub to: WireEnd,
    /// The vertices between its ends, in order from `from`, in sheet
    /// millimetres; at most `schematic::MAX_CORNERS`. None: one straight
    /// segment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corners: Vec<Corner>,
}

/// An interior vertex of a wire's path. With a `radius`, the corner is
/// rounded by the arc of that radius tangent to both of its segments; the
/// vertex stays where it is stored, so connectivity and edits work on it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Corner {
    pub at: PointNm,
    /// Fillet radius; 0 is a sharp corner. A radius larger than its
    /// segments allow is drawn as large as fits; `wire.set_radius` refuses
    /// one.
    #[serde(default, skip_serializing_if = "is_zero_radius")]
    pub radius_nm: Nm,
}

fn is_zero_radius(radius: &Nm) -> bool {
    *radius == Nm::ZERO
}

impl Corner {
    /// A sharp corner at `at`.
    pub const fn sharp(at: PointNm) -> Self {
        Self {
            at,
            radius_nm: Nm::ZERO,
        }
    }

    /// A sharp corner at `at` mm, rounded to whole nanometres.
    pub fn sharp_mm(at: Point) -> crate::diagnostics::Result<Self> {
        PointNm::from_mm_f64_rounded(at).map(Self::sharp)
    }

    pub fn at_mm(&self) -> Point {
        self.at.to_mm_f64()
    }

    pub fn radius_mm(&self) -> f64 {
        self.radius_nm.to_mm_f64()
    }
}

/// One end of a wire: an instance terminal, a junction, a label (or power
/// port), a sheet pin, or a free end attached to nothing. A terminal end is
/// written as the terminal itself, `{"instance", "terminal"}`; a junction
/// end as `{"junction"}`, a label end as `{"label"}`, a sheet-pin end as
/// `{"sheet_pin"}` and a free end as `{"free": {"x_nm", "y_nm"}}`. An
/// attached end lies on its wire's sheet.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(untagged)]
pub enum WireEnd {
    Terminal(Endpoint),
    Junction(JunctionRef),
    Label(LabelRef),
    SheetPin(SheetPinRef),
    Free(FreeEnd),
}

/// A wire end attached to nothing, at a point on its wire's sheet, in
/// whole nanometres so ends compare exactly and a [`WireEnd`] stays a map
/// key.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(deny_unknown_fields)]
pub struct FreeEnd {
    pub free: PointNm,
}

impl FreeEnd {
    /// A free end at `p` mm, rounded to whole nanometres; `None` off the
    /// sheet's range.
    pub fn new(p: Point) -> Option<Self> {
        PointNm::from_mm_f64_rounded(p)
            .ok()
            .map(|free| Self { free })
    }
    pub fn point(self) -> Point {
        self.free.to_mm_f64()
    }
}

/// A wire end on a label or power port.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(deny_unknown_fields)]
pub struct LabelRef {
    pub label: Id,
}

/// A wire end on a sheet symbol's pin.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(deny_unknown_fields)]
pub struct SheetPinRef {
    pub sheet_pin: Id,
}

/// A wire end on a junction.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(deny_unknown_fields)]
pub struct JunctionRef {
    pub junction: Id,
}

impl WireEnd {
    pub fn junction(id: Id) -> Self {
        Self::Junction(JunctionRef { junction: id })
    }
    pub fn label(id: Id) -> Self {
        Self::Label(LabelRef { label: id })
    }
    pub fn sheet_pin(id: Id) -> Self {
        Self::SheetPin(SheetPinRef { sheet_pin: id })
    }
    /// A free end at `p`; `None` off the sheet's range.
    pub fn free(p: Point) -> Option<Self> {
        FreeEnd::new(p).map(Self::Free)
    }
    /// Where a free end is; `None` for an attached end.
    pub fn free_point(self) -> Option<Point> {
        match self {
            Self::Free(f) => Some(f.point()),
            _ => None,
        }
    }
    /// The terminal this end lands on, if it is a terminal end.
    pub fn terminal(self) -> Option<Endpoint> {
        match self {
            Self::Terminal(endpoint) => Some(endpoint),
            _ => None,
        }
    }
    pub fn junction_id(self) -> Option<Id> {
        match self {
            Self::Junction(j) => Some(j.junction),
            _ => None,
        }
    }
    pub fn label_id(self) -> Option<Id> {
        match self {
            Self::Label(l) => Some(l.label),
            _ => None,
        }
    }
    pub fn sheet_pin_id(self) -> Option<Id> {
        match self {
            Self::SheetPin(p) => Some(p.sheet_pin),
            _ => None,
        }
    }
}

impl From<Endpoint> for WireEnd {
    fn from(endpoint: Endpoint) -> Self {
        Self::Terminal(endpoint)
    }
}

/// A connection point on the sheet where wires meet away from a terminal:
/// made where a wire end lands on another wire (a T), or put on a wire or
/// crossing with `junction.place`. Never a bend: bends are corners. Any
/// wire running over it connects to it. The dot is derived where three or
/// more conductors meet, and always drawn for a placed junction point.
/// After `schematic::tidy` an unplaced junction has three or more wire ends
/// and a placed one at least one.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Junction {
    pub id: Id,
    #[serde(default, skip_serializing_if = "SheetId::is_first")]
    pub sheet: SheetId,
    pub position: PointNm,
    /// Put there as a junction point (`junction.place`), so it shows as a
    /// dot even where only two wires pass through it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub placed: bool,
}

/// A persistent electrical net. Its ID survives renaming, unrelated edits and
/// saving; the name is a display label. Every instance terminal belongs to
/// exactly one net, and a net's terminals are joined by its wires.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Net {
    pub id: Id,
    pub name: String,
    pub terminals: Vec<Endpoint>,
}

/// A board side: where a component sits and which outer copper, mask and
/// silkscreen a footprint layer lands on. Copper routing names any copper
/// layer with [`CopperLayer`].
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub enum Layer {
    Top,
    Bottom,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub instance: Id,
    pub position: Point,
    pub rotation_deg: f64,
    pub layer: Layer,
    pub locked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: Id,
    pub net: Id,
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub layer: CopperLayer,
    pub locked: bool,
}

/// Soldermask colour as fabricators offer it.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MaskColour {
    #[default]
    Green,
    Blue,
    Red,
    Purple,
    Yellow,
    Black,
    White,
    /// Unpigmented mask over the natural laminate.
    Clear,
}

/// Soldermask surface texture.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MaskFinish {
    #[default]
    Gloss,
    Satin,
    Matte,
}

/// Surface finish plated over copper that the mask leaves exposed.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PadFinish {
    /// Electroless nickel, immersion gold (IPC-4552).
    #[default]
    Enig,
    /// Hot-air solder levelling with tin-lead solder.
    Hasl,
    /// Hot-air solder levelling with a lead-free tin alloy.
    LeadFreeHasl,
    /// Organic solderability preservative over bare copper.
    Osp,
    /// Immersion silver (IPC-4553).
    ImmersionSilver,
}

/// Legend (silkscreen) ink colour.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SilkscreenColour {
    #[default]
    White,
    Black,
    Yellow,
}

/// Board fabrication attributes: the finish choices a fabricator builds.
/// Every combination is valid data; whether a fabricator offers it is their
/// concern.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct Fabrication {
    pub mask_colour: MaskColour,
    pub mask_finish: MaskFinish,
    pub pad_finish: PadFinish,
    pub silkscreen_colour: SilkscreenColour,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    /// Object and definition-lineage identities share this allocator.
    pub next_id: Id,
    /// Every published revision this project can reference.
    pub library: Library,
    pub instances: Shared<Vec<ComponentInstance>>,
    pub occurrences: Shared<Vec<SymbolOccurrence>>,
    pub wires: Shared<Vec<Wire>>,
    /// Wire-to-wire connection points.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub junctions: Shared<Vec<Junction>>,
    /// Net IDs have their own allocation space, independent of object IDs.
    pub next_net: Id,
    pub nets: Shared<Vec<Net>>,
    pub placements: Shared<Vec<Placement>>,
    pub tracks: Shared<Vec<Track>>,
    pub vias: Shared<Vec<Via>>,
    pub board_size: Point,
    /// Copper and dielectric layers; sets the board's thickness.
    pub stackup: Stackup,
    /// What the fabricator is asked to build. Saved with the design; the 3D
    /// view draws it, but it never changes the saved native model.
    pub fabrication: Fabrication,
    /// The built-in material table revision (`materials::BUILTIN_REVISION`)
    /// the library snapshot's package materials are read from. Package
    /// revisions store only `MaterialId`s, so a project of another table
    /// revision is refused rather than reopened with different looks.
    pub material_table: u16,
    /// Design rules: profile snapshot, overrides and net classes.
    #[serde(
        default,
        skip_serializing_if = "crate::rules::ProjectRules::is_default"
    )]
    pub rules: crate::rules::ProjectRules,
    /// The board outline; a rectangle is `board_size`. A sketch keeps
    /// `board_size` at its solved bounding box.
    #[serde(default, skip_serializing_if = "BoardShape::is_rectangle")]
    pub board_shape: BoardShape,
    /// Dimensions and constraints that place parts relative to the board,
    /// the linked design and each other (`layout::dimensions`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub part_dimensions: Shared<Vec<PartDimension>>,
    /// The host's traces of the 3D model faces, edges and vertices the
    /// part dimensions name (`layout::dimensions::model`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_traces: Shared<Vec<crate::model::dimension::ModelTrace>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fills: Shared<Vec<Fill>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keepouts: Shared<Vec<Keepout>>,
    /// Schematic sheets in page order.
    #[serde(default, skip_serializing_if = "Sheets::is_default")]
    pub sheets: Sheets,
    /// Title-block properties.
    #[serde(default, skip_serializing_if = "ProjectProperties::is_default")]
    pub properties: ProjectProperties,
    /// Net labels and power ports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Shared<Vec<Label>>,
    /// Hierarchical sheet symbols.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheet_symbols: Shared<Vec<SheetSymbol>>,
    /// Sheet furniture: notes, lines and rectangles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drawings: Shared<Vec<SheetDrawing>>,
    /// Waived pre-flight findings for fabrication output.
    #[serde(default, skip_serializing_if = "production::Production::is_default")]
    pub production: production::Production,
    /// The board interface: the datums a linked design may reference and
    /// their append-only slot table (docs/architecture/board-interface.md).
    #[serde(default, skip_serializing_if = "interface_is_empty")]
    pub interface: Shared<interface::InterfaceTable>,
    /// The link to a linked design, its references, bindings and
    /// snapshot (`interface::assembly_link`); `None` until `assembly.link`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly_link: Option<Box<assembly_link::AssemblyLink>>,
}

fn interface_is_empty(table: &Shared<interface::InterfaceTable>) -> bool {
    table.is_empty()
}

/// The name of a new project, and the stem of the file name proposed for
/// a project without one.
pub const UNTITLED: &str = "Untitled";

/// What a new project's board starts as: shipped data
/// (`library/defaults/new-project.json` and its stackup family) that
/// `sparksmith-io` loads and hands in.
#[derive(Clone, Debug, PartialEq)]
pub struct NewBoard {
    /// The board rectangle, mm.
    pub size: Point,
    pub stackup: Stackup,
}

/// A test fixture, never in the product: a project on the fixture stackup
/// ([`StackupPresets::fixture`]) and a 60 × 40 mm board, the board the
/// tests were measured on.
#[cfg(any(test, feature = "test-fixtures"))]
impl Default for Project {
    fn default() -> Self {
        Self::new(&NewBoard::fixture())
    }
}

impl NewBoard {
    /// No board: no layers and no size, which every check and file refuses.
    /// For a project that only carries a library to read (a catalog
    /// preview or index), never one that is saved or edited.
    pub fn without_board() -> Self {
        Self {
            size: [0.0, 0.0],
            stackup: Stackup {
                copper: Vec::new(),
                dielectrics: Vec::new(),
                mask_top: false,
                mask_bottom: false,
                mask_um: 0.0,
                mask_laminate_nm: crate::units::Nm::ZERO,
                silkscreen_top: false,
                silkscreen_bottom: false,
                dk_estimates: Unsaved::default(),
            },
        }
    }

    /// A test fixture, never in the product: a 60 × 40 mm board on the
    /// fixture stackup ([`Stackup::default`]).
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn fixture() -> Self {
        Self {
            size: [60.0, 40.0],
            stackup: Stackup::default(),
        }
    }
}

impl Project {
    /// An empty project named [`UNTITLED`] on `board`, without rules.
    pub fn new(board: &NewBoard) -> Self {
        Self {
            name: UNTITLED.into(),
            next_id: 1,
            library: Library::default(),
            instances: Shared::default(),
            occurrences: Shared::default(),
            wires: Shared::default(),
            junctions: Shared::default(),
            next_net: 1,
            nets: Shared::default(),
            placements: Shared::default(),
            tracks: Shared::default(),
            vias: Shared::default(),
            board_size: board.size,
            stackup: board.stackup.clone(),
            fabrication: Fabrication::default(),
            material_table: crate::materials::BUILTIN_REVISION,
            rules: crate::rules::ProjectRules::default(),
            board_shape: BoardShape::Rectangle,
            part_dimensions: Shared::default(),
            model_traces: Shared::default(),
            fills: Shared::default(),
            keepouts: Shared::default(),
            sheets: Sheets::default(),
            properties: ProjectProperties::default(),
            labels: Shared::default(),
            sheet_symbols: Shared::default(),
            drawings: Shared::default(),
            production: production::Production::default(),
            interface: Shared::default(),
            assembly_link: None,
        }
    }
}

impl Project {
    pub fn allocate(&mut self) -> crate::Result<Id> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| crate::Error::Limit("object identity space exhausted".into()))?;
        Ok(id)
    }
    pub fn allocate_net(&mut self) -> crate::Result<Id> {
        let id = self.next_net;
        self.next_net = id
            .checked_add(1)
            .ok_or_else(|| crate::Error::Limit("net identity space exhausted".into()))?;
        Ok(id)
    }
    pub fn instance(&self, id: Id) -> Option<&ComponentInstance> {
        self.instances.iter().find(|i| i.id == id)
    }
    /// The instance with this designator, compared case-insensitively.
    pub fn instance_by_designator(&self, designator: &str) -> Option<&ComponentInstance> {
        self.instances
            .iter()
            .find(|i| i.designator.eq_ignore_ascii_case(designator))
    }
    pub fn occurrence(&self, id: Id) -> Option<&SymbolOccurrence> {
        self.occurrences.iter().find(|o| o.id == id)
    }
    pub fn occurrences_of(&self, instance: Id) -> impl Iterator<Item = &SymbolOccurrence> {
        self.occurrences
            .iter()
            .filter(move |o| o.instance == instance)
    }
    /// Wires with an end on `end` (a terminal or a junction).
    pub fn wires_at(&self, end: impl Into<WireEnd>) -> impl Iterator<Item = &Wire> {
        let end = end.into();
        self.wires
            .iter()
            .filter(move |w| w.from == end || w.to == end)
    }
    pub fn wire(&self, id: Id) -> Option<&Wire> {
        self.wires.iter().find(|w| w.id == id)
    }
    pub fn junction(&self, id: Id) -> Option<&Junction> {
        self.junctions.iter().find(|j| j.id == id)
    }
    pub fn label(&self, id: Id) -> Option<&Label> {
        self.labels.iter().find(|l| l.id == id)
    }
    pub fn sheet(&self, id: SheetId) -> Option<&Sheet> {
        self.sheets.get(id)
    }
    pub fn sheet_symbol(&self, id: Id) -> Option<&SheetSymbol> {
        self.sheet_symbols.iter().find(|s| s.id == id)
    }
    /// A sheet pin and the symbol it sits on.
    pub fn sheet_pin(&self, id: Id) -> Option<(&SheetSymbol, &SheetPin)> {
        self.sheet_symbols
            .iter()
            .find_map(|s| s.pin(id).map(|p| (s, p)))
    }
    /// The sheet symbol that uses `sheet` as its child, if any.
    pub fn parent_symbol(&self, sheet: SheetId) -> Option<&SheetSymbol> {
        self.sheet_symbols.iter().find(|s| s.child == sheet)
    }
    pub fn drawing(&self, id: Id) -> Option<&SheetDrawing> {
        self.drawings.iter().find(|d| d.id == id)
    }
    pub fn placement_of(&self, instance: Id) -> Option<&Placement> {
        self.placements.iter().find(|p| p.instance == instance)
    }
    pub fn via(&self, id: Id) -> Option<&Via> {
        self.vias.iter().find(|v| v.id == id)
    }
    pub fn fill(&self, id: Id) -> Option<&Fill> {
        self.fills.iter().find(|f| f.id == id)
    }
    pub fn keepout(&self, id: Id) -> Option<&Keepout> {
        self.keepouts.iter().find(|k| k.id == id)
    }
    /// The solved board outline: the sketch's last solve, or the
    /// `board_size` rectangle.
    pub fn board(&self) -> std::borrow::Cow<'_, SolvedBoard> {
        match &self.board_shape {
            BoardShape::Rectangle => {
                std::borrow::Cow::Owned(SolvedBoard::rectangle(self.board_size))
            }
            BoardShape::Sketch(sketch) => std::borrow::Cow::Borrowed(&sketch.solved),
        }
    }
    pub fn net(&self, id: Id) -> Option<&Net> {
        self.nets.iter().find(|n| n.id == id)
    }
    /// The net containing a terminal; every valid terminal has one.
    pub fn net_of(&self, terminal: Endpoint) -> Option<&Net> {
        self.nets
            .iter()
            .find(|n| n.terminals.binary_search(&terminal).is_ok())
    }
    pub fn net_name(&self, id: Id) -> &str {
        self.net(id).map_or("unknown net", |n| n.name.as_str())
    }
}

/// Whether an edit may make consequential changes (remove wires, placements)
/// that a reviewed update plan lists, or must refuse them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditPolicy {
    Direct,
    Consequential,
}

/// Everything one undo step restores: the project and the drafts being
/// authored. Unchanged halves are shared between history entries.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(any(test, feature = "test-fixtures"), derive(Default))]
pub struct Document {
    pub project: Arc<Project>,
    pub drafts: Arc<Drafts>,
}

impl Document {
    /// A placeholder that a file replaces before anyone reads it: no board
    /// layers and no size, which every check refuses.
    pub(crate) fn empty_for_file() -> Self {
        Self {
            project: Arc::new(Project::new(&NewBoard::without_board())),
            drafts: Arc::default(),
        }
    }
}
