//! The search grammar of on-demand generic parts. Each family declares its
//! patterns as data with typed slots (`header {rows}x{columns}
//! {pitch:length}?`); one matcher reads them all, and the family's own form
//! decides whether the values can be built (`library::templates::generic`).
//!
//! Patterns match the whole text, word by word, with no reordering: "5x2" is
//! 5 rows of 2 columns and "2x5" another part. A size code reads as imperial
//! whatever the PCB's display units, unless spelled `1608M` or `metric 1608`.
//! Matching is bounded: at most [`MAX_WORDS`] words over patterns of at most
//! [`MAX_STEPS`] steps, each trying at most two spans.
use super::{MAX_QUERY, rkm};
use crate::ids::ComponentRef;
use crate::library::Drafts;
use crate::library::standard_fields;
use crate::library::templates::catalog::NewFromTemplate;
use crate::library::templates::generic::{
    self, ChipPart, GenericPlan, GenericSpec, GenericState, GenericValue,
};
use crate::model::Project;
use crate::quantity::{self, Decimal, UnitOfMeasure, UnitSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Words of a text the matcher reads.
pub const MAX_WORDS: usize = 8;
/// Steps of one declared pattern.
pub const MAX_STEPS: usize = 6;
/// A header's pitch when the text gives none.
const DEFAULT_PITCH_MM: &str = "2.54";

/// The families that build on demand.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GenericFamily {
    Header,
    Resistor,
    Capacitor,
    Inductor,
    Led,
    Diode,
}

/// One family's declared patterns, and example texts each building a part.
pub struct FamilyPatterns {
    pub family: GenericFamily,
    pub patterns: &'static [&'static str],
    pub examples: &'static [&'static str],
}

/// Every declared pattern. Slots: `{name}` a count; `{name:length}` a length
/// (mm unless it says); `{name:code}` a size code; `{name:ohm}`, `:farad`,
/// `:henry`, `:volt` a typed quantity; `{colour:list}` and `{package:list}` a
/// listed word. A trailing `?` makes a step optional.
pub const FAMILIES: [FamilyPatterns; 6] = [
    FamilyPatterns {
        family: GenericFamily::Header,
        patterns: &[
            "header {rows}x{columns} {pitch:length}?",
            "pin header {rows}x{columns} {pitch:length}?",
        ],
        examples: &["header 1x4", "header 2x5 2mm", "pin header 1×3"],
    },
    FamilyPatterns {
        family: GenericFamily::Resistor,
        patterns: &[
            "resistor {size:code} {value:ohm}?",
            "{value:ohm} {size:code}",
        ],
        examples: &[
            "resistor 0603",
            "resistor 0402 4k7",
            "10k 0603",
            "10 kΩ 0805",
        ],
    },
    FamilyPatterns {
        family: GenericFamily::Capacitor,
        patterns: &[
            "capacitor {size:code} {value:farad}? {voltage:volt}?",
            "{value:farad} {size:code} {voltage:volt}?",
        ],
        examples: &["capacitor 0603", "100nF 0402", "100n 0603 16V"],
    },
    FamilyPatterns {
        family: GenericFamily::Inductor,
        patterns: &[
            "inductor {size:code} {value:henry}?",
            "{value:henry} {size:code}",
        ],
        examples: &["inductor 0603", "4.7uH 0805"],
    },
    FamilyPatterns {
        family: GenericFamily::Led,
        patterns: &["led {size:code} {colour:list}?"],
        examples: &["led 0603", "led 0805 red"],
    },
    FamilyPatterns {
        family: GenericFamily::Diode,
        patterns: &["diode {package:list}"],
        examples: &["diode sod-123", "diode sma"],
    },
];

/// A typed slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Count,
    Length,
    Code,
    Value(ValueKind),
    Colour,
    Package,
}

/// A slot's quantity: the standard field it fills, and the suffixes it reads
/// without a unit, so "10k" is a resistance and "100n" a capacitance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ValueKind {
    field: &'static str,
    /// `""` is a plain number.
    bare: &'static [&'static str],
}

const OHM: ValueKind = ValueKind {
    field: "Resistance",
    bare: &["", "m", "k", "M", "G"],
};
const FARAD: ValueKind = ValueKind {
    field: "Capacitance",
    bare: &["p", "n", "u", "µ", "\u{3bc}"],
};
const HENRY: ValueKind = ValueKind {
    field: "Inductance",
    bare: &["n", "u", "µ", "\u{3bc}", "m"],
};
const VOLT: ValueKind = ValueKind {
    field: "Rated voltage",
    bare: &[],
};

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

/// Parse a declared pattern, or say why it does not parse.
pub fn parse_pattern(text: &str) -> Result<Vec<Step>, String> {
    let steps = text
        .split_whitespace()
        .map(parse_step)
        .collect::<Result<Vec<Step>, String>>()?;
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

fn parse_step(token: &str) -> Result<Step, String> {
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
            pieces.push(Piece::Slot(name.to_owned(), slot_of(name, kind, token)?));
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

fn slot_of(name: &str, kind: &str, token: &str) -> Result<Slot, String> {
    Ok(match (name, kind) {
        (_, "") => Slot::Count,
        (_, "length") => Slot::Length,
        (_, "code") => Slot::Code,
        (_, "ohm") => Slot::Value(OHM),
        (_, "farad") => Slot::Value(FARAD),
        (_, "henry") => Slot::Value(HENRY),
        (_, "volt") => Slot::Value(VOLT),
        ("colour", "list") => Slot::Colour,
        ("package", "list") => Slot::Package,
        _ => return Err(format!("‘{token}’ has an unknown slot type ‘{kind}’")),
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

fn length_units() -> UnitSet {
    let unit = |symbol: &str, scale: f64| UnitOfMeasure {
        symbol: symbol.into(),
        aliases: Vec::new(),
        scale,
        si_prefixes: false,
    };
    UnitSet {
        dimension: quantity::LENGTH,
        units: vec![unit("mm", 1e-3), unit("mil", 2.54e-5), unit("in", 0.0254)],
    }
}

fn read_length(text: &str) -> Option<Bound> {
    let parsed = quantity::parse(&folded(text), &length_units(), "mm").ok()?;
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
fn read_value(text: &str, kind: ValueKind) -> Option<Bound> {
    let units = standard_fields::units(kind.field)?;
    let symbol = units.units.first()?.symbol.clone();
    let spelled = text
        .replace('K', "k")
        .replace("Ohm", "ohm")
        .replace("OHM", "ohm");
    let spelled = rkm(&spelled, &units).unwrap_or(spelled);
    if !spelled.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let suffix = spelled
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
        .trim();
    let bare = units.resolve(suffix).is_none();
    // "0603" alone is a size code, not 603 Ω.
    let code_like = suffix.is_empty() && spelled.starts_with('0') && !spelled.contains('.');
    if code_like || (bare && !kind.bare.contains(&suffix)) {
        return None;
    }
    let parsed = quantity::parse(&spelled, &units, &symbol).ok()?;
    if parsed.si.is_zero() || parsed.si.is_negative() {
        return None;
    }
    Some(Bound::Value(GenericValue {
        field: kind.field.to_owned(),
        text: quantity::format_decimal(parsed.si, &units, &symbol, None),
    }))
}

fn read_slot(slot: Slot, words: &[&str]) -> Option<Bound> {
    let text = words.join(" ");
    match slot {
        Slot::Count => None,
        Slot::Length => read_length(&text),
        Slot::Code => read_code(&text),
        Slot::Value(kind) => read_value(&text, kind),
        Slot::Colour => standard_fields::LED_COLOURS
            .into_iter()
            .find(|c| c.eq_ignore_ascii_case(&text))
            .map(|colour| {
                Bound::Value(GenericValue {
                    field: "Colour".to_owned(),
                    text: colour.to_owned(),
                })
            }),
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

fn walk(steps: &[Step], words: &[&str], found: &mut Vec<(String, Bound)>) -> bool {
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
                read_slot(*slot, taken).map(|b| vec![(name.clone(), b)])
            }
            (pieces, [word]) => read_glued(pieces, word),
            _ => None,
        };
        if let Some(bound) = bound {
            let kept = found.len();
            found.extend(bound);
            if walk(rest, left, found) {
                return true;
            }
            found.truncate(kept);
        }
    }
    step.optional && walk(rest, words, found)
}

/// The slots of `words` when `pattern` reads all of them.
fn read_pattern(pattern: &str, words: &[&str]) -> Option<Vec<(String, Bound)>> {
    let steps = parse_pattern(pattern).ok()?;
    let mut found = Vec::new();
    walk(&steps, words, &mut found).then_some(found)
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

fn header_spec(bounds: &[(String, Bound)]) -> Option<(GenericSpec, Vec<GenericBadge>)> {
    let (rows, columns) = (count(bounds, "rows")?, count(bounds, "columns")?);
    let typed = bounds.iter().find_map(|(_, b)| match b {
        Bound::Length(mm) => Some(*mm),
        _ => None,
    });
    let pitch_mm = match typed {
        Some(mm) => mm,
        None => DEFAULT_PITCH_MM.parse().ok()?,
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
    family: GenericFamily,
    bounds: &[(String, Bound)],
) -> Option<(GenericSpec, Vec<GenericBadge>)> {
    match family {
        GenericFamily::Header => header_spec(bounds),
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
    project: &Project,
    drafts: &Drafts,
    family: GenericFamily,
    pattern: &str,
    bounds: &[(String, Bound)],
) -> Option<GenericCandidate> {
    let (spec, mut badges) = spec_of(family, bounds)?;
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
pub fn search(project: &Project, drafts: &Drafts, query: &str) -> GenericReport {
    let words: Vec<&str> = query.split_whitespace().collect();
    if query.len() > MAX_QUERY || words.is_empty() || words.len() > MAX_WORDS {
        return GenericReport::default();
    }
    let mut candidates: Vec<GenericCandidate> = Vec::new();
    for family in &FAMILIES {
        for pattern in family.patterns {
            let Some(bounds) = read_pattern(pattern, &words) else {
                continue;
            };
            let Some(found) = candidate(project, drafts, family.family, pattern, &bounds) else {
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
    GenericReport { candidates }
}

#[cfg(test)]
mod tests;
