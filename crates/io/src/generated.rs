//! Generated parts on load (slice GP1, `docs/generator-plugins.md`): every
//! part a generator module made is made again from the module embedded in
//! the project and its recorded parameters, and must give the recorded
//! result hash and exactly the stored revisions. Each distinct module is
//! checked and compiled once; every call runs under the host's fuel and
//! memory limits, so a hostile module costs bounded work.
use crate::{Error, Result};
use sparksmith_core::format::EditorFile;
use sparksmith_core::ids::ContentHash;
use sparksmith_core::library::generated;
use sparksmith_generators::abi::{Failure, Generated};
use sparksmith_generators::{Limits, WasmiRuntime};
use std::collections::BTreeMap;

/// Check every generated part of `file` against its embedded module.
pub fn verify(file: &EditorFile) -> Result<()> {
    let library = &file.project.library;
    if library.generated.is_empty() {
        return Ok(());
    }
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let mut loaded = BTreeMap::new();
    let mut run = |module: &sparksmith_core::library::PackedAsset,
                   request: &sparksmith_generators::abi::v1::Request|
     -> std::result::Result<Generated, Failure> {
        let hash: ContentHash = module.hash;
        let compiled = match loaded.entry(hash) {
            std::collections::btree_map::Entry::Occupied(found) => found.into_mut(),
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(runtime.load(&module.data)?)
            }
        };
        compiled.generate(request)
    };
    generated::verify(library, &mut run).map_err(Error::Document)
}
