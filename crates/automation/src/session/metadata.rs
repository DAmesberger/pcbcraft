//! What each catalog entry (the requests, `project.apply`'s operations and
//! `project.query`'s queries) is, for the MCP surface (`docs/mcp-surface.md`
//! §3) and `surface.lock`: a title, a one-line summary, its module, kind,
//! whether the person owns a step of it, whether it can run long, whether
//! its result fits inline, its reach and effect, and the MCP tool
//! annotations its class gives.
//!
//! Everything here is derived from the typed catalog: the declarations
//! beside each enum ([`sparksmith_core::declare_entries!`]), summaries from
//! the request catalog and the operations' and queries' doc comments, and
//! whether a request names a path from its parameter types
//! ([`super::paths`]). No list here names an entry.
use super::discovery::{self, Entry};
use serde::Serialize;
use sparksmith_core::Class;
use sparksmith_core::permission::{
    Effect, EntryDecl, HostFeature, Needs, Reach, Reaches, Time, Version,
};
pub use sparksmith_core::permission::{Person, ResultSize};
use std::sync::OnceLock;

/// The longest one-line summary, in bytes: what a `find` hit and a catalog
/// index row carry.
pub const MAX_SUMMARY: usize = sparksmith_core::permission::MAX_SUMMARY;
/// A result larger than this many bytes is a candidate for a resource link
/// instead of inline content (`docs/mcp-surface.md` §3.4).
pub const INLINE_RESULT: usize = 2 * 1024;

/// How an entry is reached: a read, an undoable document edit, or an action
/// on files, folders or what undo cannot take back.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Query,
    Operation,
    Action,
}

/// MCP `ToolAnnotations` (`2026-07-28` schema). `destructiveHint` and
/// `idempotentHint` mean something only when `readOnlyHint` is false, so a
/// read leaves `destructiveHint` out.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Annotations {
    #[serde(rename = "readOnlyHint")]
    pub read_only: bool,
    #[serde(rename = "destructiveHint", skip_serializing_if = "Option::is_none")]
    pub destructive: Option<bool>,
    #[serde(rename = "idempotentHint")]
    pub idempotent: bool,
    #[serde(rename = "openWorldHint")]
    pub open_world: bool,
}

/// One catalog entry's metadata.
#[derive(Clone, Debug, Serialize)]
pub struct EntryMeta {
    pub name: String,
    /// `command`, `operation` or `query`: where in today's catalog it is.
    pub family: &'static str,
    /// The module it is declared in.
    #[serde(skip)]
    pub module: &'static str,
    pub title: &'static str,
    pub summary: String,
    pub class: Class,
    pub takes_path: bool,
    /// `None` for the Never class, which no model reaches.
    pub kind: Option<Kind>,
    pub person: Option<Person>,
    pub long: bool,
    pub result: ResultSize,
    /// What it touches outside the document: as declared, and files when
    /// it names a path.
    #[serde(skip)]
    pub reach: Reaches,
    #[serde(skip)]
    pub effect: Effect,
    #[serde(skip)]
    pub needs: Needs,
    #[serde(skip)]
    pub since: Version,
    /// `None` for the Never class.
    pub annotations: Option<Annotations>,
}

/// Every catalog entry, in catalog order.
pub fn entries() -> &'static [EntryMeta] {
    static ENTRIES: OnceLock<Vec<EntryMeta>> = OnceLock::new();
    ENTRIES.get_or_init(|| discovery::entries().iter().map(meta).collect())
}

/// An entry's reach: its declared one, with files when it names a path.
pub fn reach(decl: Option<&EntryDecl>, takes_path: bool) -> Reaches {
    let declared = decl.map_or(Reaches::NONE, |d| d.reach);
    if takes_path {
        declared.with(Reach::Files)
    } else {
        declared
    }
}

fn meta(entry: &Entry) -> EntryMeta {
    let decl = entry.decl;
    let kind = kind(entry.class, entry.takes_path);
    let reach = reach(decl, entry.takes_path);
    let needs = decl.map_or(Needs::NONE, |d| d.needs);
    EntryMeta {
        name: entry.name.clone(),
        family: entry.kind,
        module: decl.map_or("", |d| d.module),
        title: entry.title,
        summary: discovery::summary_of(entry),
        class: entry.class,
        takes_path: entry.takes_path,
        kind,
        person: match kind {
            Some(Kind::Action) => Some(Person::Approval),
            _ if decl.is_some_and(|d| d.person == Person::Review) => Some(Person::Review),
            _ => None,
        },
        long: decl.is_some_and(|d| d.time == Time::Long),
        result: decl.map_or(ResultSize::Inline, |d| d.result),
        reach,
        effect: decl.map_or(Effect::World, |d| d.effective_effect(reach)),
        needs: if reach.contains(Reach::Files) {
            needs.with(HostFeature::Files)
        } else {
            needs
        },
        since: decl.map_or(sparksmith_core::permission::FIRST, |d| d.since),
        annotations: annotations(entry.class, !reach.is_empty()),
    }
}

/// A read without a path is a query and an edit without one an operation;
/// a path, or the Ask class, makes an action.
pub fn kind(class: Class, takes_path: bool) -> Option<Kind> {
    match (class, takes_path) {
        (Class::Never, _) => None,
        (Class::Ask, _) | (_, true) => Some(Kind::Action),
        (Class::Read, false) => Some(Kind::Query),
        // No entry steers the window yet; until the `show` verb a window
        // entry is reached like an edit.
        (Class::Edit | Class::Show, false) => Some(Kind::Operation),
    }
}

/// The hints a class gives (`docs/mcp-surface.md` §3.1): a read is
/// read-only and idempotent; an edit is undoable inside the reviewable turn,
/// so not destructive; Ask is what undo cannot take back, so destructive.
/// Open world means it reaches outside the document: a path, a library
/// folder or the person's settings. Never has none, since no model reaches it.
pub fn annotations(class: Class, outside: bool) -> Option<Annotations> {
    let (read_only, destructive) = match class {
        Class::Read => (true, None),
        Class::Show | Class::Edit => (false, Some(false)),
        Class::Ask => (false, Some(true)),
        Class::Never => return None,
    };
    Some(Annotations {
        read_only,
        destructive,
        idempotent: read_only,
        open_world: outside,
    })
}

impl super::CommandInfo {
    /// This command's annotations, as its catalog entry has them.
    pub fn annotations(&self) -> Option<Annotations> {
        annotations(self.class, !reach(self.decl, self.takes_path).is_empty())
    }
}
