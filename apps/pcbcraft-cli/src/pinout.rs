//! Slice C3: `pinout preview|apply`, a thin transport over the
//! `library.pinout_preview` query and the `library.pinout_apply` command.
//! Preview prints the plan (changes, findings, review items). Apply
//! previews, applies at that plan's base (or at `--base`, refusing a
//! stale plan), saves the project and prints the plan and the result.
//! Imported rows stay unreviewed drafts; nothing is published.
use super::{Result, execute, open, print};
use pcbcraft_automation::Session;
use pcbcraft_core::product::SHORT_NAME as CLI;
use serde_json::{Value, json};

/// The usage lines.
pub const USAGE: &str = "pinout preview FILE.json --project P.pcbcraft [--draft ID | --name NAME]
                                   preview a typed pinout document (see library.pinout_schema)
  pinout apply FILE.json --project P.pcbcraft [--draft ID | --name NAME] [--base B] [--except ROW,...]
                                   import it as unreviewed drafts and save the project";

/// Largest pinout document read, in bytes.
const MAX_DOCUMENT: u64 = 8 * 1024 * 1024;

struct Args {
    apply: bool,
    file: String,
    project: String,
    draft: Option<u64>,
    name: Option<String>,
    base: Option<u64>,
    except: Option<Vec<String>>,
}

fn parse(args: &[String]) -> Result<Args> {
    let (mode, file, rest) = match args {
        [mode, file, rest @ ..] if mode == "preview" || mode == "apply" => (mode, file, rest),
        _ => return Err(format!("usage: {CLI} {USAGE}").into()),
    };
    let mut parsed = Args {
        apply: mode == "apply",
        file: file.clone(),
        project: String::new(),
        draft: None,
        name: None,
        base: None,
        except: None,
    };
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let value = it
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?
            .clone();
        match flag.as_str() {
            "--project" => parsed.project = value,
            "--draft" => {
                parsed.draft = Some(value.parse().map_err(|_| "--draft takes a draft ID")?)
            }
            "--name" => parsed.name = Some(value),
            "--base" if parsed.apply => {
                parsed.base = Some(value.parse().map_err(|_| "--base takes a plan base")?);
            }
            "--except" if parsed.apply => {
                parsed.except = Some(value.split(',').map(|s| s.trim().to_owned()).collect());
            }
            _ => return Err(format!("unknown option {flag}; usage: {CLI} {USAGE}").into()),
        }
    }
    if parsed.project.is_empty() {
        return Err("--project names the .pcbcraft file to import into".into());
    }
    if parsed.draft.is_some() && parsed.name.is_some() {
        return Err("give either --draft or --name, not both".into());
    }
    Ok(parsed)
}

fn read_document(path: &str) -> Result<Value> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_DOCUMENT {
        return Err(format!("{path} is larger than 8 MiB").into());
    }
    Ok(serde_json::from_reader(std::io::BufReader::new(file))?)
}

fn preview(session: &mut Session, target: &Value, document: &Value) -> Result<Value> {
    let plan = execute(
        session,
        json!({"command": "project.query", "params": {
            "query": "library.pinout_preview",
            "params": {"target": target, "document": document},
        }}),
    )?;
    Ok(plan["result"].clone())
}

/// Run the subcommand with the arguments after its name.
pub fn run(args: &[String]) -> Result<()> {
    let args = parse(args)?;
    let document = read_document(&args.file)?;
    let mut session = open(Some(&args.project))?;
    let target = match (args.draft, &args.name) {
        (Some(id), _) => json!({"target": "draft", "id": id}),
        (None, Some(name)) => json!({"target": "new", "name": name}),
        (None, None) => {
            let name = document["device"]["name"]
                .as_str()
                .ok_or("the document names no device; pass --name")?;
            json!({"target": "new", "name": name})
        }
    };
    let plan = preview(&mut session, &target, &document)?;
    if !args.apply {
        return print(&plan);
    }
    let base = match args.base {
        Some(base) => json!(base),
        None => plan["base"].clone(),
    };
    let accept = match &args.except {
        Some(rows) => json!({"accept": "except", "rows": rows}),
        None => json!({"accept": "all"}),
    };
    let revision = session.engine().revision();
    let outcome = execute(
        &mut session,
        json!({"command": "project.apply", "params": {
            "expected_revision": revision,
            "operation": {"editor": "Library", "operation": {
                "command": "library.pinout_apply",
                "params": {"target": target, "document": document, "base": base, "accept": accept},
            }},
        }}),
    )?;
    let saved = execute(
        &mut session,
        json!({"command": "project.save", "params": {}}),
    )?;
    print(&json!({"plan": plan, "applied": outcome["outcome"], "saved": saved}))
}
