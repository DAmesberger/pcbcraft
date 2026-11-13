//! Slice D2: `library.inspect_model_file` reads a STEP, GLB or
//! `.solidcraft` file through io (bounded) and returns the integration
//! crate's inspection; the import itself is `project.apply`
//! `library.import_model` with the wizard's mapping. Not a document change.
use super::Session;
use crate::Result;
use pcbcraft_core::library::model3d::{ModelFormat, SourceFrame, SourceFrameParams};
use pcbcraft_solidcraft::models::inspect;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// Longest accepted path, bytes.
const MAX_PATH: usize = 4095;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelFileParams {
    pub path: String,
    pub format: ModelFormat,
    /// The frame to report bounds in; the format's default frame (glTF:
    /// metres, Y up; STEP and `.solidcraft`: as stated, Z up) when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<SourceFrameParams>,
}

/// The file extensions each format accepts.
fn extensions(format: ModelFormat) -> &'static [&'static str] {
    match format {
        ModelFormat::Step => &["step", "stp"],
        ModelFormat::Gltf => &["glb", "gltf"],
        ModelFormat::Solidcraft => &["solidcraft"],
    }
}

fn invalid(message: String) -> crate::Error {
    pcbcraft_solidcraft::Error::Invalid(message).into()
}

impl Session {
    /// Inspect a model file: read bounded, never changing the document.
    pub(super) fn inspect_model_file(&mut self, params: ModelFileParams) -> Result<Value> {
        if params.path.len() > MAX_PATH {
            return Err(invalid("model.asset: the path is too long".into()));
        }
        let path = Path::new(&params.path);
        let accepted = extensions(params.format);
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if extension.as_deref().is_none_or(|e| !accepted.contains(&e)) {
            return Err(invalid(format!(
                "model.asset: a {:?} model file ends in .{}",
                params.format,
                accepted.join(" or .")
            )));
        }
        let bytes = pcbcraft_io::persistence::read_bytes(path)?;
        let inspection = match &params.frame {
            Some(frame) => inspect::inspect_in(&bytes, params.format, &SourceFrame::from(*frame))?,
            None => inspect::inspect(&bytes, params.format)?,
        };
        Ok(serde_json::to_value(inspection)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::Engine;
    use serde_json::json;

    type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    /// A one-box GLB, metres, Y up: 2 × 1 × 3 mm standing on the ground.
    fn one_box_glb() -> Vec<u8> {
        let (lo, hi) = ([-0.001f32, 0.0, -0.0015], [0.001f32, 0.001, 0.0015]);
        let mut positions = Vec::new();
        for i in 0..8u32 {
            for (axis, bit) in [1u32, 2, 4].into_iter().enumerate() {
                positions.push(if i & bit == 0 { lo[axis] } else { hi[axis] });
            }
        }
        let quads = [
            [0u32, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let indices: Vec<u32> = quads
            .iter()
            .flat_map(|[a, b, c, d]| [*a, *b, *c, *a, *c, *d])
            .collect();
        let mut bin: Vec<u8> = positions.iter().flat_map(|v| v.to_le_bytes()).collect();
        let at = bin.len();
        bin.extend(indices.iter().flat_map(|v| v.to_le_bytes()));
        let document = json!({
            "asset": {"version": "2.0"},
            "scene": 0, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0}],
            "meshes": [{"name": "Assembly", "primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "material": 0}]}],
            "materials": [{"name": "Grey", "pbrMetallicRoughness": {"metallicFactor": 0.0}}],
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": at},
                {"buffer": 0, "byteOffset": at, "byteLength": bin.len() - at}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 8, "type": "VEC3", "min": lo, "max": hi},
                {"bufferView": 1, "componentType": 5125, "count": indices.len(), "type": "SCALAR"}],
        });
        glb(document.to_string().as_bytes(), &bin)
    }

    /// The GLB container layout (Khronos glTF 2.0, binary format).
    fn glb(json: &[u8], bin: &[u8]) -> Vec<u8> {
        let pad = |n: usize| (4 - n % 4) % 4;
        let (jl, bl) = (json.len() + pad(json.len()), bin.len() + pad(bin.len()));
        let total = 12 + 8 + jl + 8 + bl;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(jl as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        out.extend_from_slice(json);
        out.extend(std::iter::repeat_n(b' ', pad(json.len())));
        out.extend_from_slice(&(bl as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        out.extend_from_slice(bin);
        out.extend(std::iter::repeat_n(0u8, pad(bin.len())));
        out
    }

    #[test]
    fn inspects_a_glb_without_changing_the_document() -> Fallible<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("assembly.glb");
        std::fs::write(&path, one_box_glb())?;
        let mut session = Session::new(Engine::demo()?);
        let before = session.dispatch(json!({"command": "project.inspect"}));
        let reply = session.dispatch(json!({"command": "library.inspect_model_file",
            "params": {"path": path.to_string_lossy(), "format": "gltf"}}));
        assert_eq!(reply["ok"], true, "{reply}");
        let result = &reply["result"];
        assert_eq!(result["bodies"][0]["key"], "Assembly");
        assert_eq!(result["materials"][0]["name"], "Grey");
        let hi = &result["bounds_mm"][1];
        let near = |v: &Value, x: f64| v.as_f64().is_some_and(|v| (v - x).abs() < 1e-4);
        assert!(
            near(&hi[0], 1.0) && near(&hi[1], 1.5) && near(&hi[2], 1.0),
            "{hi}"
        );
        // A stated frame: Z up kept, so the file's 1 mm height stays on y.
        let reply = session.dispatch(json!({"command": "library.inspect_model_file",
            "params": {"path": path.to_string_lossy(), "format": "gltf",
                "frame": {"unit_mm": 1000.0, "up": "z", "origin_offset": [0.0, 0.0, 0.0]}}}));
        let hi = &reply["result"]["bounds_mm"][1];
        assert!(near(&hi[1], 1.0) && near(&hi[2], 1.5), "{reply}");
        let after = session.dispatch(json!({"command": "project.inspect"}));
        assert_eq!(before, after, "inspection is not a document change");
        Ok(())
    }

    #[test]
    fn refuses_a_mismatched_extension_a_missing_file_and_garbage() -> Fallible<()> {
        let dir = tempfile::tempdir()?;
        let mut session = Session::new(Engine::demo()?);
        let step = dir.path().join("part.step");
        std::fs::write(&step, b"ISO-10303-21;\nnot a model")?;
        let glb = dir.path().join("part.glb");
        std::fs::write(&glb, b"glTF\x02\x00\x00\x00short")?;
        for (path, format, code) in [
            (step.clone(), "gltf", "invalid_input"),
            (step, "step", "invalid_input"),
            (glb, "gltf", "invalid_input"),
            (dir.path().join("missing.glb"), "gltf", ""),
        ] {
            let reply = session.dispatch(json!({"command": "library.inspect_model_file",
                "params": {"path": path.to_string_lossy(), "format": format}}));
            assert_eq!(reply["ok"], false, "{reply}");
            if !code.is_empty() {
                assert_eq!(reply["error"]["code"], code, "{reply}");
            }
        }
        let reply = session.dispatch(json!({"command": "library.inspect_model_file",
            "params": {"path": "x.glb", "format": "gltf", "extra": 1}}));
        assert_eq!(reply["ok"], false, "unknown fields are refused: {reply}");
        Ok(())
    }
}
