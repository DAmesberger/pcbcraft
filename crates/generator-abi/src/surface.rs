//! The capability surface's descriptors (`docs/extension-surface.md`):
//! what every catalog entry and module says about itself, the call
//! envelope and the one error shape.
//!
//! An entry is declared once, beside its Rust type, as an [`EntryDecl`]
//! (`sparksmith_core::declare_entries!` builds them). Everything here is
//! plain data, `no_std` + `alloc` like the rest of this crate, so a plugin
//! can read the same descriptors the host writes. Nothing in a generator
//! module uses this module, so its bytes do not change.
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// How much trust an entry needs. Ordered from least to most strict, so a
/// batch takes [`Class::strictest`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Queries, views and checks: changes nothing.
    Read,
    /// The window only (selection, camera, workspace): no document change,
    /// no revision, no undo step.
    Show,
    /// A document change inside the turn's reviewable, undoable step.
    Edit,
    /// Files, publishing and anything else undo cannot take back: a person
    /// answers Allow or Deny.
    Ask,
    /// Keys, settings, crash reports and the like: refused to any model.
    Never,
}

impl Class {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Show => "show",
            Self::Edit => "edit",
            Self::Ask => "ask",
            Self::Never => "never",
        }
    }

    /// The class of a batch: its strictest member's. An empty batch only reads.
    pub fn strictest(classes: impl IntoIterator<Item = Self>) -> Self {
        classes.into_iter().max().unwrap_or(Self::Read)
    }
}

/// How an entry is reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Reads; changes nothing.
    Query,
    /// Changes the document at an expected revision, as an undo step.
    Operation,
    /// Changes the window only.
    Show,
    /// Files, publishing, anything undo cannot take back.
    Action,
    /// Not callable: a topic to subscribe to.
    Event,
}

/// What an entry's answer depends on. It decides what a plugin may call
/// and what may be cached.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Its parameters only.
    Pure,
    /// The document at a revision.
    Revision,
    /// The clock, files, network, window or settings.
    World,
}

/// A step the person owns, which a caller cannot take for them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Person {
    None,
    /// The person sees the impact or the imported rows before it applies.
    Review,
    /// Allow or Deny before it runs: every action.
    Approval,
}

/// How long an entry runs (targets until measured).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Time {
    /// Under 100 ms.
    Instant,
    /// Under 2 s.
    Short,
    /// Long enough to want progress, cancellation or a job.
    Long,
}

/// Whether an entry's result fits inline (2 KiB) or can grow with the
/// project, the library or a part.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSize {
    Inline,
    Link,
}

/// What an entry can touch outside the document.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    Window,
    Files,
    Network,
    Settings,
    /// Runs a plugin.
    Runtime,
}

impl Reach {
    pub const ALL: [Self; 5] = [
        Self::Window,
        Self::Files,
        Self::Network,
        Self::Settings,
        Self::Runtime,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Files => "files",
            Self::Network => "network",
            Self::Settings => "settings",
            Self::Runtime => "runtime",
        }
    }

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// A host feature an entry or module needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostFeature {
    Window,
    Files,
    /// The plugin runtime.
    Runtime,
    /// Solidcraft's engine: part files, exact models.
    Solidcraft,
}

impl HostFeature {
    pub const ALL: [Self; 4] = [Self::Window, Self::Files, Self::Runtime, Self::Solidcraft];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Files => "files",
            Self::Runtime => "runtime",
            Self::Solidcraft => "solidcraft",
        }
    }

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// A set of [`Reach`]es, built in a `const` declaration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Reaches(u8);

impl Reaches {
    pub const NONE: Self = Self(0);

    #[must_use]
    pub const fn with(self, reach: Reach) -> Self {
        Self(self.0 | reach.bit())
    }

    pub const fn contains(self, reach: Reach) -> bool {
        self.0 & reach.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The members, in [`Reach::ALL`]'s order.
    pub fn iter(self) -> impl Iterator<Item = Reach> {
        Reach::ALL.into_iter().filter(move |r| self.contains(*r))
    }
}

/// A set of [`HostFeature`]s, built in a `const` declaration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Needs(u8);

impl Needs {
    pub const NONE: Self = Self(0);

    #[must_use]
    pub const fn with(self, feature: HostFeature) -> Self {
        Self(self.0 | feature.bit())
    }

    pub const fn contains(self, feature: HostFeature) -> bool {
        self.0 & feature.bit() != 0
    }

    pub fn iter(self) -> impl Iterator<Item = HostFeature> {
        HostFeature::ALL
            .into_iter()
            .filter(move |f| self.contains(*f))
    }
}

/// A module or entry version: `major.minor`.
pub type Version = (u16, u16);

/// The version every entry is `since` unless it says otherwise: the first
/// recorded version of every module.
pub const FIRST: Version = (0, 1);

/// The longest entry title, in bytes.
pub const MAX_TITLE: usize = 40;
/// The longest one-line summary, in bytes.
pub const MAX_SUMMARY: usize = 140;

/// One entry as declared beside its Rust type. Defaults: instant, inline,
/// no person step, no reach, no host needs, the effect its kind gives,
/// since [`FIRST`]. The kind and the summary come from the type (its class,
/// whether it names a path, its doc comment), not from here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntryDecl {
    /// The stable dotted wire name: `layout.add_via`.
    pub name: &'static str,
    /// The module it belongs to: `layout`.
    pub module: &'static str,
    pub class: Class,
    /// A short name for people, at most [`MAX_TITLE`] bytes.
    pub title: &'static str,
    pub time: Time,
    pub result: ResultSize,
    /// [`Person::Review`] where declared; actions add Approval.
    pub person: Person,
    /// What it touches outside the document, as declared. A path-typed
    /// parameter adds [`Reach::Files`] where the catalog is built.
    pub reach: Reaches,
    /// Declared where the kind's default is wrong.
    pub effect: Option<Effect>,
    pub since: Version,
    pub needs: Needs,
}

impl EntryDecl {
    pub const fn new(
        name: &'static str,
        module: &'static str,
        class: Class,
        title: &'static str,
    ) -> Self {
        Self {
            name,
            module,
            class,
            title,
            time: Time::Instant,
            result: ResultSize::Inline,
            person: Person::None,
            reach: Reaches::NONE,
            effect: None,
            since: FIRST,
            needs: Needs::NONE,
        }
    }

    #[must_use]
    pub const fn in_module(mut self, module: &'static str) -> Self {
        self.module = module;
        self
    }

    #[must_use]
    pub const fn short(mut self) -> Self {
        self.time = Time::Short;
        self
    }

    #[must_use]
    pub const fn long(mut self) -> Self {
        self.time = Time::Long;
        self
    }

    #[must_use]
    pub const fn link(mut self) -> Self {
        self.result = ResultSize::Link;
        self
    }

    #[must_use]
    pub const fn review(mut self) -> Self {
        self.person = Person::Review;
        self
    }

    #[must_use]
    pub const fn reach(mut self, reach: Reach) -> Self {
        self.reach = self.reach.with(reach);
        self
    }

    #[must_use]
    pub const fn effect(mut self, effect: Effect) -> Self {
        self.effect = Some(effect);
        self
    }

    #[must_use]
    pub const fn since(mut self, major: u16, minor: u16) -> Self {
        self.since = (major, minor);
        self
    }

    #[must_use]
    pub const fn needs(mut self, feature: HostFeature) -> Self {
        self.needs = self.needs.with(feature);
        self
    }

    /// The effect: as declared, else `world` for an entry that reaches
    /// outside the document and `revision` for every other.
    pub const fn effective_effect(&self, reach: Reaches) -> Effect {
        match self.effect {
            Some(effect) => effect,
            None if reach.is_empty() => Effect::Revision,
            None => Effect::World,
        }
    }
}

/// One module as declared: its identity and version. The digest is
/// computed from its entries (`describe`), never written by hand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModuleDecl {
    /// A lowercase id: `layout`.
    pub id: &'static str,
    pub version: Version,
    pub title: &'static str,
    /// At most [`MAX_SUMMARY`] bytes.
    pub summary: &'static str,
    pub needs: Needs,
}

/// A module as described to a caller.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub id: String,
    /// `major.minor`.
    pub version: String,
    /// `sha256:` and the hex SHA-256 of the module's canonical description.
    pub digest: String,
    pub title: String,
    pub summary: String,
    pub needs: Vec<HostFeature>,
    pub entries: usize,
}

/// One entry as described to a caller (the full level of detail without
/// its schemas, which a host serves on request).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EntryInfo {
    pub name: String,
    pub module: String,
    pub kind: Kind,
    pub class: Class,
    pub effect: Effect,
    pub reach: Vec<Reach>,
    pub person: Person,
    pub time: Time,
    pub result: ResultSize,
    /// `major.minor`.
    pub since: String,
    pub needs: Vec<HostFeature>,
    pub title: String,
    pub summary: String,
}

/// One call, the same for every transport. `project`, `expected_revision`,
/// `dry_run`, `confirm`, `turn` and `job` are the envelope's, never an
/// entry's parameters. `P` is the parameters' type: a host's JSON value (this
/// crate stays free of a JSON tree, so plugins carry no float path).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call<P> {
    pub entry: String,

    pub params: P,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default)]
    pub dry_run: bool,
    /// A plan digest from a dry run, to apply exactly that plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(default)]
    pub job: bool,
}

/// The one error shape, in plain words. `code` is stable and dotted, its
/// first segment the module or `gate`, `catalog`, `host` or `plugin`;
/// `message` ends by saying what was or was not changed.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
    /// A JSON pointer into the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Nearest names for a misspelt entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closest: Vec<String>,
    /// What to do next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instead: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<u64>,
}

/// The `surface.lock` file format this build reads and writes.
pub const LOCK_FORMAT: u32 = 1;

/// `surface.lock`: per module its version and digest, per entry what the
/// ratchet compares (`docs/extension-surface.md`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    pub format: u32,
    pub modules: Vec<LockedModule>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedModule {
    pub id: String,
    /// `major.minor`.
    pub version: String,
    /// `sha256:` and hex.
    pub digest: String,
    /// Sorted by name.
    pub entries: Vec<LockedEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedEntry {
    pub name: String,
    pub class: Class,
    /// The first 16 hex digits of the SHA-256 of the canonical input
    /// schema.
    pub schema: String,
    /// Each top-level parameter with the first 16 hex digits of its
    /// canonical schema, sorted by name.
    pub fields: Vec<(String, String)>,
    /// The required parameters, sorted.
    pub required: Vec<String>,
}

/// `major.minor` as text.
pub fn version_text(version: Version) -> String {
    alloc::format!("{}.{}", version.0, version.1)
}

/// `major.minor` read back.
pub fn parse_version(text: &str) -> Option<Version> {
    let (major, minor) = text.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_order_from_read_to_never() {
        assert!(Class::Read < Class::Show && Class::Show < Class::Edit);
        assert!(Class::Edit < Class::Ask && Class::Ask < Class::Never);
        assert_eq!(Class::strictest([]), Class::Read);
        assert_eq!(Class::strictest([Class::Show, Class::Read]), Class::Show);
    }

    #[test]
    fn a_declaration_takes_its_defaults_and_attributes() {
        const PLAIN: EntryDecl =
            EntryDecl::new("layout.add_via", "layout", Class::Edit, "Add a via");
        assert_eq!(PLAIN.time, Time::Instant);
        assert_eq!(PLAIN.result, ResultSize::Inline);
        assert_eq!(PLAIN.since, FIRST);
        assert_eq!(PLAIN.effective_effect(PLAIN.reach), Effect::Revision);
        const EXPORT: EntryDecl = EntryDecl::new("fab.export", "project", Class::Ask, "Export")
            .in_module("fab")
            .long()
            .link()
            .reach(Reach::Files)
            .needs(HostFeature::Files);
        assert_eq!(EXPORT.module, "fab");
        assert_eq!(EXPORT.time, Time::Long);
        assert_eq!(EXPORT.reach.iter().collect::<Vec<_>>(), [Reach::Files]);
        assert_eq!(EXPORT.effective_effect(EXPORT.reach), Effect::World);
        assert!(EXPORT.needs.contains(HostFeature::Files));
    }

    #[test]
    fn versions_read_back() {
        assert_eq!(version_text((0, 1)), "0.1");
        assert_eq!(parse_version("1.12"), Some((1, 12)));
        assert_eq!(parse_version("1"), None);
    }
}
