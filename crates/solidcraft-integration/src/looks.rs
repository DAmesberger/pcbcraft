//! The material library the 3D view and the exact export read: package
//! materials, board and finish looks, film thicknesses and display-layer
//! sizes. It is shipped data that `sparksmith-io` loads; the program hands
//! it in once ([`ensure`], at start-up or before its first 3D build) and
//! every build after that reads the same values, so the live view and the
//! export never differ and no build reloads it.
//!
//! Before it is handed in, or when loading it failed, every build that
//! needs it fails with that error: there are no values in code to fall
//! back to.
use crate::{Error, Result};
use sparksmith_core::materials::MaterialLibrary;
use std::sync::OnceLock;

/// The library, or why loading it failed.
static LOADED: OnceLock<std::result::Result<MaterialLibrary, String>> = OnceLock::new();

/// The loaded library, running `load` the first time this process asks.
/// A failed load is kept: later calls return the same error.
pub fn ensure<E: std::fmt::Display>(
    load: impl FnOnce() -> std::result::Result<MaterialLibrary, E>,
) -> Result<&'static MaterialLibrary> {
    LOADED
        .get_or_init(|| load().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|message| Error::Invalid(message.clone()))
}

/// The library handed in with [`ensure`].
pub fn get() -> Result<&'static MaterialLibrary> {
    match LOADED.get() {
        Some(loaded) => loaded
            .as_ref()
            .map_err(|message| Error::Invalid(message.clone())),
        #[cfg(test)]
        None => ensure(source_tree),
        #[cfg(not(test))]
        None => Err(Error::Invalid(
            "Sparksmith's material data is not loaded: the program loads the shipped library's materials/ folder before it builds a 3D model.".into(),
        )),
    }
}

/// This crate's own tests read the repository's `library/materials/`.
#[cfg(test)]
fn source_tree() -> std::result::Result<MaterialLibrary, sparksmith_core::Error> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/materials");
    MaterialLibrary::load(|name| {
        std::fs::read_to_string(dir.join(name))
            .map_err(|error| sparksmith_core::Error::Invalid(error.to_string()))
    })
}

/// The library as a [`crate::stack::Stack`] carries it: compared by
/// identity and shown by name only, since there is one per process.
#[derive(Clone, Copy)]
pub struct Looks(pub &'static MaterialLibrary);

impl Looks {
    /// The loaded library ([`get`]).
    pub fn get() -> Result<Self> {
        get().map(Self)
    }
}

impl std::ops::Deref for Looks {
    type Target = MaterialLibrary;
    fn deref(&self) -> &MaterialLibrary {
        self.0
    }
}

impl std::fmt::Debug for Looks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Looks")
    }
}

impl PartialEq for Looks {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}
