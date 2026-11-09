//! The review report of an assisted run: the report itself (its types and
//! checks are `pcbcraft_automation::review`, shared with the MCP workflow's
//! `library.review_check`) and what the runner adds to it (requests,
//! tokens, drafts, findings), written as JSON and Markdown beside the
//! project.
pub use pcbcraft_automation::review::{
    Citation, Confidence, ExtractedValue, MAX_TEXT, MAX_VALUES, ReviewReport, SourceDocument,
};
use pcbcraft_automation::review::{cell, report_markdown};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

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
    /// Where the responses came from: an endpoint, or a replay file.
    #[serde(default)]
    pub transport: String,
    /// The run profile the model ran with ([`crate::profile`]).
    #[serde(default)]
    pub run_profile: String,
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
    /// Each request: why it stopped, its tokens and tool calls.
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
    tally: &crate::usage::Tally,
    stop: &crate::Stop,
    session: &pcbcraft_automation::Session,
) {
    facts.requests = tally.requests;
    facts.input_tokens = tally.usage.input_tokens;
    facts.output_tokens = tally.usage.output_tokens;
    facts.cache_write_tokens = tally.usage.cache_write_tokens;
    facts.cache_read_tokens = tally.usage.cache_read_tokens;
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
        "Run: model `{}`{via}, {} requests in {:.0} s; stopped: {}.\n",
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
    if !facts.run_profile.is_empty() {
        let _ = writeln!(s, "Run profile: `{}`.\n", facts.run_profile);
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
        s.push_str(&report_markdown(r));
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
            "| # | Stop | Cache write | Cache read | Input | Output | Seconds | Tools |\n|---|---|---|---|---|---|---|---|"
        );
        for r in &facts.request_log {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} | {} | {:.0} | {} |",
                r.n,
                r.stop_reason,
                r.cache_write_tokens,
                r.cache_read_tokens,
                r.input_tokens,
                r.output_tokens,
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
    fn markdown_lists_values_and_questions_and_no_prices() {
        let facts = RunFacts {
            model: "claude-opus-5-5".into(),
            requests: 3,
            ..RunFacts::default()
        };
        let md = markdown(Some(&report()), &facts);
        assert!(md.contains("| pitch | 1.27 mm | DS p.12 Figure 3 | High |"));
        assert!(md.contains("- [ ] Is the land pattern the pad?"));
        assert!(md.contains("3 requests"));
        assert!(!md.contains('$') && !md.contains("USD"), "{md}");
        assert!(md.contains("not published"));
    }
}
