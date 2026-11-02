//! The review report an assisted run ends with: every value taken from a
//! datasheet with its page and table or figure, a confidence, and the open
//! questions a person must settle before publishing. The model submits it
//! through the `submit_review` tool; the runner adds what it measured
//! (spend, drafts, findings) and writes JSON and Markdown beside the
//! project.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// Bytes of one free-text field.
pub const MAX_TEXT: usize = 4096;
/// Values one report lists.
pub const MAX_VALUES: usize = 1024;

#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Read directly from a dimensioned drawing or a table cell.
    High,
    /// Derived arithmetically from stated values, or read from a drawing
    /// whose dimension line is ambiguous.
    Medium,
    /// Inferred, measured from a drawing's scale, or not stated.
    Low,
}

/// Where in which document a value comes from.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    /// The document's short name, as listed in `sources`.
    pub document: String,
    /// 1-based page number of the PDF.
    pub page: u32,
    /// Table, figure or section, e.g. "Table 3-1" or "Figure 10-1".
    #[serde(default)]
    pub location: String,
}

/// One source document.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceDocument {
    /// Short name used by citations.
    pub name: String,
    pub title: String,
    /// The document's own revision and date.
    #[serde(default)]
    pub revision: String,
}

/// One value taken from the datasheets.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExtractedValue {
    /// What it is, e.g. "pad pitch", "pin 45 strapping default".
    pub item: String,
    /// The value with its unit as used, e.g. "0.85 mm".
    pub value: String,
    pub source: Citation,
    pub confidence: Confidence,
    #[serde(default)]
    pub note: String,
}

/// The model's report (`submit_review`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReviewReport {
    /// The part, e.g. its manufacturer part name.
    pub part: String,
    /// What was built and what a reviewer should check first.
    pub summary: String,
    pub sources: Vec<SourceDocument>,
    pub values: Vec<ExtractedValue>,
    /// Questions the datasheets leave open, each to settle before publish.
    #[serde(default)]
    pub open_questions: Vec<String>,
}

impl ReviewReport {
    /// Bounds a submitted report must meet.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let long = |s: &str| s.len() > MAX_TEXT;
        if self.part.trim().is_empty() || long(&self.part) || long(&self.summary) {
            out.push("part and summary must be non-empty text of at most 4 KiB".into());
        }
        if self.values.len() > MAX_VALUES {
            out.push(format!("at most {MAX_VALUES} values"));
        }
        if self.sources.is_empty() {
            out.push("list the source documents".into());
        }
        for (k, v) in self.values.iter().enumerate() {
            if !self.sources.iter().any(|s| s.name == v.source.document) {
                out.push(format!(
                    "value {} ({}) cites document {:?}, which sources does not list",
                    k + 1,
                    v.item,
                    v.source.document
                ));
            }
            if v.source.page == 0 {
                out.push(format!("value {} ({}) needs a page", k + 1, v.item));
            }
        }
        out.truncate(20);
        out
    }
}

/// What the runner adds to the model's report.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RunFacts {
    pub model: String,
    /// Where the responses came from: the Claude API, or a replay file
    /// (whose spend is not real).
    #[serde(default)]
    pub transport: String,
    pub spent_usd: f64,
    pub cap_usd: f64,
    pub requests: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    /// Why the run stopped.
    pub stop: String,
    /// Draft IDs in the saved project, with kind and name.
    pub drafts: Vec<(u64, String, String)>,
    /// Finding codes per draft (blocking ones marked `!`).
    pub findings: Vec<(u64, Vec<String>)>,
    /// Operations applied, in order (tool and outcome).
    pub steps: Vec<String>,
}

/// The report as Markdown for people.
pub fn markdown(report: Option<&ReviewReport>, facts: &RunFacts) -> String {
    let mut s = String::new();
    let part = report.map_or("(no report submitted)", |r| r.part.as_str());
    let _ = writeln!(s, "# Datasheet import review: {part}\n");
    let _ = writeln!(
        s,
        "Status: **draft, not published**. Nothing enters a library until a person reviews the rows below and publishes.\n"
    );
    let via = if facts.transport.is_empty() {
        String::new()
    } else {
        format!(" via {}", facts.transport)
    };
    let _ = writeln!(
        s,
        "Run: model `{}`{via}, {} requests, spend ${:.2} of the ${:.2} cap; stopped: {}.\n",
        facts.model, facts.requests, facts.spent_usd, facts.cap_usd, facts.stop
    );
    let _ = writeln!(
        s,
        "Tokens: {} input, {} cache write, {} cache read, {} output.\n",
        facts.input_tokens, facts.cache_write_tokens, facts.cache_read_tokens, facts.output_tokens
    );
    if let Some(r) = report {
        let _ = writeln!(s, "## Summary\n\n{}\n", r.summary);
        let _ = writeln!(s, "## Sources\n");
        for d in &r.sources {
            let _ = writeln!(s, "- **{}**: {} {}", d.name, d.title, d.revision);
        }
        let _ = writeln!(s, "\n## Open questions\n");
        if r.open_questions.is_empty() {
            let _ = writeln!(s, "None listed.");
        }
        for q in &r.open_questions {
            let _ = writeln!(s, "- [ ] {q}");
        }
        let _ = writeln!(s, "\n## Extracted values\n");
        let _ = writeln!(s, "| Item | Value | Source | Confidence | Note |");
        let _ = writeln!(s, "|---|---|---|---|---|");
        let mut values: Vec<&ExtractedValue> = r.values.iter().collect();
        // Least confident first: they need the reviewer most.
        values.sort_by_key(|v| std::cmp::Reverse(v.confidence));
        for v in values {
            let cell = |t: &str| t.replace('|', "\\|").replace('\n', " ");
            let _ = writeln!(
                s,
                "| {} | {} | {} p.{} {} | {:?} | {} |",
                cell(&v.item),
                cell(&v.value),
                cell(&v.source.document),
                v.source.page,
                cell(&v.source.location),
                v.confidence,
                cell(&v.note)
            );
        }
    }
    let _ = writeln!(s, "\n## Drafts\n");
    for (id, kind, name) in &facts.drafts {
        let codes = facts
            .findings
            .iter()
            .find(|(d, _)| d == id)
            .map(|(_, c)| c.join(", "))
            .unwrap_or_default();
        let _ = writeln!(s, "- {kind} {id} ‘{name}’: {codes}");
    }
    let _ = writeln!(s, "\n## Steps\n");
    for step in &facts.steps {
        let _ = writeln!(s, "1. {step}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> ReviewReport {
        ReviewReport {
            part: "EX-MOD-1".into(),
            summary: "A module.".into(),
            sources: vec![SourceDocument {
                name: "DS".into(),
                title: "EX-MOD-1 datasheet".into(),
                revision: "v1".into(),
            }],
            values: vec![ExtractedValue {
                item: "pitch".into(),
                value: "1.27 mm".into(),
                source: Citation {
                    document: "DS".into(),
                    page: 12,
                    location: "Figure 3".into(),
                },
                confidence: Confidence::High,
                note: String::new(),
            }],
            open_questions: vec!["Is the land pattern the pad?".into()],
        }
    }

    #[test]
    fn a_report_cites_listed_documents_with_pages() {
        let mut r = report();
        assert!(r.problems().is_empty());
        r.values[0].source.document = "Other".into();
        r.values[0].source.page = 0;
        assert_eq!(r.problems().len(), 2);
    }

    #[test]
    fn markdown_lists_values_questions_and_spend() {
        let facts = RunFacts {
            model: "claude-opus-5-5".into(),
            spent_usd: 1.234,
            cap_usd: 15.0,
            ..RunFacts::default()
        };
        let md = markdown(Some(&report()), &facts);
        assert!(md.contains("| pitch | 1.27 mm | DS p.12 Figure 3 | High |"));
        assert!(md.contains("- [ ] Is the land pattern the pad?"));
        assert!(md.contains("$1.23 of the $15.00 cap"));
        assert!(md.contains("not published"));
    }
}
