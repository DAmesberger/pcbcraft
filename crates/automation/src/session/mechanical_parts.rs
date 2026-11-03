//! Mechanical parts placed directly on the board (docs/mechanical-parts.md
//! M4): `library.part_files` lists the part files a project can place,
//! `layout.place_part_file` reads, checks and evaluates one and applies the
//! core `layout.place_part`, and `layout.set_part_file_values` evaluates a
//! placed part at new values and applies `layout.set_part`. The builders
//! ([`place_part_operation`], [`set_part_operation`]) return the core
//! operation, so the desktop applies exactly what automation applies.
use super::Session;
use crate::Result;
use pcbcraft_core::geometry::Point;
use pcbcraft_core::ids::{ContentHash, FootprintRef, Id, ModelRef};
use pcbcraft_core::layout::LayoutCommand;
use pcbcraft_core::library::bundle::{AssetBytes, PackedAsset, asset_hash};
use pcbcraft_core::library::mechanical::{
    self, LandShape, MechanicalLand, PadUse, PartModelSource, PartSpec,
};
use pcbcraft_core::library::model3d::{
    AssetRef, ModelDefinition, ModelFormat, ModelProvenance, Redistribution,
};
use pcbcraft_core::library::{Basis, PackageModel, PadKind, PadShape, Provenance};
use pcbcraft_core::model::Layer;
use pcbcraft_core::{Engine, Operation};
use pcbcraft_solidcraft::models::part_file::{self, EvaluatedPart, PartReport, PartValues};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// Longest accepted path, bytes.
const MAX_PATH: usize = 4095;
/// The media type of a part file asset.
const MEDIA_TYPE: &str = "application/x-solidcraft";
/// Licence of Pcbcraft's built-in part files.
pub const BUILTIN_LICENCE: &str = "MIT OR Apache-2.0";
/// The designator prefix of a part file that names none.
pub const DEFAULT_PREFIX: &str = "MP";

/// `diameter = scale × value(param) + add_mm` over a part's own parameter
/// (its value in mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Term {
    pub param: &'static str,
    pub scale: f64,
    pub add_mm: f64,
}

/// A built-in part's land rule: Pcbcraft data over the part's parameters
/// (the part file never supplies lands, M1.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LandRule {
    /// A round copper land around a non-plated hole, at the origin.
    Annulus { outer: Term, hole: Term },
    /// A bare non-plated hole at the origin.
    Hole { diameter: Term },
}

/// What a built-in part is placed as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuiltinPart {
    /// The `part_file::BUILTIN` name.
    pub name: &'static str,
    pub title: &'static str,
    pub prefix: &'static str,
    /// Whether it is a mechanical part (placed on the board) rather than a
    /// package body (used by a package).
    pub mechanical: bool,
    pub lands: &'static [LandRule],
}

const fn term(param: &'static str, add_mm: f64) -> Term {
    Term {
        param,
        scale: 1.0,
        add_mm,
    }
}

/// Pcbcraft's data about each built-in part file (docs/mechanical-parts.md
/// M7). Generic illustrative land sizes; verify against the chosen part.
pub const BUILTIN_PARTS: &[BuiltinPart] = &[
    BuiltinPart {
        name: "smd-nut",
        title: "SMD nut",
        prefix: "H",
        mechanical: true,
        lands: &[LandRule::Annulus {
            outer: term("outer_d", 0.6),
            hole: term("thread_d", 0.2),
        }],
    },
    BuiltinPart {
        name: "round-spacer",
        title: "Round spacer",
        prefix: "H",
        mechanical: true,
        lands: &[LandRule::Hole {
            diameter: term("inner_d", 0.0),
        }],
    },
    BuiltinPart {
        name: "finned-heatsink",
        title: "Finned heatsink",
        prefix: "HS",
        mechanical: true,
        lands: &[],
    },
    BuiltinPart {
        name: "esp32-s31-wroom-3",
        title: "ESP32-S31-WROOM-3 module body",
        prefix: "U",
        mechanical: false,
        lands: &[],
    },
];

/// Pcbcraft's data about a built-in part file.
pub fn builtin_part(name: &str) -> Option<&'static BuiltinPart> {
    BUILTIN_PARTS.iter().find(|b| b.name == name)
}

/// Where a part file comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PartFileSource {
    /// A `.solidcraft` file on disk.
    Path(String),
    /// A built-in part file by name.
    Builtin(String),
    /// A Model of format solidcraft the project holds.
    Model(ModelRef),
}

fn invalid(message: String) -> crate::Error {
    crate::Error::Document(pcbcraft_core::Error::Invalid(message))
}

/// A part file's bytes, the name it goes by and its built-in data.
pub struct ReadPart {
    pub bytes: Vec<u8>,
    pub file_name: String,
    pub builtin: Option<&'static BuiltinPart>,
    pub model: Option<ModelRef>,
}

/// Read a part file's bytes (bounded).
pub fn read_part(engine: &Engine, source: &PartFileSource) -> Result<ReadPart> {
    match source {
        PartFileSource::Path(path) => {
            if path.len() > MAX_PATH {
                return Err(invalid("part_file.read: the path is too long".into()));
            }
            let p = Path::new(path);
            if p.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                != Some("solidcraft".into())
            {
                return Err(invalid(
                    "part_file.read: a part file ends in .solidcraft".into(),
                ));
            }
            let file_name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("part.solidcraft")
                .to_owned();
            Ok(ReadPart {
                bytes: pcbcraft_io::persistence::read_bytes(p)?,
                file_name,
                builtin: None,
                model: None,
            })
        }
        PartFileSource::Builtin(name) => {
            let bytes = part_file::builtin(name).ok_or_else(|| {
                let known: Vec<&str> = part_file::BUILTIN.iter().map(|(n, _)| *n).collect();
                invalid(format!(
                    "part_file.read: no built-in part file {name}; built-ins: {}",
                    known.join(", ")
                ))
            })?;
            Ok(ReadPart {
                bytes: bytes.to_vec(),
                file_name: format!("{name}.solidcraft"),
                builtin: builtin_part(name),
                model: None,
            })
        }
        PartFileSource::Model(model) => {
            let library = &engine.project().library;
            let published = library.model(*model).ok_or_else(|| {
                invalid(format!("part_file.model: the project has no model {model}"))
            })?;
            if published.definition.format != ModelFormat::Solidcraft {
                return Err(invalid(format!(
                    "part_file.model: the model {model} is not a SolidCraft part file"
                )));
            }
            let bytes = library
                .assets
                .iter()
                .find(|a| a.hash == published.definition.asset.hash)
                .map(|a| a.data.as_slice().to_vec())
                .ok_or_else(|| {
                    invalid(format!(
                        "part_file.model: the file of {model} is not packed"
                    ))
                })?;
            let file_name = published.definition.asset.file_name.clone();
            // A project copy of a built-in is the built-in only with its
            // exact bytes.
            let builtin = file_name
                .strip_suffix(".solidcraft")
                .and_then(builtin_part)
                .filter(|b| part_file::builtin(b.name).is_some_and(|own| own == bytes.as_slice()));
            Ok(ReadPart {
                bytes,
                file_name,
                builtin,
                model: Some(*model),
            })
        }
    }
}

/// Evaluate and refuse on a blocking finding.
pub fn checked(bytes: &[u8], values: &PartValues) -> Result<Arc<EvaluatedPart>> {
    let part = part_file::evaluate(bytes, values)?;
    if let Some(refusal) = part.refusal() {
        return Err(refusal.into());
    }
    Ok(part)
}

fn term_value(part: &EvaluatedPart, t: Term) -> Result<f64> {
    let p = part
        .parameters
        .iter()
        .find(|p| p.name == t.param && p.unit == "mm")
        .and_then(|p| p.value)
        .ok_or_else(|| {
            invalid(format!(
                "part.lands: the part file has no length parameter {}",
                t.param
            ))
        })?;
    Ok(t.scale * p + t.add_mm)
}

/// A built-in's lands at the part's evaluated values.
pub fn rule_lands(part: &EvaluatedPart, rules: &[LandRule]) -> Result<Vec<MechanicalLand>> {
    rules
        .iter()
        .enumerate()
        .map(|(k, rule)| {
            let name = (k + 1).to_string();
            let shape = match *rule {
                LandRule::Annulus { outer, hole } => LandShape::Circle {
                    diameter: term_value(part, outer)?,
                    hole: Some(term_value(part, hole)?),
                },
                LandRule::Hole { diameter } => LandShape::Hole {
                    diameter: term_value(part, diameter)?,
                },
            };
            Ok(MechanicalLand {
                name,
                at: [0.0, 0.0],
                shape,
            })
        })
        .collect()
}

/// The lands of a footprint's pads: position, circle or rectangle and hole;
/// terminals dropped. Other pad shapes are refused.
pub fn footprint_lands(engine: &Engine, footprint: FootprintRef) -> Result<Vec<MechanicalLand>> {
    let published = engine
        .project()
        .library
        .footprint(footprint)
        .ok_or_else(|| {
            invalid(format!(
                "part.lands: the project has no footprint {footprint}"
            ))
        })?;
    published
        .definition
        .pads
        .iter()
        .enumerate()
        .map(|(k, pad)| {
            let hole = pad.hole.as_ref().and_then(|h| match h.shape {
                pcbcraft_core::library::HoleShape::Round { diameter } => Some(diameter),
                pcbcraft_core::library::HoleShape::Slot { .. } => None,
            });
            let name: String = if pad.number.trim().is_empty() {
                (k + 1).to_string()
            } else {
                pad.number
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .take(mechanical::MAX_LAND_NAME)
                    .collect()
            };
            let shape = match (&pad.kind, &pad.shape) {
                (PadKind::NonPlatedHole, PadShape::Circle { diameter }) => LandShape::Hole {
                    diameter: hole.unwrap_or(*diameter),
                },
                (_, PadShape::Circle { diameter }) => LandShape::Circle {
                    diameter: *diameter,
                    hole,
                },
                (_, PadShape::Rect { size }) if hole.is_none() => LandShape::Rect {
                    size: *size,
                    rotation_deg: pad.rotation_deg,
                },
                _ => {
                    return Err(invalid(format!(
                        "part.lands: pad {} of {footprint} is not a plain circle or rectangle",
                        pad.number
                    )));
                }
            };
            Ok(MechanicalLand {
                name: if name.is_empty() {
                    (k + 1).to_string()
                } else {
                    name
                },
                at: pad.position,
                shape,
            })
        })
        .collect()
}

/// `layout.place_part_file`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlacePartFileParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// Literal values by the part's own parameter names.
    #[serde(default)]
    pub values: PartValues,
    /// Board position of the part origin, mm.
    pub position: Point,
    #[serde(default)]
    pub rotation_deg: f64,
    /// `top` (default) or `bottom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<Layer>,
    /// `mechanical` (default): pads land no terminal; `terminals`: each
    /// copper land its own terminal.
    #[serde(default)]
    pub pads: PadUse,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bom: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_and_place: Option<bool>,
    /// Explicit lands (footprint frame, mm); replaces a built-in's rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lands: Option<Vec<MechanicalLand>>,
    /// Copy the lands from this project footprint's pads instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lands_from: Option<FootprintRef>,
    /// The part's name (the built-in's title or the file name otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The designator prefix (the built-in's, else `MP`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designator: Option<String>,
    /// A file's licence (SPDX), required for `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redistribution: Option<Redistribution>,
    pub expected_revision: u64,
}

impl PlacePartFileParams {
    /// A placement of `source` at `position` with every default.
    pub fn new(source: PartFileSource, position: Point, expected_revision: u64) -> Self {
        let (path, builtin, model) = match source {
            PartFileSource::Path(p) => (Some(p), None, None),
            PartFileSource::Builtin(b) => (None, Some(b), None),
            PartFileSource::Model(m) => (None, None, Some(m)),
        };
        Self {
            path,
            builtin,
            model,
            values: PartValues::new(),
            position,
            rotation_deg: 0.0,
            layer: None,
            pads: PadUse::Mechanical,
            bom: None,
            pick_and_place: None,
            lands: None,
            lands_from: None,
            name: None,
            prefix: None,
            designator: None,
            licence: None,
            redistribution: None,
            expected_revision,
        }
    }

    pub fn source(&self) -> Result<PartFileSource> {
        match (&self.path, &self.builtin, &self.model) {
            (Some(p), None, None) => Ok(PartFileSource::Path(p.clone())),
            (None, Some(b), None) => Ok(PartFileSource::Builtin(b.clone())),
            (None, None, Some(m)) => Ok(PartFileSource::Model(*m)),
            _ => Err(invalid(
                "part_file.read: give exactly one of path, builtin and model".into(),
            )),
        }
    }
}

/// `layout.set_part_file_values`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPartFileValuesParams {
    pub instance: Id,
    /// The new values (they replace the previous ones).
    #[serde(default)]
    pub values: PartValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pads: Option<PadUse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bom: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_and_place: Option<bool>,
    pub expected_revision: u64,
}

fn model_source(
    engine: &Engine,
    read: &ReadPart,
    hash: ContentHash,
    licence: Option<&str>,
    redistribution: Option<Redistribution>,
    part: &EvaluatedPart,
) -> Result<PartModelSource> {
    if let Some(model) = read.model {
        return Ok(PartModelSource::Model { model });
    }
    let library = &engine.project().library;
    if let Some(model) = library
        .models
        .iter()
        .filter(|m| {
            m.definition.format == ModelFormat::Solidcraft && m.definition.asset.hash == hash
        })
        .max_by_key(|m| (m.id, m.revision))
        .map(|m| ModelRef::new(m.id, m.revision))
    {
        return Ok(PartModelSource::Model { model });
    }
    let builtin = read.builtin.is_some();
    let licence = if builtin {
        BUILTIN_LICENCE.to_owned()
    } else {
        licence
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                invalid("part_file.licence: a part file needs its licence stated".into())
            })?
    };
    // The model's own bounds: the file as it is.
    let as_is = if part.parameters.iter().any(|p| p.given) {
        checked(&read.bytes, &PartValues::new())?.bounds_mm
    } else {
        part.bounds_mm
    };
    let definition = ModelDefinition {
        asset: AssetRef {
            hash,
            bytes: u64::try_from(read.bytes.len()).unwrap_or(u64::MAX),
            media_type: MEDIA_TYPE.into(),
            file_name: read.file_name.clone(),
        },
        format: ModelFormat::Solidcraft,
        bounds_mm: as_is,
        provenance: ModelProvenance {
            redistribution: if builtin {
                Redistribution::Permitted
            } else {
                redistribution.unwrap_or_default()
            },
            record: Provenance {
                origin: None,
                basis: if builtin {
                    Basis::Authored
                } else {
                    Basis::Imported
                },
                licence: Some(licence),
            },
            ..ModelProvenance::default()
        },
        ..ModelDefinition::default()
    };
    Ok(PartModelSource::File {
        name: read.file_name.trim_end_matches(".solidcraft").to_owned(),
        definition: Box::new(definition),
        asset: PackedAsset {
            hash,
            media_type: MEDIA_TYPE.into(),
            data: AssetBytes::new(read.bytes.clone()),
        },
    })
}

/// The core `layout.place_part` for a request, and the check report.
pub fn place_part_operation(
    engine: &Engine,
    params: &PlacePartFileParams,
) -> Result<(Operation, PartReport)> {
    let read = read_part(engine, &params.source()?)?;
    let part = checked(&read.bytes, &params.values)?;
    let lands = match (&params.lands, params.lands_from) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                "part.lands: give lands or lands_from, not both".into(),
            ));
        }
        (Some(lands), None) => lands.clone(),
        (None, Some(footprint)) => footprint_lands(engine, footprint)?,
        (None, None) => match read.builtin {
            Some(b) => rule_lands(&part, b.lands)?,
            None => Vec::new(),
        },
    };
    let hash = asset_hash(&read.bytes);
    let model = model_source(
        engine,
        &read,
        hash,
        params.licence.as_deref(),
        params.redistribution,
        &part,
    )?;
    let spec = PartSpec {
        name: params
            .name
            .clone()
            .or_else(|| read.builtin.map(|b| b.title.to_owned()))
            .unwrap_or_else(|| read.file_name.trim_end_matches(".solidcraft").to_owned()),
        designator_prefix: params
            .prefix
            .clone()
            .or_else(|| read.builtin.map(|b| b.prefix.to_owned()))
            .unwrap_or_else(|| DEFAULT_PREFIX.to_owned()),
        source: match read.builtin {
            Some(b) => b.name.to_owned(),
            None => read.file_name.clone(),
        },
        model,
        values: params.values.clone(),
        derived: part_file::derived(&part),
        lands,
        pads: params.pads,
        bom: params.bom.unwrap_or(true),
        pick_and_place: params.pick_and_place,
    };
    let operation = Operation::Layout(LayoutCommand::PlacePart {
        part: Box::new(spec),
        position: params.position,
        rotation_deg: params.rotation_deg,
        layer: params.layer.unwrap_or(Layer::Top),
        designator: params.designator.clone(),
    });
    Ok((operation, part.report()))
}

/// What a placed mechanical part was made from.
#[derive(Clone, Debug)]
pub struct PlacedPart {
    pub record: mechanical::MechanicalPart,
    pub model: ModelRef,
    pub values: PartValues,
    pub lands: Vec<MechanicalLand>,
    pub prefix: String,
}

/// The mechanical part an instance is, with its current values and lands.
pub fn placed_part(engine: &Engine, instance: Id) -> Result<PlacedPart> {
    let project = engine.project();
    let found = project
        .instance(instance)
        .ok_or_else(|| invalid(format!("part.instance: no instance {instance}")))?;
    let library = &project.library;
    let refs = mechanical::variant_refs(library, found.component).ok_or_else(|| {
        invalid(format!(
            "part.not_mechanical: {} is not a part placed from a part file",
            found.designator
        ))
    })?;
    let component = library
        .component(refs.component)
        .ok_or_else(|| invalid("part.instance: the component is missing".into()))?;
    let package = library
        .package(refs.package)
        .ok_or_else(|| invalid("part.instance: the package is missing".into()))?;
    let record = package
        .definition
        .mechanical
        .as_deref()
        .cloned()
        .ok_or_else(|| invalid("part.not_mechanical: no mechanical record".into()))?;
    let PackageModel::PartFile(file) = &package.definition.model else {
        return Err(invalid(
            "part.not_mechanical: the package does not use a part file".into(),
        ));
    };
    let lands = footprint_lands(engine, refs.footprint)?;
    Ok(PlacedPart {
        record,
        model: file.model,
        values: file.values.clone(),
        lands,
        prefix: component.definition.designator_prefix.clone(),
    })
}

/// The core `layout.set_part` for a request, and the check report.
pub fn set_part_operation(
    engine: &Engine,
    params: &SetPartFileValuesParams,
) -> Result<(Operation, PartReport)> {
    let placed = placed_part(engine, params.instance)?;
    let read = read_part(engine, &PartFileSource::Model(placed.model))?;
    let part = checked(&read.bytes, &params.values)?;
    let builtin = builtin_part(&placed.record.source)
        .filter(|_| !placed.record.source.ends_with(".solidcraft"));
    let lands = match builtin {
        Some(b) => rule_lands(&part, b.lands)?,
        None => placed.lands.clone(),
    };
    let had_copper = placed.lands.iter().any(|l| l.shape.copper());
    let spec = PartSpec {
        name: placed.record.name.clone(),
        designator_prefix: placed.prefix.clone(),
        source: placed.record.source.clone(),
        model: PartModelSource::Model {
            model: placed.model,
        },
        values: params.values.clone(),
        derived: part_file::derived(&part),
        lands,
        pads: params.pads.unwrap_or(placed.record.pads),
        bom: params.bom.unwrap_or(placed.record.bom),
        // Keep an explicit choice; the default follows the lands.
        pick_and_place: params.pick_and_place.or_else(|| {
            (placed.record.pick_and_place != had_copper).then_some(placed.record.pick_and_place)
        }),
    };
    let operation = Operation::Layout(LayoutCommand::SetPart {
        instance: params.instance,
        part: Box::new(spec),
    });
    Ok((operation, part.report()))
}

impl Session {
    pub(super) fn part_files(&self) -> Result<Value> {
        let mut builtin = Vec::new();
        for (name, bytes) in part_file::BUILTIN {
            let data = builtin_part(name);
            let report = part_file::evaluate(bytes, &PartValues::new())
                .ok()
                .map(|p| p.report());
            builtin.push(json!({
                "name": name,
                "title": data.map_or(*name, |b| b.title),
                "prefix": data.map_or(DEFAULT_PREFIX, |b| b.prefix),
                "kind": match data {
                    Some(b) if b.mechanical && !b.lands.is_empty() => "pads",
                    Some(b) if b.mechanical => "mechanical",
                    _ => "package",
                },
                "parameters": report.as_ref().map(|r| r.parameters.clone()).unwrap_or_default(),
                "licence": BUILTIN_LICENCE,
            }));
        }
        let project: Vec<Value> = self
            .engine
            .project()
            .library
            .models
            .iter()
            .filter(|m| m.definition.format == ModelFormat::Solidcraft)
            .map(|m| {
                json!({
                    "model": ModelRef::new(m.id, m.revision),
                    "name": m.meta.name,
                    "file": m.definition.asset.file_name,
                    "licence": m.definition.provenance.record.licence,
                })
            })
            .collect();
        Ok(json!({"builtin": builtin, "project": project}))
    }

    pub(super) fn place_part_file(&mut self, params: PlacePartFileParams) -> Result<Value> {
        let (operation, report) = place_part_operation(&self.engine, &params)?;
        let mut reply = self.apply(params.expected_revision, operation)?;
        reply["part"] = serde_json::to_value(report)?;
        Ok(reply)
    }

    pub(super) fn set_part_file_values(
        &mut self,
        params: SetPartFileValuesParams,
    ) -> Result<Value> {
        let (operation, report) = set_part_operation(&self.engine, &params)?;
        let mut reply = self.apply(params.expected_revision, operation)?;
        reply["part"] = serde_json::to_value(report)?;
        Ok(reply)
    }
}

#[cfg(test)]
#[path = "mechanical_parts_tests.rs"]
mod tests;
