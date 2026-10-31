//! [`MaterialId`] → the looks a generated body carries: the opaque PBR
//! projection the native file saves (`appearance.assign`), and the full
//! product look (clear coat, procedural pattern) the 3D view draws per
//! triangle. Display-only parts of the look are never saved.
//!
//! Values come from the core's cited table (`materials::builtin`), the one
//! source of package material values.
use crate::material::{Coat, Detail, Material, Pattern, linear_to_srgb8};
use crate::native::Look;
use crate::{Error, Result};
use pcbcraft_core::materials::{self, MaterialCategory, MaterialDef, MaterialId, PatternKind};
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::render::clear_coat::ClearCoat;
use solidcraft_engine::render::surface::OpaqueSurface;

/// Metals ignore the dielectric IOR; Solidcraft still requires one in [1, 3].
const METAL_IOR: f64 = 1.5;

/// The definition of `id` from the core's built-in table.
pub fn definition(id: MaterialId) -> Result<MaterialDef> {
    materials::builtin(id).map_err(|error| Error::Invalid(error.to_string()))
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
