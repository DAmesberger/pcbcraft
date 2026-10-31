//! Physical materials of package bodies: the one built-in table that 3D
//! models, the product look and the saved model's opaque projection read.
//!
//! A [`MaterialId`] names a physical material, never a colour or a vendor
//! look. Optical values follow `docs/materials.md`: linear-sRGB reflectance,
//! metallic/roughness/IOR in Solidcraft's `OpaqueSurface` convention, each
//! value cited or marked estimated. Materials are never inferred from names.
//!
//! The table is app data revisioned with [`BUILTIN_REVISION`]: a project
//! stores only [`MaterialId`]s, and the saved `.solidcraft` model carries
//! each body's opaque projection, so a project reopens with the same native
//! looks. User materials, once they exist, become a revisioned library
//! definition kind and are snapshotted like any other definition.
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A built-in physical material, revisioned with the app
/// ([`BUILTIN_REVISION`]).
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

/// Revision of the built-in table; bumped whenever a value changes.
pub const BUILTIN_REVISION: u16 = 1;

/// Solidcraft's lowest perceptual roughness.
pub const MIN_ROUGHNESS: f64 = 0.045;

/// Metals ignore the dielectric IOR; Solidcraft still requires one in
/// [1, 3], so metals store 1.5.
const METAL_IOR: f64 = 1.5;
/// Silica-filled epoxy (filler n ≈ 1.46, resin ≈ 1.57; Palik, CRC).
const EPOXY_IOR: f64 = 1.55;

/// Bare copper F0 (Johnson & Christy 1972, via Hoffman's table).
const COPPER: [f64; 3] = [0.955, 0.638, 0.538];
/// Gold F0 (Johnson & Christy 1972, via Hoffman's table).
const GOLD: [f64; 3] = [1.000, 0.782, 0.344];
/// Nickel F0 (Rakić 1998).
const NICKEL: [f64; 3] = [0.660, 0.609, 0.526];
/// Tin F0 of a reflowed or plated surface (est.; below polished tin for
/// its oxide film). The same value `docs/materials.md` gives terminations.
const TIN: [f64; 3] = [0.64, 0.64, 0.66];
/// Aluminium F0 (Hoffman's table).
const ALUMINIUM: [f64; 3] = [0.913, 0.922, 0.924];
/// Brass F0 (Hoffman's table).
const BRASS: [f64; 3] = [0.910, 0.778, 0.423];

/// The reference key of an estimated value.
pub const EST: &str = "est";

fn source(reference: &str, note: &str) -> SourceNote {
    SourceNote {
        reference: reference.into(),
        note: note.into(),
    }
}

/// A material's values before its identity is attached.
struct Values {
    name: &'static str,
    base_linear: [f64; 3],
    metallic: f64,
    roughness: f64,
    ior: f64,
    coat: Option<Coat>,
    transmission: Option<Transmission>,
    pattern: Option<SurfacePattern>,
    density_kg_m3: Option<f64>,
    source: Vec<SourceNote>,
}

fn metal(name: &'static str, base_linear: [f64; 3], roughness: f64) -> Values {
    Values {
        name,
        base_linear,
        metallic: 1.0,
        roughness,
        ior: METAL_IOR,
        coat: None,
        transmission: None,
        pattern: None,
        density_kg_m3: None,
        source: Vec::new(),
    }
}

fn dielectric(name: &'static str, base_linear: [f64; 3], roughness: f64, ior: f64) -> Values {
    Values {
        metallic: 0.0,
        ior,
        ..metal(name, base_linear, roughness)
    }
}

impl Values {
    fn density(mut self, kg_m3: f64) -> Self {
        self.density_kg_m3 = Some(kg_m3);
        self
    }

    fn cite(mut self, reference: &str, note: &str) -> Self {
        self.source.push(source(reference, note));
        self
    }

    fn pattern(mut self, kind: PatternKind, strength: f64) -> Self {
        self.pattern = Some(SurfacePattern { kind, strength });
        self
    }

    fn transmission(mut self, weight: f64, attenuation_linear: [f64; 3], distance_mm: f64) -> Self {
        self.transmission = Some(Transmission {
            weight,
            attenuation_linear,
            distance_mm,
        });
        self
    }
}

fn values(id: MaterialId) -> Values {
    use MaterialId::*;
    match id {
        EpoxyMould => dielectric("Epoxy mould compound", [0.022, 0.022, 0.024], 0.45, EPOXY_IOR)
            .pattern(PatternKind::EdmTexture, 1.0)
            .density(1950.0)
            .cite("palik-crc", "est. colour: carbon-black pigmented, silica-filled epoxy; n from the filler and resin indices")
            .cite("est", "EDM mould texture; density typical of 80–90 % silica-filled compounds"),
        EpoxyLaserMarked => dielectric("Laser-marked epoxy", [0.10, 0.10, 0.105], 0.75, EPOXY_IOR)
            .density(1950.0)
            .cite("est", "laser ablation exposes lighter silica filler and roughens the surface"),
        MarkingInkWhite => dielectric("White marking ink", [0.80, 0.80, 0.78], 0.60, 1.50)
            .cite("est", "TiO2-pigmented epoxy ink, as silkscreen legend ink"),
        ResistorOvercoat => dielectric("Resistor overcoat", [0.025, 0.025, 0.027], 0.30, EPOXY_IOR)
            .cite("est", "glass or epoxy overcoat with a black marking layer (docs/materials.md)"),
        Alumina96 => dielectric("96 % alumina", [0.78, 0.77, 0.73], 0.55, 1.77)
            .density(3720.0)
            .cite("palik-crc", "n from sapphire, n_o = 1.77")
            .cite("est", "reflectance of thick-film substrates, 0.75–0.85 across 400–700 nm (published substrate data)"),
        BariumTitanate => dielectric("Barium titanate ceramic", [0.32, 0.25, 0.15], 0.55, 2.4)
            .density(6020.0)
            .cite("palik-crc", "n ≈ 2.4; density of BaTiO3")
            .cite("est", "tan colour of sintered MLCC bodies"),
        Ferrite => dielectric("Ferrite", [0.045, 0.045, 0.047], 0.60, 2.0)
            .density(4800.0)
            .cite("est", "dark grey sintered Mn-Zn/Ni-Zn ferrite; n and density estimated"),
        TinMatte => metal("Matte tin", TIN, 0.28)
            .density(7265.0)
            .cite("palik-crc", "density of beta tin; Palik's Sn n, k to confirm the F0")
            .cite("est", "F0 below polished tin for its oxide film; electroplated matte tin is rough"),
        TinBright => metal("Bright tin", TIN, 0.14)
            .density(7265.0)
            .cite("palik-crc", "density of beta tin")
            .cite("est", "same F0 as matte tin, a smoother brightened deposit"),
        SolderSac => metal("SAC solder", TIN, 0.14)
            .density(7400.0)
            .cite("est", "SAC alloys are about 96 % Sn, so the Sn F0; a reflowed meniscus freezes smooth"),
        Gold => metal("Gold plating", GOLD, 0.22)
            .density(19300.0)
            .cite("johnson-christy-1972", "Au F0 via Hoffman's table")
            .cite("est", "roughness of hard gold over nickel"),
        Nickel => metal("Nickel", NICKEL, 0.30)
            .density(8908.0)
            .cite("rakic-1998", "Ni F0")
            .cite("est", "roughness of electroplated nickel"),
        CopperAlloy => metal("Copper-alloy leadframe", COPPER, 0.32)
            .density(8910.0)
            .cite("johnson-christy-1972", "Cu F0 via Hoffman's table; C194-like alloys are over 97 % Cu")
            .cite("est", "roughness of rolled and etched leadframe strip"),
        PhosphorBronze => metal("Phosphor bronze", [0.90, 0.70, 0.52], 0.30)
            .density(8860.0)
            .cite("est", "Cu with about 5 % Sn: copper F0 shifted toward tin"),
        Brass => metal("Brass", BRASS, 0.28)
            .density(8500.0)
            .cite("hoffman", "brass F0")
            .cite("est", "roughness of a turned or rolled surface"),
        Aluminium => metal("Aluminium", ALUMINIUM, 0.30)
            .density(2700.0)
            .cite("hoffman", "Al F0; CRC density")
            .cite("est", "roughness of a drawn aluminium can"),
        PvcSleeve => dielectric("PVC sleeve", [0.020, 0.045, 0.16], 0.35, 1.54)
            .density(1380.0)
            .cite("palik-crc", "n of PVC ≈ 1.54")
            .cite("est", "pigmented heat-shrink sleeve colour; density typical of rigid PVC"),
        BorosilicateGlass => dielectric("Borosilicate glass", [0.85, 0.88, 0.86], 0.05, 1.47)
            .transmission(1.0, [0.92, 0.95, 0.93], 1.0)
            .density(2230.0)
            .cite("palik-crc", "n_d ≈ 1.47 and density of borosilicate glass")
            .cite("est", "slight green attenuation of diode glass; opaque tint until a layered saved appearance exists"),
        Pbt => dielectric("PBT", [0.030, 0.030, 0.032], 0.50, 1.57)
            .density(1530.0)
            .cite("est", "black glass-filled PBT housing; n ≈ 1.57 and density typical of 30 % glass-filled grades"),
        Lcp => dielectric("LCP", [0.025, 0.025, 0.028], 0.35, 1.60)
            .density(1620.0)
            .cite("est", "black glass-filled LCP housing; n and density typical"),
        Pa9t => dielectric("PA9T", [0.030, 0.030, 0.030], 0.45, 1.53)
            .density(1450.0)
            .cite("est", "black glass-filled PA9T housing; n and density typical"),
        SiliconeClear => dielectric("Clear silicone", [0.90, 0.90, 0.90], 0.08, 1.41)
            .transmission(1.0, [0.97, 0.97, 0.96], 1.0)
            .density(1030.0)
            .cite("palik-crc", "n of PDMS ≈ 1.41")
            .cite("est", "attenuation of optical-grade encapsulant"),
        SiliconeDiffuse => dielectric("Diffuse silicone", [0.75, 0.75, 0.72], 0.40, 1.41)
            .transmission(0.3, [0.80, 0.80, 0.78], 0.5)
            .density(1100.0)
            .cite("palik-crc", "n of PDMS ≈ 1.41")
            .cite("est", "scattering filler: a white, partly transmitting body"),
        Polyimide => dielectric("Polyimide", [0.40, 0.15, 0.02], 0.25, 1.70)
            .density(1420.0)
            .cite("palik-crc", "n ≈ 1.70 and density of polyimide film")
            .cite("est", "amber colour of polyimide film"),
    }
}

/// The built-in definition of one material. Every value is cited or marked
/// estimated (`SourceNote { reference: "est" }`); keys are the reference
/// list of `docs/materials.md`.
pub fn builtin(id: MaterialId) -> Result<MaterialDef> {
    let v = values(id);
    let def = MaterialDef {
        id,
        name: v.name.into(),
        category: id.category(),
        base_linear: v.base_linear,
        metallic: v.metallic,
        roughness: v.roughness,
        ior: v.ior,
        coat: v.coat,
        transmission: v.transmission,
        pattern: v.pattern,
        density_kg_m3: v.density_kg_m3,
        estimated: v.source.iter().any(|s| s.reference == EST),
        source: v.source,
    };
    def.validate()?;
    Ok(def)
}

/// Every built-in definition, in [`MaterialId::ALL`] order.
pub fn all() -> Result<Vec<MaterialDef>> {
    MaterialId::ALL.into_iter().map(builtin).collect()
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
                "material.invalid: {:?} has a value outside the opaque-surface bounds",
                self.id
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
        assert_eq!((tin.base_linear, tin.roughness), ([0.64, 0.64, 0.66], 0.28));
        assert_eq!(builtin(MaterialId::SolderSac)?.roughness, 0.14);
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
                assert_eq!(def.ior, METAL_IOR);
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
