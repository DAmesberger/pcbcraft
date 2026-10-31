//! Symbol definitions: units, each an ordered list of drawn graphics plus
//! pins. A symbol belongs to one component lineage, whose terminal IDs its
//! pins use; unit and graphic IDs share the symbol's local namespace.
use super::graphics::{
    self, Graphic, Item, MAX_GRAPHICS, MAX_GRAPHICS_PER_UNIT, MAX_VERTICES, PIN_TEXT_HEIGHT_MM,
    Pin, TEXT_HEIGHT_MM, TextMetric,
};
use super::publish::{issue, object};
use super::{Library, MAX_LABEL, MAX_TERMINALS, MAX_UNITS, printable};
use crate::diagnostics::{Finding, ObjectRef};
use crate::geometry::Bounds;
use crate::ids::{DefinitionId, GraphicId, TerminalId, UnitId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SymbolDefinition {
    /// The owning component lineage; pins use its terminal IDs.
    pub component: DefinitionId,
    pub next_local: u32,
    /// Height of pin names and numbers, 0.1–50 mm.
    pub pin_text_height: f64,
    pub units: Vec<Unit>,
}

impl Default for SymbolDefinition {
    fn default() -> Self {
        Self {
            component: DefinitionId::default(),
            next_local: 0,
            pin_text_height: PIN_TEXT_HEIGHT_MM,
            units: vec![],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub id: UnitId,
    /// Presentation label appended to the designator, e.g. `A` in `Q7A`.
    pub label: String,
    /// Drawn in order; later graphics are on top.
    pub graphics: Vec<Graphic>,
    pub pins: Vec<Pin>,
}

impl Unit {
    pub fn graphic(&self, id: GraphicId) -> Option<&Graphic> {
        self.graphics.iter().find(|g| g.id == id)
    }
    pub fn pin(&self, terminal: TerminalId) -> Option<&Pin> {
        self.pins.iter().find(|p| p.terminal == terminal)
    }

    /// The extent of visible graphics and pin stubs in the unit frame. Text
    /// uses the approximate core metric; pin names and numbers resolve on
    /// instances and are not included. `None` for an empty unit.
    pub fn bounds(&self, m: TextMetric) -> Option<Bounds> {
        let graphics = self.graphics.iter().filter_map(|g| match &g.item {
            Item::Text(t) if !t.visible => None,
            item => Some(item.bounds(m)),
        });
        let pins = self
            .pins
            .iter()
            .map(|p| Bounds::point(p.position).including(p.body_end()));
        graphics.chain(pins).reduce(Bounds::union)
    }
}

impl SymbolDefinition {
    pub fn unit(&self, id: UnitId) -> Option<&Unit> {
        self.units.iter().find(|u| u.id == id)
    }
    /// The unit drawing `terminal` and its pin.
    pub fn pin(&self, terminal: TerminalId) -> Option<(&Unit, &Pin)> {
        self.units
            .iter()
            .find_map(|unit| unit.pin(terminal).map(|pin| (unit, pin)))
    }
    /// Every graphic of every unit.
    pub fn graphics(&self) -> impl Iterator<Item = &Graphic> {
        self.units.iter().flat_map(|u| u.graphics.iter())
    }
    /// Field lineages the symbol's text shows.
    pub fn text_fields(&self) -> BTreeSet<DefinitionId> {
        self.graphics()
            .filter_map(|g| match &g.item {
                Item::Text(t) => match t.content {
                    graphics::TextContent::Field(id) => Some(id),
                    _ => None,
                },
                Item::Shape { .. } => None,
            })
            .collect()
    }

    pub(super) fn within_bounds(&self) -> bool {
        self.units.len() <= MAX_UNITS
            && self.units.iter().map(|u| u.pins.len()).sum::<usize>() <= MAX_TERMINALS
            && self
                .units
                .iter()
                .all(|u| u.label.len() <= MAX_LABEL && u.graphics.len() <= MAX_GRAPHICS_PER_UNIT)
            && self.graphics().count() <= MAX_GRAPHICS
            && self.graphics().all(|g| g.item.within_bounds())
            && self.graphics().map(|g| g.item.vertices()).sum::<usize>() <= MAX_VERTICES
    }

    pub(super) fn geometry_finite(&self) -> bool {
        self.pin_text_height.is_finite()
            && TEXT_HEIGHT_MM.contains(&self.pin_text_height)
            && self.units.iter().all(|u| {
                u.pins.iter().all(Pin::finite) && u.graphics.iter().all(|g| g.item.finite())
            })
    }

    /// Unit and graphic IDs: one namespace.
    pub(super) fn local_ids(&self) -> Vec<u32> {
        self.units
            .iter()
            .map(|u| u.id.0)
            .chain(self.graphics().map(|g| g.id.0))
            .collect()
    }

    pub(super) fn findings(&self, id: DefinitionId, library: &Library, out: &mut Vec<Finding>) {
        let this = ObjectRef::Definition { id };
        // The owner is a component lineage that is published, or staged in
        // the same publish set; never unset, unknown or this lineage itself.
        if !matches!(
            library.latest(self.component),
            Some((crate::ids::DefinitionKind::Component, _))
        ) || self.component == id
        {
            out.push(issue(
                "reference.owner_mismatch",
                "The owner is not a component that is published or published with it.".into(),
                vec![this],
            ));
        }
        if !(1..=MAX_UNITS).contains(&self.units.len()) {
            out.push(issue(
                "symbol.unit_count",
                "A symbol needs 1–64 units.".into(),
                vec![this],
            ));
        }
        let mut labels = BTreeSet::new();
        let mut pinned = BTreeSet::new();
        for unit in &self.units {
            let unit_object = object(id, unit.id.0);
            if !printable(&unit.label) || !labels.insert(unit.label.to_lowercase()) {
                out.push(issue(
                    "symbol.unit_label",
                    "A unit needs a unique printable label.".into(),
                    vec![unit_object],
                ));
            }
            if unit.graphics.is_empty() && unit.pins.is_empty() {
                out.push(issue(
                    "symbol.unit_empty",
                    format!("Unit ‘{}’ draws nothing and has no pins.", unit.label),
                    vec![unit_object],
                ));
            }
            for pin in &unit.pins {
                if pin.terminal.0 == 0 || !pinned.insert(pin.terminal) {
                    out.push(issue(
                        "symbol.pin_duplicate",
                        format!(
                            "Unit ‘{}’ has a pin with no terminal, or a terminal with several pins.",
                            unit.label
                        ),
                        vec![unit_object],
                    ));
                }
            }
            for graphic in &unit.graphics {
                graphics::graphic_findings(id, graphic, out);
            }
        }
    }
}
