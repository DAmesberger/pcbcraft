//! The No connect tool: a click on a pin marks it intentionally unused, or
//! clears the mark; a click on a symbol marks every pin of it still needing
//! a decision. Each click is one `instance.set_no_connect`, so one undo step.
use super::{PinMark, SchematicEditor};
use crate::kit;
use pcbcraft_core::ids::{Endpoint, Id, TerminalId};
use pcbcraft_core::instances::MAX_NO_CONNECT_EDIT;
use pcbcraft_core::{InstanceCommand, Operation, Project};

/// The tool's canvas hint.
pub(super) const HINT: &str =
    "Click a pin to mark or clear it · click a symbol for all open pins · Esc ends";
/// A click on neither a pin nor a symbol.
pub(super) const MISS: &str = "Click a pin, or a symbol to mark all its open pins";
/// A click on a wired pin.
pub(super) const WIRED: &str = "That pin is wired: delete its wire to mark it no-connect";
/// A click on a pin the part leaves unconnected (device NC or reserved).
pub(super) const NEEDS_NONE: &str = "That pin needs no mark: the part leaves it unconnected";
/// A click on a symbol without open pins.
pub(super) const ALL_DECIDED: &str = "Every pin of that symbol is connected or marked";

/// The tool's card.
pub(super) fn section(ui: &mut egui::Ui) {
    kit::section(ui, "no-connect-tool", "No connect", true, None, |ui| {
        kit::hint(
            ui,
            "Marks a pin intentionally unused",
            "A cross marks it on the sheet and in the PDF. The checks then accept it unconnected, \
             and report it if a wire reaches it.",
        );
    });
}

/// The operation a click sends, or why it sends none.
pub(super) fn click(
    project: &Project,
    marks: &std::collections::BTreeMap<Endpoint, PinMark>,
    pins_of: impl FnOnce() -> Vec<TerminalId>,
    pin: Option<Endpoint>,
    body: Option<Id>,
) -> Result<Operation, &'static str> {
    let set = |instance, terminals, no_connect| {
        Operation::Instance(InstanceCommand::SetNoConnect {
            instance,
            terminals,
            no_connect,
        })
    };
    if let Some(pin) = pin {
        let marked = project
            .instance(pin.instance)
            .is_some_and(|i| i.no_connect.contains(&pin.terminal));
        return match marks.get(&pin).copied() {
            _ if marked => Ok(set(pin.instance, vec![pin.terminal], false)),
            Some(PinMark::ToDo) | None => Ok(set(pin.instance, vec![pin.terminal], true)),
            Some(PinMark::Wired) => Err(WIRED),
            Some(PinMark::NoConnect) => Err(NEEDS_NONE),
        };
    }
    let instance = body.ok_or(MISS)?;
    let open: Vec<TerminalId> = pins_of()
        .into_iter()
        .filter(|t| {
            marks.get(&Endpoint {
                instance,
                terminal: *t,
            }) == Some(&PinMark::ToDo)
        })
        .take(MAX_NO_CONNECT_EDIT)
        .collect();
    if open.is_empty() {
        return Err(ALL_DECIDED);
    }
    Ok(set(instance, open, true))
}

impl SchematicEditor {
    /// A No connect tool click on `pin`, else on the occurrence `body`
    /// (instance, occurrence). A click that sends nothing says why.
    pub(super) fn no_connect_click(
        &self,
        ctx: &egui::Context,
        project: &Project,
        pin: Option<Endpoint>,
        body: Option<(Id, Id)>,
    ) -> Option<Operation> {
        let pins_of = || {
            body.and_then(|(_, occurrence)| self.placed.get(&occurrence))
                .map(|placed| placed.pins.iter().map(|p| p.terminal).collect())
                .unwrap_or_default()
        };
        match click(
            project,
            &self.pin_marks,
            pins_of,
            pin,
            body.map(|(instance, _)| instance),
        ) {
            Ok(operation) => Some(operation),
            Err(why) => {
                kit::toast::post(ctx, kit::severity::Level::Info, why, Vec::new());
                None
            }
        }
    }
}
