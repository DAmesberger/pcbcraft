//! Physical materials of package bodies: the one built-in table that 3D
//! models, the product look and the saved model's opaque projection read.
//!
//! A [`MaterialId`] names a physical material, never a colour or a vendor
//! look. Optical values follow `docs/materials.md`: linear-sRGB reflectance,
//! metallic/roughness/IOR in Solidcraft's `OpaqueSurface` convention, each
//! value cited or marked estimated. Materials are never inferred from names.
//!
//! M1-M3 contract (`docs/m13-contract.md`): the identifiers, categories and
//! the [`MaterialDef`] shape are fixed here; the cited values are filled by
//! the M1 core task, so [`builtin`] refuses until then.
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A built-in physical material. Revisioned with the app; a project
/// snapshot copies every [`MaterialDef`] it uses.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum MaterialId {
    /// Black epoxy mould compound (EDM-textured).
    EpoxyMould,
    /// Epoxy mould compound where a laser has marked it.
    EpoxyLaserMarked,
    /// White pad-print marking ink.
    MarkingInkWhite,
    /// Black thick-film resistor overcoat.
    ResistorOvercoat,
    /// 96 % alumina substrate.
    Alumina96,
    /// Barium-titanate MLCC ceramic.
    BariumTitanate,
    Ferrite,
    TinMatte,
    TinBright,
    /// SAC solder alloy.
    SolderSac,
    /// ENIG / hard gold plating.
    Gold,
    Nickel,
    /// C194-like copper-alloy leadframe.
    CopperAlloy,
    PhosphorBronze,
    Brass,
    Aluminium,
    PvcSleeve,
    BorosilicateGlass,
    /// PBT plastic (connector housings, headers).
    Pbt,
    /// LCP plastic (high-temperature connector housings).
    Lcp,
    Pa9t,
    SiliconeClear,
    SiliconeDiffuse,
    Polyimide,
}

impl MaterialId {
    pub const ALL: [Self; 24] = [
        Self::EpoxyMould,
        Self::EpoxyLaserMarked,
        Self::MarkingInkWhite,
        Self::ResistorOvercoat,
        Self::Alumina96,
        Self::BariumTitanate,
        Self::Ferrite,
        Self::TinMatte,
        Self::TinBright,
        Self::SolderSac,
        Self::Gold,
        Self::Nickel,
        Self::CopperAlloy,
        Self::PhosphorBronze,
        Self::Brass,
        Self::Aluminium,
        Self::PvcSleeve,
        Self::BorosilicateGlass,
        Self::Pbt,
        Self::Lcp,
        Self::Pa9t,
        Self::SiliconeClear,
        Self::SiliconeDiffuse,
        Self::Polyimide,
    ];

    pub fn category(self) -> MaterialCategory {
        use MaterialId::*;
        match self {
            TinMatte | TinBright | SolderSac | Gold | Nickel | CopperAlloy | PhosphorBronze
            | Brass | Aluminium => MaterialCategory::Metal,
            Alumina96 | BariumTitanate | Ferrite => MaterialCategory::Ceramic,
            BorosilicateGlass => MaterialCategory::Glass,
            EpoxyMould | EpoxyLaserMarked | MarkingInkWhite | ResistorOvercoat | PvcSleeve
            | Pbt | Lcp | Pa9t | SiliconeClear | SiliconeDiffuse | Polyimide => {
                MaterialCategory::Polymer
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialCategory {
    Polymer,
    Ceramic,
    Metal,
    Glass,
}

/// A clear coat over the base layer.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Coat {
    pub weight: f64,
    pub roughness: f64,
    pub ior: f64,
}

/// Real-time transmission (glass, clear silicone). The saved projection is
/// opaque until a layered saved appearance exists (fork step R3).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Transmission {
    pub weight: f64,
    /// Linear attenuation colour at `distance_mm`.
    pub attenuation_linear: [f64; 3],
    pub distance_mm: f64,
}

/// A procedural surface pattern the product look draws per triangle.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfacePattern {
    pub kind: PatternKind,
    pub strength: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatternKind {
    /// Electrical-discharge-machined mould texture.
    EdmTexture,
    /// Fine grain of plated or rolled metal.
    MetalGrain,
}

/// Where a value comes from (`docs/materials.md` reference key).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceNote {
    pub reference: String,
    pub note: String,
}

/// One physical material's optical (and optional mass) definition.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialDef {
    pub id: MaterialId,
    pub name: String,
    pub category: MaterialCategory,
    pub base_linear: [f64; 3],
    pub metallic: f64,
    pub roughness: f64,
    pub ior: f64,
    pub coat: Option<Coat>,
    pub transmission: Option<Transmission>,
    pub pattern: Option<SurfacePattern>,
    pub density_kg_m3: Option<f64>,
    pub source: Vec<SourceNote>,
    /// True when any value is an estimate rather than cited.
    pub estimated: bool,
}

/// The built-in definition of one material.
///
/// Contract stub: the cited table lands with the M1 core task; until then
/// every lookup refuses explicitly rather than inventing values.
pub fn builtin(id: MaterialId) -> Result<MaterialDef> {
    Err(Error::Unsupported(format!(
        "material.unsupported: the built-in values of {id:?} are not defined yet"
    )))
}
