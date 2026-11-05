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
    /// The wire format it was reached by.
    #[serde(default)]
    pub provider: String,
    /// What was declared for it.
    #[serde(default)]
    pub capabilities: String,
    /// Where the responses came from: an endpoint, or a replay file (whose
    /// spend is not real).
    #[serde(default)]
    pub transport: String,
    pub spent_usd: f64,
    /// `None` when the model has no price: spend is unknown, and the token,
    /// time and request caps are what bound the run.
    #[serde(default)]
    pub cap_usd: Option<f64>,
    /// Wall-clock time of the run.
    #[serde(default)]
    pub elapsed_seconds: f64,
    /// The server gave no counts for some requests: the tokens are estimates.
    #[serde(default)]
    pub usage_estimated: bool,
    /// How each datasheet reached the model.
    #[serde(default)]
    pub documents: Vec<crate::documents::DocumentFacts>,
    /// What a person should know about this run's result.
    #[serde(default)]
    pub warnings: Vec<String>,
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
    /// Each request: why it stopped, its tokens, cost and tool calls.
    #[serde(default)]
    pub request_log: Vec<RequestFacts>,
    /// The component draft's pins, one row per package pin.
    #[serde(default)]
    pub pins: Vec<crate::pins::PinRow>,
    /// The component draft's pin rules over their samples.
    #[serde(default)]
    pub rules: Vec<pcbcraft_core::functions::rules::RuleCheck>,
}

/// One request of a run.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RequestFacts {
    pub n: u32,
    pub stop_reason: String,
    pub input_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    /// How long the response took.
    #[serde(default)]
    pub seconds: f64,
    /// The tools it called, in order (`name!` when the call failed or was
    /// not run).
    pub tools: Vec<String>,
}

/// Fill in what the runner measured once a run has stopped.
pub(crate) fn finish_facts(
    facts: &mut RunFacts,
    ledger: &crate::cost::Ledger,
    stop: &crate::Stop,
    session: &pcbcraft_automation::Session,
) {
    facts.spent_usd = ledger.spent_usd;
    facts.requests = ledger.requests;
    facts.input_tokens = ledger.usage.input_tokens;
    facts.output_tokens = ledger.usage.output_tokens;
    facts.cache_write_tokens = ledger.usage.cache_write_tokens;
    facts.cache_read_tokens = ledger.usage.cache_read_tokens;
    facts.stop = stop.to_string();
    let snapshot = session.snapshot();
    facts.pins = crate::pins::rows(&snapshot);
    facts.rules = session
        .engine()
        .drafts()
        .items
        .iter()
        .find_map(|d| match &d.definition {
            pcbcraft_core::library::Definition::Component(c) => {
                Some(pcbcraft_core::functions::rules::check_samples(
                    &c.declarations(),
                    &c.resources,
                    &c.pin_rules,
                ))
            }
            _ => None,
        })
        .unwrap_or_default();
    for d in crate::tools::drafts(&snapshot) {
        let id = d["id"].as_u64().unwrap_or(0);
        facts.drafts.push((
            id,
            d["kind"].as_str().unwrap_or("").to_owned(),
            d["name"].as_str().unwrap_or("").to_owned(),
        ));
        let codes = d["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| {
                let code = f["code"].as_str()?;
                Some(if f["blocks"].is_null() {
                    code.to_owned()
                } else {
                    format!("{code}!")
                })
            })
            .collect();
        facts.findings.push((id, codes));
    }
}

fn cell(t: &str) -> String {
    t.replace('|', "\\|").replace('\n', " ")
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
    let spend = match facts.cap_usd {
        Some(cap) => format!("spend ${:.2} of the ${cap:.2} cap", facts.spent_usd),
        None => "no price declared, so spend is not tracked".to_owned(),
    };
    let _ = writeln!(
        s,
        "Run: model `{}`{via}, {} requests in {:.0} s, {spend}; stopped: {}.\n",
        facts.model, facts.requests, facts.elapsed_seconds, facts.stop
    );
    let estimated = if facts.usage_estimated {
        " Estimated from the characters where the server reported no counts."
    } else {
        ""
    };
    let _ = writeln!(
        s,
        "Tokens: {} input, {} cache write, {} cache read, {} output.{estimated}\n",
        facts.input_tokens, facts.cache_write_tokens, facts.cache_read_tokens, facts.output_tokens
    );
    if !facts.capabilities.is_empty() {
        let _ = writeln!(s, "Declared for this model: {}.\n", facts.capabilities);
    }
    for warning in &facts.warnings {
        let _ = writeln!(s, "**Warning:** {warning}\n");
    }
    if !facts.documents.is_empty() {
        let _ = writeln!(s, "## How the model read the datasheets\n");
        for d in &facts.documents {
            let _ = write!(s, "- **{}**: {}", d.title, d.delivery.describe());
            if d.text_chars > 0 {
                let _ = write!(s, "; {} pages, {} characters", d.pages, d.text_chars);
            }
            if !d.sparse_pages.is_empty() {
                let _ = write!(s, "; almost no text on pages {:?}", d.sparse_pages);
            }
            if !d.pages_viewed.is_empty() {
                let _ = write!(s, "; it viewed pages {:?}", d.pages_viewed);
            }
            let _ = writeln!(s, ".");
        }
        let _ = writeln!(s);
    }
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
    if !facts.rules.is_empty() {
        let _ = writeln!(
            s,
            "\n## Pin rules\n\nEach rule evaluated without selections, with each function, peripheral or role it names made active, and with all of them together.\n"
        );
        let _ = writeln!(
            s,
            "| Rule | Without selections | Samples | Verdicts | Failures |"
        );
        let _ = writeln!(s, "|---|---|---|---|---|");
        for r in &facts.rules {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} |",
                cell(&r.label),
                cell(&r.default),
                r.samples,
                r.verdicts.join(", "),
                cell(&r.failures.join("; "))
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
    if !facts.pins.is_empty() {
        let _ = writeln!(
            s,
            "\n## Pins\n\nOne row per package pin as imported; check each against the datasheet in the Pins tab. `pcbcraft-cli review-diff A.pcbcraft B.pcbcraft` lists two runs' differences pin by pin.\n"
        );
        s.push_str(&crate::pins::markdown(&facts.pins));
    }
    if !facts.request_log.is_empty() {
        let _ = writeln!(s, "\n## Requests\n");
        let _ = writeln!(
            s,
            "| # | Stop | Cache write | Cache read | Input | Output | USD | Seconds | Tools |\n|---|---|---|---|---|---|---|---|---|"
        );
        for r in &facts.request_log {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} | {} | {:.3} | {:.0} | {} |",
                r.n,
                r.stop_reason,
                r.cache_write_tokens,
                r.cache_read_tokens,
                r.input_tokens,
                r.output_tokens,
                r.cost_usd,
                r.seconds,
                r.tools.join(", ")
            );
        }
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
            cap_usd: Some(15.0),
            ..RunFacts::default()
        };
        let md = markdown(Some(&report()), &facts);
        assert!(md.contains("| pitch | 1.27 mm | DS p.12 Figure 3 | High |"));
        assert!(md.contains("- [ ] Is the land pattern the pad?"));
        assert!(md.contains("$1.23 of the $15.00 cap"));
        assert!(md.contains("not published"));
    }
}
