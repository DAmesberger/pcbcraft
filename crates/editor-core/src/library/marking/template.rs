//! Marking text templates: parsing and rendering.
//!
//! Grammar: literal text with `{…}` placeholders; `{{` and `}}` are literal
//! braces and a line break starts a new marking line. Placeholders:
//! `{mpn}`, `{manufacturer}`, `{designator}`, `{var:<name>}` and
//! `{field:<lineage>}` with an optional code formatter
//! `{field:<lineage>|<code>}`. The lineage is written `D12` or `12`. Codes:
//! `code3`, `code4`, `eia96`, `cap3` and `letter:<table>`.
use super::codes::{self, CodeScheme, TableRef};
use super::{MAX_MARKING_VAR, MarkingInputs};
use crate::ids::{DefinitionId, Id};

/// One parsed piece of a template.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Piece {
    Literal(String),
    Mpn,
    Manufacturer,
    Designator,
    Var(String),
    Field(DefinitionId, Option<CodeScheme>),
}

/// A variable name: 1–32 bytes of `a-z`, `0-9` and `_`.
pub(super) fn var_name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_MARKING_VAR
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn lineage(text: &str) -> Option<DefinitionId> {
    let digits = text.strip_prefix('D').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<Id>().ok().map(DefinitionId)
}

fn formatter(text: &str) -> Result<CodeScheme, String> {
    Ok(match text {
        "code3" => CodeScheme::Resistor3Digit,
        "code4" => CodeScheme::Resistor4Digit,
        "eia96" => CodeScheme::Eia96,
        "cap3" => CodeScheme::CapacitorPf3Digit,
        other => match other.strip_prefix("letter:") {
            Some(table) => {
                let scheme = CodeScheme::VoltageLetter {
                    table: TableRef(table.into()),
                };
                if let Some(problem) = scheme.problem() {
                    return Err(problem);
                }
                scheme
            }
            None => return Err(format!("unknown code \"{other}\"")),
        },
    })
}

fn placeholder(body: &str) -> Result<Piece, String> {
    match body {
        "mpn" => return Ok(Piece::Mpn),
        "manufacturer" => return Ok(Piece::Manufacturer),
        "designator" => return Ok(Piece::Designator),
        _ => {}
    }
    if let Some(name) = body.strip_prefix("var:") {
        return if var_name_valid(name) {
            Ok(Piece::Var(name.into()))
        } else {
            Err(format!("\"{name}\" is not a variable name (a-z, 0-9, _)"))
        };
    }
    if let Some(rest) = body.strip_prefix("field:") {
        let (id, code) = match rest.split_once('|') {
            Some((id, code)) => (id, Some(formatter(code)?)),
            None => (rest, None),
        };
        let id = lineage(id).ok_or_else(|| format!("\"{id}\" is not a field lineage"))?;
        return Ok(Piece::Field(id, code));
    }
    Err(format!("unknown placeholder {{{body}}}"))
}

/// Parse a template, or say why it is not one.
pub(super) fn parse(template: &str) -> Result<Vec<Piece>, String> {
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                literal.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                literal.push('}');
            }
            '}' => return Err("an unmatched \"}\" (write \"}}\")".into()),
            '{' => {
                let mut body = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some('{') | None => {
                            return Err("an unclosed placeholder (write \"{{\")".into());
                        }
                        Some(c) => body.push(c),
                    }
                }
                if !literal.is_empty() {
                    pieces.push(Piece::Literal(std::mem::take(&mut literal)));
                }
                pieces.push(placeholder(body.trim())?);
            }
            '\n' => literal.push('\n'),
            c if c.is_control() => return Err("a control character".into()),
            c => literal.push(c),
        }
    }
    if !literal.is_empty() {
        pieces.push(Piece::Literal(literal));
    }
    Ok(pieces)
}

/// Why a template renders nothing.
#[derive(Debug, PartialEq)]
pub(super) enum Missing {
    /// A value the text needs is absent (no Part, an unset field).
    Value(String),
    /// A code cannot represent the value.
    Code(String),
}

/// Render `pieces` for `inputs`. An unset variable renders as nothing (a
/// date code not yet known); any other missing value makes the whole
/// template unrepresentable.
pub(super) fn render(pieces: &[Piece], inputs: &MarkingInputs) -> Result<String, Missing> {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Literal(text) => out.push_str(text),
            Piece::Designator => out.push_str(&inputs.designator),
            Piece::Mpn => out.push_str(
                inputs
                    .mpn
                    .as_deref()
                    .ok_or_else(|| Missing::Value("no manufacturer part number".into()))?,
            ),
            Piece::Manufacturer => out.push_str(
                inputs
                    .manufacturer
                    .as_deref()
                    .ok_or_else(|| Missing::Value("no manufacturer".into()))?,
            ),
            Piece::Var(name) => {
                if let Some(value) = inputs.vars.get(name) {
                    out.push_str(value);
                }
            }
            Piece::Field(id, code) => {
                let (text, si) = inputs
                    .fields
                    .get(id)
                    .ok_or_else(|| Missing::Value(format!("field {id} has no value")))?;
                match code {
                    None => out.push_str(text),
                    Some(scheme) => {
                        let value = si.ok_or_else(|| {
                            Missing::Code(format!(
                                "{text} is not a number for a {}",
                                scheme.label()
                            ))
                        })?;
                        match codes::encode(scheme, value) {
                            Ok(Some(code)) => out.push_str(&code),
                            Ok(None) => {
                                return Err(Missing::Code(format!(
                                    "No {} code for {text}",
                                    scheme.label()
                                )));
                            }
                            Err(error) => return Err(Missing::Code(error.to_string())),
                        }
                    }
                }
            }
        }
    }
    // Values never inject control characters into the drawn text.
    Ok(out
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_placeholder() {
        let _context = crate::library::standards::Context::fixture().map(|c| c.enter());
        assert_eq!(
            parse("{mpn} {{x}}\n{var:date_code}{field:D7|eia96}{field:9}"),
            Ok(vec![
                Piece::Mpn,
                Piece::Literal(" {x}\n".into()),
                Piece::Var("date_code".into()),
                Piece::Field(DefinitionId(7), Some(CodeScheme::Eia96)),
                Piece::Field(DefinitionId(9), None),
            ])
        );
        assert!(parse("{field:3|letter:tantalum}").is_ok());
    }

    #[test]
    fn refuses_malformed_templates() {
        for bad in [
            "{mpn",
            "mpn}",
            "{part}",
            "{var:Date}",
            "{field:x}",
            "{field:3|code5}",
            "{field:3|letter:vendor}",
            "a\tb",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn renders_and_reports_what_is_missing() {
        let mut inputs = MarkingInputs {
            designator: "R1".into(),
            ..Default::default()
        };
        inputs
            .fields
            .insert(DefinitionId(7), ("10.3 kΩ".into(), Some(10.3e3)));
        let pieces = parse("{designator}:{field:7|code4} {var:lot}").unwrap_or_default();
        assert_eq!(render(&pieces, &inputs), Ok("R1:1032 ".into()));
        let eia = parse("{field:7|eia96}").unwrap_or_default();
        assert!(matches!(render(&eia, &inputs), Err(Missing::Code(_))));
        let mpn = parse("{mpn}").unwrap_or_default();
        assert!(matches!(render(&mpn, &inputs), Err(Missing::Value(_))));
    }
}
