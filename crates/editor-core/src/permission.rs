//! The catalog's declarations: every entry's class, module and the rest of
//! what it says about itself (`docs/extension-surface.md`).
//!
//! Every command family lists its variants with [`declare_entries!`]
//! beside its enum. The listing expands to an exhaustive `match`, so a new
//! variant does not compile until it has chosen a class, and every fact
//! about an entry (module, cost, result size, person step, reach, effect,
//! since, needs) is written on that one line. The descriptor types are the
//! ABI crate's ([`sparksmith_generator_abi::surface`]); the rules that act
//! on a class live in `sparksmith_automation::gate`.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use sparksmith_generator_abi::surface::{
    Class, Effect, EntryDecl, FIRST, HostFeature, MAX_SUMMARY, MAX_TITLE, ModuleDecl, Needs,
    Person, Reach, Reaches, ResultSize, Time, Version,
};

/// The modules of the catalog, each with its version. Before the MCP
/// cut-over every module is `0.x` (`docs/design/decisions-2026-11-14.md`);
/// `cargo xtask surface --check` holds each against `surface.lock`.
pub const MODULES: &[ModuleDecl] = &[
    module_at(
        "project",
        (0, 2),
        "Project",
        "The open design: inspect, properties, undo, redo, bill of materials.",
    ),
    module("files", "Files", "New, open and save a project file."),
    module_at(
        "schematic",
        (0, 2),
        "Schematic",
        "Sheets, symbols, wires, labels, placed components and their pins.",
    ),
    module(
        "layout",
        "Layout",
        "Place parts, route copper, vias, fills, keepouts, pairs and the board shape.",
    ),
    module(
        "rules",
        "Rules",
        "Design rules, fab profiles, net classes and impedance.",
    ),
    module_at(
        "library",
        (0, 2),
        "Library",
        "Component definitions, drafts, publishing, folders, updates and pinout import.",
    ),
    module(
        "generators",
        "Generators",
        "Generator modules and the parts they make.",
    ),
    module(
        "checks",
        "Checks",
        "Issues, the production check and its waivers.",
    ),
    module(
        "fab",
        "Fabrication",
        "Fabrication output, the schematic PDF and firmware pin configuration.",
    ),
    module(
        "model3d",
        "3D model",
        "The exact 3D export, the board interface, the assembly link and part files.",
    ),
];

const fn module(id: &'static str, title: &'static str, summary: &'static str) -> ModuleDecl {
    module_at(id, (0, 1), title, summary)
}

/// A module at `version`, raised from 0.1 by a change the lock records
/// (0.2: net disagreements and pins added by import).
const fn module_at(
    id: &'static str,
    version: Version,
    title: &'static str,
    summary: &'static str,
) -> ModuleDecl {
    ModuleDecl {
        id,
        version,
        title,
        summary,
        needs: Needs::NONE,
    }
}

/// A file or folder path in a command's parameters. Its schema carries
/// `"x-kind": "path"` ([`PATH_KIND`]), so the gate finds every path a
/// request names by walking the request with its schema, and a parameter
/// struct that takes a path says so by its type. On the wire it is a plain
/// string.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FilePath(pub String);

/// The schema keyword that marks a [`FilePath`].
pub const PATH_KIND: &str = "x-kind";

impl FilePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for FilePath {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for FilePath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<std::path::Path> for FilePath {
    fn as_ref(&self) -> &std::path::Path {
        std::path::Path::new(&self.0)
    }
}

impl AsRef<std::ffi::OsStr> for FilePath {
    fn as_ref(&self) -> &std::ffi::OsStr {
        std::ffi::OsStr::new(&self.0)
    }
}

impl From<FilePath> for std::path::PathBuf {
    fn from(path: FilePath) -> Self {
        Self::from(path.0)
    }
}

impl From<FilePath> for String {
    fn from(path: FilePath) -> Self {
        path.0
    }
}

impl From<std::path::PathBuf> for FilePath {
    fn from(path: std::path::PathBuf) -> Self {
        Self(path.display().to_string())
    }
}

impl From<String> for FilePath {
    fn from(path: String) -> Self {
        Self(path)
    }
}

impl From<&str> for FilePath {
    fn from(path: &str) -> Self {
        Self(path.to_owned())
    }
}

impl std::fmt::Display for FilePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl JsonSchema for FilePath {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FilePath".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "x-kind": "path"})
    }
}

/// The entries of one command enum, one line per variant,
/// `Variant "wire.name" Class "Title" [attributes]`, in a module:
/// `declare_entries!(LayoutCommand in layout { … })`. Defines `class(&self)`,
/// `command(&self)` and the table `ENTRIES`. The title is a short name for
/// people (an MCP `title`), at most [`MAX_TITLE`] bytes; what the command
/// does is its doc comment. The optional attributes, comma separated:
/// `in module` (another module than the family's), `short`, `long`, `link`
/// (a result that can pass 2 KiB), `review` (the person reviews its impact
/// before it applies), `reach(Files | Settings | Window | Network |
/// Runtime)`, `effect(Pure | Revision | World)`, `needs(Files | Window |
/// Runtime | Solidcraft)` and `since(major, minor)`.
#[macro_export]
macro_rules! declare_entries {
    ($family:ident in $module:ident {
        $($variant:ident $name:literal $class:ident $title:literal $([$($attr:tt)*])?,)+
    }) => {
        impl $family {
            /// Every command of this enum as declared.
            pub const ENTRIES: &'static [$crate::permission::EntryDecl] = &[$(
                $crate::__entry_decl!(
                    $crate::permission::EntryDecl::new(
                        $name,
                        stringify!($module),
                        $crate::permission::Class::$class,
                        $title,
                    );
                    $($($attr)*)?
                ),
            )+];

            /// This command's permission class.
            pub const fn class(&self) -> $crate::permission::Class {
                match self {
                    $(Self::$variant { .. } => $crate::permission::Class::$class,)+
                }
            }

            /// This command's wire name.
            pub const fn command(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => $name,)+
                }
            }
        }
    };
}

/// One entry's attributes applied to its declaration ([`declare_entries!`]).
#[doc(hidden)]
#[macro_export]
macro_rules! __entry_decl {
    ($decl:expr;) => { $decl };
    ($decl:expr; , $($rest:tt)*) => { $crate::__entry_decl!($decl; $($rest)*) };
    ($decl:expr; in $module:ident $($rest:tt)*) => {
        $crate::__entry_decl!($decl.in_module(stringify!($module)); $($rest)*)
    };
    ($decl:expr; reach($reach:ident) $($rest:tt)*) => {
        $crate::__entry_decl!($decl.reach($crate::permission::Reach::$reach); $($rest)*)
    };
    ($decl:expr; effect($effect:ident) $($rest:tt)*) => {
        $crate::__entry_decl!($decl.effect($crate::permission::Effect::$effect); $($rest)*)
    };
    ($decl:expr; needs($feature:ident) $($rest:tt)*) => {
        $crate::__entry_decl!($decl.needs($crate::permission::HostFeature::$feature); $($rest)*)
    };
    ($decl:expr; since($major:literal, $minor:literal) $($rest:tt)*) => {
        $crate::__entry_decl!($decl.since($major, $minor); $($rest)*)
    };
    ($decl:expr; $flag:ident $($rest:tt)*) => {
        $crate::__entry_decl!($decl.$flag(); $($rest)*)
    };
}

/// The declaration of the entry named `name` in `entries`.
pub fn find<'a>(
    entries: impl IntoIterator<Item = &'a EntryDecl>,
    name: &str,
) -> Option<&'a EntryDecl> {
    entries.into_iter().find(|entry| entry.name == name)
}

#[cfg(test)]
mod tests {
    use super::{Class, MODULES, Reach, ResultSize, Time};
    use crate::{Operation, Query};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;

    fn tags(value: &Value, key: &str, out: &mut BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                for (name, inner) in object {
                    if name == key
                        && let Some(tag) = inner["const"].as_str()
                    {
                        out.insert(tag.to_owned());
                    }
                    tags(inner, key, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| tags(item, key, out)),
            _ => {}
        }
    }

    /// The tables are compared with the generated schemas, which come from
    /// the serde attributes, so a mistyped wire name cannot pass.
    #[test]
    fn the_entry_tables_name_exactly_the_wire_commands() -> Result<(), serde_json::Error> {
        let schema = serde_json::to_value(schemars::schema_for!(Operation))?;
        let mut wire = BTreeSet::new();
        tags(&schema, "command", &mut wire);
        wire.insert("update".to_owned());
        let listed: BTreeSet<String> = Operation::entries()
            .map(|entry| entry.name.to_owned())
            .collect();
        assert_eq!(wire, listed);

        let schema = serde_json::to_value(schemars::schema_for!(Query))?;
        let mut wire = BTreeSet::new();
        tags(&schema, "query", &mut wire);
        let listed: BTreeSet<String> = Query::ENTRIES
            .iter()
            .map(|entry| entry.name.to_owned())
            .collect();
        assert_eq!(wire, listed);
        Ok(())
    }

    #[test]
    fn every_query_only_reads() {
        for entry in Query::ENTRIES {
            assert_eq!(entry.class, Class::Read, "{}", entry.name);
        }
    }

    #[test]
    fn every_entry_is_in_a_declared_module() {
        let modules: BTreeSet<&str> = MODULES.iter().map(|m| m.id).collect();
        assert_eq!(modules.len(), MODULES.len(), "a module is declared twice");
        for entry in Operation::entries().chain(Query::ENTRIES) {
            assert!(
                modules.contains(entry.module),
                "{}: {}",
                entry.name,
                entry.module
            );
        }
    }

    #[test]
    fn attributes_land_on_their_entry() -> Result<(), String> {
        let entry = |name: &str| {
            super::find(Operation::entries().chain(Query::ENTRIES), name)
                .ok_or_else(|| format!("{name} is not declared"))
        };
        let refill = entry("fill.refill")?;
        assert_eq!((refill.module, refill.time), ("layout", Time::Long));
        let issues = entry("project.issues")?;
        assert_eq!(issues.module, "checks");
        assert_eq!((issues.time, issues.result), (Time::Long, ResultSize::Link));
        assert!(entry("layout.add_via")?.reach.is_empty());
        assert!(!entry("library.publish")?.reach.contains(Reach::Files));
        assert_eq!(entry("update")?.person, super::Person::Review);
        Ok(())
    }

    #[test]
    fn an_instance_answers_with_its_own_wire_name_and_class() -> Result<(), serde_json::Error> {
        let publish: Operation = serde_json::from_value(json!({
            "editor": "Library",
            "operation": {"command": "library.publish", "params": {"drafts": []}},
        }))?;
        let wire = serde_json::to_value(&publish)?;
        assert_eq!(wire["operation"]["command"], publish.command());
        assert_eq!(publish.class(), Class::Ask);
        let discard: Operation = serde_json::from_value(json!({
            "editor": "Library",
            "operation": {"command": "library.discard_draft", "params": {"id": 1}},
        }))?;
        let wire = serde_json::to_value(&discard)?;
        assert_eq!(wire["operation"]["command"], discard.command());
        assert_eq!(discard.class(), Class::Edit);
        Ok(())
    }

    #[test]
    fn a_path_is_a_string_marked_in_its_schema() -> Result<(), serde_json::Error> {
        let schema = serde_json::to_value(schemars::schema_for!(super::FilePath))?;
        assert_eq!(schema["type"], "string");
        assert_eq!(schema[super::PATH_KIND], "path");
        let path: super::FilePath = serde_json::from_value(json!("/a/b"))?;
        assert_eq!(serde_json::to_value(&path)?, json!("/a/b"));
        Ok(())
    }

    #[test]
    fn a_batch_takes_its_strictest_member() {
        assert_eq!(Class::strictest([]), Class::Read);
        assert_eq!(Class::strictest([Class::Read, Class::Edit]), Class::Edit);
        assert_eq!(
            Class::strictest([Class::Never, Class::Read, Class::Ask]),
            Class::Never
        );
    }
}
