//! `design-part`: author a component from datasheets with a configured model
//! (`sparksmith-assist`): Anthropic Messages or any OpenAI-compatible endpoint,
//! hosted or on this computer. The model works through the Sparksmith tools in
//! an in-process session; the result is saved as a new project of drafts
//! (component, symbol, package, footprint, binding, generated 3D model),
//! with the review report beside it as `.review.json` and `.review.md`.
//! Nothing is published.
//!
//! The model's capabilities are what the flags declare; nothing is detected.
//! A key comes from a named environment variable or the system keychain, is
//! never printed or written, and is refused over plain HTTP to another
//! computer. `llm-key` stores one in the keychain for one endpoint.
use super::Result;
use sparksmith_assist::model::profile::ANTHROPIC_BASE_URL;
use sparksmith_assist::model::record::{Recording, Replay};
use sparksmith_assist::model::{
    self, Budget, Connection, Endpoint, KeySource, ModelBackend, ModelProfile, OutputLimits,
    Provider, Secret, keys,
};
use sparksmith_assist::profile as run_profile;
use sparksmith_assist::{AssistLibrary, Datasheet, Event, Goal, RunConfig};
use sparksmith_core::product::SHORT_NAME as CLI;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub const USAGE: &str = "design-part --datasheet PDF|URL [--datasheet …] --name NAME --out NEW.sparksmith
              [--provider anthropic|openai] [--base-url URL] [--model ID] [--key none|keychain|env:NAME]
              [--context-tokens N] [--max-output-tokens N] [--vision yes|no] [--native-pdf yes|no]
              [--tools yes|no] [--effort LEVEL] [--request-timeout SECONDS]
              [--max-requests N] [--max-tokens N] [--max-seconds N]
              [--profile NAME] [--notes TEXT] [--record FILE.jsonl | --replay FILE.jsonl]
                                   draft a part from datasheets with a configured model;
                                   writes the project and its .review.md/.review.json, publishes nothing
  llm-key set|delete|status --base-url URL
                                   keep a model API key (read from stdin) in the system keychain,
                                   for that endpoint only
  llm-profile list | copy NAME     list run profiles, or copy a built-in one to the config folder to edit";

struct Args {
    datasheets: Vec<Datasheet>,
    name: String,
    out: PathBuf,
    notes: String,
    record: Option<PathBuf>,
    replay: Option<PathBuf>,
    config: RunConfig,
    connection: Connection,
}

/// The flags as given, before the profile is built from them.
#[derive(Default)]
struct Flags {
    provider: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    key: Option<String>,
    context: Option<u64>,
    max_output: Option<u32>,
    vision: Option<bool>,
    native_pdf: Option<bool>,
    tools: Option<bool>,
    effort: Option<String>,
    request_timeout: Option<u64>,
    profile: Option<String>,
}

fn yes_no(flag: &str, value: &str) -> Result<bool> {
    match value {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => Err(format!("{flag} is yes or no").into()),
    }
}

fn count<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T> {
    value
        .parse::<T>()
        .map_err(|_| format!("{flag} takes a whole number").into())
}

fn key_source(text: &str) -> Result<KeySource> {
    match text {
        "none" => Ok(KeySource::None),
        "keychain" => Ok(KeySource::Keychain),
        other => match other.strip_prefix("env:") {
            Some(name) if !name.is_empty() => Ok(KeySource::Env(name.to_owned())),
            _ => Err("--key is none, keychain or env:NAME".into()),
        },
    }
}

/// The profile and connection the flags declare.
fn configure(flags: Flags, shipped: &AssistLibrary) -> Result<(RunConfig, Connection)> {
    let provider = Provider::parse(flags.provider.as_deref().unwrap_or("anthropic"))?;
    let (mut profile, base, key) = match provider {
        Provider::Anthropic => {
            let model = flags.model.unwrap_or_else(|| shipped.default_model.clone());
            let base = flags.base_url.unwrap_or_else(|| {
                std::env::var("ANTHROPIC_BASE_URL")
                    .ok()
                    .filter(|b| b.starts_with("https://"))
                    .unwrap_or_else(|| ANTHROPIC_BASE_URL.into())
            });
            let mut profile = ModelProfile::anthropic(&model);
            if let Some(context) = flags.context {
                profile.capabilities.context_tokens = context;
            }
            let key = KeySource::Env("ANTHROPIC_API_KEY".into());
            (profile, base, key)
        }
        Provider::OpenAiCompatible => {
            let base = flags.base_url.ok_or(
                "--provider openai needs --base-url (for example http://localhost:11434/v1)",
            )?;
            let model = flags.model.ok_or("--provider openai needs --model")?;
            let context = flags.context.ok_or(
                "--provider openai needs --context-tokens: the context the server gives this model",
            )?;
            let profile = ModelProfile::openai_compatible(&model, context);
            (profile, base, KeySource::None)
        }
    };
    if let Some(n) = flags.max_output {
        let after = OutputLimits::for_context(profile.capabilities.context_tokens).after_cut;
        profile.output = OutputLimits {
            per_request: n,
            after_cut: after.max(n),
        };
    }
    if let Some(v) = flags.vision {
        profile.capabilities.vision = v;
    }
    if let Some(v) = flags.native_pdf {
        profile.capabilities.native_pdf = v;
    }
    if let Some(v) = flags.tools {
        profile.capabilities.tool_calling = v;
    }
    if let Some(effort) = flags.effort {
        let allowed: &[&str] = match provider {
            Provider::Anthropic => &["low", "medium", "high", "xhigh", "max"],
            Provider::OpenAiCompatible => &["none", "minimal", "low", "medium", "high"],
        };
        if !allowed.contains(&effort.as_str()) {
            return Err(format!("--effort is one of {}", allowed.join(", ")).into());
        }
        profile.effort = Some(effort);
    }
    if let Some(seconds) = flags.request_timeout {
        profile.request_timeout = Duration::from_secs(seconds);
    }
    let config = RunConfig::new(
        shipped,
        profile,
        flags.profile.as_deref(),
        user_profiles().as_deref(),
    )?;
    let key = match flags.key {
        Some(text) => key_source(&text)?,
        None => key,
    };
    let connection = Connection {
        endpoint: Endpoint::parse(&base)?,
        key,
    };
    Ok((config, connection))
}

fn parse(args: &[String]) -> Result<Args> {
    let mut datasheets = Vec::new();
    let (mut name, mut out, mut notes) = (String::new(), PathBuf::new(), String::new());
    let (mut record, mut replay) = (None, None);
    let mut flags = Flags::default();
    let mut budget = Budget::default();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it
            .next()
            .ok_or_else(|| format!("{flag} needs a value\nusage: {CLI} {USAGE}"))?
            .clone();
        let f = flag.as_str();
        match f {
            "--datasheet" => datasheets.push(Datasheet::parse(&value)),
            "--name" => name = value,
            "--out" => out = PathBuf::from(value),
            "--notes" => notes = value,
            "--provider" => flags.provider = Some(value),
            "--base-url" => flags.base_url = Some(value),
            "--model" => flags.model = Some(value),
            "--key" => flags.key = Some(value),
            "--context-tokens" => flags.context = Some(count(f, &value)?),
            "--max-output-tokens" => flags.max_output = Some(count(f, &value)?),
            "--vision" => flags.vision = Some(yes_no(f, &value)?),
            "--native-pdf" => flags.native_pdf = Some(yes_no(f, &value)?),
            "--tools" => flags.tools = Some(yes_no(f, &value)?),
            "--effort" => flags.effort = Some(value),
            "--request-timeout" => flags.request_timeout = Some(count(f, &value)?),
            "--profile" => flags.profile = Some(value),
            "--max-requests" => budget.max_requests = count(f, &value)?,
            "--max-tokens" => budget.max_total_tokens = count(f, &value)?,
            "--max-seconds" => {
                budget.max_duration = Duration::from_secs(count(f, &value)?);
            }
            "--record" => record = Some(PathBuf::from(value)),
            "--replay" => replay = Some(PathBuf::from(value)),
            other => {
                return Err(format!("unknown option {other}\nusage: {CLI} {USAGE}").into());
            }
        }
    }
    if datasheets.is_empty() || name.trim().is_empty() || out.as_os_str().is_empty() {
        return Err(format!("usage: {CLI} {USAGE}").into());
    }
    if record.is_some() && replay.is_some() {
        return Err("--record and --replay exclude each other".into());
    }
    let shipped = AssistLibrary::locate()?;
    let (mut config, connection) = configure(flags, &shipped)?;
    config.budget = budget;
    config.policy = sparksmith_assist::output_policy(&out)?;
    Ok(Args {
        datasheets,
        name,
        out,
        notes,
        record,
        replay,
        config,
        connection,
    })
}

fn print_event(event: Event) {
    match event {
        Event::Request { n, prompt_tokens } => {
            eprintln!("request {n}: ~{prompt_tokens} prompt tokens");
        }
        Event::Response {
            n,
            stop_reason,
            output_tokens,
            seconds,
        } => {
            eprintln!(
                "response {n}: {stop_reason}; {output_tokens} output tokens in {seconds:.0} s"
            );
        }
        Event::Tool { name, error, step } => {
            let status = if error { "refused" } else { "ok" };
            eprintln!(
                "  {name}: {status}{}",
                step.map(|s| format!(" — {s}")).unwrap_or_default()
            );
        }
        Event::Text(text) => {
            let line: String = text.chars().take(200).collect();
            eprintln!("  model: {line}");
        }
        Event::Warning(text) => eprintln!("warning: {text}"),
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let args = parse(args)?;
    for path in std::iter::once(args.out.clone()).chain(sparksmith_assist::review_paths(&args.out))
    {
        if path.try_exists()? {
            return Err(format!("{} exists; choose a new output name", path.display()).into());
        }
    }
    let mut backend: Box<dyn ModelBackend> = match (&args.replay, &args.record) {
        (Some(path), _) => Box::new(Replay::from_file(path)?),
        (None, Some(path)) => Box::new(Recording::new(
            model::connect(&args.config.profile, &args.connection)?,
            path.clone(),
        )),
        (None, None) => model::connect(&args.config.profile, &args.connection)?,
    };
    let goal = Goal {
        name: args.name.clone(),
        datasheets: args.datasheets.clone(),
        notes: args.notes.clone(),
    };
    let mut session = super::user_session(sparksmith_io::new_project::profiled_engine()?);
    let cancel = AtomicBool::new(false);
    let outcome = sparksmith_assist::run(
        &goal,
        &args.config,
        backend.as_mut(),
        &mut session,
        &mut print_event,
        &cancel,
    )?;
    let [_, review] = sparksmith_assist::write_outputs(&mut session, &args.out, &outcome)?;
    let facts = &outcome.facts;
    eprintln!(
        "{}: {} requests, {} output tokens, {:.0} s; drafts saved to {} (review {})",
        outcome.stop,
        facts.requests,
        facts.output_tokens,
        facts.elapsed_seconds,
        args.out.display(),
        review.display()
    );
    if outcome.report.is_none() {
        return Err(format!("no review report: {}", outcome.stop).into());
    }
    Ok(())
}

/// The user's run-profile folder, when the user directories are known.
fn user_profiles() -> Option<PathBuf> {
    sparksmith_io::settings::UserDirs::from_env()
        .ok()
        .map(|dirs| run_profile::user_folder(&dirs.config))
}

/// `llm-profile list | copy NAME`.
pub fn profile(args: &[String]) -> Result<()> {
    let folder = user_profiles().ok_or("no config folder: set SPARKSMITH_CONFIG_DIR or HOME")?;
    let shipped = AssistLibrary::locate()?;
    match args {
        [action] if action == "list" => {
            for name in shipped.built_in_names() {
                let p = shipped.built_in(name).ok_or("listed")?;
                println!("{name} (built in) {}", p.models.join(", "));
            }
            if let Ok(entries) = std::fs::read_dir(&folder) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    match run_profile::read_file(&path) {
                        Ok(p) => {
                            println!("{} ({}) {}", p.name, path.display(), p.models.join(", "))
                        }
                        Err(e) => println!("broken: {e}"),
                    }
                }
            }
        }
        [action, name] if action == "copy" => {
            let path = shipped.copy_to_user(name, &folder)?;
            eprintln!(
                "copied to {}; edit it, and it takes the built-in's place",
                path.display()
            );
        }
        _ => return Err(format!("usage: {CLI} {USAGE}").into()),
    }
    Ok(())
}

/// `llm-key set|delete|status --base-url URL`.
pub fn key(args: &[String]) -> Result<()> {
    let [action, flag, url] = args else {
        return Err(format!("usage: {CLI} {USAGE}").into());
    };
    if flag != "--base-url" {
        return Err(format!("usage: {CLI} {USAGE}").into());
    }
    let endpoint = Endpoint::parse(url)?;
    match action.as_str() {
        "set" => {
            let mut text = String::new();
            std::io::stdin().take(16 * 1024).read_to_string(&mut text)?;
            let secret = Secret::new(&text).ok_or("give the key on standard input")?;
            keys::keychain_store(&endpoint, &secret)?;
            eprintln!("key stored in the system keychain for {}", endpoint.base());
        }
        "delete" => {
            let removed = keys::keychain_delete(&endpoint)?;
            eprintln!(
                "{} {}",
                if removed {
                    "key removed for"
                } else {
                    "no key for"
                },
                endpoint.base()
            );
        }
        "status" => {
            let has = keys::keychain_has(&endpoint);
            eprintln!(
                "{}: {} ({})",
                endpoint.base(),
                if has {
                    "a key is stored"
                } else {
                    "no key stored"
                },
                endpoint.location()
            );
        }
        _ => return Err(format!("usage: {CLI} {USAGE}").into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn a_local_model_is_configured_from_its_flags() -> Result<()> {
        let parsed = parse(&args(
            "--datasheet a.pdf --name X --out x.sparksmith --provider openai \
             --base-url http://localhost:11434/v1 --model qwen --context-tokens 65536 \
             --vision yes --max-seconds 3600",
        ))?;
        let p = &parsed.config.profile;
        assert_eq!(p.provider, Provider::OpenAiCompatible);
        assert_eq!(p.model, "qwen");
        assert!(p.capabilities.vision && !p.capabilities.native_pdf);
        assert_eq!(p.capabilities.context_tokens, 65_536);
        assert_eq!(p.output.per_request, 32_000);
        assert_eq!(parsed.connection.key, KeySource::None);
        assert!(parsed.connection.endpoint.is_loopback());
        assert_eq!(parsed.config.budget.max_duration, Duration::from_secs(3600));
        Ok(())
    }

    #[test]
    fn the_defaults_are_todays_anthropic_run() -> Result<()> {
        let parsed = parse(&args("--datasheet a.pdf --name X --out x.sparksmith"))?;
        let defaults = RunConfig::shipped_default(&AssistLibrary::locate()?)?;
        assert_eq!(parsed.config.profile, defaults.profile);
        assert_eq!(parsed.config.run_profile, defaults.run_profile);
        assert_eq!(parsed.config.base_prompt, defaults.base_prompt);
        assert_eq!(parsed.config.budget, defaults.budget);
        // The run may name only its output folder.
        let folder = std::path::absolute(".")?;
        assert_eq!(parsed.config.policy.folders, vec![folder]);
        assert_eq!(
            parsed.connection.key,
            KeySource::Env("ANTHROPIC_API_KEY".into())
        );
        Ok(())
    }

    #[test]
    fn missing_declarations_and_bad_values_are_refused() {
        for bad in [
            "--provider openai --model m --context-tokens 1000",
            "--provider openai --base-url http://localhost:1/v1 --context-tokens 1000",
            "--provider openai --base-url http://localhost:1/v1 --model m",
            "--provider other",
            "--vision maybe",
            "--key secret",
            "--effort extreme",
            "--provider openai --base-url ftp://x --model m --context-tokens 1",
        ] {
            let text = format!("--datasheet a.pdf --name X --out x.sparksmith {bad}");
            assert!(parse(&args(&text)).is_err(), "{bad}");
        }
    }
}
