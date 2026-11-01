//! Slice D2 (contract C0): inspection of an imported model file (STEP
//! AP214/242, GLB, `.solidcraft`) through Solidcraft's public readers:
//! bodies with their keys, source colours and glTF materials, bounds and
//! texture presence. The import wizard (D3) shows the result; nothing is
//! mapped automatically (material and lead suggestions are only proposals).
use crate::{Error, Result};
use pcbcraft_core::library::model3d::{CustomMaterial, ModelFormat};
use serde::{Deserialize, Serialize};

/// Bytes one inspected file may have (the packed-asset bound until the D0
/// container raises it).
pub const MAX_INSPECT_BYTES: usize = pcbcraft_core::library::bundle::MAX_ASSET_BYTES;

/// One body of the source file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InspectedBody {
    /// Stable body path from the file's structure.
    pub key: String,
    /// The file's colour for the body, a hint only.
    pub colour: Option<[f64; 4]>,
    /// Index into `ModelInspection::materials` (glTF).
    pub material: Option<usize>,
    pub bounds_mm: [[f64; 3]; 2],
}

/// What a model file contains.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelInspection {
    pub format: ModelFormat,
    pub bodies: Vec<InspectedBody>,
    pub materials: Vec<CustomMaterial>,
    pub bounds_mm: [[f64; 3]; 2],
    pub has_textures: bool,
}

/// Inspect `bytes` as `format`. Contract stub until D2.
pub fn inspect(_bytes: &[u8], _format: ModelFormat) -> Result<ModelInspection> {
    Err(Error::Invalid(
        "model.unsupported: model file inspection is not available yet (slice D2)".into(),
    ))
}
