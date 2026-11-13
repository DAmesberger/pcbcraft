//! The shipped material library (`materials/` in the shipped library):
//! package materials, board and finish looks, film thicknesses and the
//! display layers' sizes, read once and handed to the 3D build as a typed
//! [`MaterialLibrary`]. A missing or broken file is an error naming it.
use crate::shipped::{Shelf, ShippedLibrary};
use crate::{Error, Result};
use sparksmith_core::materials::MaterialLibrary;

/// The largest material file read.
const MAX_BYTES: u64 = 1 << 20;

/// The material library of `shipped`.
pub fn load(shipped: &ShippedLibrary) -> Result<MaterialLibrary> {
    MaterialLibrary::load(|name| {
        let bytes = shipped
            .read(Shelf::Materials, name, MAX_BYTES)
            .map_err(|error| {
                sparksmith_core::Error::Invalid(format!(
                    "{}: {error}",
                    shipped.shelf(Shelf::Materials).join(name).display()
                ))
            })?;
        String::from_utf8(bytes)
            .map_err(|_| sparksmith_core::Error::Invalid("the file is not UTF-8".into()))
    })
    .map_err(|error| {
        Error::Invalid(format!(
            "Sparksmith's material data in {} could not be loaded: {error}",
            shipped.root().display()
        ))
    })
}

/// The material library of the shipped library this program finds
/// ([`ShippedLibrary::locate`]).
pub fn load_shipped() -> Result<MaterialLibrary> {
    load(&ShippedLibrary::locate()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shipped::MARKER;

    #[test]
    fn the_repository_materials_load() -> Result<()> {
        let library = load_shipped()?;
        assert_eq!(
            library.packages.all().len(),
            sparksmith_core::materials::MaterialId::ALL.len()
        );
        Ok(())
    }

    /// Copy the repository's materials into a temporary library.
    fn copy() -> Result<(tempfile::TempDir, std::path::PathBuf)> {
        let source = ShippedLibrary::locate()?.shelf(Shelf::Materials);
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("library");
        std::fs::create_dir_all(root.join("materials"))?;
        std::fs::write(root.join(MARKER), "{\"format\":1}\n")?;
        for name in MaterialLibrary::FILES {
            std::fs::copy(source.join(name), root.join("materials").join(name))?;
        }
        Ok((temp, root))
    }

    #[test]
    fn a_missing_file_is_an_error_naming_it() -> Result<()> {
        let (_temp, root) = copy()?;
        std::fs::remove_file(root.join("materials/finishes.json"))?;
        let error = load(&ShippedLibrary::at(&root)?).map(|_| ());
        assert!(
            matches!(&error, Err(Error::Invalid(m)) if m.contains("finishes.json")),
            "{error:?}"
        );
        Ok(())
    }

    #[test]
    fn a_broken_file_is_an_error_naming_it() -> Result<()> {
        let (_temp, root) = copy()?;
        let path = root.join("materials/board-looks.json");
        let text = std::fs::read_to_string(&path)?.replacen("\"laminate\"", "\"laminat\"", 1);
        std::fs::write(&path, text)?;
        let error = load(&ShippedLibrary::at(&root)?).map(|_| ());
        assert!(
            matches!(&error, Err(Error::Invalid(m)) if m.contains("board-looks.json")),
            "{error:?}"
        );
        let packages = root.join("materials/package-materials.json");
        let text = std::fs::read_to_string(&packages)?.replacen("0.022", "7.5", 1);
        std::fs::write(&packages, text)?;
        let error = load(&ShippedLibrary::at(&root)?).map(|_| ());
        assert!(
            matches!(&error, Err(Error::Invalid(m)) if m.contains("package-materials.json")),
            "{error:?}"
        );
        Ok(())
    }
}
