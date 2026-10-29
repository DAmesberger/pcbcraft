//! Typed command failures and design-check findings. Codes are stable strings
//! for automation; messages are for people and may change wording.
use crate::{Endpoint, Id};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Malformed or out-of-range input.
    #[error("{0}")]
    Invalid(String),
    /// A referenced object, terminal or net does not exist.
    #[error("{0}")]
    NotFound(String),
    /// Valid input that the current document state does not permit
    /// (locked objects, routed copper, duplicates, newer revisions).
    #[error("{0}")]
    Conflict(String),
    /// A supported size, count or identity limit would be exceeded.
    #[error("{0}")]
    Limit(String),
    /// The request was made against an older document revision.
    #[error("stale revision: expected {expected}, current {actual}")]
    Stale { expected: u64, actual: u64 },
    /// A file or feature version this build cannot represent.
    #[error("{0}")]
    Unsupported(String),
    /// Undo or redo with an empty history.
    #[error("{0}")]
    History(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

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
    pub fn diagnostic(&self) -> Diagnostic {
        let (expected_revision, actual_revision) = match self {
            Self::Stale { expected, actual } => (Some(*expected), Some(*actual)),
            _ => (None, None),
        };
        Diagnostic {
            code: self.code().into(),
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
    Instance { id: Id },
    Terminal { endpoint: Endpoint },
    Net { id: Id },
    Track { id: Id },
    Pad { instance: Id, pad: Id },
}

/// One electrical-rule or board-check result.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Finding {
    /// Stable machine-readable rule code, e.g. `board.clearance`.
    pub code: String,
    pub message: String,
    pub objects: Vec<ObjectRef>,
}

impl Finding {
    pub fn new(code: &str, message: String, objects: Vec<ObjectRef>) -> Self {
        Self {
            code: code.into(),
            message,
            objects,
        }
    }
}
