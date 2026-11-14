//! Generator bundles on the shipped library's Generators shelf (slice GP2,
//! docs/generator-plugins.md): one folder per generator,
//!
//! ```text
//! generators/<name>/generator.json   the manifest: module and part files by SHA-256
//! generators/<name>/<name>.wasm      the module
//! generators/<name>/parts/*.solidcraft   the part files its 3D models name
//! ```
//!
//! The bundle is identified, and approved, by one hash: the SHA-256 of the
//! manifest's bytes, which name every other file by its SHA-256. Reading a
//! bundle checks each file against the manifest, so a missing or changed
//! file is refused with the file named.
use crate::shipped::{Shelf, ShippedLibrary};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sparksmith_core::ids::ContentHash;
use sparksmith_core::library::bundle::asset_hash;

/// The manifest's file name in a bundle.
pub const MANIFEST: &str = "generator.json";
/// The bundle manifest format this program reads.
pub const FORMAT: u32 = 1;
/// Most part files one bundle ships.
pub const MAX_PARTS: usize = 64;
/// Largest manifest read.
const MANIFEST_BYTES: u64 = 256 * 1024;
/// Largest part file read (the document's asset bound).
const PART_BYTES: u64 = sparksmith_core::library::bundle::MAX_ASSET_BYTES as u64;

/// A bundle's manifest (`generator.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    /// The generator's folder name.
    pub name: String,
    pub module: ManifestFile,
    pub parts: Vec<ManifestPart>,
}

/// A file of the bundle by its path inside the bundle and SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFile {
    pub file: String,
    pub sha256: ContentHash,
}

/// A part file of the bundle, with what a Model of it records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPart {
    pub file: String,
    pub sha256: ContentHash,
    pub title: String,
    /// SPDX.
    pub licence: String,
    pub provenance: String,
}

/// A part file read from a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundledPart {
    pub entry: ManifestPart,
    pub bytes: Vec<u8>,
}

/// A generator bundle, every file checked against its manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratorBundle {
    /// SHA-256 of the manifest's bytes: the bundle's identity.
    pub hash: ContentHash,
    pub manifest: Manifest,
    pub module: Vec<u8>,
    pub parts: Vec<BundledPart>,
}

impl GeneratorBundle {
    /// The part file `hash`, if the bundle ships it.
    pub fn part(&self, hash: ContentHash) -> Option<&BundledPart> {
        self.parts.iter().find(|p| p.entry.sha256 == hash)
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

impl ShippedLibrary {
    /// The names of the shipped generator bundles (folders under
    /// `generators/` holding a manifest), sorted.
    pub fn generators(&self) -> Result<Vec<String>> {
        let dir = self.shelf(Shelf::Generators);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && entry.path().join(MANIFEST).is_file()
                && let Some(name) = entry.file_name().to_str()
                && valid_name(name)
            {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// The shipped generator bundle `name`, each file checked against the
    /// manifest. A missing bundle names the folder it was looked for in; a
    /// missing or changed file of the bundle is named.
    pub fn generator(&self, name: &str) -> Result<GeneratorBundle> {
        if !valid_name(name) {
            return Err(Error::Invalid(format!(
                "{name:?} is not a generator name: lower-case letters, digits, - and _."
            )));
        }
        if !self
            .shelf(Shelf::Generators)
            .join(name)
            .join(MANIFEST)
            .is_file()
        {
            let shipped = self.generators().unwrap_or_default();
            return Err(Error::Invalid(format!(
                "No generator {name} is shipped in {}; shipped: {}.",
                self.shelf(Shelf::Generators).display(),
                if shipped.is_empty() {
                    "none".to_string()
                } else {
                    shipped.join(", ")
                }
            )));
        }
        let bytes = self.read(
            Shelf::Generators,
            &format!("{name}/{MANIFEST}"),
            MANIFEST_BYTES,
        )?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| {
            Error::Invalid(format!(
                "The manifest of the generator {name} cannot be read: {e}."
            ))
        })?;
        if manifest.format != FORMAT {
            return Err(Error::Invalid(format!(
                "The generator {name} is a bundle of format {}; this program reads format {FORMAT}.",
                manifest.format
            )));
        }
        if manifest.name != name || manifest.parts.len() > MAX_PARTS {
            return Err(Error::Invalid(format!(
                "The manifest of the generator {name} names another generator or more than {MAX_PARTS} part files."
            )));
        }
        let module_limit = u64::try_from(sparksmith_core::library::generated::MAX_MODULE_BYTES)
            .unwrap_or(u64::MAX);
        let module = self.checked(name, &manifest.module, module_limit)?;
        let mut parts = Vec::with_capacity(manifest.parts.len());
        for entry in &manifest.parts {
            let file = ManifestFile {
                file: entry.file.clone(),
                sha256: entry.sha256,
            };
            parts.push(BundledPart {
                bytes: self.checked(name, &file, PART_BYTES)?,
                entry: entry.clone(),
            });
        }
        Ok(GeneratorBundle {
            hash: asset_hash(&bytes),
            manifest,
            module,
            parts,
        })
    }

    /// The bytes of `file` in bundle `name`, refused when missing or not
    /// what the manifest records.
    fn checked(&self, name: &str, file: &ManifestFile, limit: u64) -> Result<Vec<u8>> {
        let path = format!("{name}/{}", file.file);
        let bytes = self
            .read(Shelf::Generators, &path, limit)
            .map_err(|e| match e {
                Error::Io(_) => Error::Invalid(format!(
                    "The generator {name} lacks {}, which its manifest names. Reinstall Sparksmith.",
                    file.file
                )),
                other => other,
            })?;
        let actual = asset_hash(&bytes);
        if actual != file.sha256 {
            return Err(Error::Invalid(format!(
                "The generator {name}'s {} has been changed: its manifest records sha256 {}, the file hashes to sha256 {actual}. Reinstall Sparksmith.",
                file.file, file.sha256
            )));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shipped::MARKER;

    fn bundle(root: &std::path::Path) -> Result<()> {
        let dir = root.join("generators/chip");
        std::fs::create_dir_all(dir.join("parts"))?;
        std::fs::write(dir.join("chip.wasm"), b"\0asm")?;
        std::fs::write(dir.join("parts/a.solidcraft"), b"{}")?;
        let manifest = Manifest {
            format: FORMAT,
            name: "chip".into(),
            module: ManifestFile {
                file: "chip.wasm".into(),
                sha256: asset_hash(b"\0asm"),
            },
            parts: vec![ManifestPart {
                file: "parts/a.solidcraft".into(),
                sha256: asset_hash(b"{}"),
                title: "A".into(),
                licence: "MIT".into(),
                provenance: "Original.".into(),
            }],
        };
        std::fs::write(
            dir.join(MANIFEST),
            serde_json::to_vec_pretty(&manifest).map_err(|e| Error::Invalid(e.to_string()))?,
        )?;
        Ok(())
    }

    #[test]
    fn a_bundle_is_read_by_name_and_checked_file_by_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("library");
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join(MARKER), "{\"format\":1}\n")?;
        bundle(&root)?;
        let shipped = ShippedLibrary::at(&root)?;
        assert_eq!(shipped.generators()?, vec!["chip".to_string()]);
        let read = shipped.generator("chip")?;
        assert_eq!(read.module, b"\0asm");
        assert_eq!(
            read.part(asset_hash(b"{}")).map(|p| p.bytes.clone()),
            Some(b"{}".to_vec())
        );
        assert!(matches!(
            shipped.generator("../chip"),
            Err(Error::Invalid(_))
        ));
        assert!(
            matches!(shipped.generator("gone"), Err(Error::Invalid(m)) if m.contains("shipped: chip"))
        );
        // A changed part file is refused, named.
        std::fs::write(root.join("generators/chip/parts/a.solidcraft"), b"[]")?;
        let changed = shipped.generator("chip");
        assert!(
            matches!(&changed, Err(Error::Invalid(m)) if m.contains("parts/a.solidcraft has been changed")),
            "{changed:?}"
        );
        // A missing one too.
        std::fs::remove_file(root.join("generators/chip/parts/a.solidcraft"))?;
        let missing = shipped.generator("chip");
        assert!(
            matches!(&missing, Err(Error::Invalid(m)) if m.contains("lacks parts/a.solidcraft")),
            "{missing:?}"
        );
        Ok(())
    }
}
