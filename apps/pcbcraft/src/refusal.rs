//! Refusal routing: a refused operation goes back to the editor that
//! produced it, so the refusal shows where it was made (docs/sidebar-ia.md,
//! "Refusal routing").
//!
//! Contract from slice S0; slice K3 makes `App::apply` hand each refusal
//! back as a [`Refused`] on the next frame. The editor claims it for a
//! field ([`crate::kit::field_error`]) or a dialog's refusal row; a canvas
//! gesture shows a toast with Show; only refusals no editor claims fall back
//! to a toast. Wording is chosen by `Error` variant, never by matching text.
//! Adapted from Solidcraft 4f3ac07a `crates/ui-egui/src/dialogs.rs`
//! (`reveal_refusal`, `shown_error`), MIT option; see ATTRIBUTION.md.
#![allow(dead_code, reason = "S0 contract; slice K3 and the W slices use it")]

use pcbcraft_core::Error;

/// Where a refused operation came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A card field, by the id of its control.
    Field(egui::Id),
    /// A docked dialog or review, by its panel id.
    Dialog(egui::Id),
    /// A canvas gesture (routing, dragging, placing).
    Canvas,
    /// A ribbon, menu or keyboard command.
    Command,
}

/// A refusal handed back to the editor that produced the operation.
#[derive(Debug)]
pub struct Refused {
    pub origin: Origin,
    pub error: Error,
}

impl Refused {
    /// The refusal for `origin`, if it is this one; otherwise it stays
    /// unclaimed.
    pub fn claim(&self, origin: Origin) -> Option<String> {
        (self.origin == origin).then(|| shown_error(&self.error))
    }
}

/// The wording a person sees for `error`, chosen by variant. The full
/// engine text still goes to Messages.
pub fn shown_error(error: &Error) -> String {
    match error {
        Error::Stale { .. } => "The project changed meanwhile; try again".into(),
        Error::History(_) => "Nothing to undo or redo".into(),
        Error::Json(_) => "The request could not be read".into(),
        Error::Invalid(text)
        | Error::NotFound(text)
        | Error::Conflict(text)
        | Error::Limit(text)
        | Error::Unsupported(text) => text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::issues::style;

    /// Skeleton of the refusal-mapping style test: wording chosen by variant
    /// follows the message style. Slice K3 extends it to every mapped code.
    #[test]
    fn variant_wording_follows_the_message_style() -> Result<(), Box<dyn std::error::Error>> {
        let json = serde_json::from_str::<u32>("x")
            .err()
            .ok_or("parse error")?;
        for error in [
            Error::Stale {
                expected: 3,
                actual: 4,
            },
            Error::History("Nothing to undo".into()),
            Error::Json(json),
        ] {
            let shown = shown_error(&error);
            assert!(style::check(&shown).is_empty(), "{shown}");
        }
        Ok(())
    }

    #[test]
    fn a_refusal_is_claimed_only_by_its_origin() {
        let id = egui::Id::new("width");
        let refused = Refused {
            origin: Origin::Field(id),
            error: Error::Invalid("Enter a width".into()),
        };
        assert_eq!(
            refused.claim(Origin::Field(id)).as_deref(),
            Some("Enter a width")
        );
        assert_eq!(refused.claim(Origin::Canvas), None);
    }
}
