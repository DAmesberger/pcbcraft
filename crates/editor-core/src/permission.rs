//! Permission classes: how much trust a command needs when an LLM asks for it.
//!
//! Every command family lists its variants with [`declare_classes!`] beside
//! its enum. The listing expands to an exhaustive `match`, so a new variant
//! does not compile until it has chosen a class. The rules that act on a
//! class live in `pcbcraft_automation::gate`.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Ordered from least to most strict, so a batch takes [`Class::strictest`].
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Queries, views and checks: changes nothing.
    Read,
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

/// The longest title [`declare_classes!`] may give a command, in bytes.
pub const MAX_TITLE: usize = 40;

/// The classes of one command enum, one line per variant:
/// `Variant "wire.name" Class "Title"`. Defines `class(&self)`,
/// `command(&self)` and the tables `CLASSES` and `TITLES` that discovery
/// reads by name. The title is a short name for people (an MCP `title`), at
/// most [`MAX_TITLE`] bytes; what the command does is its doc comment.
#[macro_export]
macro_rules! declare_classes {
    ($family:ident { $($variant:ident $name:literal $class:ident $title:literal,)+ }) => {
        impl $family {
            /// Every command of this enum with its permission class.
            pub const CLASSES: &'static [(&'static str, $crate::permission::Class)] =
                &[$(($name, $crate::permission::Class::$class),)+];

            /// Every command of this enum with its title.
            pub const TITLES: &'static [(&'static str, &'static str)] =
                &[$(($name, $title),)+];

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

#[cfg(test)]
mod tests {
    use super::Class;
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
    fn the_class_tables_name_exactly_the_wire_commands() -> Result<(), serde_json::Error> {
        let schema = serde_json::to_value(schemars::schema_for!(Operation))?;
        let mut wire = BTreeSet::new();
        tags(&schema, "command", &mut wire);
        wire.insert("update".to_owned());
        let listed: BTreeSet<String> = Operation::classes()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        assert_eq!(wire, listed);

        let schema = serde_json::to_value(schemars::schema_for!(Query))?;
        let mut wire = BTreeSet::new();
        tags(&schema, "query", &mut wire);
        let listed: BTreeSet<String> = Query::CLASSES
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        assert_eq!(wire, listed);
        Ok(())
    }

    #[test]
    fn every_query_only_reads() {
        for (name, class) in Query::CLASSES {
            assert_eq!(*class, Class::Read, "{name}");
        }
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
    fn a_batch_takes_its_strictest_member() {
        assert_eq!(Class::strictest([]), Class::Read);
        assert_eq!(Class::strictest([Class::Read, Class::Edit]), Class::Edit);
        assert_eq!(
            Class::strictest([Class::Never, Class::Read, Class::Ask]),
            Class::Never
        );
    }
}
