//! What each of the 237 catalog entries (the requests, `project.apply`'s
//! operations and `project.query`'s queries) is, for the MCP surface
//! (`docs/mcp-surface.md` §3): a title, a one-line summary, its kind, whether
//! the person owns a step of it, whether it can run long, whether its result
//! fits inline, and the MCP tool annotations its class gives.
//!
//! Everything here is derived from the typed catalog: titles from the class
//! listings beside each enum ([`sparksmith_core::declare_classes!`]), summaries
//! from the request catalog and the operations' and queries' doc comments,
//! and the rest from [`Class`], [`super::Request::NAMING_PATHS`] and the short lists
//! below, each checked against the catalog by `metadata_tests.rs`.
use super::discovery::{self, Entry};
use serde::Serialize;
use sparksmith_core::Class;
use std::sync::OnceLock;

/// The longest one-line summary, in bytes: what a `find` hit and a catalog
/// index row carry.
pub const MAX_SUMMARY: usize = 140;
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

/// A step the person owns, which a model cannot take for them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Person {
    /// Allow or Deny before it runs: every action.
    Approval,
    /// The person sees the impact or the imported rows before it applies.
    Review,
}

/// Whether a result fits [`INLINE_RESULT`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSize {
    /// Bounded by the request: one object, one calculation, a compact
    /// mutation reply.
    Inline,
    /// Grows with the project, the library or a part: lists, reports,
    /// schemas, previews.
    Link,
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

/// Requests that read or write outside the document without naming a path:
/// the person's library folders and their settings.
pub const OUTSIDE_DOCUMENT: &[&str] = &[
    "library.sources",
    "library.rescan",
    "library.favourite",
    "library.bundle",
    "library.import_from_folder",
    "library.place_from_folder",
    "library.publish_to_folder",
    "library.open_preview",
];

/// Entries whose impact the person reviews before it applies: an update
/// that moves instances, wiring or routed copper, and a datasheet import.
pub const REVIEWED: &[&str] = &["project.apply_update", "update", "library.pinout_apply"];

/// Entries that can run long enough to want progress, cancellation or a
/// task: production checks and output, 3D export, fills, folder rescans and
/// previews of large parts (`docs/mcp-surface.md` §3.8).
pub const LONG: &[&str] = &[
    "project.export_fabrication",
    "project.export_native",
    "fabrication.preflight",
    "project.issues",
    "library.rescan",
    "fill.refill",
    "fill.store_all",
    "library.footprint_preview",
    "library.pinout_preview",
    "library.package_plan",
];

/// Entries whose result grows with the project, the library or a part, so
/// it can pass [`INLINE_RESULT`]. Every other entry answers with a compact
/// reply bounded by its request.
pub const LINKED: &[&str] = &[
    // Requests.
    "project.inspect",
    "project.schema",
    "project.query",
    "project.preview_update",
    "project.export_fabrication",
    "library.sources",
    "library.rescan",
    "library.bundle",
    "library.open_preview",
    "library.pinout_schema",
    "firmware.export_preview",
    "library.inspect_model_file",
    "library.marking_surfaces",
    "library.check_part_file",
    "library.use_part_file",
    "library.set_part_values",
    "library.part_files",
    "layout.place_part_file",
    "layout.set_part_file_values",
    // Queries.
    "component.inspect",
    "component.check",
    "component.check_drafts",
    "library.search",
    "library.generic_parts",
    "library.facets",
    "library.inspect",
    "project.bom",
    "library.resolve_groups",
    "library.footprint_preview",
    "library.updates_available",
    "library.promote_preview",
    "project.issues",
    "library.templates",
    "library.standard_fields",
    "library.package_plan",
    "library.models",
    "library.pinout_preview",
    "functions.suggest",
    "project.pin_report",
    "project.pin_config",
    "rules.effective",
    "rules.fab_profiles",
    "rules.preview_fab_profile",
    "net_class.list",
    "net_class.preview_patterns",
    "board.shape",
    "fill.status",
    "schematic.sheets",
    "schematic.net_places",
    "schematic.copy",
    "schematic.scene",
    "impedance.table",
    "diff_pair.suggest",
    "layout.length_report",
    "layout.route_preview",
    "layout.track_drag_preview",
    "layout.tune_preview",
    "fabrication.preflight",
];

/// One catalog entry's metadata.
#[derive(Clone, Debug, Serialize)]
pub struct EntryMeta {
    pub name: String,
    /// `command`, `operation` or `query`: where in today's catalog it is.
    pub family: &'static str,
    pub title: &'static str,
    pub summary: String,
    pub class: Class,
    pub takes_path: bool,
    /// `None` for the Never class, which no model reaches.
    pub kind: Option<Kind>,
    pub person: Option<Person>,
    pub long: bool,
    pub result: ResultSize,
    /// `None` for the Never class.
    pub annotations: Option<Annotations>,
}

/// Every catalog entry, in catalog order.
pub fn entries() -> &'static [EntryMeta] {
    static ENTRIES: OnceLock<Vec<EntryMeta>> = OnceLock::new();
    ENTRIES.get_or_init(|| discovery::entries().iter().map(meta).collect())
}

fn meta(entry: &Entry) -> EntryMeta {
    let name = entry.name.as_str();
    let kind = kind(entry.class, entry.takes_path);
    EntryMeta {
        name: entry.name.clone(),
        family: entry.kind,
        title: entry.title,
        summary: discovery::summary_of(entry),
        class: entry.class,
        takes_path: entry.takes_path,
        kind,
        person: match kind {
            Some(Kind::Action) => Some(Person::Approval),
            _ if REVIEWED.contains(&name) => Some(Person::Review),
            _ => None,
        },
        long: LONG.contains(&name),
        result: if LINKED.contains(&name) {
            ResultSize::Link
        } else {
            ResultSize::Inline
        },
        annotations: annotations(
            entry.class,
            entry.takes_path || OUTSIDE_DOCUMENT.contains(&name),
        ),
    }
}

/// A read without a path is a query and an edit without one an operation;
/// a path, or the Ask class, makes an action.
pub fn kind(class: Class, takes_path: bool) -> Option<Kind> {
    match (class, takes_path) {
        (Class::Never, _) => None,
        (Class::Ask, _) | (_, true) => Some(Kind::Action),
        (Class::Read, false) => Some(Kind::Query),
        (Class::Edit, false) => Some(Kind::Operation),
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
        Class::Edit => (false, Some(false)),
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
        annotations(
            self.class,
            self.takes_path || OUTSIDE_DOCUMENT.contains(&self.id),
        )
    }
}
