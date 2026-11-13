//! Physical materials of package bodies: the one table that 3D models, the
//! product look and the saved model's opaque projection read, and (in
//! `board`) the board's looks, finishes and display-layer sizes.
//!
//! A [`MaterialId`] names a physical material, never a colour or a vendor
//! look. Optical values follow `docs/materials.md`: linear-sRGB reflectance,
//! metallic/roughness/IOR in Solidcraft's `OpaqueSurface` convention, each
//! value cited or marked estimated. Materials are never inferred from names.
//!
//! The values are shipped data (`materials/` in the shipped library, see
//! `docs/shipped-library.md`), loaded once by `sparksmith-io` into a
//! [`MaterialLibrary`] and handed in: the core holds the types, the bounds
//! and the identities, never the values. [`MaterialId`] stays a closed set
//! in code because package builders, markings and saved projects name
//! materials by it. The table is revisioned with [`BUILTIN_REVISION`]: a project
//! stores only [`MaterialId`]s, and the saved `.solidcraft` model carries
//! each body's opaque projection, so a project reopens with the same native
//! looks. User materials, once they exist, become a revisioned library
//! definition kind and are snapshotted like any other definition.
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod board;
pub use board::{
    AppearanceSizes, BoardLooks, Finishes, LegendInk, Look, Mask, MaskPigment, PadFinishDef,
    PerInk, PerMaskColour, PerMaskFinish, PerPadFinish, Plating, Size, Thickness,
};

/// A physical material of the shipped table ([`BUILTIN_REVISION`]).
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
    /// Metal (a nickel-silver shield can) where a laser has marked it.
    MetalLaserMarked,
    /// Pad-printed marking ink (white; a chosen colour replaces it).
    PadPrintInk,
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
    /// PBT plastic (connector bodies, headers).
    Pbt,
    /// LCP plastic (high-temperature connector bodies).
    Lcp,
    Pa9t,
    SiliconeClear,
    SiliconeDiffuse,
    Polyimide,
    /// Nickel silver (CuNiZn) shield can of a radio module.
    NickelSilver,
    /// Green soldermask over FR-4: a module board's face.
    SolderMaskGreen,
    /// Green soldermask over copper: a module's printed antenna trace.
    SolderMaskOverCopper,
}

impl MaterialId {
    pub const ALL: [Self; 29] = [
        Self::EpoxyMould,
        Self::EpoxyLaserMarked,
        Self::MarkingInkWhite,
        Self::MetalLaserMarked,
        Self::PadPrintInk,
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
        Self::NickelSilver,
        Self::SolderMaskGreen,
        Self::SolderMaskOverCopper,
    ];

    /// The id as files and messages write it (`epoxy_mould`).
    pub fn key(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    pub fn category(self) -> MaterialCategory {
        use MaterialId::*;
        match self {
            TinMatte | TinBright | SolderSac | Gold | Nickel | CopperAlloy | PhosphorBronze
            | Brass | Aluminium | NickelSilver | MetalLaserMarked => MaterialCategory::Metal,
            Alumina96 | BariumTitanate | Ferrite => MaterialCategory::Ceramic,
            BorosilicateGlass => MaterialCategory::Glass,
            EpoxyMould | EpoxyLaserMarked | MarkingInkWhite | PadPrintInk | ResistorOvercoat
            | PvcSleeve | Pbt | Lcp | Pa9t | SiliconeClear | SiliconeDiffuse | Polyimide
            | SolderMaskGreen | SolderMaskOverCopper => MaterialCategory::Polymer,
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

/// The material table revision this build reads: the shipped
/// `materials/package-materials.json` must state it, and a project records
/// it. Bumped together with the data whenever a value changes.
pub const BUILTIN_REVISION: u16 = 1;

/// Solidcraft's lowest perceptual roughness.
pub const MIN_ROUGHNESS: f64 = 0.045;

/// The reference key of an estimated value.
pub const EST: &str = "est";

/// One material as the shipped table states it: a [`MaterialDef`] without
/// what follows from its id (category) or its sources (estimated).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct MaterialRecord {
    id: MaterialId,
    name: String,
    base_linear: [f64; 3],
    metallic: f64,
    roughness: f64,
    ior: f64,
    #[serde(default)]
    coat: Option<Coat>,
    #[serde(default)]
    transmission: Option<Transmission>,
    #[serde(default)]
    pattern: Option<SurfacePattern>,
    #[serde(default)]
    density_kg_m3: Option<f64>,
    source: Vec<SourceNote>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TableFile {
    #[serde(default, rename = "about")]
    _about: String,
    revision: u16,
    materials: Vec<MaterialRecord>,
}

/// The package material table, one valid definition per [`MaterialId`] in
/// [`MaterialId::ALL`] order. It is shipped data (`materials/` in the
/// shipped library), loaded by `sparksmith-io` and handed in; the core
/// holds no values of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialTable {
    defs: Vec<MaterialDef>,
}

impl MaterialTable {
    /// The table in `text` (the shipped JSON). Refuses another revision, a
    /// missing, repeated or unknown material, and any value outside
    /// [`MaterialDef::validate`]'s bounds.
    pub fn from_json(text: &str) -> Result<Self> {
        let file: TableFile = serde_json::from_str(text)?;
        if file.revision != BUILTIN_REVISION {
            return Err(Error::Unsupported(format!(
                "material table revision {} (this build reads revision {BUILTIN_REVISION})",
                file.revision
            )));
        }
        let mut defs = Vec::with_capacity(MaterialId::ALL.len());
        for id in MaterialId::ALL {
            let mut found = file.materials.iter().filter(|r| r.id == id);
            let (Some(record), None) = (found.next(), found.next()) else {
                return Err(Error::Invalid(format!(
                    "material.table: {} must be defined exactly once",
                    id.key()
                )));
            };
            let def = MaterialDef {
                id,
                name: record.name.clone(),
                category: id.category(),
                base_linear: record.base_linear,
                metallic: record.metallic,
                roughness: record.roughness,
                ior: record.ior,
                coat: record.coat,
                transmission: record.transmission,
                pattern: record.pattern,
                density_kg_m3: record.density_kg_m3,
                estimated: record.source.iter().any(|s| s.reference == EST),
                source: record.source.clone(),
            };
            def.validate()?;
            defs.push(def);
        }
        let names: std::collections::BTreeSet<_> = defs.iter().map(|d| d.name.as_str()).collect();
        if names.len() != defs.len() || file.materials.len() != defs.len() {
            return Err(Error::Invalid(
                "material.table: material names must be unique".into(),
            ));
        }
        Ok(Self { defs })
    }

    /// The definition of `id`.
    pub fn get(&self, id: MaterialId) -> Result<&MaterialDef> {
        self.defs
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| Error::NotFound(format!("material.table: no {}", id.key())))
    }

    /// Every definition, in [`MaterialId::ALL`] order.
    pub fn all(&self) -> &[MaterialDef] {
        &self.defs
    }
}

/// Everything the shipped `materials/` folder holds: package materials,
/// board and finish looks, film thicknesses and display-layer sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialLibrary {
    pub packages: MaterialTable,
    pub board: BoardLooks,
    pub finishes: Finishes,
    pub appearance: AppearanceSizes,
}

impl MaterialLibrary {
    /// The files of the `materials/` shelf this library is made of.
    pub const FILES: [&'static str; 4] = [
        "package-materials.json",
        "board-looks.json",
        "finishes.json",
        "appearance.json",
    ];

    /// The library from the shelf's files, each read through `read` (given
    /// a name of [`Self::FILES`]). The core reads no files: the caller
    /// does. An error names the file it is about.
    pub fn load(mut read: impl FnMut(&str) -> Result<String>) -> Result<Self> {
        let [packages, board, finishes, appearance] = Self::FILES;
        let within =
            |name: &str, error: Error| Error::Invalid(format!("materials/{name}: {error}"));
        let mut text = |name: &str| read(name).map_err(|e| within(name, e));
        let packages_text = text(packages)?;
        let board_text = text(board)?;
        let finishes_text = text(finishes)?;
        let appearance_text = text(appearance)?;
        Ok(Self {
            packages: MaterialTable::from_json(&packages_text).map_err(|e| within(packages, e))?,
            board: BoardLooks::from_json(&board_text).map_err(|e| within(board, e))?,
            finishes: Finishes::from_json(&finishes_text).map_err(|e| within(finishes, e))?,
            appearance: AppearanceSizes::from_json(&appearance_text)
                .map_err(|e| within(appearance, e))?,
        })
    }
}

impl MaterialDef {
    /// The values Solidcraft's opaque surface accepts, plus the bounds of
    /// the optional layers. A definition outside them is refused.
    pub fn validate(&self) -> Result<()> {
        let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        let rough = |v: f64| v.is_finite() && (MIN_ROUGHNESS..=1.0).contains(&v);
        let ior = |v: f64| v.is_finite() && (1.0..=3.0).contains(&v);
        let mut ok = self.base_linear.iter().all(|c| unit(*c))
            && unit(self.metallic)
            && rough(self.roughness)
            && ior(self.ior)
            && self.category == self.id.category()
            && (self.metallic == 1.0) == (self.category == MaterialCategory::Metal)
            && !self.name.trim().is_empty()
            && !self.source.is_empty();
        if let Some(c) = self.coat {
            ok &= unit(c.weight) && rough(c.roughness) && ior(c.ior);
        }
        if let Some(t) = self.transmission {
            ok &= unit(t.weight)
                && t.attenuation_linear.iter().all(|c| unit(*c))
                && t.distance_mm.is_finite()
                && t.distance_mm > 0.0
                && self.metallic == 0.0;
        }
        if let Some(p) = self.pattern {
            ok &= unit(p.strength);
        }
        if let Some(d) = self.density_kg_m3 {
            ok &= d.is_finite() && (1.0..=25000.0).contains(&d);
        }
        if ok {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "material.invalid: material ‘{}’ has a value outside the opaque-surface bounds",
                self.name
            )))
        }
    }

    /// Normal-incidence reflectance of a dielectric, ((n − 1)/(n + 1))².
    pub fn dielectric_f0(&self) -> f64 {
        let r = (self.ior - 1.0) / (self.ior + 1.0);
        r * r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped table, which these tests check.
    fn table() -> Result<MaterialTable> {
        MaterialTable::from_json(include_str!(
            "../../../library/materials/package-materials.json"
        ))
    }

    fn all() -> Result<Vec<MaterialDef>> {
        Ok(table()?.all().to_vec())
    }

    fn builtin(id: MaterialId) -> Result<MaterialDef> {
        table()?.get(id).cloned()
    }

    #[test]
    fn the_shipped_library_loads_and_names_a_broken_file() -> Result<()> {
        let shipped = |name: &str| -> Result<String> {
            Ok(match name {
                "package-materials.json" => {
                    include_str!("../../../library/materials/package-materials.json")
                }
                "board-looks.json" => include_str!("../../../library/materials/board-looks.json"),
                "finishes.json" => include_str!("../../../library/materials/finishes.json"),
                "appearance.json" => include_str!("../../../library/materials/appearance.json"),
                other => return Err(Error::NotFound(other.into())),
            }
            .to_owned())
        };
        let library = MaterialLibrary::load(shipped)?;
        // ENIG: nickel 3–6 µm under gold 0.025–0.05 µm, drawn at the middle.
        let enig = library.finishes.pad.enig.thickness.mid_um();
        assert_eq!(enig, ((3.0 + 0.025) + (6.0 + 0.05)) / 2.0);
        assert_eq!(library.finishes.legend_ink.thickness_mm, 0.015);
        assert_eq!(library.appearance.tent_sag_mm.value, 0.012);
        assert_eq!(library.board.mask.colours.green.t20, [0.10, 0.45, 0.18]);
        // A file with an unknown field, or missing, is refused by name.
        let broken = MaterialLibrary::load(|name| {
            let text = shipped(name)?;
            Ok(if name == "appearance.json" {
                text.replacen("\"tent_sag_mm\"", "\"tent_sagg_mm\"", 1)
            } else {
                text
            })
        });
        assert!(matches!(broken, Err(Error::Invalid(m)) if m.contains("appearance.json")));
        let missing = MaterialLibrary::load(|name| {
            if name == "finishes.json" {
                Err(Error::NotFound("no such file".into()))
            } else {
                shipped(name)
            }
        });
        assert!(matches!(missing, Err(Error::Invalid(m)) if m.contains("finishes.json")));
        Ok(())
    }

    #[test]
    fn a_table_of_another_revision_or_with_a_material_missing_is_refused() {
        let text = include_str!("../../../library/materials/package-materials.json");
        let other = text.replacen("\"revision\": 1", "\"revision\": 2", 1);
        assert!(matches!(
            MaterialTable::from_json(&other),
            Err(Error::Unsupported(_))
        ));
        let renamed = text.replacen("\"id\": \"ferrite\"", "\"id\": \"nickel\"", 1);
        assert!(MaterialTable::from_json(&renamed).is_err());
    }

    #[test]
    fn every_material_is_defined_valid_and_sourced() -> Result<()> {
        let all = all()?;
        assert_eq!(all.len(), MaterialId::ALL.len());
        for (def, id) in all.iter().zip(MaterialId::ALL) {
            assert_eq!(def.id, id);
            def.validate()?;
            // An estimate is always labelled; a cited value names a source.
            assert!(
                def.source
                    .iter()
                    .all(|s| !s.reference.is_empty() && !s.note.is_empty())
            );
            assert_eq!(
                def.estimated,
                def.source.iter().any(|s| s.reference == "est"),
                "{id:?}"
            );
        }
        let names: std::collections::BTreeSet<_> = all.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names.len(), all.len(), "names are unique");
        Ok(())
    }

    #[test]
    fn cited_values_match_docs() -> Result<()> {
        // Measured metal F0 (docs/materials.md sources).
        assert_eq!(
            builtin(MaterialId::Gold)?.base_linear,
            [1.000, 0.782, 0.344]
        );
        assert_eq!(
            builtin(MaterialId::Nickel)?.base_linear,
            [0.660, 0.609, 0.526]
        );
        assert_eq!(
            builtin(MaterialId::CopperAlloy)?.base_linear,
            [0.955, 0.638, 0.538]
        );
        // The values the integration crate's envelope looks use today.
        let epoxy = builtin(MaterialId::EpoxyMould)?;
        assert_eq!(
            (epoxy.base_linear, epoxy.roughness, epoxy.ior),
            ([0.022, 0.022, 0.024], 0.45, 1.55)
        );
        let alumina = builtin(MaterialId::Alumina96)?;
        assert_eq!(
            (alumina.base_linear, alumina.roughness, alumina.ior),
            ([0.78, 0.77, 0.73], 0.55, 1.77)
        );
        let mlcc = builtin(MaterialId::BariumTitanate)?;
        assert_eq!((mlcc.base_linear, mlcc.ior), ([0.32, 0.25, 0.15], 2.4));
        let tin = builtin(MaterialId::TinMatte)?;
        assert_eq!((tin.base_linear, tin.roughness), ([0.78, 0.78, 0.80], 0.32));
        assert_eq!(builtin(MaterialId::SolderSac)?.roughness, 0.38);
        // n = 1.55 gives F0 ≈ 0.047 (docs/materials.md conventions).
        assert!((epoxy.dielectric_f0() - 0.0465).abs() < 5e-4);
        Ok(())
    }

    #[test]
    fn categories_set_metallic_and_transmission() -> Result<()> {
        for def in all()? {
            let metal = def.category == MaterialCategory::Metal;
            assert_eq!(def.metallic, if metal { 1.0 } else { 0.0 }, "{:?}", def.id);
            if metal {
                assert_eq!(def.ior, 1.5);
                assert!(def.transmission.is_none());
            }
        }
        // Only glass and clear or diffuse silicone transmit.
        let transmitting: Vec<_> = all()?
            .into_iter()
            .filter(|d| d.transmission.is_some())
            .map(|d| d.id)
            .collect();
        assert_eq!(
            transmitting,
            [
                MaterialId::BorosilicateGlass,
                MaterialId::SiliconeClear,
                MaterialId::SiliconeDiffuse
            ]
        );
        Ok(())
    }

    #[test]
    fn out_of_range_definitions_are_refused() -> Result<()> {
        let mut def = builtin(MaterialId::Pbt)?;
        def.roughness = 0.01;
        assert!(def.validate().is_err());
        let mut def = builtin(MaterialId::Pbt)?;
        def.ior = 3.5;
        assert!(def.validate().is_err());
        let mut def = builtin(MaterialId::Gold)?;
        def.metallic = 0.0;
        assert!(def.validate().is_err(), "a metal is metallic");
        let mut def = builtin(MaterialId::Pbt)?;
        def.base_linear[1] = f64::NAN;
        assert!(def.validate().is_err());
        let mut def = builtin(MaterialId::Pbt)?;
        def.source.clear();
        assert!(def.validate().is_err(), "every value is sourced");
        Ok(())
    }
}
