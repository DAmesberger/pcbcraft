//! The expansion: a validated generation (ABI 1 wire types, keyed by
//! generation keys) mapped to the project's definitions (keyed by local
//! ids). A pure function of the generation, the five lineage references
//! and the part's key-to-id map ([`KeyIds`]), so a stored part can be
//! expanded again and compared exactly (`docs/generator-plugins.md`,
//! "Expansion").
//!
//! **Stable keys (slice B1).** Every generated object has a generation key
//! (terminal, unit, symbol drawing, package terminal, pad, footprint
//! drawing), and its local id comes from the map: a key the map holds
//! keeps its id, a new key takes the next id after every id the map has
//! ever held. The map never forgets a key, so a key that comes back gets
//! its old id and no id is ever reused for another key. A part's ids
//! therefore do not depend on the order the module emits things in, and
//! regenerating with a module that inserts a pad leaves every other pad's
//! id, and the tracks on it, where they were. For a fresh map the ids
//! follow the generation's order: terminals 1…n with their fixed functions
//! n+1…2n; symbol units, each followed by its graphics; package terminals
//! 1…m; pads 1…p, then the footprint's drawings (silkscreen, assembly,
//! courtyard).
use crate::diagnostics::{Error, Result};
use crate::functions::{Direction, TerminalFunctions};
use crate::ids::ContentHash;
use crate::ids::{
    BindingRef, DefinitionId, FootprintRef, FunctionId, GraphicId, PackageRef, PackageTerminalId,
    PadId, SymbolRef, TerminalId, UnitId,
};
use crate::library::graphics::{
    self, FootprintGraphic, Graphic, HAlign, Item, Justify, Pin, PinDirection, Rotation, ShapeNm,
    TextContent, VAlign,
};
use crate::library::package_model::PartValue;
use crate::library::{
    BindingDefinition, ComponentDefinition, CornerMask, Definition, DrawingLayer,
    FootprintDefinition, Hole, HoleShape, MaskRule, ModelPlacement, Outline, PackageDefinition,
    PackageForm, PackageModel, PackageTerminal, Pad, PadKind, PadShape, PadSide, PasteRule,
    Segment, SymbolDefinition, Terminal, TerminalClass, TerminalMap, Unit,
};
use crate::units::{Angle, ArcDirection, MILLIDEGREES_PER_TURN, Nm, PointNm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sparksmith_generator_abi::v1;
use std::collections::BTreeMap;

/// The lineages and revisions one expansion fills: all five for a complete
/// part, `package` and `footprint` alone for a package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refs {
    pub component: Option<(DefinitionId, SymbolRef, BindingRef)>,
    pub package: PackageRef,
    pub footprint: FootprintRef,
}

/// The courtyard outline's stroke, 0.05 mm, as the footprint templates
/// draw it (`templates::footprint::COURTYARD_STROKE_MM`).
const COURTYARD_STROKE: Nm = match Nm::try_new(50_000) {
    Some(nm) => nm,
    None => Nm::ZERO,
};

fn refused(message: String) -> Error {
    Error::Invalid(format!("generated.expansion: {message}"))
}

fn nm(value: i64) -> Result<Nm> {
    Nm::new(value)
}

fn point(p: v1::Point) -> Result<PointNm> {
    Ok(PointNm::new(nm(p[0])?, nm(p[1])?))
}

/// Keys a part's map holds, all namespaces together.
pub const MAX_KEYS: usize = 16 * 1024;

/// A generated part's generation keys and the local ids they were given,
/// one map per definition (each definition numbers its own locals). Keys
/// carry their kind: `t:` a terminal, `f:` its function (component); `u:` a
/// unit, `g:UNIT/KEY` a unit's drawing (symbol); `t:` a package terminal
/// (package); `p:` a pad, `g:` a drawing, `courtyard` (footprint).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeyIds {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub component: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub symbol: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub package: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub footprint: BTreeMap<String, u32>,
}

impl KeyIds {
    /// Every key, all namespaces.
    pub fn len(&self) -> usize {
        self.component.len() + self.symbol.len() + self.package.len() + self.footprint.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bounded, and every id in a namespace used once and nonzero.
    pub fn validate(&self) -> Result<()> {
        if self.len() > MAX_KEYS {
            return Err(Error::Limit(format!(
                "generated.record: a generated part maps at most {MAX_KEYS} keys"
            )));
        }
        for map in [
            &self.component,
            &self.symbol,
            &self.package,
            &self.footprint,
        ] {
            let mut ids = std::collections::BTreeSet::new();
            for (key, id) in map {
                if *id == 0 || !ids.insert(*id) || key.is_empty() || key.len() > 2048 {
                    return Err(Error::Invalid(format!(
                        "generated.record: the key map gives \"{}\" an id another key has, or none",
                        key.chars().take(80).collect::<String>()
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Ids of one namespace: the map's, then new ones after every id it holds.
struct Ids {
    map: BTreeMap<String, u32>,
    next: u32,
    /// Keys given in this expansion, to refuse one given twice.
    given: std::collections::BTreeSet<String>,
}

impl Ids {
    fn new(map: &BTreeMap<String, u32>) -> Self {
        let next = map.values().max().map_or(1, |m| m.saturating_add(1));
        Self {
            map: map.clone(),
            next,
            given: Default::default(),
        }
    }

    /// The id of `key`: the map's, or the next free one.
    fn id(&mut self, key: String) -> Result<u32> {
        if !self.given.insert(key.clone()) {
            return Err(refused(format!("the key \"{key}\" is used twice")));
        }
        if let Some(id) = self.map.get(&key) {
            return Ok(*id);
        }
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| refused("the part has more items than local ids".into()))?;
        self.map.insert(key, id);
        Ok(id)
    }

    /// The id `key` was given in this expansion.
    fn of(&self, key: &str, what: &str) -> Result<u32> {
        self.map
            .get(key)
            .copied()
            .filter(|_| self.given.contains(key))
            .ok_or_else(|| refused(format!("{what} \"{key}\" is not declared")))
    }

    /// The definition's next local id: after every id the map holds.
    fn next_local(&self) -> u32 {
        self.next
    }
}

/// The definitions of `generation` at `refs`, its package holding `model`
/// (the generation's part file as a Model revision of the project, at the
/// generation's values), with local ids from `ids` (a new part's map is
/// empty), in order: component, symbol, package, footprint, binding for a
/// complete part; package and footprint for a package. Also returns the
/// map with every new key added.
pub fn definitions(
    generation: &v1::Generation,
    refs: Refs,
    model: PackageModel,
    ids: &KeyIds,
) -> Result<(Vec<(DefinitionId, Definition)>, KeyIds)> {
    let mut package_terminals = Ids::new(&ids.package);
    for key in &generation.package.terminals {
        package_terminals.id(format!("t:{key}"))?;
    }
    let package = package(generation, &package_terminals, model)?;
    let mut pads = Ids::new(&ids.footprint);
    let footprint = footprint(generation, refs.package, &package_terminals, &mut pads)?;
    let mut out = Vec::with_capacity(5);
    let mut used = KeyIds {
        package: package_terminals.map.clone(),
        footprint: pads.map,
        ..KeyIds::default()
    };
    match (refs.component, &generation.component, &generation.symbol) {
        (Some((component_id, symbol_ref, binding_ref)), Some(component), Some(symbol)) => {
            let mut terminals = Ids::new(&ids.component);
            let definition =
                component_definition(component, symbol_ref, binding_ref, &mut terminals)?;
            out.push((component_id, Definition::Component(definition)));
            let mut units = Ids::new(&ids.symbol);
            out.push((
                symbol_ref.id,
                Definition::Symbol(symbol_definition(
                    symbol,
                    component_id,
                    &terminals,
                    &mut units,
                )?),
            ));
            out.push((refs.package.id, Definition::Package(package)));
            out.push((refs.footprint.id, Definition::Footprint(footprint)));
            let mappings = generation
                .binding
                .iter()
                .map(|entry| {
                    Ok(TerminalMap {
                        package_terminal: PackageTerminalId(
                            package_terminals
                                .of(&format!("t:{}", entry.package_terminal), "package terminal")?,
                        ),
                        terminal: TerminalId(
                            terminals.of(&format!("t:{}", entry.terminal), "terminal")?,
                        ),
                    })
                })
                .collect::<Result<_>>()?;
            used.component = terminals.map;
            used.symbol = units.map;
            out.push((
                binding_ref.id,
                Definition::Binding(BindingDefinition {
                    component: component_id,
                    package: refs.package,
                    mappings,
                    footprints: vec![refs.footprint],
                    unavailable: Vec::new(),
                }),
            ));
        }
        (None, None, None) => {
            out.push((refs.package.id, Definition::Package(package)));
            out.push((refs.footprint.id, Definition::Footprint(footprint)));
            used.component = ids.component.clone();
            used.symbol = ids.symbol.clone();
        }
        _ => {
            return Err(refused(
                "a complete part needs a component and a symbol, and a package needs neither"
                    .into(),
            ));
        }
    }
    Ok((out, used))
}

fn direction(d: v1::Direction) -> Direction {
    match d {
        v1::Direction::Input => Direction::Input,
        v1::Direction::Output => Direction::Output,
        v1::Direction::Bidirectional => Direction::Bidirectional,
        v1::Direction::Passive => Direction::Passive,
        v1::Direction::PowerInput => Direction::PowerInput,
        v1::Direction::PowerOutput => Direction::PowerOutput,
    }
}

fn component_definition(
    component: &v1::Component,
    symbol: SymbolRef,
    binding: BindingRef,
    ids: &mut Ids,
) -> Result<ComponentDefinition> {
    // Every terminal first, then every function, so a fresh map numbers
    // terminals 1…n and functions n+1…2n.
    let terminal_ids = component
        .terminals
        .iter()
        .map(|t| ids.id(format!("t:{}", t.key)))
        .collect::<Result<Vec<_>>>()?;
    let function_ids = component
        .terminals
        .iter()
        .map(|t| ids.id(format!("f:{}", t.key)))
        .collect::<Result<Vec<_>>>()?;
    let terminals = component
        .terminals
        .iter()
        .zip(terminal_ids.into_iter().zip(function_ids))
        .map(|(t, (id, function))| Terminal {
            id: TerminalId(id),
            name: t.name.clone(),
            class: TerminalClass::Electrical(TerminalFunctions::fixed(
                FunctionId(function),
                direction(t.direction),
            )),
        })
        .collect();
    Ok(ComponentDefinition {
        next_local: ids.next_local(),
        designator_prefix: component.prefix.clone(),
        terminals,
        resources: Vec::new(),
        constraints: Vec::new(),
        representations: vec![symbol],
        implementations: vec![binding],
        // Standard field keys stay in the generation (and its result hash);
        // GP1 selects no fields (docs/generator-plugins.md).
        fields: Vec::new(),
        pin_rules: Vec::new(),
        marking: None,
        review: None,
        generic: None,
    })
}

fn symbol_definition(
    symbol: &v1::Symbol,
    component: DefinitionId,
    terminals: &Ids,
    ids: &mut Ids,
) -> Result<SymbolDefinition> {
    let mut units = Vec::with_capacity(symbol.units.len());
    for unit in &symbol.units {
        let id = UnitId(ids.id(format!("u:{}", unit.key))?);
        let mut graphics = Vec::with_capacity(unit.graphics.len());
        for graphic in &unit.graphics {
            graphics.push(Graphic {
                id: GraphicId(ids.id(format!("g:{}/{}", unit.key, graphic.key))?),
                item: item(graphic)?,
            });
        }
        let pins = unit
            .pins
            .iter()
            .map(|pin| {
                let mut out = Pin::new(
                    TerminalId(terminals.of(&format!("t:{}", pin.terminal), "terminal")?),
                    point(pin.at)?,
                    pin_direction(pin.rot_mdeg)?,
                    nm(pin.length_nm)?,
                );
                out.name.visible = pin.show_name;
                out.number.visible = pin.show_number;
                Ok(out)
            })
            .collect::<Result<_>>()?;
        units.push(Unit {
            id,
            label: unit.key.clone(),
            graphics,
            pins,
        });
    }
    Ok(SymbolDefinition {
        component,
        next_local: ids.next_local(),
        pin_text_height_nm: graphics::PIN_TEXT_HEIGHT,
        units,
    })
}

fn pin_direction(rot_mdeg: i64) -> Result<PinDirection> {
    match rot_mdeg.rem_euclid(i64::from(MILLIDEGREES_PER_TURN)) {
        0 => Ok(PinDirection::Right),
        90_000 => Ok(PinDirection::Up),
        180_000 => Ok(PinDirection::Left),
        270_000 => Ok(PinDirection::Down),
        _ => Err(refused(format!(
            "a pin points at {rot_mdeg} millidegrees; symbol pins point along the axes"
        ))),
    }
}

fn fill(f: v1::Fill) -> graphics::Fill {
    match f {
        v1::Fill::None => graphics::Fill::None,
        v1::Fill::Body => graphics::Fill::Body,
        v1::Fill::Solid => graphics::Fill::Solid,
    }
}

fn shape(primitive: &v1::Primitive) -> Result<ShapeNm> {
    Ok(match primitive {
        v1::Primitive::Line { a, b } => ShapeNm::Line {
            a: point(*a)?,
            b: point(*b)?,
        },
        v1::Primitive::Rect { min, max } => ShapeNm::Rect {
            a: point(*min)?,
            b: point(*max)?,
            radius_nm: Nm::ZERO,
        },
        v1::Primitive::Polyline { points } => ShapeNm::Polyline {
            points: points.iter().map(|p| point(*p)).collect::<Result<_>>()?,
            radii_nm: Vec::new(),
        },
        v1::Primitive::Polygon { points } => ShapeNm::Polygon {
            points: points.iter().map(|p| point(*p)).collect::<Result<_>>()?,
            radii_nm: Vec::new(),
        },
        v1::Primitive::Circle { center, radius_nm } => ShapeNm::Circle {
            center: point(*center)?,
            radius_nm: nm(*radius_nm)?,
        },
        v1::Primitive::Arc {
            center,
            start,
            end,
            clockwise,
        } => ShapeNm::Arc {
            start: point(*start)?,
            end: point(*end)?,
            centre: point(*center)?,
            direction: if *clockwise {
                ArcDirection::Clockwise
            } else {
                ArcDirection::CounterClockwise
            },
        },
        v1::Primitive::Text { .. } => {
            return Err(refused("text is not a shape".into()));
        }
    })
}

fn item(graphic: &v1::Graphic) -> Result<Item> {
    if let v1::Primitive::Text {
        at,
        height_nm,
        content,
        h_align,
        v_align,
    } = &graphic.shape
    {
        return Ok(Item::Text(graphics::Text {
            content: match content {
                v1::TextContent::Designator => TextContent::Designator,
                v1::TextContent::ComponentName => TextContent::ComponentName,
                v1::TextContent::Literal(text) => TextContent::Literal(text.clone()),
            },
            at: point(*at)?,
            height_nm: nm(*height_nm)?,
            rotation: Rotation::R0,
            justify: Justify {
                h: match h_align {
                    v1::HAlign::Left => HAlign::Left,
                    v1::HAlign::Center => HAlign::Center,
                    v1::HAlign::Right => HAlign::Right,
                },
                v: match v_align {
                    v1::VAlign::Top => VAlign::Top,
                    v1::VAlign::Middle => VAlign::Middle,
                    v1::VAlign::Bottom => VAlign::Bottom,
                },
            },
            visible: true,
            line_spacing: graphics::TEXT_LINE_SPACING,
        }));
    }
    Ok(Item::Shape {
        shape: shape(&graphic.shape)?,
        width_nm: nm(graphic.stroke_nm)?,
        fill: fill(graphic.fill),
    })
}

/// The part file a generation's model names, by its SHA-256, and the
/// values it gives it; refused when the generation has no model.
pub fn part_file(generation: &v1::Generation) -> Result<(ContentHash, &v1::Params)> {
    match &generation.model {
        Some(v1::Model::PartFile { asset, values }) => {
            let hash = ContentHash::parse(asset).ok_or_else(|| {
                refused(format!(
                    "the generator names its part file \"{asset}\", which is not a SHA-256"
                ))
            })?;
            Ok((hash, values))
        }
        None => Err(refused(
            "the generator made no 3D model; a part needs one".into(),
        )),
    }
}

/// Whether `stored` are the generation's part-file `values`: the same
/// names, each a length of the same nanometres, an angle of the same
/// millidegrees, or the same count.
pub fn part_values_match(values: &v1::Params, stored: &BTreeMap<String, PartValue>) -> Result<()> {
    let differs = |name: &str| {
        refused(format!(
            "the part file's value \"{name}\" is not the generator's"
        ))
    };
    if values.len() != stored.len() {
        return Err(refused(
            "the part file is given other values than the generator's".into(),
        ));
    }
    for (name, value) in values {
        let (Some(stored), v1::ParamValue::Int(int)) = (stored.get(name), value) else {
            return Err(differs(name));
        };
        let same = match *stored {
            PartValue::Nm(v) | PartValue::Mdeg(v) => v == *int,
            PartValue::Count(n) => i64::from(n) == *int,
            PartValue::Milli(_) => false,
        };
        if !same {
            return Err(differs(name));
        }
    }
    Ok(())
}

fn package(
    generation: &v1::Generation,
    terminals: &Ids,
    model: PackageModel,
) -> Result<PackageDefinition> {
    let geometry = &generation.package.geometry;
    let [length, width, height] = geometry.envelope_nm;
    let (_, values) = part_file(generation)?;
    let PackageModel::PartFile(part) = &model else {
        return Err(refused(
            "a generated package's 3D model is its generator's part file".into(),
        ));
    };
    part_values_match(values, &part.values)?;
    let package = PackageDefinition {
        next_local: terminals.next_local(),
        terminals: generation
            .package
            .terminals
            .iter()
            .map(|key| {
                Ok(PackageTerminal {
                    id: PackageTerminalId(terminals.of(&format!("t:{key}"), "package terminal")?),
                    designation: key.clone(),
                })
            })
            .collect::<Result<_>>()?,
        // The envelope is the generation's; the model is the part file
        // the generator names, at its values (slice GP2), resolved to a
        // Model revision of this project by the caller.
        form: PackageForm::Generic,
        envelope_nm: [nm(length)?, nm(width)?, nm(height)?],
        model,
        placement: ModelPlacement {
            translation_nm: [Nm::ZERO, Nm::ZERO, nm(geometry.seating_nm)?],
            rotation_mdeg: [Angle::ZERO; 3],
        },
        mechanical: None,
        datums: Vec::new(),
    };
    Ok(package)
}

fn pad_shape(shape: &v1::PadShape) -> Result<PadShape> {
    Ok(match shape {
        v1::PadShape::Rect { size } => PadShape::Rect {
            size: point(*size)?,
        },
        v1::PadShape::RoundRect { size, radius_nm } => PadShape::RoundRect {
            size: point(*size)?,
            // Half the short side, capped at the radius: exactly the radius
            // whenever it fits.
            radius_ratio: 0.5,
            max_radius_nm: Some(nm(*radius_nm)?),
            corners: CornerMask::ALL,
        },
        v1::PadShape::Circle { diameter_nm } => PadShape::Circle {
            diameter_nm: nm(*diameter_nm)?,
        },
        v1::PadShape::Oval { size } => PadShape::Oblong {
            size: point(*size)?,
        },
        v1::PadShape::Polygon { points } => {
            let mm = |p: &v1::Point| point(*p).map(PointNm::to_mm_f64);
            let Some(first) = points.first() else {
                return Err(refused("a polygon pad has no points".into()));
            };
            let start = mm(first)?;
            let mut segments = points
                .iter()
                .skip(1)
                .map(|p| Ok(Segment::Line { to: mm(p)? }))
                .collect::<Result<Vec<_>>>()?;
            segments.push(Segment::Line { to: start });
            PadShape::Custom {
                outline: Outline { start, segments },
            }
        }
    })
}

fn hole(hole: &v1::Hole) -> Result<Hole> {
    let shape = match &hole.shape {
        v1::HoleShape::Round { diameter_nm } => HoleShape::Round {
            diameter_nm: nm(*diameter_nm)?,
        },
        v1::HoleShape::Slot { size } => {
            let along_y = size[1] > size[0];
            HoleShape::Slot {
                width_nm: nm(size[0].min(size[1]))?,
                length_nm: nm(size[0].max(size[1]))?,
                angle_mdeg: if along_y {
                    Angle::normalized(90_000)
                } else {
                    Angle::ZERO
                },
            }
        }
    };
    Ok(Hole {
        shape,
        plated: hole.plated,
        offset: PointNm::ORIGIN,
    })
}

fn footprint(
    generation: &v1::Generation,
    package: PackageRef,
    terminals: &Ids,
    ids: &mut Ids,
) -> Result<FootprintDefinition> {
    let source = &generation.footprint;
    let mut pads = Vec::with_capacity(source.pads.len());
    for pad in &source.pads {
        let shape = pad_shape(&pad.shape)?;
        let rotation = shape.canonical_rotation(Angle::normalized(pad.rot_mdeg));
        let hole = pad.hole.as_ref().map(hole).transpose()?;
        let kind = match &hole {
            None => PadKind::Smd,
            Some(h) if h.plated => PadKind::PlatedThrough,
            Some(_) => PadKind::NonPlatedHole,
        };
        pads.push(Pad {
            id: PadId(ids.id(format!("p:{}", pad.key))?),
            kind,
            terminal: Some(PackageTerminalId(
                terminals.of(&format!("t:{}", pad.terminal), "package terminal")?,
            )),
            number: pad.key.clone(),
            position: point(pad.at)?,
            rotation_mdeg: rotation,
            side: match pad.side {
                v1::Side::Top => PadSide::Top,
                v1::Side::Bottom => PadSide::Bottom,
            },
            shape,
            hole,
            mask: match pad.mask {
                v1::Opening::Default => MaskRule::Inherit,
                v1::Opening::Expansion(by) => MaskRule::Expansion(nm(by)?),
                v1::Opening::None => MaskRule::Tented,
            },
            paste: match pad.paste {
                v1::Opening::Default => PasteRule::Inherit,
                v1::Opening::Expansion(by) => PasteRule::Reduction(nm(by.saturating_neg())?),
                v1::Opening::None => PasteRule::None,
            },
            vias: Vec::new(),
        });
    }
    let mut graphics = Vec::new();
    let mut push = |key: String, layer: DrawingLayer, item: Item| {
        graphics.push(FootprintGraphic {
            id: GraphicId(ids.id(key)?),
            layer,
            side: graphics::Side::Component,
            item,
        });
        Ok::<(), Error>(())
    };
    for graphic in &source.silk {
        push(
            format!("g:{}", graphic.key),
            DrawingLayer::Silkscreen,
            item(graphic)?,
        )?;
    }
    for graphic in &source.assembly {
        push(
            format!("g:{}", graphic.key),
            DrawingLayer::Assembly,
            item(graphic)?,
        )?;
    }
    if !source.courtyard.is_empty() {
        let courtyard = Item::Shape {
            shape: ShapeNm::Polygon {
                points: source
                    .courtyard
                    .iter()
                    .map(|p| point(*p))
                    .collect::<Result<_>>()?,
                radii_nm: Vec::new(),
            },
            width_nm: COURTYARD_STROKE,
            fill: graphics::Fill::None,
        };
        push("courtyard".into(), DrawingLayer::Courtyard, courtyard)?;
    }
    Ok(FootprintDefinition {
        package,
        next_local: ids.next_local(),
        source: None,
        pads,
        graphics,
        polarity: None,
        datum: Default::default(),
        defaults: Default::default(),
        keepouts: Vec::new(),
    })
}
