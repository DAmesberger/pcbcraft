//! The catalog behind the tools: the 229 entries of
//! [`crate::session::metadata`] (requests, operations and queries), looked
//! up by name. A model reaches an entry only through the verb of its
//! [`Kind`] (`query` a query, `apply` an operation, `act` an action), and
//! never a Never-class entry, which has no kind and is unknown here. P2's
//! verbs resolve their `name` argument through [`resolve`]; `find` and the
//! catalog resources (P4) list [`reachable`].
use crate::session::metadata::{self, EntryMeta, Kind};
use pcbcraft_core::Diagnostic;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Every entry a model can reach, in catalog order.
pub fn reachable() -> impl Iterator<Item = &'static EntryMeta> {
    metadata::entries()
        .iter()
        .filter(|entry| entry.kind.is_some())
}

/// The reachable entry named `name`.
pub fn entry(name: &str) -> Option<&'static EntryMeta> {
    static INDEX: OnceLock<BTreeMap<&'static str, &'static EntryMeta>> = OnceLock::new();
    INDEX
        .get_or_init(|| reachable().map(|e| (e.name.as_str(), e)).collect())
        .get(name)
        .copied()
}

/// The entry `name`, reached as `kind`.
pub fn resolve(kind: Kind, name: &str) -> Result<&'static EntryMeta, Unreachable> {
    let entry = entry(name).ok_or_else(|| Unreachable::Unknown(name.to_owned()))?;
    match entry.kind {
        Some(found) if found != kind => Err(Unreachable::OtherKind {
            name: name.to_owned(),
            asked: kind,
            kind: found,
        }),
        // Reachable entries all have a kind.
        _ => Ok(entry),
    }
}

/// Why a name does not resolve. It is the model's mistake, so it answers as
/// a tool result with `isError` ([`Self::reply`]), not a protocol error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Unreachable {
    Unknown(String),
    OtherKind {
        name: String,
        asked: Kind,
        kind: Kind,
    },
}

impl Unreachable {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unknown(_) => "catalog.unknown",
            Self::OtherKind { .. } => "catalog.other_kind",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Unknown(name) => {
                format!("no catalog entry {name}: find {{text}} names the entries")
            }
            Self::OtherKind { name, asked, kind } => format!(
                "{name} is {}, not {}: {}",
                article(*kind),
                article(*asked),
                match kind {
                    Kind::Query => "read it with query",
                    Kind::Operation => "make it with apply",
                    Kind::Action => "run it with act",
                }
            ),
        }
    }

    /// The session envelope of this failure, as [`super::outcome`] reads it.
    pub fn reply(&self) -> Value {
        let diagnostic = Diagnostic {
            code: self.code().into(),
            rule: None,
            message: self.message(),
            expected_revision: None,
            actual_revision: None,
        };
        json!({"ok": false, "error": diagnostic})
    }
}

fn article(kind: Kind) -> &'static str {
    match kind {
        Kind::Query => "a query",
        Kind::Operation => "an operation",
        Kind::Action => "an action",
    }
}
