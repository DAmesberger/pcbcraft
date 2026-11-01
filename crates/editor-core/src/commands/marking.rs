//! Slice D4 (contract C0): handler of `instance.set_marking` and the
//! `instance.marking` query (the resolved layout D5 draws). Component and
//! Part marking specs are edited through `library.edit` like any other
//! body data.
use super::Outcome;
use crate::diagnostics::{Error, Result};
use crate::ids::Id;
use crate::library::marking::{InstanceMarking, MarkingLayout};
use crate::model::Project;

fn unsupported() -> Error {
    Error::Unsupported(
        "marking.unsupported: instance markings are not available yet (slice D4)".into(),
    )
}

/// `instance.set_marking`.
pub(crate) fn set_marking(
    _project: &mut Project,
    _instance: Id,
    _marking: InstanceMarking,
) -> Result<Outcome> {
    Err(unsupported())
}

/// `instance.marking`: the effective spec laid out for one instance;
/// `None` when nothing is marked.
pub(crate) fn layout(_project: &Project, _instance: Id) -> Result<Option<MarkingLayout>> {
    Err(unsupported())
}
