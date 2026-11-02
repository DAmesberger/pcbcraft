//! `design-part`: author a component from datasheets with the Claude API
//! (`pcbcraft-assist`). The model works through the Pcbcraft tools in an
//! in-process session; the result is saved as a new project of drafts
//! (component, symbol, package, footprint, binding, generated 3D model),
//! with the review report beside it as `.review.json` and `.review.md`.
//! Nothing is published. The API key comes from `ANTHROPIC_API_KEY` and is
//! never printed or written.
use super::{Result, user_session};
use pcbcraft_assist::api::{HttpTransport, RecordingTransport, ReplayTransport, Transport};
use pcbcraft_assist::cost::PriceTable;
use pcbcraft_assist::{Datasheet, Event, Goal, Settings};
use pcbcraft_core::Engine;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

pub const USAGE: &str = "design-part --datasheet PDF|URL [--datasheet …] --name NAME --out NEW.pcbcraft
              [--model ID] [--effort LEVEL] [--max-usd N] [--max-requests N] [--prices FILE.json]
              [--notes TEXT] [--record FILE.jsonl | --replay FILE.jsonl]
                                   draft a part from datasheets with Claude (ANTHROPIC_API_KEY);
                                   writes the project and its .review.md/.review.json, publishes nothing";

struct Args {
    datasheets: Vec<Datasheet>,
    name: String,
    out: PathBuf,
    notes: String,
    record: Option<PathBuf>,
    replay: Option<PathBuf>,
    settings: Settings,
}

fn parse(args: &[String]) -> Result<Args> {
    let mut parsed = Args {
        datasheets: Vec::new(),
        name: String::new(),
        out: PathBuf::new(),
        notes: String::new(),
        record: None,
        replay: None,
        settings: Settings::default(),
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it
            .next()
            .ok_or_else(|| format!("{flag} needs a value\nusage: pcbcraft-cli {USAGE}"))?
            .clone();
        let number = |what: &str| -> Result<f64> {
            value
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v > 0.0)
                .ok_or_else(|| format!("{what} takes a positive number").into())
        };
        match flag.as_str() {
            "--datasheet" => parsed.datasheets.push(Datasheet::parse(&value)),
            "--name" => parsed.name = value,
            "--out" => parsed.out = PathBuf::from(value),
            "--notes" => parsed.notes = value,
            "--model" => parsed.settings.model = value,
            "--effort" => {
                if !["low", "medium", "high", "xhigh", "max"].contains(&value.as_str()) {
                    return Err("--effort is low, medium, high, xhigh or max".into());
                }
                parsed.settings.effort = value;
            }
            "--max-usd" => parsed.settings.max_usd = number("--max-usd")?,
            "--max-requests" => {
                parsed.settings.max_requests = value
                    .parse()
                    .map_err(|_| "--max-requests takes a count")?;
            }
            "--prices" => {
                let text = std::fs::read_to_string(&value)?;
                parsed.settings.prices = PriceTable::parse(&text)?;
            }
            "--record" => parsed.record = Some(PathBuf::from(value)),
            "--replay" => parsed.replay = Some(PathBuf::from(value)),
            other => return Err(format!("unknown option {other}\nusage: pcbcraft-cli {USAGE}").into()),
        }
    }
    if parsed.datasheets.is_empty() || parsed.name.trim().is_empty() || parsed.out.as_os_str().is_empty() {
        return Err(format!("usage: pcbcraft-cli {USAGE}").into());
    }
    if parsed.record.is_some() && parsed.replay.is_some() {
        return Err("--record and --replay exclude each other".into());
    }
    Ok(parsed)
}

pub fn run(args: &[String]) -> Result<()> {
    let args = parse(args)?;
    for path in std::iter::once(args.out.clone()).chain(pcbcraft_assist::review_paths(&args.out)) {
        if path.try_exists()? {
            return Err(format!("{} exists; choose a new output name", path.display()).into());
        }
    }
    let mut transport: Box<dyn Transport> = match (&args.replay, &args.record) {
        (Some(path), _) => Box::new(ReplayTransport::from_file(path)?),
        (None, Some(path)) => Box::new(RecordingTransport::new(HttpTransport::from_env()?, path.clone())),
        (None, None) => Box::new(HttpTransport::from_env()?),
    };
    let goal = Goal {
        name: args.name.clone(),
        datasheets: args.datasheets.clone(),
        notes: args.notes.clone(),
    };
    let mut session = user_session(Engine::default());
    let cancel = AtomicBool::new(false);
    let outcome = pcbcraft_assist::run(
        &goal,
        &args.settings,
        transport.as_mut(),
        &mut session,
        &mut |event| match event {
            Event::Request { n, prompt_tokens } => {
                eprintln!("request {n}: ~{prompt_tokens} prompt tokens");
            }
            Event::Response { n, stop_reason, spent_usd } => {
                eprintln!("response {n}: {stop_reason}; spent ${spent_usd:.3}");
            }
            Event::Tool { name, error, step } => {
                let status = if error { "refused" } else { "ok" };
                eprintln!("  {name}: {status}{}", step.map(|s| format!(" — {s}")).unwrap_or_default());
            }
            Event::Text(text) => {
                let line: String = text.chars().take(200).collect();
                eprintln!("  model: {line}");
            }
        },
        &cancel,
    )?;
    let [_, review] = pcbcraft_assist::write_outputs(&mut session, &args.out, &outcome)?;
    let facts = &outcome.facts;
    eprintln!(
        "{}: ${:.3} over {} requests; drafts saved to {} (review {})",
        outcome.stop,
        facts.spent_usd,
        facts.requests,
        args.out.display(),
        review.display()
    );
    if outcome.report.is_none() {
        return Err(format!("no review report: {}", outcome.stop).into());
    }
    Ok(())
}
