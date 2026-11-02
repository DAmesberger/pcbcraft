//! Typed command failures and design-check findings. Codes are stable strings
//! for automation; messages are for people and may change wording.
use crate::ids::{DefinitionId, Endpoint, Id, PadId, UnitId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Malformed or out-of-range input.
    #[error("{}", shown(.0))]
    Invalid(String),
    /// A referenced object, terminal or net does not exist.
    #[error("{}", shown(.0))]
    NotFound(String),
    /// Valid input that the current document state does not permit
    /// (locked objects, routed copper, duplicates, newer revisions).
    #[error("{}", shown(.0))]
    Conflict(String),
    /// A supported size, count or identity limit would be exceeded.
    #[error("{}", shown(.0))]
    Limit(String),
    /// The request was made against an older document revision.
    #[error("stale revision: expected {expected}, current {actual}")]
    Stale { expected: u64, actual: u64 },
    /// A file or feature version this build cannot represent.
    #[error("{}", shown(.0))]
    Unsupported(String),
    /// Undo or redo with an empty history.
    #[error("{}", shown(.0))]
    History(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A refusal text split into its leading rule code and the sentence for
/// people: `"footprint.explicit: this footprint has explicit pads"` gives
/// `(Some("footprint.explicit"), "this footprint has explicit pads")`. A
/// rule code is two or more lower-case words joined by dots.
pub fn split_rule(text: &str) -> (Option<&str>, &str) {
    let Some((head, rest)) = text.split_once(": ") else {
        return (None, text);
    };
    let word =
        |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    if head.split('.').count() > 1 && head.split('.').all(word) {
        (Some(head), rest)
    } else {
        (None, text)
    }
}

/// The text people see: the sentence without its rule code. A sentence
/// that followed a code starts with a capital letter.
fn shown(text: &str) -> String {
    let (rule, sentence) = split_rule(text);
    if rule.is_none() {
        return sentence.to_owned();
    }
    let mut chars = sentence.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_input",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Limit(_) => "limit_exceeded",
            Self::Stale { .. } => "stale_revision",
            Self::Unsupported(_) => "unsupported",
            Self::History(_) => "history_empty",
            Self::Json(_) => "malformed_request",
        }
    }
    /// The rule code a refusal names, such as `footprint.explicit`, for
    /// hosts that choose wording or a follow-up by code rather than by text.
    pub fn rule(&self) -> Option<&str> {
        match self {
            Self::Invalid(text)
            | Self::NotFound(text)
            | Self::Conflict(text)
            | Self::Limit(text)
            | Self::Unsupported(text)
            | Self::History(text) => split_rule(text).0,
            Self::Stale { .. } | Self::Json(_) => None,
        }
    }
    pub fn diagnostic(&self) -> Diagnostic {
        let (expected_revision, actual_revision) = match self {
            Self::Stale { expected, actual } => (Some(*expected), Some(*actual)),
            _ => (None, None),
        };
        Diagnostic {
            code: self.code().into(),
            rule: self.rule().map(str::to_owned),
            message: self.to_string(),
            expected_revision,
            actual_revision,
        }
    }
}

/// Serializable form of a failure, shared by every transport.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Diagnostic {
    pub code: String,
    /// The rule a refusal names, such as `footprint.explicit`, when it
    /// names one. The message never repeats it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<u64>,
}

/// An object a finding refers to, by stable identity.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectRef {
    Instance {
        id: Id,
    },
    Terminal {
        endpoint: Endpoint,
    },
    Net {
        id: Id,
    },
    Track {
        id: Id,
    },
    Pad {
        instance: Id,
        pad: PadId,
    },
    /// A definition lineage (draft or published revision).
    Definition {
        id: DefinitionId,
    },
    /// A local object of a definition (terminal, unit, package terminal or
    /// pad), by its local ID within that lineage.
    LibraryObject {
        definition: DefinitionId,
        local: u32,
    },
    /// A symbol occurrence on the sheet.
    Occurrence {
        id: Id,
    },
    /// One unit of one instance, placed or not.
    Unit {
        instance: Id,
        unit: UnitId,
    },
    /// One field of one instance, by field lineage.
    Property {
        instance: Id,
        field: DefinitionId,
    },
    /// A board via.
    Via {
        id: Id,
    },
    /// A copper fill (zone).
    Fill {
        id: Id,
    },
    /// A keepout region.
    Keepout {
        id: Id,
    },
    /// The board as a whole, for a finding about the board rather than any
    /// one object on it (a check that stopped at its limit). Locating it
    /// frames the board.
    Board,
    /// A net label or power port on a schematic sheet.
    Label {
        id: Id,
    },
    /// A hierarchical sheet symbol.
    SheetSymbol {
        id: Id,
    },
    /// A pin on a sheet symbol.
    SheetPin {
        id: Id,
    },
    /// A schematic sheet.
    Sheet {
        sheet: crate::ids::SheetId,
    },
}

/// A derived completeness state a finding can block. Nothing stores these
/// states; they hold when no current finding blocks them.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// A definition revision may be published.
    Publish,
    /// An instance can be placed on the board.
    BoardPlaceable,
    /// Every unit and terminal of an instance has an explicit decision.
    ElectricallyResolved,
}

/// One check result: an electrical rule, a board check or a completeness
/// finding. Circuit and board findings never block a component stage.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Finding {
    /// Stable machine-readable rule code, e.g. `board.clearance`.
    pub code: String,
    pub message: String,
    pub objects: Vec<ObjectRef>,
    /// The stage this finding prevents, if any. Advisory findings have none.
    #[serde(default)]
    pub blocks: Option<Stage>,
}

impl Finding {
    /// An advisory finding that blocks no stage.
    pub fn new(code: &str, message: String, objects: Vec<ObjectRef>) -> Self {
        Self {
            code: code.into(),
            message,
            objects,
            blocks: None,
        }
    }
    /// A finding that prevents `stage` while it exists.
    pub fn blocking(code: &str, message: String, objects: Vec<ObjectRef>, stage: Stage) -> Self {
        Self {
            blocks: Some(stage),
            ..Self::new(code, message, objects)
        }
    }
    pub fn blocks_stage(&self, stage: Stage) -> bool {
        self.blocks == Some(stage)
    }
}
