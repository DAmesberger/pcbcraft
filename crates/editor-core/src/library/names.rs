//! User names of library definitions for messages. A message names a
//! definition by its kind, its name and its revision ("symbol ‘Resistor’
//! r2"), never by its internal lineage ID (docs/sidebar-ia.md, "Message
//! style").
use super::Library;
use crate::ids::{DefinitionId, DefinitionKind, DefinitionRef};

/// The lower-case noun of a definition kind, as messages use it.
pub fn noun(kind: DefinitionKind) -> &'static str {
    match kind {
        DefinitionKind::Component => "component",
        DefinitionKind::Symbol => "symbol",
        DefinitionKind::Package => "package",
        DefinitionKind::Footprint => "footprint",
        DefinitionKind::Binding => "binding",
        DefinitionKind::QuantityKind => "quantity kind",
        DefinitionKind::Field => "field",
        DefinitionKind::Part => "Part",
    }
}

/// `text` with its first letter in upper case, for a name that starts a
/// sentence.
pub fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl Library {
    /// The name of a lineage: the name of its latest published revision of
    /// any kind, or `None` when nothing of it is published.
    pub fn lineage_name(&self, id: DefinitionId) -> Option<&str> {
        fn latest<T>(items: &[super::Published<T>], id: DefinitionId) -> Option<(u32, &str)> {
            items
                .iter()
                .filter(|p| p.id == id)
                .max_by_key(|p| p.revision)
                .map(|p| (p.revision.0, p.meta.name.as_str()))
        }
        [
            latest(&self.components, id),
            latest(&self.symbols, id),
            latest(&self.packages, id),
            latest(&self.footprints, id),
            latest(&self.bindings, id),
            latest(&self.quantity_kinds, id),
            latest(&self.fields, id),
            latest(&self.parts, id),
        ]
        .into_iter()
        .flatten()
        .max_by_key(|(revision, _)| *revision)
        .map(|(_, name)| name)
        .filter(|name| !name.trim().is_empty())
    }

    /// One exact revision for a message: "symbol ‘Resistor’ r2". The name
    /// is the revision's own, else its lineage's; a revision of an unknown
    /// lineage is "symbol r2".
    pub fn describe(&self, r: DefinitionRef) -> String {
        let own = self
            .definition(r)
            .map(|(meta, _)| meta.name)
            .filter(|name| !name.trim().is_empty());
        let name = own.or_else(|| self.lineage_name(r.id).map(str::to_owned));
        match name {
            Some(name) => format!("{} ‘{name}’ {}", noun(r.kind), r.revision),
            None => format!("{} {}", noun(r.kind), r.revision),
        }
    }

    /// A lineage for a message, without a revision: "component ‘Resistor’",
    /// or "a component" when nothing of it is published.
    pub fn describe_lineage(&self, kind: DefinitionKind, id: DefinitionId) -> String {
        match self.lineage_name(id) {
            Some(name) => format!("{} ‘{name}’", noun(kind)),
            None => format!("a {}", noun(kind)),
        }
    }
}
