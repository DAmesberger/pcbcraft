//! Command-line access to Pcbcraft's command engine. Every subcommand is a
//! thin transport over `pcbcraft_automation::Session`.
mod design_part;
mod fab;
mod firmware;
mod part;
mod pinout;
mod review_diff;
mod rules_profile;
mod sample_packages;

use pcbcraft_automation::{Mode, Policy, Session, lines, mcp, session};
use pcbcraft_core::Engine;
use serde_json::{Value, json};
use std::io::{self, Write};
use std::process::ExitCode;

/// Large enough for one batch of draft definition edits.
const MAX_REQUEST: usize = 4 * 1024 * 1024;

const USAGE: &str = "usage: pcbcraft-cli <command>
  commands                         print request, document and diagnostic schemas
  stdio [FILE.pcbcraft]            JSON-lines requests on stdin, replies on stdout
  mcp [--permission-mode MODE] [--allow-folder DIR]... [FILE.pcbcraft]
                                   Model Context Protocol server on stdio; a mode
                                   (ask-every-time, auto-allow-edits, allow-all) turns
                                   on the permission gate, limited to the given folders
                                   and FILE's, and refuses what would need asking
  demo NEW_FILE.pcbcraft           write the two-component demo project
  showcase NEW_FILE.pcbcraft       write the 4-layer PCB showcase project
  parts NEW_FILE.pcbcraft          write a synthetic catalog of 24 invented EX- parts
  sample-library NEW_DIR [NAME]    write the synthetic catalog as a library folder
  export FILE.pcbcraft NEW_FILE.solidcraft
                                   export a project as a native Solidcraft model
  export-schematic-pdf FILE.pcbcraft OUT.pdf [--sheet ID]...
                                   print the schematic (all sheets, or the given
                                   sheets in page order) as a vector PDF";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// [`USAGE`] with the slice subcommands that are available.
fn usage() -> String {
    [
        pinout::USAGE,
        sample_packages::USAGE,
        firmware::USAGE,
        rules_profile::USAGE,
        design_part::USAGE,
        fab::USAGE,
        review_diff::USAGE,
        part::USAGE,
    ]
    .into_iter()
    .filter(|u| !u.is_empty())
    .fold(USAGE.to_owned(), |text, line| format!("{text}\n  {line}"))
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn print(value: &Value) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}

/// Run one request; a failure becomes the process error.
fn execute(session: &mut Session, request: Value) -> Result<Value> {
    let reply = session.dispatch(request);
    if reply["ok"] == true {
        Ok(reply["result"].clone())
    } else {
        Err(reply["error"].to_string().into())
    }
}

/// A session over the user's library sources (`PCBCRAFT_CONFIG_DIR` and
/// `PCBCRAFT_CACHE_DIR` when set). When they cannot be read the session
/// still runs without sources, says so, and refuses source changes instead
/// of reporting them saved.
fn user_session(engine: Engine) -> Session {
    let sources = session::sources::Sources::from_env().unwrap_or_else(|error| {
        eprintln!("library sources unavailable: {error}");
        session::sources::Sources::unavailable(error.to_string())
    });
    Session::with_sources(engine, sources)
}

/// A session, optionally starting from a project file.
fn open(path: Option<&String>) -> Result<Session> {
    let mut session = user_session(Engine::default());
    if let Some(path) = path {
        execute(
            &mut session,
            json!({"command": "project.open", "params": {"path": path}}),
        )?;
    }
    Ok(session)
}

/// The `mcp` command's arguments. Without a mode the server is ungated, as
/// the user who starts it has already chosen to trust its client.
#[derive(Debug, Default, PartialEq)]
struct McpOptions {
    mode: Option<Mode>,
    folders: Vec<String>,
    project: Option<String>,
}

impl McpOptions {
    fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |flag: &str| {
                args.next()
                    .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))
            };
            match arg.as_str() {
                "--permission-mode" => {
                    options.mode = Some(value("--permission-mode")?.parse::<Mode>()?);
                }
                "--allow-folder" => options.folders.push(value("--allow-folder")?.clone()),
                other if other.starts_with("--") || options.project.is_some() => {
                    return Err(format!("unexpected argument {other}\n{USAGE}").into());
                }
                _ => options.project = Some(arg.clone()),
            }
        }
        if options.mode.is_none() && !options.folders.is_empty() {
            return Err(format!("--allow-folder needs --permission-mode\n{USAGE}").into());
        }
        Ok(options)
    }

    /// The gate's policy; the folder of a project named here is allowed, as
    /// the user just chose that file.
    fn policy(&self) -> Result<Option<Policy>> {
        let Some(mode) = self.mode else {
            return Ok(None);
        };
        let mut folders = Vec::new();
        for folder in &self.folders {
            folders.push(std::path::absolute(folder)?);
        }
        if let Some(project) = &self.project
            && let Some(folder) = std::path::absolute(project)?.parent()
        {
            folders.push(folder.to_path_buf());
        }
        Ok(Some(Policy::new(mode, folders)))
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "commands" => print(&session::schemas()),
        [command, rest @ ..] if command == "stdio" && rest.len() <= 1 => stdio(open(rest.first())?),
        [command, rest @ ..] if command == "mcp" => {
            let options = McpOptions::parse(rest)?;
            let session = open(options.project.as_ref())?;
            let mut server = match options.policy()? {
                Some(policy) => mcp::Server::gated(session, policy),
                None => mcp::Server::new(session),
            };
            Ok(server.serve(io::stdin().lock(), io::stdout().lock())?)
        }
        [command, path] if ["demo", "showcase", "parts"].contains(&command.as_str()) => {
            if std::path::Path::new(path).try_exists()? {
                return Err("output exists; choose a new .pcbcraft filename".into());
            }
            let engine = if command == "demo" {
                Engine::demo()?
            } else if command == "showcase" {
                session::showcase()?
            } else {
                Engine::from_file(pcbcraft_core::format::EditorFile {
                    version: pcbcraft_core::format::FILE_VERSION,
                    project: pcbcraft_core::catalog::synthetic::project(24)?,
                    drafts: Default::default(),
                })?
            };
            let mut session = Session::new(engine);
            let saved = execute(
                &mut session,
                json!({"command": "project.save", "params": {"path": path}}),
            )?;
            print(&json!({"saved": saved, "document": session.snapshot()}))
        }
        [command, path, rest @ ..] if command == "sample-library" && rest.len() <= 1 => {
            sample_library(path, rest.first().map_or("Example library", String::as_str))
        }
        [command, source, target] if command == "export" => {
            let mut session = open(Some(source))?;
            print(&execute(
                &mut session,
                json!({"command": "project.export_native", "params": {"path": target}}),
            )?)
        }
        [command, source, target, rest @ ..] if command == "export-schematic-pdf" => {
            let sheets = sheet_ids(rest)?;
            let mut session = open(Some(source))?;
            print(&execute(
                &mut session,
                json!({"command": "schematic.export_pdf",
                       "params": {"path": target, "sheets": sheets}}),
            )?)
        }
        [command, rest @ ..] if command == "fab" => fab::run(rest),
        [command, rest @ ..] if command == "pinout" => pinout::run(rest),
        [command, rest @ ..] if command == "sample-packages" => sample_packages::run(rest),
        [command, rest @ ..] if command == "firmware" => firmware::run(rest),
        [command, rest @ ..] if command == "rules-profile" => rules_profile::run(rest),
        [command, rest @ ..] if command == "design-part" => design_part::run(rest),
        [command, rest @ ..] if command == "llm-key" => design_part::key(rest),
        [command, rest @ ..] if command == "llm-profile" => design_part::profile(rest),
        [command, rest @ ..] if command == "review-diff" => review_diff::run(rest),
        [command, rest @ ..] if command == "part" => part::run(rest),
        _ => Err(usage().into()),
    }
}

/// Sheet IDs from `--sheet ID` pairs.
fn sheet_ids(args: &[String]) -> Result<Vec<u32>> {
    let mut sheets = Vec::new();
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        match (flag.as_str(), rest.next()) {
            ("--sheet", Some(id)) => sheets.push(
                id.parse()
                    .map_err(|_| format!("--sheet takes a sheet ID, not {id}"))?,
            ),
            _ => return Err(usage().into()),
        }
    }
    Ok(sheets)
}

/// Write the synthetic catalog (24 components plus generic components and
/// `ExampleCo` Parts) to a new library folder through the same requests the
/// JSON transports use. The user's settings are not changed.
fn sample_library(path: &str, name: &str) -> Result<()> {
    if std::path::Path::new(path).try_exists()? {
        return Err("output exists; choose a new folder".into());
    }
    let engine = Engine::from_file(pcbcraft_core::format::EditorFile {
        version: pcbcraft_core::format::FILE_VERSION,
        project: pcbcraft_core::catalog::synthetic::project_with_parts(24)?,
        drafts: Default::default(),
    })?;
    let mut session = Session::new(engine);
    print(&session::publish_library(&mut session, path, name)?)
}

fn stdio(mut session: Session) -> Result<()> {
    let mut input = io::stdin().lock();
    while let Some(line) = lines::next(&mut input, MAX_REQUEST)? {
        let reply = match line {
            lines::Line::Text(text) if text.trim().is_empty() => continue,
            lines::Line::Text(text) => match serde_json::from_str::<Value>(&text) {
                Ok(request) => session.respond(request),
                Err(error) => failure("malformed_request", &error.to_string()),
            },
            lines::Line::TooLarge => failure("limit_exceeded", "request exceeds 4 MiB"),
            lines::Line::NotUtf8 => failure("malformed_request", "request is not UTF-8"),
        };
        let mut stdout = io::stdout().lock();
        serde_json::to_writer(&mut stdout, &reply)?;
        writeln!(stdout)?;
        stdout.flush()?;
    }
    Ok(())
}

fn failure(code: &str, message: &str) -> Value {
    json!({"ok": false, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod mcp_option_tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<McpOptions> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        McpOptions::parse(&args)
    }

    #[test]
    fn without_a_mode_the_server_is_ungated() -> Result<()> {
        assert!(parse(&[])?.policy()?.is_none());
        assert!(parse(&["board.pcbcraft"])?.policy()?.is_none());
        Ok(())
    }

    #[test]
    fn a_mode_gates_within_the_named_folders_and_the_projects() -> Result<()> {
        let options = parse(&[
            "--permission-mode",
            "allow-all",
            "--allow-folder",
            "/work/out",
            "/work/board/x.pcbcraft",
        ])?;
        let policy = options.policy()?.ok_or("gated")?;
        assert_eq!(policy.mode, Mode::AllowAll);
        assert_eq!(
            policy.folders,
            [
                std::path::PathBuf::from("/work/out"),
                std::path::PathBuf::from("/work/board")
            ]
        );
        Ok(())
    }

    #[test]
    fn bad_arguments_are_refused() {
        assert!(parse(&["--permission-mode", "yolo"]).is_err());
        assert!(parse(&["--permission-mode"]).is_err());
        assert!(parse(&["--allow-folder", "/x"]).is_err());
        assert!(parse(&["a.pcbcraft", "b.pcbcraft"]).is_err());
        assert!(parse(&["--gate"]).is_err());
    }
}
