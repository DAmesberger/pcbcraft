//! Wire-level library and instance command families. Handlers look up,
//! stage on the candidate document and call domain functions; they hold no
//! domain rules. The engine reconciles nets and validates afterwards.
mod instance;
mod library;

pub use instance::InstanceCommand;
pub(crate) use instance::apply_instance;
pub use library::LibraryCommand;
pub(crate) use library::{apply_library, preview_footprint};

use crate::fields::EffectiveField;
use crate::functions::Resolution;
use crate::ids::{DefinitionId, DefinitionRef, GraphicId, Id, TerminalId};
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
    /// Draft pins off the symbol grid after `library.set_grid`.
    GridSet {
        off_grid: Vec<OffGridPin>,
    },
    /// Graphic IDs `library.edit_graphics` allocated, in edit order.
    GraphicsEdited {
        added: Vec<GraphicId>,
    },
    /// What `library.edit_footprint` created and its evaluation findings.
    FootprintEdited {
        report: crate::library::footprint_edit::FootprintEditReport,
    },
    /// The terminal changes of `library.assign_terminals`; `applied` is
    /// false for a preview.
    TerminalsAssigned {
        changes: Vec<crate::library::footprint_edit::TerminalChange>,
        applied: bool,
    },
    /// What `library.import` appended or reused.
    Imported {
        report: crate::library::import::ImportReport,
    },
    /// `library.place_from`: the import (absent for a project revision) and
    /// the placed instance, as `InstancePlaced` reports it.
    PlacedFrom {
        import: Option<crate::library::import::ImportReport>,
        instance: Id,
        occurrence: Id,
        designator: String,
    },
    /// The origins `library.promote` recorded.
    Promoted {
        origins: Vec<crate::library::OriginRecord>,
    },
    /// New draft lineages of `library.duplicate`, component first.
    Duplicated {
        lineages: Vec<DefinitionId>,
    },
}

/// A symbol draft's pin, by its terminal, that lies off the authoring grid.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct OffGridPin {
    pub draft: DefinitionId,
    pub terminal: TerminalId,
}
