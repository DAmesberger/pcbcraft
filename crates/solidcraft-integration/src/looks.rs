//! The material library the 3D view and the exact export read: package
//! materials, board and finish looks, film thicknesses and display-layer
//! sizes. It is shipped data that `sparksmith-io` loads into the host's
//! context ([`sparksmith_core::library::standards::Context`]); a build reads
//! the library of the context current on its thread, so the live view and
//! the export of one host never differ and no build reloads it.
//!
//! With no context current, or when loading the library failed, every
//! build that needs it fails with that error: there are no values in code
//! to fall back to.
use crate::{Error, Result};
use sparksmith_core::library::standards::Context;
use sparksmith_core::materials::MaterialLibrary;
#[cfg(any(test, feature = "test-fixtures"))]
use std::sync::Arc;

/// The library of the current context.
pub fn get() -> Result<&'static MaterialLibrary> {
    Context::current()
        .and_then(|context| context.materials())
        .map_err(|e| Error::Invalid(e.to_string()))
}

/// A build that spreads over cores carries the current context to each.
pub use sparksmith_core::library::standards::carried;

/// A context for this crate's tests: the core's fixture, with the
/// repository's standards and materials.
#[cfg(any(test, feature = "test-fixtures"))]
pub fn test_context() -> Result<Arc<Context>> {
    Context::fixture().map_err(|e| Error::Invalid(e.to_string()))
}

/// The library as a [`crate::stack::Stack`] carries it: compared by
/// identity and shown by name only.
#[derive(Clone, Copy)]
pub struct Looks(pub &'static MaterialLibrary);

impl Looks {
    /// The current context's library ([`get`]).
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
