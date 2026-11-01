//! Slice D2 (contract C0): glTF 2.0 material extensions Pcbcraft reads
//! itself with `serde_json` (Solidcraft's reader takes the PBR factors):
//! KHR_materials_clearcoat, _transmission, _ior, _volume and
//! _emissive_strength. Textures are noted, not shown, until Solidcraft
//! persists textures (R2).
use crate::{Error, Result};
use pcbcraft_core::library::model3d::CustomMaterial;

/// Materials one GLB may carry.
pub const MAX_GLTF_MATERIALS: usize = pcbcraft_core::library::model3d::MAX_CUSTOM_MATERIALS;

/// The materials of a GLB's JSON chunk. Contract stub until D2.
pub fn materials(_json: &[u8]) -> Result<Vec<CustomMaterial>> {
    Err(Error::Invalid(
        "model.unsupported: glTF material extensions are not read yet (slice D2)".into(),
    ))
}
