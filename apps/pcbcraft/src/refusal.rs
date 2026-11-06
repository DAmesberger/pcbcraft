//! Refusal routing: a refused operation goes back to the editor that
//! produced it, so the refusal shows where it was made (docs/sidebar-ia.md,
//! "Refusal routing").
//!
//! `App::apply` hands each refusal back as a [`Refused`] on the next frame.
//! An editor that wants a refusal at its source calls [`mark`] with the
//! origin while it returns the operation, and [`claim`]s it on the next
//! frame for a field ([`crate::kit::field_error`]) or a dialog's refusal
//! row. A canvas gesture also gets a toast; only refusals no editor claims
//! fall back to a toast. The full engine text always goes to Messages.
//! Wording is chosen by `Error` variant, never by matching text.
//! Adapted from Solidcraft 4f3ac07a `crates/ui-egui/src/dialogs.rs`
//! (`reveal_refusal`, `shown_error`), MIT option; see ATTRIBUTION.md.

use crate::kit::severity::Level;
use pcbcraft_core::Error;
use pcbcraft_core::issues::{MAX_TITLE, style};
use std::sync::Arc;

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

/// How a refusal reads in a toast or Messages: an empty history is only
/// information, a stale request a warning, every other refusal an error.
pub fn level(error: &Error) -> Level {
    match error {
        Error::History(_) => Level::Info,
        Error::Stale { .. } => Level::Warning,
        _ => Level::Error,
    }
}

/// The wording a person sees for `error`, chosen by variant. Engine text is
/// shown by its first sentence when that follows the message style;
/// otherwise the variant's own wording stands in. The full engine text
/// still goes to Messages.
pub fn shown_error(error: &Error) -> String {
    let fallback = match error {
        Error::Stale { .. } => return "The project changed meanwhile; try again".into(),
        Error::Json(_) => return "The request could not be read".into(),
        Error::History(_) => "Nothing to undo or redo",
        Error::Invalid(_) => "That value is not accepted",
        Error::NotFound(_) => "That object no longer exists",
        Error::Conflict(_) => "The project does not allow that change now",
        Error::Limit(_) => "That goes beyond a supported limit",
        Error::Unsupported(_) => "This version does not support that",
    };
    match error {
        Error::History(text)
        | Error::Invalid(text)
        | Error::NotFound(text)
        | Error::Conflict(text)
        | Error::Limit(text)
        | Error::Unsupported(text) => presentable(text).unwrap_or_else(|| fallback.into()),
        Error::Stale { .. } | Error::Json(_) => fallback.into(),
    }
}

/// The first sentence of `text`, without its full stop and clipped to
/// [`MAX_TITLE`] characters at a word, if it then follows the message
/// style; `None` when it names IDs, codes or Debug output.
pub fn presentable(text: &str) -> Option<String> {
    let line = text.lines().next().unwrap_or_default().trim();
    let line = quoted(without_code(line));
    let line = line.as_str();
    let sentence = match line.find(". ") {
        Some(end) => &line[..end],
        None => line,
    };
    let sentence = sentence.trim_end().trim_end_matches('.').trim_end();
    let shown = clip(sentence, MAX_TITLE);
    style::check(&shown).is_empty().then_some(shown)
}

/// `line` without the rule code engine text leads with
/// (`board.shape_unsolved: …`): the code is for tools and Messages.
fn without_code(line: &str) -> &str {
    let Some((head, rest)) = line.split_once(": ") else {
        return line;
    };
    let part = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    if head.split('.').count() > 1 && head.split('.').all(part) {
        rest.trim_start()
    } else {
        line
    }
}

/// `line` with names quoted in backticks (`width`) quoted as the app
/// quotes them (‘width’); an unpaired backtick stays.
fn quoted(line: &str) -> String {
    if !line.matches('`').count().is_multiple_of(2) {
        return line.to_owned();
    }
    let mut open = true;
    line.chars()
        .map(|c| match c {
            '`' => {
                open = !open;
                if open { '’' } else { '‘' }
            }
            c => c,
        })
        .collect()
}

/// `text` within `max` characters, cut at a word and ending in "…".
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    let cut = match kept.rfind(' ') {
        Some(space) if space > kept.len() / 2 => &kept[..space],
        _ => kept.as_str(),
    };
    format!("{}…", cut.trim_end_matches([' ', ',', ';', ':', '.']))
}

/// Refusals handed back and not yet claimed, kept in the egui context so
/// every editor reaches them without a new parameter.
#[derive(Clone, Default)]
struct Inbox {
    pending: Vec<Pending>,
    /// The origin an editor declared for the operation it returns this
    /// frame, with that frame's number.
    mark: Option<(u64, Origin)>,
}

#[derive(Clone)]
struct Pending {
    /// The frame that refused it; it is handed back on the next one.
    frame: u64,
    refused: Arc<Refused>,
    /// A toast already shows it (a canvas gesture).
    toasted: bool,
}

fn inbox_id() -> egui::Id {
    egui::Id::new("pcbcraft-refusals")
}

fn with_inbox<R>(ctx: &egui::Context, f: impl FnOnce(&mut Inbox) -> R) -> R {
    ctx.data_mut(|d| f(d.get_temp_mut_or_default::<Inbox>(inbox_id())))
}

/// Declare where the operation an editor returns this frame comes from, so
/// a refusal of it is handed back there. Without a mark, the call site's
/// default origin applies.
pub fn mark(ctx: &egui::Context, origin: Origin) {
    let frame = ctx.cumulative_frame_nr();
    with_inbox(ctx, |inbox| inbox.mark = Some((frame, origin)));
}

/// The origin marked this frame, if any, cleared once read.
pub(crate) fn take_mark(ctx: &egui::Context) -> Option<Origin> {
    let frame = ctx.cumulative_frame_nr();
    with_inbox(ctx, |inbox| {
        inbox
            .mark
            .take()
            .and_then(|(at, origin)| (at == frame).then_some(origin))
    })
}

/// Hand `refused` back to its origin on the next frame. `toasted` says a
/// toast already shows it, so an unclaimed refusal is not shown twice.
pub(crate) fn hand_back(ctx: &egui::Context, refused: Refused, toasted: bool) {
    let frame = ctx.cumulative_frame_nr();
    with_inbox(ctx, |inbox| {
        inbox.pending.push(Pending {
            frame,
            refused: Arc::new(refused),
            toasted,
        });
    });
    ctx.request_repaint();
}

/// The refusal for `origin` handed back from an earlier frame, worded for
/// display, if there is one. Claiming it means the editor shows it at its
/// source, so no toast appears for it.
pub fn claim(ctx: &egui::Context, origin: Origin) -> Option<String> {
    let frame = ctx.cumulative_frame_nr();
    with_inbox(ctx, |inbox| {
        let index = inbox
            .pending
            .iter()
            .position(|p| p.frame < frame && p.refused.origin == origin)?;
        let pending = inbox.pending.remove(index);
        pending.refused.claim(origin)
    })
}

/// End of a frame: refusals handed back before this frame that no editor
/// claimed are dropped from the inbox. Those no toast shows yet are
/// returned so they fall back to one.
pub(crate) fn unclaimed(ctx: &egui::Context) -> Vec<Arc<Refused>> {
    let frame = ctx.cumulative_frame_nr();
    with_inbox(ctx, |inbox| {
        let (old, kept): (Vec<_>, Vec<_>) = inbox.pending.drain(..).partition(|p| p.frame < frame);
        inbox.pending = kept;
        old.into_iter()
            .filter(|p| !p.toasted)
            .map(|p| p.refused)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_error() -> Result<serde_json::Error, Box<dyn std::error::Error>> {
        Ok(serde_json::from_str::<u32>("x")
            .err()
            .ok_or("parse error")?)
    }

    /// Every variant's wording follows the message style, whatever the
    /// engine text says: IDs, codes, Debug output, several sentences and
    /// long text never reach the toast.
    #[test]
    fn variant_wording_follows_the_message_style() -> Result<(), Box<dyn std::error::Error>> {
        let texts = [
            "Nothing to undo",
            "Nothing was published. D20 r2: the symbol needs a pin",
            "placement datum must lie inside the board with a finite rotation",
            "Explicit removal required for tracks [68]; tracks are never retagged",
            "Remove instance R1 (ID 8).",
            "footprint.param_expr: `gap`: circular reference",
            "route violates 0.2 mm clearance to another net's pad on net 32",
            "Some(Id(3)) is not in the project library",
            "line one\nline two",
            "",
            "   ",
            &"a long refusal that keeps going ".repeat(10),
            &"x".repeat(200),
        ];
        let mut errors = vec![
            Error::Stale {
                expected: 3,
                actual: 4,
            },
            Error::Json(json_error()?),
        ];
        for text in texts {
            errors.extend([
                Error::Invalid(text.into()),
                Error::NotFound(text.into()),
                Error::Conflict(text.into()),
                Error::Limit(text.into()),
                Error::Unsupported(text.into()),
                Error::History(text.into()),
            ]);
        }
        for error in &errors {
            let shown = shown_error(error);
            assert!(
                style::check(&shown).is_empty(),
                "{:?} → {shown:?}: {:?}",
                error,
                style::check(&shown)
            );
        }
        Ok(())
    }

    /// Clean engine text is shown by its first sentence; text that leaks
    /// internals gets the variant's wording instead of a rewrite by string.
    #[test]
    fn clean_text_shows_and_leaking_text_falls_back_by_variant() {
        assert_eq!(
            shown_error(&Error::Conflict(
                "Nothing was published. D20 r2: the symbol needs a pin".into()
            )),
            "Nothing was published"
        );
        assert_eq!(
            shown_error(&Error::Invalid(
                "placement datum must lie inside the board.".into()
            )),
            "placement datum must lie inside the board"
        );
        assert_eq!(
            shown_error(&Error::Conflict("Remove instance R1 (ID 8)".into())),
            "The project does not allow that change now"
        );
        assert_eq!(
            shown_error(&Error::Invalid("Resistance: 4.7 kΩ is too small".into())),
            "Resistance: 4.7 kΩ is too small",
            "a decimal point is not a sentence end"
        );
        assert_eq!(
            shown_error(&Error::History("Nothing to undo".into())),
            "Nothing to undo"
        );
        // A refusal names the problem: the rule code goes and names keep
        // their quotes, so the field says more than "not accepted".
        assert_eq!(
            shown_error(&Error::Invalid(
                "board.shape_unsolved: board parameters: `width` = 1000000 mm is out of range"
                    .into()
            )),
            "board parameters: ‘width’ = 1000000 mm is out of range"
        );
        assert_eq!(
            shown_error(&Error::Invalid(
                "footprint.solve_failed: the dimension ‘width’ cannot be satisfied".into()
            )),
            "the dimension ‘width’ cannot be satisfied"
        );
        assert_eq!(
            shown_error(&Error::Invalid("an odd ` stays".into())),
            "That value is not accepted"
        );
        let long = shown_error(&Error::Invalid("word ".repeat(40)));
        assert!(long.ends_with('…') && long.chars().count() <= MAX_TITLE);
    }

    #[test]
    fn severity_comes_from_the_variant() {
        assert_eq!(
            level(&Error::History("Nothing to redo".into())),
            Level::Info
        );
        assert_eq!(
            level(&Error::Stale {
                expected: 1,
                actual: 2
            }),
            Level::Warning
        );
        assert_eq!(level(&Error::Conflict("Locked".into())), Level::Error);
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

    fn frame(ctx: &egui::Context, mut add: impl FnMut(&egui::Context)) {
        ctx.run_ui(Default::default(), |ui| add(ui.ctx()))
            .textures_delta
            .clear();
    }

    /// A field refusal reaches its field on the next frame and nowhere
    /// else; an unclaimed one falls back once, after its editor had a
    /// frame to claim it; a toasted canvas refusal never falls back twice.
    #[test]
    fn hand_back_reaches_the_origin_on_the_next_frame() {
        let ctx = egui::Context::default();
        let field = Origin::Field(egui::Id::new("width"));
        let other = Origin::Field(egui::Id::new("height"));
        let refuse = |ctx: &egui::Context, origin, toasted| {
            hand_back(
                ctx,
                Refused {
                    origin,
                    error: Error::Invalid("Enter a width".into()),
                },
                toasted,
            );
        };
        frame(&ctx, |ctx| {
            refuse(ctx, field, false);
            assert_eq!(claim(ctx, field), None, "not in the frame that refused it");
            assert!(unclaimed(ctx).is_empty(), "nor swept in that frame");
        });
        frame(&ctx, |ctx| {
            assert_eq!(claim(ctx, other), None);
            assert_eq!(claim(ctx, field).as_deref(), Some("Enter a width"));
            assert_eq!(claim(ctx, field), None, "claimed once");
            assert!(
                unclaimed(ctx).is_empty(),
                "a claimed refusal shows no toast"
            );
        });
        frame(&ctx, |ctx| refuse(ctx, other, false));
        frame(&ctx, |ctx| {
            let left = unclaimed(ctx);
            assert_eq!(left.len(), 1);
            assert_eq!(left.first().map(|r| r.origin), Some(other));
        });
        frame(&ctx, |ctx| {
            assert!(unclaimed(ctx).is_empty(), "it fell back only once");
            refuse(ctx, Origin::Canvas, true);
        });
        frame(&ctx, |ctx| {
            assert!(unclaimed(ctx).is_empty(), "a toast already shows it");
        });
    }

    #[test]
    fn a_mark_holds_for_its_own_frame_only() {
        let ctx = egui::Context::default();
        let field = Origin::Field(egui::Id::new("width"));
        frame(&ctx, |ctx| {
            mark(ctx, field);
            assert_eq!(take_mark(ctx), Some(field));
            assert_eq!(take_mark(ctx), None, "read once");
            mark(ctx, field);
        });
        frame(&ctx, |ctx| assert_eq!(take_mark(ctx), None, "a stale mark"));
    }
}
