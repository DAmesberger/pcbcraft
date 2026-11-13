//! The search grammar of on-demand generic parts. Each family declares its
//! patterns as data with typed slots (`header {rows}x{columns}
//! {pitch:length}?`); one matcher reads them all, and the family's own form
//! decides whether the values can be built (`library::templates::generic`).
//!
//! The families, their patterns and examples, the value slot types and the
//! defaults are data (the shipped library's `catalog/generic-families.json`,
//! [`GenericGrammar`]); this module is the matcher that reads them.
//!
//! Patterns match the whole text, word by word, with no reordering: "5x2" is
//! 5 rows of 2 columns and "2x5" another part. A size code reads as imperial
//! whatever the PCB's display units, unless spelled `1608M` or `metric 1608`.
//! Matching is bounded: at most [`MAX_WORDS`] words over patterns of at most
//! [`MAX_STEPS`] steps, each trying at most two spans.
use super::{MAX_QUERY, rkm};
use crate::diagnostics::{Error, Result};
use crate::ids::ComponentRef;
use crate::library::Drafts;
use crate::library::standards::{self, GenericGrammar, Standards};
use crate::library::templates::catalog::NewFromTemplate;
use crate::library::templates::generic::{
    self, ChipPart, GenericPlan, GenericSpec, GenericState, GenericValue,
};
use crate::model::Project;
use crate::quantity::{self, Decimal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Words of a text the matcher reads.
pub const MAX_WORDS: usize = 8;
/// Steps of one declared pattern.
pub const MAX_STEPS: usize = 6;

pub use crate::library::standards::{FamilyPatterns, GenericFamily};

/// A typed slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Count,
    Length,
    Code,
    /// A value slot type, by its index in the grammar's value slots.
    Value(usize),
    Colour,
    Package,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Word(String),
    Slot(String, Slot),
}

/// One whitespace-separated token of a pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pieces: Vec<Piece>,
    optional: bool,
}

/// Every declared pattern of `grammar` parses: what loading the grammar
/// checks beyond [`GenericGrammar::check`].
pub fn check(grammar: &GenericGrammar) -> Result<()> {
    grammar.check()?;
    for family in &grammar.families {
        for pattern in &family.patterns {
            parse_pattern(grammar, pattern)
                .map_err(|why| Error::Invalid(format!("the generic-part grammar: {why}")))?;
        }
    }
    Ok(())
}

/// Parse a declared pattern, or say why it does not parse.
pub fn parse_pattern(
    grammar: &GenericGrammar,
    text: &str,
) -> std::result::Result<Vec<Step>, String> {
    let steps = text
        .split_whitespace()
        .map(|token| parse_step(grammar, token))
        .collect::<std::result::Result<Vec<Step>, String>>()?;
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(format!("‘{text}’ needs 1–{MAX_STEPS} steps"));
    }
    let mut names: Vec<&str> = steps
        .iter()
        .flat_map(|s| &s.pieces)
        .filter_map(|p| match p {
            Piece::Slot(name, _) => Some(name.as_str()),
            Piece::Word(_) => None,
        })
        .collect();
    let slots = names.len();
    names.sort_unstable();
    names.dedup();
    if names.len() != slots {
        return Err(format!("‘{text}’ names a slot twice"));
    }
    Ok(steps)
}

fn parse_step(grammar: &GenericGrammar, token: &str) -> std::result::Result<Step, String> {
    let (body, optional) = match token.strip_suffix('?') {
        Some(body) => (body, true),
        None => (token, false),
    };
    let mut pieces = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        if let Some(open) = rest.strip_prefix('{') {
            let (slot, after) = open
                .split_once('}')
                .ok_or_else(|| format!("‘{token}’ has an open slot"))?;
            let (name, kind) = slot.split_once(':').unwrap_or((slot, ""));
            pieces.push(Piece::Slot(
                name.to_owned(),
                slot_of(grammar, name, kind, token)?,
            ));
            rest = after;
        } else {
            let (word, after) = rest.split_at(rest.find('{').unwrap_or(rest.len()));
            pieces.push(Piece::Word(word.to_owned()));
            rest = after;
        }
    }
    let glued = pieces.len() > 1;
    if pieces.is_empty()
        || (glued
            && pieces
                .iter()
                .any(|p| matches!(p, Piece::Slot(_, s) if *s != Slot::Count)))
    {
        return Err(format!("‘{token}’: only a count slot joins text"));
    }
    Ok(Step { pieces, optional })
}

fn slot_of(
    grammar: &GenericGrammar,
    name: &str,
    kind: &str,
    token: &str,
) -> std::result::Result<Slot, String> {
    Ok(match (name, kind) {
        (_, "") => Slot::Count,
        (_, "length") => Slot::Length,
        (_, "code") => Slot::Code,
        ("colour", "list") => Slot::Colour,
        ("package", "list") => Slot::Package,
        _ => match grammar.value_slots.iter().position(|v| v.slot == kind) {
            Some(index) => Slot::Value(index),
            None => return Err(format!("‘{token}’ has an unknown slot type ‘{kind}’")),
        },
    })
}

/// What a slot read.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Bound {
    Count(u32),
    Length(Decimal),
    Code { code: String, metric: bool },
    Value(GenericValue),
    Package(String),
}

/// `×` and `x` are one letter; literals compare without case.
fn folded(word: &str) -> String {
    word.to_lowercase().replace('×', "x")
}

/// A length in the grammar's length units; a bare number is in the first.
fn read_length(grammar: &GenericGrammar, text: &str) -> Option<Bound> {
    let units = grammar.length_unit_set();
    let bare = units.units.first()?.symbol.clone();
    let parsed = quantity::parse(&folded(text), &units, &bare).ok()?;
    if parsed.si.is_zero() || parsed.si.is_negative() {
        return None;
    }
    parsed.si.shift(3).ok().map(Bound::Length)
}

fn read_code(text: &str) -> Option<Bound> {
    let text = folded(text);
    let digits = |code: &str, metric: bool| {
        ((3..=5).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_digit())).then(|| {
            Bound::Code {
                code: code.to_owned(),
                metric,
            }
        })
    };
    if let Some(code) = text.strip_prefix("metric ") {
        return digits(code, true);
    }
    if let Some(code) = text.strip_prefix("imperial ") {
        return digits(code, false);
    }
    match text.strip_suffix('m') {
        Some(code) => digits(code, true),
        None => digits(&text, false),
    }
}

/// A quantity through the one parser, so it stays an exact decimal: `10k`,
/// `4k7`, `10 kΩ`. It is not read as this kind when its suffix is neither a
/// unit of the kind nor one the kind reads bare, or when it is not positive.
fn read_value(standards: &Standards, text: &str, kind: usize) -> Option<Bound> {
    let kind = standards.generic.value_slots.get(kind)?;
    let units = standards.fields.units(&kind.field)?;
    let symbol = units.units.first()?.symbol.clone();
    let spelled = text
        .replace('K', "k")
        .replace("Ohm", "ohm")
        .replace("OHM", "ohm");
    let spelled = rkm(&spelled, units).unwrap_or(spelled);
    if !spelled.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let suffix = spelled
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
        .trim();
    let bare = units.resolve(suffix).is_none();
    // "0603" alone is a size code, not 603 Ω.
    let code_like = suffix.is_empty() && spelled.starts_with('0') && !spelled.contains('.');
    if code_like || (bare && !kind.bare.iter().any(|b| b == suffix)) {
        return None;
    }
    let parsed = quantity::parse(&spelled, units, &symbol).ok()?;
    if parsed.si.is_zero() || parsed.si.is_negative() {
        return None;
    }
    Some(Bound::Value(GenericValue {
        field: kind.field.clone(),
        text: quantity::format_decimal(parsed.si, units, &symbol, None),
    }))
}

fn read_slot(standards: &Standards, slot: Slot, words: &[&str]) -> Option<Bound> {
    let text = words.join(" ");
    match slot {
        Slot::Count => None,
        Slot::Length => read_length(&standards.generic, &text),
        Slot::Code => read_code(&text),
        Slot::Value(kind) => read_value(standards, &text, kind),
        Slot::Colour => {
            let field = &standards.generic.colour_field;
            standards
                .fields
                .choices(field)?
                .iter()
                .find(|c| c.eq_ignore_ascii_case(&text))
                .map(|colour| {
                    Bound::Value(GenericValue {
                        field: field.clone(),
                        text: colour.clone(),
                    })
                })
        }
        Slot::Package => generic::diode_row(&text).map(|r| Bound::Package(r.name.clone())),
    }
}

/// One word against a step of joined pieces (`{rows}x{columns}`).
fn read_glued(pieces: &[Piece], word: &str) -> Option<Vec<(String, Bound)>> {
    let word = folded(word);
    let mut rest = word.as_str();
    let mut out = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Word(text) => rest = rest.strip_prefix(text.as_str())?,
            Piece::Slot(name, _) => {
                let end = rest
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(rest.len());
                let (digits, after) = rest.split_at(end);
                if digits.is_empty() {
                    return None;
                }
                // More digits than a count holds still reach the form's own
                // refusal, as a count beyond any limit.
                let count = digits.parse().unwrap_or(u32::MAX);
                out.push((name.clone(), Bound::Count(count)));
                rest = after;
            }
        }
    }
    rest.is_empty().then_some(out)
}

fn walk(
    standards: &Standards,
    steps: &[Step],
    words: &[&str],
    found: &mut Vec<(String, Bound)>,
) -> bool {
    let Some((step, rest)) = steps.split_first() else {
        return words.is_empty();
    };
    // A value or a length may be two words ("10 kΩ"), a code `metric 1608`.
    for span in 1..=2 {
        let (Some(taken), Some(left)) = (words.get(..span), words.get(span..)) else {
            break;
        };
        let bound = match (step.pieces.as_slice(), taken) {
            ([Piece::Slot(name, slot)], _) if *slot != Slot::Count => {
                read_slot(standards, *slot, taken).map(|b| vec![(name.clone(), b)])
            }
            (pieces, [word]) => read_glued(pieces, word),
            _ => None,
        };
        if let Some(bound) = bound {
            let kept = found.len();
            found.extend(bound);
            if walk(standards, rest, left, found) {
                return true;
            }
            found.truncate(kept);
        }
    }
    step.optional && walk(standards, rest, words, found)
}

/// The slots of `words` when `pattern` reads all of them; an error when the
/// pattern does not parse.
fn read_pattern(
    standards: &Standards,
    pattern: &str,
    words: &[&str],
) -> Result<Option<Vec<(String, Bound)>>> {
    let steps = parse_pattern(&standards.generic, pattern)
        .map_err(|why| Error::Invalid(format!("the generic-part grammar: {why}")))?;
    let mut found = Vec::new();
    Ok(walk(standards, &steps, words, &mut found).then_some(found))
}

/// One interpretation badge of a result row.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GenericBadge {
    pub label: String,
    /// What it means, in the row's hover.
    pub tip: String,
}

/// One part a text reads as.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GenericCandidate {
    pub family: GenericFamily,
    /// The declared pattern that read the text.
    pub pattern: String,
    /// The component's name, or the part's label when refused.
    pub name: String,
    /// The dim line: its pins and how its values are sourced, or why it is
    /// refused.
    pub detail: String,
    pub badges: Vec<GenericBadge>,
    /// Identity: the generating parameters.
    pub spec: GenericSpec,
    pub state: GenericState,
    /// What choosing it creates (`library.publish_from_template`), when it
    /// is not yet created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub would_create: Option<NewFromTemplate>,
    /// Values for each placed instance, not part of the identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<GenericValue>,
    /// The starter row that is this same part, not to list beside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covers_row: Option<String>,
}

impl GenericCandidate {
    /// The published part, when it is one.
    pub fn component(&self) -> Option<ComponentRef> {
        match self.state {
            GenericState::Existing { component } => Some(component),
            _ => None,
        }
    }
}

/// `library.generic_parts`'s result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GenericReport {
    pub candidates: Vec<GenericCandidate>,
}

fn badge(label: impl Into<String>, tip: impl Into<String>) -> GenericBadge {
    GenericBadge {
        label: label.into(),
        tip: tip.into(),
    }
}

/// `length × width` of a chip or a molded body, from the form.
fn body_badge(plan: &GenericPlan) -> Option<GenericBadge> {
    let [l, w, _] = plan.form.envelope()?;
    let (l, w) = (quantity::format_number(l), quantity::format_number(w));
    Some(badge(
        format!("{l} × {w} mm"),
        "Body length × width of this package",
    ))
}

fn count(bounds: &[(String, Bound)], name: &str) -> Option<u32> {
    bounds.iter().find_map(|(n, b)| match b {
        Bound::Count(c) if n == name => Some(*c),
        _ => None,
    })
}

/// The values a text gave each placed instance, as badges and values.
fn value_parts(bounds: &[(String, Bound)]) -> (Vec<GenericBadge>, Vec<GenericValue>) {
    let values: Vec<GenericValue> = bounds
        .iter()
        .filter_map(|(_, b)| match b {
            Bound::Value(v) => Some(v.clone()),
            _ => None,
        })
        .collect();
    let badges = values
        .iter()
        .map(|v| badge(&v.text, format!("{} of each instance you place", v.field)))
        .collect();
    (badges, values)
}

fn chip_spec(
    part: ChipPart,
    bounds: &[(String, Bound)],
) -> Option<(GenericSpec, Vec<GenericBadge>)> {
    bounds.iter().find_map(|(_, b)| {
        let Bound::Code { code, metric } = b else {
            return None;
        };
        let (imperial, system, tip) = if *metric {
            let tip = |imperial: &str| format!("{code} is the metric code of imperial {imperial}");
            match generic::imperial_of(part, code) {
                Some(imperial) => {
                    let tip = tip(&imperial);
                    (imperial, "metric", tip)
                }
                // Refused by the plan, under the code as typed.
                None => (format!("{code}M"), "metric", tip("no row")),
            }
        } else {
            let tip = format!("Size codes read as imperial; {code}M names the metric {code}");
            (code.clone(), "imperial", tip)
        };
        let badges = vec![badge(system, tip)];
        Some((GenericSpec::Chip { part, imperial }, badges))
    })
}

fn header_spec(
    grammar: &GenericGrammar,
    bounds: &[(String, Bound)],
) -> Option<(GenericSpec, Vec<GenericBadge>)> {
    let (rows, columns) = (count(bounds, "rows")?, count(bounds, "columns")?);
    let typed = bounds.iter().find_map(|(_, b)| match b {
        Bound::Length(mm) => Some(*mm),
        _ => None,
    });
    let pitch_mm = match typed {
        Some(mm) => mm,
        None => grammar.header_pitch_mm.parse().ok()?,
    };
    let badges = vec![
        badge(
            "rows × columns",
            format!("{rows} rows of {columns} pins; {columns}x{rows} is a different part"),
        ),
        match typed {
            Some(_) => badge(format!("{pitch_mm} mm"), "The pitch as typed"),
            None => badge(
                format!("{pitch_mm} mm (default)"),
                "No pitch typed: add one, such as 2 mm",
            ),
        },
    ];
    let spec = GenericSpec::Header {
        rows,
        columns,
        pitch_mm,
    };
    Some((spec, badges))
}

/// The spec a family's bound slots name, with the badges that say how the
/// text was read.
fn spec_of(
    grammar: &GenericGrammar,
    family: GenericFamily,
    bounds: &[(String, Bound)],
) -> Option<(GenericSpec, Vec<GenericBadge>)> {
    match family {
        GenericFamily::Header => header_spec(grammar, bounds),
        GenericFamily::Resistor => chip_spec(ChipPart::Resistor, bounds),
        GenericFamily::Capacitor => chip_spec(ChipPart::Capacitor, bounds),
        GenericFamily::Inductor => chip_spec(ChipPart::Inductor, bounds),
        GenericFamily::Led => chip_spec(ChipPart::Led, bounds),
        GenericFamily::Diode => bounds.iter().find_map(|(_, b)| match b {
            Bound::Package(package) => Some((
                GenericSpec::Diode {
                    package: package.clone(),
                },
                Vec::new(),
            )),
            _ => None,
        }),
    }
}

fn candidate(
    grammar: &GenericGrammar,
    project: &Project,
    drafts: &Drafts,
    family: GenericFamily,
    pattern: &str,
    bounds: &[(String, Bound)],
) -> Option<GenericCandidate> {
    let (spec, mut badges) = spec_of(grammar, family, bounds)?;
    let (value_badges, values) = value_parts(bounds);
    let plan = match spec.plan() {
        Ok(plan) => plan,
        // A word of the family's own makes this a request for this family; a
        // pattern of slots alone may simply be another search.
        Err(reason) if pattern.split_whitespace().any(|t| !t.contains('{')) => {
            return Some(GenericCandidate {
                family,
                pattern: pattern.to_owned(),
                name: spec.label(),
                detail: reason.clone(),
                badges: Vec::new(),
                spec,
                state: GenericState::Refused { reason },
                would_create: None,
                values,
                covers_row: None,
            });
        }
        Err(_) => return None,
    };
    if !matches!(family, GenericFamily::Header) {
        badges.extend(body_badge(&plan));
    }
    badges.extend(value_badges);
    let state = generic::existing(&project.library, drafts, &spec, &plan.form)
        .unwrap_or(GenericState::NotCreated);
    let pins = plan.form.lead_count();
    let body = format!("{pins} pins · {} values", plan.basis);
    let detail = match &state {
        GenericState::Existing { component } => {
            format!("In this project, {} · {body}", component.revision)
        }
        GenericState::Draft { .. } => format!("A draft in this project · {body}"),
        _ => body,
    };
    Some(GenericCandidate {
        family,
        pattern: pattern.to_owned(),
        name: match &state {
            GenericState::Existing { component } => project
                .library
                .component(*component)
                .map_or_else(|| plan.request.name.clone(), |c| c.meta.name.clone()),
            _ => plan.request.name.clone(),
        },
        detail,
        badges,
        would_create: matches!(state, GenericState::NotCreated).then(|| plan.request.clone()),
        spec,
        state,
        values,
        covers_row: plan.covers_row,
    })
}

/// `library.generic_parts`: every part `query` reads as under the declared
/// patterns, in family order; none when it reads as none. "10u 0603" is a
/// capacitor and an inductor: both are listed, each saying which it is.
/// The grammar is the installed standards'; a pattern of it that does not
/// parse is an error.
pub fn search(project: &Project, drafts: &Drafts, query: &str) -> Result<GenericReport> {
    let words: Vec<&str> = query.split_whitespace().collect();
    if query.len() > MAX_QUERY || words.is_empty() || words.len() > MAX_WORDS {
        return Ok(GenericReport::default());
    }
    let standards = standards::get()?;
    let grammar = &standards.generic;
    let mut candidates: Vec<GenericCandidate> = Vec::new();
    for family in &grammar.families {
        for pattern in &family.patterns {
            let Some(bounds) = read_pattern(standards, pattern, &words)? else {
                continue;
            };
            let Some(found) = candidate(grammar, project, drafts, family.family, pattern, &bounds)
            else {
                continue;
            };
            if !candidates
                .iter()
                .any(|c| c.spec == found.spec && c.values == found.values)
            {
                candidates.push(found);
            }
        }
    }
    Ok(GenericReport { candidates })
}

#[cfg(test)]
mod tests;
