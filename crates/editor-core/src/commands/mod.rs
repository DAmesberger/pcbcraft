//! Wire-level library and instance command families. Handlers look up,
//! stage on the candidate document and call domain functions; they hold no
//! domain rules. The engine reconciles nets and validates afterwards.
mod instance;
mod library;

pub use instance::InstanceCommand;
pub(crate) use instance::apply_instance;
pub use library::LibraryCommand;
pub(crate) use library::apply_library;

use crate::fields::EffectiveField;
use crate::functions::Resolution;
use crate::ids::{DefinitionId, DefinitionRef, Id};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a successful mutation produced, so callers need not search for new
/// identities.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    #[default]
    None,
    DraftCreated {
        id: DefinitionId,
    },
    Published {
        revisions: Vec<DefinitionRef>,
    },
    /// The new instance, its first occurrence and the designator it took
    /// (the explicit one, or the one the core numbered).
    InstancePlaced {
        instance: Id,
        occurrence: Id,
        designator: String,
    },
    UnitPlaced {
        occurrence: Id,
    },
    /// Every effective field of the instance after `instance.set_properties`.
    PropertiesSet {
        effective: Vec<EffectiveField>,
    },
    /// The instance's function resolution after `instance.select_functions`.
    FunctionsSelected {
        resolution: Resolution,
    },
}
