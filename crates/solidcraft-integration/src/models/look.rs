//! [`MaterialId`] → the looks a generated body carries: the opaque PBR
//! projection the native file saves (`appearance.assign`), and the full
//! product look (clear coat, procedural pattern) the 3D view draws per
//! triangle. Display-only parts of the look are never saved.
//!
//! Values come from the core's cited table (`materials::builtin`). Until the
//! M1 core task fills it, [`definition`] falls back to [`provisional`]: the
//! values of `docs/materials.md` where they exist, every other value marked
//! **est.**; the fallback goes when `builtin` stops refusing.
use crate::material::{Coat, Detail, Material, Pattern, linear_to_srgb8};
use crate::native::Look;
use crate::{Error, Result};
use pcbcraft_core::Error as CoreError;
use pcbcraft_core::materials::{self, MaterialCategory, MaterialDef, MaterialId, PatternKind};
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::render::clear_coat::ClearCoat;
use solidcraft_engine::render::surface::OpaqueSurface;

/// Metals ignore the dielectric IOR; Solidcraft still requires one in [1, 3].
const METAL_IOR: f64 = 1.5;

/// The definition of `id`: the core's built-in table, or the provisional
/// values while the table refuses (`material.unsupported`).
pub fn definition(id: MaterialId) -> Result<MaterialDef> {
    match materials::builtin(id) {
        Ok(def) => Ok(def),
        Err(CoreError::Unsupported(message)) if message.starts_with("material.unsupported") => {
            Ok(provisional(id))
        }
        Err(error) => Err(Error::Invalid(error.to_string())),
    }
}

/// The opaque projection a native look stores. Glass and clear silicone
/// are projected as an opaque tint (a saved look refuses opacity ≠ 1).
pub fn saved(def: &MaterialDef) -> Look<'_> {
    let metal = def.category == MaterialCategory::Metal;
    Look {
        name: &def.name,
        srgb8: def.base_linear.map(linear_to_srgb8),
        metallic: def.metallic.clamp(0.0, 1.0),
        roughness: def.roughness.clamp(0.0, 1.0),
        ior: if metal {
            METAL_IOR
        } else {
            def.ior.clamp(1.0, 3.0)
        },
    }
}

/// The product look of `def`: Solidcraft's validated surface with its coat.
pub fn surface(def: &MaterialDef) -> Result<OpaqueSurface> {
    let invalid = |error: solidcraft_engine::render::surface::SurfaceError| {
        Error::Invalid(format!("{}: {error}", def.name))
    };
    let [r, g, b] = def.base_linear;
    let ior = if def.category == MaterialCategory::Metal {
        METAL_IOR
    } else {
        def.ior.clamp(1.0, 3.0)
    };
    let surface = OpaqueSurface::new(Vec3::new(r, g, b), def.metallic, def.roughness, ior)
        .map_err(invalid)?;
    match def.coat {
        Some(c) => {
            Ok(surface
                .with_clear_coat(ClearCoat::new(c.weight, c.roughness, c.ior).map_err(invalid)?))
        }
        None => Ok(surface),
    }
}

/// The procedural detail the product look draws over `def` (display only).
/// The fork has no mould-texture pattern yet; an EDM texture is drawn as
/// its nearest, a fine albedo and relief grain.
pub fn detail(def: &MaterialDef) -> Detail {
    match def.pattern {
        Some(p) if p.kind == PatternKind::EdmTexture => Detail {
            pattern: Pattern::Silkscreen,
            strength: p.strength.clamp(0.0, 1.0),
        },
        _ => Detail::NONE,
    }
}

/// [`Material`] form of a definition (the integration crate's look type).
pub fn material(def: &MaterialDef) -> Material {
    Material {
        name: "generated body",
        base_linear: def.base_linear,
        metallic: def.metallic,
        roughness: def.roughness,
        ior: def.ior,
        coat: def.coat.map(|c| Coat {
            weight: c.weight,
            roughness: c.roughness,
            ior: c.ior,
        }),
        detail: detail(def),
    }
}

/// Provisional values: linear-sRGB base (F0 for metals), metallic,
/// perceptual roughness, IOR. Cited ones repeat `docs/materials.md` and
/// `material.rs`; the rest are **est.**
pub fn provisional(id: MaterialId) -> MaterialDef {
    use MaterialId::*;
    let (name, base, metallic, roughness, ior, estimated): (&str, [f64; 3], f64, f64, f64, bool) =
        match id {
            // docs/materials.md: black epoxy mould compound.
            EpoxyMould => (
                "Epoxy mould compound",
                [0.022, 0.022, 0.024],
                0.0,
                0.45,
                1.55,
                true,
            ),
            EpoxyLaserMarked => (
                "Laser-marked epoxy",
                [0.075, 0.075, 0.078],
                0.0,
                0.65,
                1.55,
                true,
            ),
            MarkingInkWhite => (
                "White marking ink",
                [0.70, 0.70, 0.67],
                0.0,
                0.55,
                1.5,
                true,
            ),
            ResistorOvercoat => (
                "Resistor overcoat",
                [0.025, 0.025, 0.027],
                0.0,
                0.30,
                1.55,
                true,
            ),
            Alumina96 => ("96 % alumina", [0.78, 0.77, 0.73], 0.0, 0.55, 1.77, true),
            BariumTitanate => ("MLCC ceramic", [0.32, 0.25, 0.15], 0.0, 0.55, 2.4, true),
            Ferrite => ("Ferrite", [0.045, 0.045, 0.048], 0.0, 0.60, 2.3, true),
            TinMatte => ("Matte tin", [0.64, 0.64, 0.66], 1.0, 0.28, METAL_IOR, true),
            TinBright => ("Bright tin", [0.64, 0.64, 0.66], 1.0, 0.14, METAL_IOR, true),
            SolderSac => ("SAC solder", [0.64, 0.64, 0.66], 1.0, 0.14, METAL_IOR, true),
            // Au F0 (J&C/Hoffman, as the ENIG blend in docs/materials.md);
            // hard gold over nickel is satin-bright (est. roughness).
            Gold => (
                "Gold plating",
                [1.000, 0.782, 0.344],
                1.0,
                0.22,
                METAL_IOR,
                true,
            ),
            // Ni F0 (Rakić 1998, as in docs/materials.md).
            Nickel => ("Nickel", [0.660, 0.609, 0.526], 1.0, 0.30, METAL_IOR, true),
            // Copper F0 (J&C via Hoffman); C194 is ≥ 97 % Cu.
            CopperAlloy => (
                "Copper alloy",
                [0.955, 0.638, 0.538],
                1.0,
                0.30,
                METAL_IOR,
                true,
            ),
            PhosphorBronze => (
                "Phosphor bronze",
                [0.90, 0.68, 0.48],
                1.0,
                0.30,
                METAL_IOR,
                true,
            ),
            Brass => ("Brass", [0.91, 0.78, 0.42], 1.0, 0.30, METAL_IOR, true),
            Aluminium => (
                "Aluminium",
                [0.913, 0.922, 0.924],
                1.0,
                0.35,
                METAL_IOR,
                true,
            ),
            PvcSleeve => ("PVC sleeve", [0.10, 0.10, 0.12], 0.0, 0.40, 1.54, true),
            BorosilicateGlass => (
                "Borosilicate glass",
                [0.55, 0.60, 0.55],
                0.0,
                0.05,
                1.47,
                true,
            ),
            Pbt => ("PBT", [0.030, 0.030, 0.032], 0.0, 0.50, 1.57, true),
            Lcp => ("LCP", [0.035, 0.035, 0.036], 0.0, 0.45, 1.60, true),
            Pa9t => ("PA9T", [0.040, 0.040, 0.040], 0.0, 0.50, 1.55, true),
            SiliconeClear => ("Clear silicone", [0.80, 0.80, 0.80], 0.0, 0.10, 1.41, true),
            SiliconeDiffuse => (
                "Diffuse silicone",
                [0.85, 0.85, 0.83],
                0.0,
                0.40,
                1.41,
                true,
            ),
            Polyimide => ("Polyimide", [0.55, 0.28, 0.05], 0.0, 0.25, 1.70, true),
        };
    let pattern =
        matches!(id, EpoxyMould | Pbt | Lcp | Pa9t).then_some(materials::SurfacePattern {
            kind: PatternKind::EdmTexture,
            strength: 0.35,
        });
    MaterialDef {
        id,
        name: name.into(),
        category: id.category(),
        base_linear: base,
        metallic,
        roughness,
        ior,
        coat: None,
        transmission: None,
        pattern,
        density_kg_m3: None,
        source: Vec::new(),
        estimated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_material_has_a_valid_look_and_saved_projection() -> Result<()> {
        for id in MaterialId::ALL {
            let def = definition(id)?;
            assert_eq!(def.id, id);
            surface(&def)?;
            let look = saved(&def);
            assert!((1.0..=3.0).contains(&look.ior), "{id:?}");
            assert_eq!(
                def.metallic > 0.5,
                id.category() == MaterialCategory::Metal,
                "{id:?} metallic"
            );
        }
        Ok(())
    }

    #[test]
    fn gold_is_yellow_and_tin_is_neutral() -> Result<()> {
        let gold = definition(MaterialId::Gold)?.base_linear;
        let tin = definition(MaterialId::TinMatte)?.base_linear;
        assert!(gold[0] > gold[2] + 0.4);
        assert!((tin[0] - tin[2]).abs() < 0.05);
        Ok(())
    }
}
