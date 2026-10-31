//! Docked command dialogs in the subject slot, and the destructive
//! confirmation.
//!
//! Contract from slice S0 (docs/sidebar-ia.md, "Command and review
//! dialogs"); slice K4 owns the final look. A dialog has a 40 pt header, a
//! body capped at `body_rows` that is its only scroll, and a pinned 48 pt
//! footer with Cancel and the primary, primary last. A refusal shows once,
//! as the last body row; a disabled primary explains itself on hover; a
//! destructive choice is the primary's variant. Adapted from Solidcraft
//! 4f3ac07a `crates/ui-egui/src/ui_kit/panel.rs` (`Panel::feature`,
//! `body_rows`), `dialogs.rs` (`reveal_refusal`, `shown_error`) and
//! `ui_kit/confirm.rs`, MIT option; see ATTRIBUTION.md.
#![allow(
    dead_code,
    reason = "S0 contract; slices K4, R1 and the W slices use it"
)]

use super::card::FOOTER_HEIGHT;
use egui::Ui;
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit;

/// A docked dialog's header height, in points.
pub const DIALOG_HEADER_HEIGHT: f32 = 40.0;

/// How the primary action looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrimaryKind {
    #[default]
    Normal,
    /// Removes or replaces work ("Replace footprint", "Remove pin").
    Destructive,
}

/// A docked command dialog.
pub struct DialogPanel<'a> {
    pub id: egui::Id,
    pub icon: &'a str,
    pub title: &'a str,
    /// The header's kind chip ("New", "Review").
    pub kind: &'a str,
    /// Body height cap, in control rows.
    pub body_rows: f32,
    pub primary: &'a str,
    pub primary_kind: PrimaryKind,
    /// Why the primary is disabled; `None` enables it.
    pub primary_disabled: Option<&'a str>,
    /// The engine's refusal of the last submit, in shown wording.
    pub refusal: Option<&'a str>,
    /// True on the frame the dialog opens: the body scrolls to the top.
    pub reset_scroll: bool,
}

/// What the dialog's footer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogAction {
    Cancel,
    Primary,
}

impl<'a> DialogPanel<'a> {
    pub fn new(id: egui::Id, icon: &'a str, title: &'a str, primary: &'a str) -> Self {
        Self {
            id,
            icon,
            title,
            kind: "",
            body_rows: 12.0,
            primary,
            primary_kind: PrimaryKind::Normal,
            primary_disabled: None,
            refusal: None,
            reset_scroll: false,
        }
    }

    /// Draw the dialog: header, `body` in its bounded scroll with the
    /// refusal as its last row, then the pinned footer.
    pub fn show(&self, ui: &mut Ui, body: impl FnOnce(&mut Ui)) -> Option<DialogAction> {
        let t = Tokens::get();
        super::inspector_header(ui, self.icon, self.title, self.kind);
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt(self.id)
            .max_height(
                (self.body_rows * t.control_height)
                    .min((ui.available_height() - FOOTER_HEIGHT).max(0.0)),
            )
            .auto_shrink([false, true]);
        if self.reset_scroll {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll.show(ui, |ui| {
            super::form(ui, |ui| {
                body(ui);
                if let Some(refusal) = self.refusal {
                    super::refusal(ui, refusal);
                }
            });
        });
        let mut action = None;
        super::actions(ui, |ui| {
            let primary = super::primary(ui, self.primary, self.primary_disabled.is_none());
            let primary = match self.primary_disabled {
                Some(reason) => primary.on_disabled_hover_text(reason),
                None => primary,
            };
            if primary.clicked() {
                action = Some(DialogAction::Primary);
            }
            if ui_kit::button(ui, "Cancel", ui_kit::Button::Enabled(true)).clicked() {
                action = Some(DialogAction::Cancel);
            }
        });
        action
    }
}

/// A destructive confirmation window. Returns `Some(true)` to proceed,
/// `Some(false)` to cancel, `None` while it stays open.
pub fn confirm(
    ctx: &egui::Context,
    id: egui::Id,
    title: &str,
    text: &str,
    proceed: &str,
) -> Option<bool> {
    let mut answer = None;
    let closed = ui_kit::Window::new(id, "library", title).show_body(ctx, |ui| {
        super::hint(ui, text, text);
        super::actions(ui, |ui| {
            if super::primary(ui, proceed, true).clicked() {
                answer = Some(true);
            }
            if ui_kit::button(ui, "Cancel", ui_kit::Button::Enabled(true)).clicked() {
                answer = Some(false);
            }
        });
    });
    if closed {
        answer = Some(false);
    }
    answer
}
