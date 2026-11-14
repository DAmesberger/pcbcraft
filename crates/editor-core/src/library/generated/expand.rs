//! The expansion: a validated generation (ABI 1 wire types, keyed by
//! generation keys) mapped to the project's definitions (keyed by local
//! ids). A pure function of the generation and the five lineage references,
//! so a stored part can be expanded again and compared exactly
//! (`docs/generator-plugins.md`, "Expansion").
//!
//! Local ids are numbered in the generation's own order: terminals
//! 1…n with their fixed functions n+1…2n; symbol units, each followed by
//! its graphics; package terminals 1…m; pads 1…p, then the footprint's
//! drawings (silkscreen, assembly, courtyard).
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
use crate::library::templates::footprint::COURTYARD_STROKE_MM;
use crate::library::{
    BindingDefinition, ComponentDefinition, CornerMask, Definition, DrawingLayer,
    FootprintDefinition, Hole, HoleShape, MaskRule, ModelPlacement, Outline, PackageDefinition,
    PackageForm, PackageModel, PackageTerminal, Pad, PadKind, PadShape, PadSide, PasteRule,
    Segment, SymbolDefinition, Terminal, TerminalClass, TerminalMap, Unit,
};
use crate::units::{Angle, ArcDirection, MILLIDEGREES_PER_TURN, Nm, PointNm};
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

fn refused(message: String) -> Error {
    Error::Invalid(format!("generated.expansion: {message}"))
}

fn nm(value: i64) -> Result<Nm> {
    Nm::new(value)
}

fn point(p: v1::Point) -> Result<PointNm> {
    Ok(PointNm::new(nm(p[0])?, nm(p[1])?))
}

fn local(index: usize, base: u32) -> Result<u32> {
    u32::try_from(index)
        .ok()
        .and_then(|i| i.checked_add(base))
        .ok_or_else(|| refused("the part has more items than local ids".into()))
}

/// Keys to the local ids they were numbered as.
fn numbered<'a>(keys: impl Iterator<Item = &'a str>, base: u32) -> Result<BTreeMap<&'a str, u32>> {
    let mut map = BTreeMap::new();
    for (index, key) in keys.enumerate() {
        if map.insert(key, local(index, base)?).is_some() {
            return Err(refused(format!("the key \"{key}\" is used twice")));
        }
    }
    Ok(map)
}

fn lookup(map: &BTreeMap<&str, u32>, key: &str, what: &str) -> Result<u32> {
    map.get(key)
        .copied()
        .ok_or_else(|| refused(format!("{what} \"{key}\" is not declared")))
}

/// The definitions of `generation` at `refs`, its package holding `model`
/// (the generation's part file as a Model revision of the project, at the
/// generation's values), in order: component, symbol,
/// package, footprint, binding for a complete part; package and footprint
/// for a package.
pub fn definitions(
    generation: &v1::Generation,
    refs: Refs,
    model: PackageModel,
) -> Result<Vec<(DefinitionId, Definition)>> {
    let package_terminals = numbered(generation.package.terminals.iter().map(String::as_str), 1)?;
    let package = package(generation, &package_terminals, model)?;
    let footprint = footprint(generation, refs.package, &package_terminals)?;
    let mut out = Vec::with_capacity(5);
    match (refs.component, &generation.component, &generation.symbol) {
        (Some((component_id, symbol_ref, binding_ref)), Some(component), Some(symbol)) => {
            let terminals = numbered(component.terminals.iter().map(|t| t.key.as_str()), 1)?;
            out.push((
                component_id,
                Definition::Component(component_definition(component, symbol_ref, binding_ref)?),
            ));
            out.push((
                symbol_ref.id,
                Definition::Symbol(symbol_definition(symbol, component_id, &terminals)?),
            ));
            out.push((refs.package.id, Definition::Package(package)));
            out.push((refs.footprint.id, Definition::Footprint(footprint)));
            let mappings = generation
                .binding
                .iter()
                .map(|entry| {
                    Ok(TerminalMap {
                        package_terminal: PackageTerminalId(lookup(
                            &package_terminals,
                            &entry.package_terminal,
                            "package terminal",
                        )?),
                        terminal: TerminalId(lookup(&terminals, &entry.terminal, "terminal")?),
                    })
                })
                .collect::<Result<_>>()?;
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
        }
        _ => {
            return Err(refused(
                "a complete part needs a component and a symbol, and a package needs neither"
                    .into(),
            ));
        }
    }
    Ok(out)
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
) -> Result<ComponentDefinition> {
    let n = component.terminals.len();
    let terminals = component
        .terminals
        .iter()
        .enumerate()
        .map(|(index, t)| {
            Ok(Terminal {
                id: TerminalId(local(index, 1)?),
                name: t.name.clone(),
                class: TerminalClass::Electrical(TerminalFunctions::fixed(
                    FunctionId(local(index, local(n, 1)?)?),
                    direction(t.direction),
                )),
            })
        })
        .collect::<Result<_>>()?;
    Ok(ComponentDefinition {
        next_local: local(2 * n, 1)?,
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
    terminals: &BTreeMap<&str, u32>,
) -> Result<SymbolDefinition> {
    let mut next: u32 = 1;
    let mut take = || -> Result<u32> {
        let id = next;
        next = next
            .checked_add(1)
            .ok_or_else(|| refused("the symbol has more items than local ids".into()))?;
        Ok(id)
    };
    let mut units = Vec::with_capacity(symbol.units.len());
    for unit in &symbol.units {
        let id = UnitId(take()?);
        let mut graphics = Vec::with_capacity(unit.graphics.len());
        for graphic in &unit.graphics {
            graphics.push(Graphic {
                id: GraphicId(take()?),
                item: item(graphic)?,
            });
        }
        let pins = unit
            .pins
            .iter()
            .map(|pin| {
                let mut out = Pin::new(
                    TerminalId(lookup(terminals, &pin.terminal, "terminal")?),
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
        next_local: take()?,
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

/// The exact cosine and sine of a quarter-turn angle, else `None`.
fn quarter(mdeg: i64) -> Option<(i64, i64)> {
    match mdeg.rem_euclid(i64::from(MILLIDEGREES_PER_TURN)) {
        0 => Some((1, 0)),
        90_000 => Some((0, 1)),
        180_000 => Some((-1, 0)),
        270_000 => Some((0, -1)),
        _ => None,
    }
}

/// The point at `mdeg` on the circle about `center` of `radius`: exact at
/// quarter turns, else rounded once to nanometres.
fn on_circle(center: v1::Point, radius: i64, mdeg: i64) -> Result<PointNm> {
    if let Some((c, s)) = quarter(mdeg) {
        return point([center[0] + c * radius, center[1] + s * radius]);
    }
    let angle = (mdeg as f64 / 1000.0).to_radians();
    let to_mm = |v: i64| v as f64 / 1_000_000.0;
    PointNm::from_mm_f64_rounded([
        to_mm(center[0]) + to_mm(radius) * angle.cos(),
        to_mm(center[1]) + to_mm(radius) * angle.sin(),
    ])
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
            radius_nm,
            start_mdeg,
            sweep_mdeg,
        } => {
            let start = on_circle(*center, *radius_nm, *start_mdeg)?;
            let full = sweep_mdeg.unsigned_abs() >= MILLIDEGREES_PER_TURN.unsigned_abs().into();
            ShapeNm::Arc {
                start,
                end: if full {
                    start
                } else {
                    on_circle(*center, *radius_nm, start_mdeg.saturating_add(*sweep_mdeg))?
                },
                centre: point(*center)?,
                direction: if *sweep_mdeg < 0 {
                    ArcDirection::Clockwise
                } else {
                    ArcDirection::CounterClockwise
                },
            }
        }
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
/// names, each a length in mm equal to the nanometres, an angle in degrees
/// equal to the millidegrees, or the count itself.
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
            PartValue::Mm(mm) => Nm::from_mm_f64_rounded(mm).is_ok_and(|n| n.get() == *int),
            PartValue::Deg(deg) => deg.is_finite() && (deg * 1000.0).round() == *int as f64,
            PartValue::Count(n) => i64::from(n) == *int,
            PartValue::Number(_) => false,
        };
        if !same {
            return Err(differs(name));
        }
    }
    Ok(())
}

fn package(
    generation: &v1::Generation,
    terminals: &BTreeMap<&str, u32>,
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
        next_local: local(terminals.len(), 1)?,
        terminals: generation
            .package
            .terminals
            .iter()
            .map(|key| {
                Ok(PackageTerminal {
                    id: PackageTerminalId(lookup(terminals, key, "package terminal")?),
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
    terminals: &BTreeMap<&str, u32>,
) -> Result<FootprintDefinition> {
    let source = &generation.footprint;
    let mut pads = Vec::with_capacity(source.pads.len());
    for (index, pad) in source.pads.iter().enumerate() {
        let shape = pad_shape(&pad.shape)?;
        let rotation = shape.canonical_rotation(Angle::normalized(pad.rot_mdeg));
        let hole = pad.hole.as_ref().map(hole).transpose()?;
        let kind = match &hole {
            None => PadKind::Smd,
            Some(h) if h.plated => PadKind::PlatedThrough,
            Some(_) => PadKind::NonPlatedHole,
        };
        pads.push(Pad {
            id: PadId(local(index, 1)?),
            kind,
            terminal: Some(PackageTerminalId(lookup(
                terminals,
                &pad.terminal,
                "package terminal",
            )?)),
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
    let mut next = local(pads.len(), 1)?;
    let mut graphics = Vec::new();
    let mut push = |layer: DrawingLayer, item: Item, graphics: &mut Vec<FootprintGraphic>| {
        let id = GraphicId(next);
        next = next
            .checked_add(1)
            .ok_or_else(|| refused("the footprint has more items than local ids".into()))?;
        graphics.push(FootprintGraphic {
            id,
            layer,
            side: graphics::Side::Component,
            item,
        });
        Ok::<(), Error>(())
    };
    for graphic in &source.silk {
        push(DrawingLayer::Silkscreen, item(graphic)?, &mut graphics)?;
    }
    for graphic in &source.assembly {
        push(DrawingLayer::Assembly, item(graphic)?, &mut graphics)?;
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
            width_nm: Nm::from_mm_f64_rounded(COURTYARD_STROKE_MM)?,
            fill: graphics::Fill::None,
        };
        push(DrawingLayer::Courtyard, courtyard, &mut graphics)?;
    }
    Ok(FootprintDefinition {
        package,
        next_local: next,
        source: None,
        pads,
        graphics,
        polarity: None,
        datum: Default::default(),
        defaults: Default::default(),
        keepouts: Vec::new(),
    })
}
