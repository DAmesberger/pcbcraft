//! Slice A2 (contract C0): `library.open_preview`: read a folder bundle
//! through io and return `catalog::preview::inspect_bundle`, the same
//! report `library.inspect` gives a project item, without importing.
use super::{BundleParams, Session};
use crate::Result;
use serde_json::Value;

impl Session {
    /// Contract stub until A2.
    pub(super) fn open_preview(&mut self, _params: BundleParams) -> Result<Value> {
        Err(pcbcraft_core::Error::Unsupported(
            "library.preview_unsupported: previewing library-folder items is not available yet (slice A2)"
                .into(),
        )
        .into())
    }
}
