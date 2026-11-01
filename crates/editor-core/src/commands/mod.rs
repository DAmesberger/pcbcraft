//! Wire-level library and instance command families. Handlers look up,
//! stage on the candidate document and call domain functions; they hold no
//! domain rules. The engine reconciles nets and validates afterwards.
mod assets;
mod instance;
mod library;
pub(crate) mod marking;
pub(crate) mod model3d;
pub(crate) mod pinout;
pub(crate) mod templates;

pub use instance::InstanceCommand;
pub(crate) use instance::apply_instance;
pub use library::LibraryCommand;
pub(crate) use library::{apply_library, preview_footprint};
/// `instance.marking` as a pure function of a valid project, for hosts that
/// lay out many instances at once (the model worker's decals) without an
/// engine.
pub use marking::layout as instance_marking;

use crate::fields::EffectiveField;
use crate::functions::Resolution;
use crate::ids::{ComponentRef, DefinitionId, DefinitionRef, GraphicId, Id, TerminalId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a successful mutation produced, so callers need not search for new
/// identities.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    #[default]
    None,
    /// The via `layout.add_via` created.
    ViaAdded {
        via: Id,
    },
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
    /// `library.fit_leads`: the solved placement, stored only if `applied`.
    LeadsFitted {
        fit: crate::library::package_fit::LeadFit,
        applied: bool,
    },
    /// B1 `library.new_from_template` and `library.draft_closure`: the new
    /// drafts, component first.
    DraftsCreated {
        ids: Vec<DefinitionId>,
    },
    /// `library.edit_copy`: the copied component revision, what the import
    /// appended or reused, and the lineages it drafted.
    Copied {
        component: ComponentRef,
        import: crate::library::import::ImportReport,
        drafted: Vec<DefinitionId>,
    },
    /// B1 `library.package_from_template`: what changed.
    PackageReplaced {
        plan: crate::library::templates::catalog::PackagePlan,
    },
    /// C3 `library.pinout_apply`.
    PinoutApplied {
        report: crate::library::pinout::PinoutApplied,
    },
    /// The class `net_class.add` created.
    NetClassAdded {
        class: crate::rules::NetClassId,
    },
    /// The fill `fill.add` created.
    FillAdded {
        fill: Id,
    },
    /// The keepout `keepout.add` created.
    KeepoutAdded {
        keepout: Id,
    },
    /// The constraint `board.constrain` created.
    EdgeConstraintAdded {
        constraint: Id,
    },
}

/// A symbol draft's pin, by its terminal, that lies off the authoring grid.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct OffGridPin {
    pub draft: DefinitionId,
    pub terminal: TerminalId,
}
