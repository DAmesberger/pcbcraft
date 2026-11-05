//! The tool loop of a run, on any backend: a goal, a tool set, a budget,
//! events and a recording. Every tool call goes through [`dispatch_tool_call`],
//! the one place a permission gate belongs.
use crate::cost::{Ledger, Price, PriceTable};
use crate::documents::{self, PageStore, Prepared, VIEW_PAGE_TOOL};
use crate::model::{
    Budget, Image, Message, ModelBackend, ModelProfile, ModelRequest, Part, Role, StopReason,
    appended_chars,
};
use crate::report::{self, RequestFacts, ReviewReport, RunFacts};
use crate::tools::{self, ToolOutcome};
use crate::{Error, Event, Goal, Outcome, Result, SYSTEM, Stop};
use pcbcraft_automation::{Mode, Policy, Session};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Model text answers before a nudge to submit stops being sent.
const MAX_NUDGES: u32 = 2;
/// Replies in a row cut off at the output limit before the run gives up: the
/// model is making no progress, and that is reported as what it is.
const MAX_CUTS_IN_A_ROW: u32 = 3;
/// Least room for output worth sending a request: a tool call took 806 tokens
/// on a local model, reasoning included, and even that was a simple one.
const MIN_OUTPUT_ROOM: u64 = 4_000;
/// Characters per token of what the conversation grows by, for the estimate.
const CHARS_PER_TOKEN: u64 = 3;
/// Added to the estimate of the next prompt.
const ESTIMATE_MARGIN: u64 = 4_000;

/// What a run may use, on which model.
#[derive(Clone, Debug)]
pub struct RunConfig {
    pub profile: ModelProfile,
    pub budget: Budget,
    /// Prices of models that may serve instead of the configured one.
    pub prices: PriceTable,
    /// What the run's tool calls may do: every call goes through the gate
    /// with it. The hosts give it the output folder as its only folder.
    pub policy: Policy,
}

impl Default for RunConfig {
    fn default() -> Self {
        let prices = PriceTable::default();
        Self {
            profile: ModelProfile::anthropic(crate::DEFAULT_MODEL, &prices),
            budget: Budget::default(),
            prices,
            policy: Policy::default(),
        }
    }
}

/// The default policy of a run: edits run inside its turn, files and
/// publishing are refused (there is no one to ask yet), and the folder the
/// run writes its project to is the only folder a request may name.
pub fn output_policy(out: &std::path::Path) -> Result<Policy> {
    let folder = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let folder = std::path::absolute(&folder)?;
    Ok(Policy::new(Mode::AutoAllowEdits, [folder]))
}

/// What tools act on during a run.
struct ToolContext<'a> {
    session: &'a mut Session,
    policy: &'a Policy,
    workspace: tools::Workspace,
    pages: PageStore,
    /// Reported in the review: which document's pages the model looked at.
    facts: Vec<documents::DocumentFacts>,
}

/// One tool call, whatever the model asked: the one door between a model and
/// the session. A call whose arguments could not be read never runs; its
/// error goes back so the model can send it again.
fn dispatch_tool_call(
    context: &mut ToolContext<'_>,
    name: &str,
    input: &Value,
    malformed: Option<&str>,
) -> ToolOutcome {
    if let Some(why) = malformed {
        return ToolOutcome::Reply {
            text: format!("This call was not run: {why}. Send it again with valid arguments."),
            error: true,
            step: None,
            images: Vec::new(),
        };
    }
    if name == VIEW_PAGE_TOOL && context.pages.available() {
        return match context.pages.view(input) {
            Ok(view) => {
                if let Some(facts) = context.facts.get_mut(view.facts) {
                    facts.pages_viewed.push(view.page);
                }
                ToolOutcome::Reply {
                    text: view.text,
                    error: false,
                    step: None,
                    images: vec![view.image],
                }
            }
            Err(why) => ToolOutcome::Reply {
                text: why,
                error: true,
                step: None,
                images: Vec::new(),
            },
        };
    }
    tools::execute(
        context.session,
        context.policy,
        &mut context.workspace,
        name,
        input,
    )
}

/// The first request's message: datasheets, then the goal. `note` tells a
/// model what it got instead of the PDFs.
fn first_message(goal: &Goal, prepared: &Prepared) -> Message {
    let mut content = prepared.parts.clone();
    let mut text = format!(
        "Create the part \"{}\" from these datasheets. {}",
        goal.name, goal.notes
    );
    if let Some(note) = &prepared.note {
        text = format!("{note}\n\n{text}");
    }
    content.push(Part::text(text));
    Message {
        role: Role::User,
        content,
    }
}

/// The checks that can fail before anything is sent.
fn preflight(config: &RunConfig, prepared: &Prepared, prompt: u64) -> Result<()> {
    let profile = &config.profile;
    let caps = &profile.capabilities;
    if !(config.budget.max_usd.is_finite() && config.budget.max_usd > 0.0) {
        return Err(Error::Config(
            "the cost cap must be a positive amount".into(),
        ));
    }
    if !caps.tool_calling {
        return Err(Error::Unsupported(format!(
            "Create from datasheet works through tools, and {} is declared without tool calling",
            profile.model
        )));
    }
    if prompt.saturating_add(MIN_OUTPUT_ROOM) > caps.context_tokens {
        return Err(Error::Unsupported(format!(
            "the datasheets and tools come to about {prompt} tokens ({} of them the datasheets), which does not fit the {} tokens declared for {} with room to answer. Give fewer or shorter datasheets, or a larger context",
            prepared.tokens, caps.context_tokens, profile.model
        )));
    }
    Ok(())
}

/// Run the tool loop until the model submits its report, a cap is reached,
/// the run is cancelled or the backend fails. Drafts land in `session`;
/// nothing is saved or published here.
pub fn run(
    goal: &Goal,
    config: &RunConfig,
    backend: &mut dyn ModelBackend,
    session: &mut Session,
    events: &mut dyn FnMut(Event),
    cancel: &AtomicBool,
) -> Result<Outcome> {
    let profile = &config.profile;
    profile.check()?;
    let budget = &config.budget;
    let price = profile.capabilities.price;
    let mut prepared = documents::prepare(&goal.datasheets, &profile.capabilities)?;
    let mut specs = tools::definitions();
    if prepared.pages.available() {
        specs.push(prepared.pages.tool());
    }
    let tool_chars = serde_json::to_string(&specs)?.len() as u64;
    // Without a count: the documents, the tools and the system prompt.
    let first_estimate = prepared
        .tokens
        .saturating_add((tool_chars + SYSTEM.len() as u64) / CHARS_PER_TOKEN)
        .saturating_add(ESTIMATE_MARGIN);
    preflight(config, &prepared, first_estimate)?;
    let mut messages = vec![first_message(goal, &prepared)];
    let mut context = ToolContext {
        session,
        policy: &config.policy,
        workspace: tools::Workspace::default(),
        pages: std::mem::take(&mut prepared.pages),
        facts: std::mem::take(&mut prepared.facts),
    };
    let mut ledger = Ledger::default();
    let mut facts = RunFacts {
        model: profile.model.clone(),
        provider: profile.provider.label().to_owned(),
        capabilities: profile.declared(),
        transport: backend.label(),
        cap_usd: price.map(|_| budget.max_usd),
        ..RunFacts::default()
    };
    let started = Instant::now();
    let mut report: Option<ReviewReport> = None;
    let mut limit = profile.output.per_request;
    let mut nudges = 0;
    let mut cuts = 0;
    let mut last_prompt = 0u64;
    let mut appended = 0u64;
    let stop = loop {
        if cancel.load(Ordering::Relaxed) {
            break Stop::Cancelled;
        }
        if ledger.requests >= budget.max_requests {
            break Stop::Budget(format!("{} requests", budget.max_requests));
        }
        let left = budget.max_duration.saturating_sub(started.elapsed());
        if left.is_zero() {
            break Stop::Budget(format!(
                "{} seconds of run time",
                budget.max_duration.as_secs()
            ));
        }
        let output_left = budget
            .max_output_tokens
            .saturating_sub(ledger.usage.output_tokens);
        if output_left < u64::from(limit) {
            break Stop::Budget(format!("{} output tokens", budget.max_output_tokens));
        }
        let mut request = ModelRequest {
            system: SYSTEM,
            tools: &specs,
            messages: &messages,
            max_output_tokens: limit,
            timeout: profile.request_timeout.min(left),
        };
        let prompt_tokens = if ledger.requests == 0 {
            // Count exactly when the backend can; otherwise the estimate.
            match backend.count_prompt_tokens(&request) {
                Ok(Some(n)) => n,
                _ => first_estimate,
            }
        } else {
            last_prompt + appended / CHARS_PER_TOKEN + ESTIMATE_MARGIN
        };
        // The context holds prompt and output: never ask for more than fits.
        let room = profile
            .capabilities
            .context_tokens
            .saturating_sub(prompt_tokens);
        if room < MIN_OUTPUT_ROOM {
            break Stop::Budget(format!(
                "the conversation, about {prompt_tokens} tokens, leaves no room to answer in the {} tokens declared for {}",
                profile.capabilities.context_tokens, profile.model
            ));
        }
        request.max_output_tokens = u32::try_from(room.min(u64::from(limit))).unwrap_or(limit);
        let requested = u64::from(request.max_output_tokens);
        let used = ledger
            .usage
            .prompt()
            .saturating_add(ledger.usage.output_tokens);
        if used.saturating_add(prompt_tokens).saturating_add(requested) > budget.max_total_tokens {
            break Stop::Budget(format!(
                "{} tokens: {used} used, and the next request could take {}",
                budget.max_total_tokens,
                prompt_tokens + requested
            ));
        }
        if let Some(price) = &price
            && !ledger.affords(price, prompt_tokens, requested, budget.max_usd)
        {
            break Stop::Budget(format!(
                "${:.2} spent; the next request could cost more than the ${:.2} left",
                ledger.spent_usd,
                budget.max_usd - ledger.spent_usd
            ));
        }
        let n = ledger.requests + 1;
        events(Event::Request { n, prompt_tokens });
        let sent = Instant::now();
        let reply = match backend.send(&request) {
            Ok(r) => r,
            Err(e) => {
                let over = budget
                    .max_duration
                    .saturating_sub(started.elapsed())
                    .is_zero();
                break if over {
                    Stop::Budget(format!(
                        "{} seconds of run time, during request {n}",
                        budget.max_duration.as_secs()
                    ))
                } else {
                    Stop::Failed(e.to_string())
                };
            }
        };
        let seconds = sent.elapsed().as_secs_f64();
        let rate = served_rate(price, &reply.model, profile, &config.prices);
        let cost = ledger.book(&reply.usage, &rate);
        facts.usage_estimated |= reply.usage_estimated;
        if let Some(warning) = check_prompt(&reply, n, last_prompt, prepared.tokens) {
            events(Event::Warning(warning.clone()));
            facts.warnings.push(warning);
        }
        last_prompt = reply.usage.prompt();
        let stop_label = reply.stop.label();
        let was_cut = reply.stop == StopReason::CutOff;
        // One retry with room to finish after a cut-off, then back to the
        // normal limit.
        limit = if was_cut {
            profile.output.after_cut.max(profile.output.per_request)
        } else {
            profile.output.per_request
        };
        facts.request_log.push(RequestFacts {
            n,
            stop_reason: stop_label.clone(),
            input_tokens: reply.usage.input_tokens,
            cache_write_tokens: reply.usage.cache_write_tokens,
            cache_read_tokens: reply.usage.cache_read_tokens,
            output_tokens: reply.usage.output_tokens,
            cost_usd: cost,
            seconds,
            tools: Vec::new(),
        });
        events(Event::Response {
            n,
            stop_reason: stop_label,
            spent_usd: ledger.spent_usd,
            output_tokens: reply.usage.output_tokens,
            seconds,
        });
        for part in &reply.content {
            if let Part::Text { text, .. } = part {
                events(Event::Text(text.clone()));
            }
        }
        if let StopReason::Refusal { category } = &reply.stop {
            break Stop::Refused(category.clone());
        }
        // The history is append-only: the reply goes back as it came.
        appended = appended_chars(&reply.content);
        let content = reply.content;
        let calls: Vec<(String, String, Value, Option<String>)> = content
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall {
                    id,
                    name,
                    input,
                    malformed,
                    ..
                } => Some((id.clone(), name.clone(), input.clone(), malformed.clone())),
                _ => None,
            })
            .collect();
        messages.push(Message {
            role: Role::Assistant,
            content,
        });
        cuts = if was_cut { cuts + 1 } else { 0 };
        if cuts >= MAX_CUTS_IN_A_ROW {
            break Stop::CutOff(format!(
                "{cuts} replies in a row stopped at the output limit (last {} tokens) before finishing; a cut-off reply is never used as an answer. Raise the model's output limit",
                requested
            ));
        }
        if calls.is_empty() {
            if was_cut || nudges < MAX_NUDGES {
                nudges += 1;
                let text = if was_cut {
                    "Your reply was cut off at the output limit. Continue with smaller steps (pin rows in chunks of at most 40 with library_pinout_edit)."
                } else {
                    "Continue with the tools; finish with submit_review."
                };
                appended += text.len() as u64;
                messages.push(Message {
                    role: Role::User,
                    content: vec![Part::text(text)],
                });
                continue;
            }
            break Stop::Ended;
        }
        let mut results = Vec::new();
        for (id, name, input, malformed) in calls {
            let logged = facts.request_log.last_mut();
            if was_cut {
                // The call's input may be incomplete: never run it.
                if let Some(r) = logged {
                    r.tools.push(format!("{name}!"));
                }
                events(Event::Tool {
                    name: name.clone(),
                    error: true,
                    step: None,
                });
                results.push(result(
                    id,
                    "Cut off at the output limit before this call was complete; send it again in smaller pieces (pin rows in chunks of at most 40 with library_pinout_edit; preview and apply take no document).",
                    true,
                    Vec::new(),
                ));
                continue;
            }
            let outcome = dispatch_tool_call(&mut context, &name, &input, malformed.as_deref());
            if let Some(r) = logged {
                let failed = matches!(outcome, ToolOutcome::Reply { error: true, .. });
                r.tools.push(if failed {
                    format!("{name}!")
                } else {
                    name.clone()
                });
            }
            match outcome {
                ToolOutcome::Report(r) => {
                    events(Event::Tool {
                        name: name.clone(),
                        error: false,
                        step: None,
                    });
                    report = Some(*r);
                    results.push(result(id, "Report received.", false, Vec::new()));
                }
                ToolOutcome::Reply {
                    text,
                    error,
                    step,
                    images,
                } => {
                    if let Some(step) = &step {
                        facts.steps.push(step.clone());
                    }
                    events(Event::Tool {
                        name: name.clone(),
                        error,
                        step,
                    });
                    results.push(result(id, &text, error, images));
                }
            }
        }
        appended = appended.saturating_add(appended_chars(&results));
        messages.push(Message {
            role: Role::User,
            content: results,
        });
        if report.is_some() {
            break Stop::Submitted;
        }
    };
    facts.elapsed_seconds = started.elapsed().as_secs_f64();
    facts.documents = std::mem::take(&mut context.facts);
    let session = context.session;
    report::finish_facts(&mut facts, &ledger, &stop, session);
    Ok(Outcome {
        report,
        stop,
        facts,
    })
}

fn result(id: String, text: &str, error: bool, images: Vec<Image>) -> Part {
    Part::ToolResult {
        call_id: id,
        text: text.to_owned(),
        error,
        images,
    }
}

/// The rate to book a reply at. A fallback model may have served it: book the
/// dearer rate. A model with no price books nothing, and the review says so.
fn served_rate(
    price: Option<Price>,
    served: &str,
    profile: &ModelProfile,
    prices: &PriceTable,
) -> Price {
    let Some(price) = price else {
        return Price::FREE;
    };
    if served == profile.model {
        return price;
    }
    match prices.of(served) {
        Ok(p) if p.output > price.output => p,
        _ => price,
    }
}

/// A warning when the server's prompt count shows it may have dropped part
/// of the conversation: the history only grows, and some servers silently
/// truncate a prompt that exceeds the context they gave the model.
fn check_prompt(
    reply: &crate::model::ModelReply,
    n: u32,
    last_prompt: u64,
    document_tokens: u64,
) -> Option<String> {
    if reply.usage_estimated {
        return None;
    }
    let prompt = reply.usage.prompt();
    let shrank = n > 1 && prompt.saturating_add(prompt / 10) < last_prompt;
    let far_below = n == 1 && prompt.saturating_mul(2) < document_tokens;
    (shrank || far_below).then(|| {
        format!(
            "Request {n}: the server counted {prompt} prompt tokens, less than this conversation holds (last request {last_prompt}, datasheets about {document_tokens}). It may have dropped part of it to fit its context, and a result from a truncated prompt can't be trusted. Check the context size the server gives this model."
        )
    })
}
