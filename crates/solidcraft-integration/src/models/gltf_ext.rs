//! Slice D2: glTF 2.0 materials as Pcbcraft reads them itself with
//! `serde_json` (Solidcraft's reader takes only the PBR factors): the
//! metallic-roughness factors plus KHR_materials_clearcoat, _transmission,
//! _ior and _emissive_strength. KHR_materials_volume has no place in
//! [`CustomMaterial`] yet and is reported as ignored. Textures are noted,
//! not shown, until Solidcraft persists textures (R2).
//!
//! Written from the Khronos glTF 2.0 specification and the KHR extension
//! specifications (public documents). The file is untrusted: sizes and
//! counts are bounded and a value outside its specified range refuses the
//! file naming the material, never a silent clamp.
use crate::{Error, Result};
use pcbcraft_core::library::model3d::CustomMaterial;
use serde_json::{Map, Value};

/// Materials one GLB may carry.
pub const MAX_GLTF_MATERIALS: usize = pcbcraft_core::library::model3d::MAX_CUSTOM_MATERIALS;
/// Bytes of the JSON chunk read.
pub const MAX_GLTF_JSON_BYTES: usize = 16 << 20;
/// Characters of a material name kept (longer names are cut, for display).
pub const MAX_MATERIAL_NAME: usize = 128;

const GLB_MAGIC: &[u8; 4] = b"glTF";
const CHUNK_JSON: u32 = 0x4E4F_534A;

/// The material extensions read.
pub const READ_EXTENSIONS: [&str; 4] = [
    "KHR_materials_clearcoat",
    "KHR_materials_transmission",
    "KHR_materials_ior",
    "KHR_materials_emissive_strength",
];

/// What the JSON says about looks, beyond the materials.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GltfLooks {
    pub materials: Vec<CustomMaterial>,
    /// The file has textures (none are shown until R2).
    pub has_textures: bool,
    /// Extensions the file uses that change looks and are not read, sorted.
    pub ignored_extensions: Vec<String>,
}

/// The JSON of a glTF file: the JSON chunk of a GLB container, or the
/// bytes themselves for a `.gltf` text file.
pub fn json_chunk(bytes: &[u8]) -> Result<&[u8]> {
    if !bytes.starts_with(GLB_MAGIC) {
        if bytes.len() > MAX_GLTF_JSON_BYTES {
            return Err(invalid("the glTF JSON is too large"));
        }
        return Ok(bytes);
    }
    let word = |at: usize| -> Result<u32> {
        bytes
            .get(at..at.checked_add(4).ok_or_else(|| invalid("GLB truncated"))?)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| invalid("GLB truncated"))
    };
    if word(4)? != 2 {
        return Err(invalid("the GLB container is not glTF 2.0"));
    }
    let length = usize::try_from(word(8)?).map_err(|_| invalid("GLB length"))?;
    if length > bytes.len() {
        return Err(invalid("the GLB container is truncated"));
    }
    let len = usize::try_from(word(12)?).map_err(|_| invalid("GLB chunk length"))?;
    if word(16)? != CHUNK_JSON {
        return Err(invalid(
            "the GLB container does not start with its JSON chunk",
        ));
    }
    if len > MAX_GLTF_JSON_BYTES {
        return Err(invalid("the glTF JSON is too large"));
    }
    let end = 20usize
        .checked_add(len)
        .filter(|end| *end <= length)
        .ok_or_else(|| invalid("the GLB JSON chunk runs past the end of the file"))?;
    bytes
        .get(20..end)
        .ok_or_else(|| invalid("the GLB JSON chunk runs past the end of the file"))
}

/// The materials of a glTF JSON document (a GLB's JSON chunk or a `.gltf`
/// file), in file order.
pub fn materials(json: &[u8]) -> Result<Vec<CustomMaterial>> {
    Ok(looks(json)?.materials)
}

/// The materials, texture presence and unread look extensions of a glTF
/// JSON document.
pub fn looks(json: &[u8]) -> Result<GltfLooks> {
    if json.len() > MAX_GLTF_JSON_BYTES {
        return Err(invalid("the glTF JSON is too large"));
    }
    let root: Value = serde_json::from_slice(json)
        .map_err(|error| invalid(&format!("the glTF JSON does not parse: {error}")))?;
    let root = root
        .as_object()
        .ok_or_else(|| invalid("the glTF JSON is not an object"))?;
    let list = match root.get("materials") {
        None => &[][..],
        Some(Value::Array(list)) => list.as_slice(),
        Some(_) => return Err(invalid("`materials` is not an array")),
    };
    if list.len() > MAX_GLTF_MATERIALS {
        return Err(invalid(&format!(
            "the file has {} materials; a model carries at most {MAX_GLTF_MATERIALS}",
            list.len()
        )));
    }
    let materials = list
        .iter()
        .enumerate()
        .map(|(index, value)| material(index, value))
        .collect::<Result<Vec<_>>>()?;
    let has_textures = matches!(root.get("textures"), Some(Value::Array(t)) if !t.is_empty())
        || materials.iter().any(|m| m.textures_ignored);
    let mut ignored: Vec<String> = match root.get("extensionsUsed") {
        Some(Value::Array(used)) => used
            .iter()
            .filter_map(Value::as_str)
            .filter(|e| e.starts_with("KHR_materials_") && !READ_EXTENSIONS.contains(e))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    ignored.sort();
    ignored.dedup();
    Ok(GltfLooks {
        materials,
        has_textures,
        ignored_extensions: ignored,
    })
}

fn material(index: usize, value: &Value) -> Result<CustomMaterial> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid(&format!("material {index} is not an object")))?;
    let name = match object.get("name").and_then(Value::as_str) {
        Some(n) if !n.trim().is_empty() => n.trim().chars().take(MAX_MATERIAL_NAME).collect(),
        _ => format!("glTF material {index}"),
    };
    let bad = |what: &str| invalid(&format!("material {index} `{name}`: {what}"));
    let pbr = optional_object(object, "pbrMetallicRoughness").map_err(|e| bad(&e))?;
    let base_colour = match pbr.and_then(|p| p.get("baseColorFactor")) {
        None => [1.0; 4],
        Some(v) => unit_array::<4>(v)
            .ok_or_else(|| bad("baseColorFactor must be four numbers in [0, 1]"))?,
    };
    let metallic = unit_factor(pbr, "metallicFactor", 1.0).map_err(|e| bad(&e))?;
    let roughness = unit_factor(pbr, "roughnessFactor", 1.0).map_err(|e| bad(&e))?;
    let mut textures = pbr.is_some_and(|p| {
        p.contains_key("baseColorTexture") || p.contains_key("metallicRoughnessTexture")
    }) || ["normalTexture", "occlusionTexture", "emissiveTexture"]
        .iter()
        .any(|k| object.contains_key(*k));
    let extensions = optional_object(object, "extensions").map_err(|e| bad(&e))?;
    let extension = |key: &str| -> Result<Option<&Map<String, Value>>> {
        match extensions.and_then(|e| e.get(key)) {
            None => Ok(None),
            Some(Value::Object(o)) => Ok(Some(o)),
            Some(_) => Err(bad(&format!("{key} is not an object"))),
        }
    };
    let clearcoat = match extension("KHR_materials_clearcoat")? {
        Some(e) => {
            textures |= has_texture(e);
            Some(unit_factor(Some(e), "clearcoatFactor", 0.0).map_err(|e| bad(&e))?)
        }
        None => None,
    };
    let transmission = match extension("KHR_materials_transmission")? {
        Some(e) => {
            textures |= has_texture(e);
            Some(unit_factor(Some(e), "transmissionFactor", 0.0).map_err(|e| bad(&e))?)
        }
        None => None,
    };
    let ior = match extension("KHR_materials_ior")? {
        Some(e) => match e.get("ior") {
            None => Some(1.5),
            Some(v) => {
                let ior = v.as_f64().filter(|x| x.is_finite());
                match ior {
                    // The specification allows 0 (no refraction) and [1, ∞).
                    Some(x) if x == 0.0 || x >= 1.0 => Some(x),
                    _ => return Err(bad("KHR_materials_ior ior must be 0 or at least 1")),
                }
            }
        },
        None => None,
    };
    let emissive_factor = match object.get("emissiveFactor") {
        None => [0.0; 3],
        Some(v) => unit_array::<3>(v)
            .ok_or_else(|| bad("emissiveFactor must be three numbers in [0, 1]"))?,
    };
    let strength = match extension("KHR_materials_emissive_strength")? {
        Some(e) => match e.get("emissiveStrength") {
            None => 1.0,
            Some(v) => v
                .as_f64()
                .filter(|x| x.is_finite() && *x >= 0.0)
                .ok_or_else(|| bad("emissiveStrength must be a number of at least 0"))?,
        },
        None => 1.0,
    };
    let emissive = emissive_factor.map(|c| c * strength);
    let emissive = emissive.iter().any(|c| *c > 0.0).then_some(emissive);
    for key in [
        "KHR_materials_volume",
        "KHR_materials_specular",
        "KHR_materials_sheen",
    ] {
        if let Some(e) = extension(key)? {
            textures |= has_texture(e);
        }
    }
    Ok(CustomMaterial {
        name,
        base_colour,
        metallic,
        roughness,
        clearcoat,
        transmission,
        ior,
        emissive,
        textures_ignored: textures,
    })
}

/// Any `…Texture` entry of an extension object.
fn has_texture(object: &Map<String, Value>) -> bool {
    object.keys().any(|k| k.ends_with("Texture"))
}

fn optional_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> std::result::Result<Option<&'a Map<String, Value>>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Object(o)) => Ok(Some(o)),
        Some(_) => Err(format!("{key} is not an object")),
    }
}

fn unit_factor(
    object: Option<&Map<String, Value>>,
    key: &str,
    default: f64,
) -> std::result::Result<f64, String> {
    match object.and_then(|o| o.get(key)) {
        None => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|x| (0.0..=1.0).contains(x))
            .ok_or_else(|| format!("{key} must be a number in [0, 1]")),
    }
}

fn unit_array<const N: usize>(value: &Value) -> Option<[f64; N]> {
    let list = value.as_array().filter(|l| l.len() == N)?;
    let mut out = [0.0; N];
    for (slot, v) in out.iter_mut().zip(list) {
        *slot = v.as_f64().filter(|x| (0.0..=1.0).contains(x))?;
    }
    Some(out)
}

fn invalid(what: &str) -> Error {
    Error::Invalid(format!("model.asset: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<GltfLooks> {
        looks(value.to_string().as_bytes())
    }

    #[test]
    fn factors_default_as_the_specification_says() -> Result<()> {
        let looks = parse(json!({"asset": {"version": "2.0"}, "materials": [{}]}))?;
        let [m] = looks.materials.as_slice() else {
            return Err(Error::Verification("one material".into()));
        };
        assert_eq!(m.name, "glTF material 0");
        assert_eq!(m.base_colour, [1.0; 4]);
        assert_eq!((m.metallic, m.roughness), (1.0, 1.0));
        assert_eq!(
            (m.clearcoat, m.transmission, m.ior, m.emissive),
            (None, None, None, None)
        );
        assert!(!m.textures_ignored && !looks.has_textures);
        Ok(())
    }

    #[test]
    fn khr_extensions_are_read() -> Result<()> {
        let looks = parse(json!({
            "asset": {"version": "2.0"},
            "extensionsUsed": ["KHR_materials_clearcoat", "KHR_materials_volume",
                "KHR_materials_ior", "KHR_materials_transmission",
                "KHR_materials_emissive_strength"],
            "materials": [{
                "name": " Lens ",
                "pbrMetallicRoughness": {"baseColorFactor": [0.9, 0.1, 0.1, 0.5],
                    "metallicFactor": 0.0, "roughnessFactor": 0.05},
                "emissiveFactor": [1.0, 0.5, 0.0],
                "extensions": {
                    "KHR_materials_clearcoat": {"clearcoatFactor": 0.8},
                    "KHR_materials_transmission": {"transmissionFactor": 0.9},
                    "KHR_materials_ior": {"ior": 1.49},
                    "KHR_materials_emissive_strength": {"emissiveStrength": 4.0},
                    "KHR_materials_volume": {"thicknessFactor": 1.0}
                }
            }, {
                "name": "Leads",
                "pbrMetallicRoughness": {"metallicFactor": 1.0, "roughnessFactor": 0.3,
                    "baseColorTexture": {"index": 0}},
                "extensions": {"KHR_materials_ior": {}}
            }],
            "textures": [{"source": 0}]
        }))?;
        let [lens, leads] = looks.materials.as_slice() else {
            return Err(Error::Verification("two materials".into()));
        };
        assert_eq!(lens.name, "Lens");
        assert_eq!(lens.base_colour, [0.9, 0.1, 0.1, 0.5]);
        assert_eq!((lens.metallic, lens.roughness), (0.0, 0.05));
        assert_eq!(lens.clearcoat, Some(0.8));
        assert_eq!(lens.transmission, Some(0.9));
        assert_eq!(lens.ior, Some(1.49));
        assert_eq!(lens.emissive, Some([4.0, 2.0, 0.0]));
        assert!(!lens.textures_ignored);
        assert_eq!(leads.ior, Some(1.5), "the extension's default IOR");
        assert!(leads.textures_ignored);
        assert!(looks.has_textures);
        assert_eq!(looks.ignored_extensions, ["KHR_materials_volume"]);
        Ok(())
    }

    #[test]
    fn out_of_range_values_refuse_naming_the_material() {
        for (material, needle) in [
            (
                json!({"name": "Body", "pbrMetallicRoughness": {"metallicFactor": 1.5}}),
                "metallicFactor",
            ),
            (
                json!({"name": "Body", "pbrMetallicRoughness": {"baseColorFactor": [1, 1, 1]}}),
                "baseColorFactor",
            ),
            (
                json!({"name": "Body", "extensions": {"KHR_materials_ior": {"ior": 0.5}}}),
                "ior",
            ),
            (
                json!({"name": "Body", "extensions": {"KHR_materials_clearcoat": {"clearcoatFactor": -0.1}}}),
                "clearcoatFactor",
            ),
            (
                json!({"name": "Body", "emissiveFactor": [2, 0, 0]}),
                "emissiveFactor",
            ),
            (
                json!({"name": "Body", "extensions": {"KHR_materials_emissive_strength": {"emissiveStrength": -1}}}),
                "emissiveStrength",
            ),
        ] {
            let refused = match parse(json!({"materials": [material]})) {
                Err(Error::Invalid(text)) => text,
                other => format!("{other:?}"),
            };
            assert!(
                refused.starts_with("model.asset: material 0 `Body`") && refused.contains(needle),
                "{refused}"
            );
        }
    }

    #[test]
    fn too_many_materials_refuse() {
        let list: Vec<Value> = (0..=MAX_GLTF_MATERIALS).map(|_| json!({})).collect();
        assert!(matches!(
            parse(json!({"materials": list})),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn not_json_refuses() {
        assert!(matches!(looks(b"not json"), Err(Error::Invalid(_))));
        assert!(matches!(looks(b"[1, 2]"), Err(Error::Invalid(_))));
    }

    #[test]
    fn json_chunk_reads_glb_and_text() -> Result<()> {
        let json = br#"{"asset":{"version":"2.0"}}"#;
        let glb = solidcraft_engine::io::glb_container(json, &[]);
        let chunk = json_chunk(&glb)?;
        assert!(chunk.starts_with(json), "padding follows the JSON");
        assert_eq!(json_chunk(json)?, json);
        let mut truncated = glb.clone();
        truncated.truncate(18);
        assert!(json_chunk(&truncated).is_err());
        let mut lying = glb;
        lying[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(json_chunk(&lying).is_err());
        Ok(())
    }
}
