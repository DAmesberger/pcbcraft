//! The message style every issue title, group title and shown error follows
//! (docs/sidebar-ia.md, "Message style"): one line of at most
//! [`MAX_TITLE`](super::MAX_TITLE) characters with no final full stop, naming
//! objects by their user names and never by internal IDs, rule codes or
//! debug output.
//!
//! [`check`] is the one implementation of these rules. Core tests run it over
//! every finding and every `Error` display; the desktop runs it over its
//! refusal wording.
use super::MAX_TITLE;

/// One way a message breaks the style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Empty,
    /// More than [`MAX_TITLE`] characters.
    TooLong,
    /// More than one line.
    MultiLine,
    /// Ends with a full stop.
    FullStop,
    /// `ID 8`: an internal ID.
    InternalId,
    /// `[68]` or `[3, 4]`: a list of internal IDs.
    IdList,
    /// `net 32`: a net by its internal number instead of its name.
    NetNumber,
    /// `D20 r2`: a definition by its internal ID.
    DefinitionId,
    /// `footprint.param_expr:`: a rule code as a prefix.
    CodePrefix,
    /// `Some(…)`, `Id(3)`, `Conflict { tracks: … }`: Rust debug output.
    DebugText,
}

/// Every style problem of `text`, in [`Problem`] order; empty when the text
/// follows the style.
pub fn check(text: &str) -> Vec<Problem> {
    let mut found = Vec::new();
    let tests: [(Problem, bool); 10] = [
        (Problem::Empty, text.trim().is_empty()),
        (Problem::TooLong, text.chars().count() > MAX_TITLE),
        (Problem::MultiLine, text.contains(['\n', '\r'])),
        (Problem::FullStop, text.trim_end().ends_with('.')),
        (Problem::InternalId, internal_id(text)),
        (Problem::IdList, id_list(text)),
        (Problem::NetNumber, net_number(text)),
        (Problem::DefinitionId, definition_id(text)),
        (Problem::CodePrefix, code_prefix(text)),
        (Problem::DebugText, debug_text(text)),
    ];
    for (problem, hit) in tests {
        if hit {
            found.push(problem);
        }
    }
    found
}

fn starts_word(text: &str, at: usize) -> bool {
    text[..at]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric() && c != '_')
}

fn digits_at(text: &str, at: usize) -> usize {
    text[at..].chars().take_while(char::is_ascii_digit).count()
}

fn ends_word(text: &str, at: usize) -> bool {
    text[at..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric() && c != '_')
}

/// `\bID \d`
fn internal_id(text: &str) -> bool {
    text.match_indices("ID ")
        .any(|(at, word)| starts_word(text, at) && digits_at(text, at + word.len()) > 0)
}

/// `\[\d+(, \d+)*\]`
fn id_list(text: &str) -> bool {
    text.match_indices('[').any(|(at, _)| {
        let mut rest = &text[at + 1..];
        loop {
            let digits = digits_at(rest, 0);
            if digits == 0 {
                return false;
            }
            rest = &rest[digits..];
            if rest.starts_with(']') {
                return true;
            }
            let Some(next) = rest.strip_prefix(", ") else {
                return false;
            };
            rest = next;
        }
    })
}

/// `\bnet \d+\b`
fn net_number(text: &str) -> bool {
    text.match_indices("net ").any(|(at, word)| {
        let start = at + word.len();
        let digits = digits_at(text, start);
        starts_word(text, at) && digits > 0 && ends_word(text, start + digits)
    })
}

/// `\b[DP]\d+ r\d+\b`
fn definition_id(text: &str) -> bool {
    text.char_indices().any(|(at, c)| {
        if !matches!(c, 'D' | 'P') || !starts_word(text, at) {
            return false;
        }
        let id = digits_at(text, at + 1);
        let after = at + 1 + id;
        if id == 0 || !text[after..].starts_with(" r") {
            return false;
        }
        let revision = digits_at(text, after + 2);
        revision > 0 && ends_word(text, after + 2 + revision)
    })
}

/// `^[a-z_]+(\.[a-z_]+)+:`
fn code_prefix(text: &str) -> bool {
    let Some((head, _)) = text.split_once(':') else {
        return false;
    };
    let mut parts = head.split('.');
    let lower =
        |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    parts.clone().count() > 1 && parts.all(lower)
}

/// Rust `Debug` shapes: a type or variant name glued to `(` (`Some(`,
/// `Id(3)`), or a struct literal (`Name { field: … }`).
fn debug_text(text: &str) -> bool {
    let glued = text.match_indices('(').any(|(at, _)| {
        let name: String = text[..at]
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        name.chars()
            .last()
            .is_some_and(|first| first.is_ascii_uppercase())
    });
    let literal = text.match_indices(" { ").any(|(at, word)| {
        let before = text[..at].chars().next_back();
        let field: String = text[at + word.len()..]
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || *c == '_')
            .collect();
        before.is_some_and(char::is_alphanumeric)
            && !field.is_empty()
            && text[at + word.len() + field.len()..].starts_with(':')
    });
    glued || literal
}
