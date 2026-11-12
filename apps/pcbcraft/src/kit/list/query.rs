//! A list's filter, read by the one catalog query grammar
//! (`pcbcraft_core::catalog::parse_query`): no second parser. A list names
//! its columns as query fields ([`Field`]: a length, an angle, a number, a
//! choice or text), which become the query's vocabulary; each parsed term
//! becomes a chip ("X ≥ 10 mm"), and a term the grammar cannot read
//! becomes an error chip that says why, while the other terms still apply.
//! Plain words match the row's searchable text. Pure: no egui.
use pcbcraft_core::catalog::{
    self, CatalogQuery, Term, Test, Vocabulary, VocabularyField, VocabularyKind,
};
use pcbcraft_core::fields::{Choice, FieldValue, QuantityKind, ValueType};
use pcbcraft_core::ids::{ChoiceId, DefinitionId, QuantityKindRef, Revision};
use pcbcraft_core::quantity::{Decimal, UnitOfMeasure, UnitSet};
use std::ops::Range;

/// What a column holds, as the query compares it.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A length in mm; typed in mm, mil, in, µm or m.
    Length,
    /// An angle in degrees.
    Angle,
    /// A plain number.
    Number,
    /// One of these labels (side: top, bottom).
    Choice(Vec<String>),
    /// Text, matched as a case-folded substring; `pkg:` reads the column
    /// marked [`Field::package`].
    Text,
}

/// One column a list's filter can name, by its caption ("x", "side").
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
    /// The package column `pkg:` reads.
    pub package: bool,
}

impl Field {
    pub fn new(name: &str, kind: Kind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            package: false,
        }
    }

    pub fn package(name: &str) -> Self {
        Self {
            package: true,
            ..Self::new(name, Kind::Text)
        }
    }
}

/// A row's value in one column.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    None,
    /// In mm.
    Length(f64),
    /// In degrees.
    Angle(f64),
    Number(f64),
    /// The index into the field's choices.
    Choice(usize),
    Text(String),
}

/// A list's query vocabulary: its fields as catalog fields, by position.
#[derive(Clone, Debug)]
pub struct Fields {
    fields: Vec<Field>,
    vocabulary: Vocabulary,
}

/// Field ids are the column's position plus one: local to this list.
fn id_of(index: usize) -> DefinitionId {
    DefinitionId(index as u64 + 1)
}

fn index_of(id: DefinitionId) -> Option<usize> {
    usize::try_from(id.0).ok()?.checked_sub(1)
}

fn unit(symbol: &str, aliases: &[&str], scale: f64, si_prefixes: bool) -> UnitOfMeasure {
    UnitOfMeasure {
        symbol: symbol.to_owned(),
        aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
        scale,
        si_prefixes,
    }
}

fn kind(name: &str, units: UnitSet, default_unit: &str) -> VocabularyKind {
    VocabularyKind {
        name: name.to_owned(),
        quantity: QuantityKind {
            units,
            default_unit: default_unit.to_owned(),
        },
    }
}

/// Lengths: metres with SI prefixes (mm, µm), mil and inch; bare numbers
/// in mm, as the board shows them.
fn length() -> VocabularyKind {
    kind(
        "length",
        UnitSet {
            dimension: pcbcraft_core::quantity::LENGTH,
            units: vec![
                unit("m", &[], 1.0, true),
                unit("mil", &["thou"], 2.54e-5, false),
                unit("in", &["inch"], 0.0254, false),
            ],
        },
        "mm",
    )
}

fn angle() -> VocabularyKind {
    kind(
        "angle",
        UnitSet {
            dimension: pcbcraft_core::quantity::DIMENSIONLESS,
            units: vec![unit("°", &["deg"], 1.0, false)],
        },
        "°",
    )
}

impl Fields {
    pub fn new(fields: Vec<Field>) -> Self {
        let quantity = |kind: &VocabularyKind| ValueType::Quantity {
            kind: QuantityKindRef {
                id: DefinitionId(0),
                revision: Revision::FIRST,
            },
            min: None,
            max: None,
        };
        let vocabulary = Vocabulary {
            fields: fields
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let (value_type, kind) = match &f.kind {
                        Kind::Length => {
                            let k = length();
                            (quantity(&k), Some(k))
                        }
                        Kind::Angle => {
                            let k = angle();
                            (quantity(&k), Some(k))
                        }
                        Kind::Number => (
                            ValueType::Integer {
                                min: i64::MIN,
                                max: i64::MAX,
                            },
                            None,
                        ),
                        Kind::Choice(labels) => (
                            ValueType::Enum {
                                choices: labels
                                    .iter()
                                    .enumerate()
                                    .map(|(c, label)| Choice {
                                        id: ChoiceId(c as u32 + 1),
                                        label: label.clone(),
                                    })
                                    .collect(),
                            },
                            None,
                        ),
                        Kind::Text => (ValueType::Text { max_bytes: 256 }, None),
                    };
                    VocabularyField {
                        field: id_of(i),
                        name: f.name.clone(),
                        value_type,
                        kind,
                    }
                })
                .collect(),
        };
        Self { fields, vocabulary }
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// A cell as the catalog's field value.
    fn value(&self, index: usize, cell: &Cell) -> Option<FieldValue> {
        let si = |v: f64| Decimal::from_f64(v).ok();
        match cell {
            Cell::None => None,
            Cell::Length(mm) => si(mm * 1e-3).map(|si| FieldValue::Quantity {
                si,
                display_unit: None,
            }),
            Cell::Angle(deg) => si(*deg).map(|si| FieldValue::Quantity {
                si,
                display_unit: None,
            }),
            Cell::Number(v) => Some(FieldValue::Integer(v.round() as i64)),
            Cell::Choice(c) => match self.fields.get(index).map(|f| &f.kind) {
                Some(Kind::Choice(_)) => Some(FieldValue::Enum(ChoiceId(*c as u32 + 1))),
                _ => None,
            },
            Cell::Text(t) => Some(FieldValue::Text(t.clone())),
        }
    }
}

/// One chip of a parsed filter: what it shows, and the source text its
/// close button removes.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub label: String,
    pub span: Range<usize>,
    /// Why the term is not read (an error chip), with an example.
    pub error: Option<String>,
}

/// A filter line read against a list's fields.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub text: String,
    pub chips: Vec<Chip>,
    query: CatalogQuery,
}

/// Filters a list cannot apply (they are about library entries).
fn unsupported(term: &Term) -> Option<&'static str> {
    match term {
        Term::Has { .. } => Some("has:"),
        Term::Project => Some("src:"),
        Term::Components | Term::Parts => Some("kind:"),
        Term::Mpn { .. } => Some("mpn:"),
        Term::Implements { .. } => Some("implements:"),
        Term::Pins { .. } => Some("pins:"),
        Term::Pitch { .. } => Some("pitch:"),
        Term::Body { .. } => Some("body:"),
        _ => None,
    }
}

impl Filter {
    /// Read `text`. Each term the grammar cannot read is set aside as an
    /// error chip and the rest are read again, so one typo does not empty
    /// the list.
    pub fn parse(text: &str, fields: &Fields) -> Self {
        let text = text.get(..catalog::MAX_QUERY.min(text.len())).unwrap_or("");
        let mut chips = Vec::new();
        let spans = match catalog::term_spans(text) {
            Ok(spans) => spans,
            Err(why) => {
                chips.push(Chip {
                    label: text.trim().to_owned(),
                    span: 0..text.len(),
                    error: Some(why),
                });
                return Self {
                    text: text.to_owned(),
                    chips,
                    query: CatalogQuery::default(),
                };
            }
        };
        let mut kept: Vec<Range<usize>> = spans.into_iter().take(catalog::MAX_TERMS).collect();
        let mut errors: Vec<(Range<usize>, String)> = Vec::new();
        let query = loop {
            let line: Vec<&str> = kept.iter().filter_map(|r| text.get(r.clone())).collect();
            match catalog::parse_query(&line.join(" "), &fields.vocabulary) {
                Ok(query) => break query,
                Err(error) => {
                    // The failed term, by its text; quoted terms never fail.
                    let at = kept
                        .iter()
                        .position(|r| text.get(r.clone()) == Some(error.term.as_str()));
                    match at {
                        Some(at) => {
                            let span = kept.remove(at);
                            errors.push((span, error.message));
                        }
                        None => {
                            errors.extend(kept.drain(..).map(|r| (r, error.message.clone())));
                        }
                    }
                }
            }
        };
        // Terms and kept spans pair one to one, in order.
        let mut terms = Vec::new();
        for (term, span) in query.terms.into_iter().zip(kept) {
            if let Some(key) = unsupported(&term) {
                errors.push((span, format!("{key} is a library filter, not one of this list")));
                continue;
            }
            chips.push(Chip {
                label: term.chip(),
                span,
                error: None,
            });
            terms.push(term);
        }
        let names: Vec<&str> = fields.fields.iter().map(|f| f.name.as_str()).collect();
        for (span, message) in errors {
            let example = names
                .first()
                .map(|n| format!(" (try {n}:…)"))
                .unwrap_or_default();
            chips.push(Chip {
                label: text.get(span.clone()).unwrap_or_default().to_owned(),
                span,
                error: Some(format!("{message}{example}")),
            });
        }
        chips.sort_by_key(|c| c.span.start);
        Self {
            text: text.to_owned(),
            chips,
            query: CatalogQuery { terms },
        }
    }

    /// No term applies: every row shows.
    pub fn is_empty(&self) -> bool {
        self.query.terms.is_empty()
    }

    /// Whether a row meets every term. `searchable` is the row's text;
    /// `cell(i)` its value in field `i`.
    pub fn matches(&self, fields: &Fields, searchable: &str, cell: impl Fn(usize) -> Cell) -> bool {
        let folded = catalog::fold(searchable);
        let field_value = |id: DefinitionId| {
            let i = index_of(id)?;
            fields.value(i, &cell(i))
        };
        let package = || {
            fields
                .fields
                .iter()
                .position(|f| f.package)
                .and_then(|i| match cell(i) {
                    Cell::Text(t) => Some(catalog::fold(&t)),
                    _ => None,
                })
        };
        self.query.terms.iter().all(|term| match term {
            Term::Text { text } => folded.contains(text.as_str()),
            Term::Value { text, readings } => {
                folded.contains(text.as_str())
                    || readings.iter().any(|r| {
                        field_value(r.field).is_some_and(|v| r.test.accepts(&v))
                    })
            }
            Term::Field { tests, .. } => tests.iter().any(|t| {
                field_value(t.field).is_some_and(|v| match (&t.test, &v) {
                    // A text column holds the label of a choice-like value.
                    (Test::Contains { text }, FieldValue::Text(s)) => {
                        catalog::fold(s).contains(text.as_str())
                    }
                    _ => t.test.accepts(&v),
                })
            }),
            Term::Package { text } => package().unwrap_or_else(|| folded.clone()).contains(text),
            Term::SizeCode { code, .. } => package()
                .unwrap_or_else(|| folded.clone())
                .contains(code.as_str()),
            _ => true,
        })
    }

    /// The text without the term at `span` (a chip's close button).
    pub fn without(&self, span: &Range<usize>) -> String {
        let before = self.text.get(..span.start).unwrap_or_default();
        let after = self.text.get(span.end..).unwrap_or_default();
        format!("{} {}", before.trim_end(), after.trim_start())
            .trim()
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Part {
        name: &'static str,
        x: f64,
        side: usize,
        package: &'static str,
        rot: f64,
    }

    const PARTS: [Part; 4] = [
        Part {
            name: "R1 10k",
            x: 12.0,
            side: 0,
            package: "0603",
            rot: 0.0,
        },
        Part {
            name: "R2 4k7",
            x: 3.0,
            side: 1,
            package: "0402",
            rot: 90.0,
        },
        Part {
            name: "C1 100n",
            x: 25.4,
            side: 0,
            package: "0603",
            rot: 180.0,
        },
        Part {
            name: "U1 MCU",
            x: 40.0,
            side: 1,
            package: "QFN-48",
            rot: 0.0,
        },
    ];

    fn fields() -> Fields {
        Fields::new(vec![
            Field::new("x", Kind::Length),
            Field::new("side", Kind::Choice(vec!["Top".into(), "Bottom".into()])),
            Field::package("package"),
            Field::new("rot", Kind::Angle),
        ])
    }

    fn shown(text: &str) -> Vec<&'static str> {
        let f = fields();
        let filter = Filter::parse(text, &f);
        PARTS
            .iter()
            .filter(|p| {
                filter.matches(&f, p.name, |i| match i {
                    0 => Cell::Length(p.x),
                    1 => Cell::Choice(p.side),
                    2 => Cell::Text(p.package.into()),
                    3 => Cell::Angle(p.rot),
                    _ => Cell::None,
                })
            })
            .map(|p| p.name)
            .collect()
    }

    #[test]
    fn plain_words_match_the_rows_text() {
        assert_eq!(shown("r"), vec!["R1 10k", "R2 4k7"]);
        assert_eq!(shown("mcu"), vec!["U1 MCU"]);
        assert_eq!(shown(""), vec!["R1 10k", "R2 4k7", "C1 100n", "U1 MCU"]);
    }

    #[test]
    fn column_terms_compare_in_the_columns_units() {
        assert_eq!(shown("x>=12"), vec!["R1 10k", "C1 100n", "U1 MCU"]);
        assert_eq!(shown("x>1in"), vec!["U1 MCU"], "25.4 mm is not > 1 in");
        assert_eq!(shown("x>=1000mil"), vec!["C1 100n", "U1 MCU"]);
        assert_eq!(shown("side:bottom"), vec!["R2 4k7", "U1 MCU"]);
        assert_eq!(shown("pkg:0603 side:top"), vec!["R1 10k", "C1 100n"]);
        assert_eq!(shown("rot:90"), vec!["R2 4k7"]);
        assert_eq!(shown("x:10..30"), vec!["R1 10k", "C1 100n"]);
    }

    #[test]
    fn each_term_is_a_chip_and_a_bad_one_is_an_error_chip() {
        let f = fields();
        let filter = Filter::parse("x>=12 side:left pkg:0603", &f);
        let labels: Vec<(&str, bool)> = filter
            .chips
            .iter()
            .map(|c| (c.label.as_str(), c.error.is_some()))
            .collect();
        assert_eq!(
            labels,
            vec![("x ≥ 12 mm", false), ("side:left", true), ("pkg: 0603", false)]
        );
        let error = filter.chips.get(1).and_then(|c| c.error.clone()).unwrap_or_default();
        assert!(error.contains("left"), "{error}");
        assert!(error.contains("try"), "an example: {error}");
        // The other terms still apply.
        assert_eq!(shown("x>=12 side:left pkg:0603"), vec!["R1 10k", "C1 100n"]);
        // Removing a chip removes its term.
        let bad = filter.chips.get(1).map(|c| c.span.clone()).unwrap_or_default();
        assert_eq!(filter.without(&bad), "x>=12 pkg:0603");
    }

    #[test]
    fn library_filters_and_unknown_fields_are_error_chips() {
        let f = fields();
        for text in ["mpn:abc", "pins:8", "colour:red"] {
            let filter = Filter::parse(text, &f);
            assert!(
                filter.chips.iter().all(|c| c.error.is_some()),
                "{text}: {:?}",
                filter.chips
            );
            assert!(filter.is_empty(), "{text} filters nothing out");
        }
        let open = Filter::parse("\"top", &f);
        assert_eq!(open.chips.len(), 1);
        assert!(open.chips.iter().all(|c| c.error.is_some()));
    }
}
