//! Command-line access to Pcbcraft's command engine. Every subcommand is a
//! thin transport over `pcbcraft_automation::Session`.
use pcbcraft_automation::{Session, lines, mcp, session};
use pcbcraft_core::Engine;
use serde_json::{Value, json};
use std::io::{self, Write};
use std::process::ExitCode;

/// Large enough for one batch of draft definition edits.
const MAX_REQUEST: usize = 4 * 1024 * 1024;

const USAGE: &str = "usage: pcbcraft-cli <command>
  commands                         print request, document and diagnostic schemas
  stdio [FILE.pcbcraft]            JSON-lines requests on stdin, replies on stdout
  mcp [FILE.pcbcraft]              Model Context Protocol server on stdio
  demo NEW_FILE.pcbcraft           write the two-component demo project
  parts NEW_FILE.pcbcraft          write a synthetic catalog of 24 invented EX- parts
  export FILE.pcbcraft NEW_FILE.solidcraft
                                   export a project as a native Solidcraft model";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
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

/// A session, optionally starting from a project file.
fn open(path: Option<&String>) -> Result<Session> {
    let mut session = Session::default();
    if let Some(path) = path {
        execute(
            &mut session,
            json!({"command": "project.open", "params": {"path": path}}),
        )?;
    }
    Ok(session)
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "commands" => print(&session::schemas()),
        [command, rest @ ..] if command == "stdio" && rest.len() <= 1 => stdio(open(rest.first())?),
        [command, rest @ ..] if command == "mcp" && rest.len() <= 1 => {
            let mut server = mcp::Server::new(open(rest.first())?);
            Ok(server.serve(io::stdin().lock(), io::stdout().lock())?)
        }
        [command, path] if command == "demo" || command == "parts" => {
            if std::path::Path::new(path).try_exists()? {
                return Err("output exists; choose a new .pcbcraft filename".into());
            }
            let engine = if command == "demo" {
                Engine::demo()?
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
        [command, source, target] if command == "export" => {
            let mut session = open(Some(source))?;
            print(&execute(
                &mut session,
                json!({"command": "project.export_native", "params": {"path": target}}),
            )?)
        }
        _ => Err(USAGE.into()),
    }
}

fn stdio(mut session: Session) -> Result<()> {
    let mut input = io::stdin().lock();
    while let Some(line) = lines::next(&mut input, MAX_REQUEST)? {
        let reply = match line {
            lines::Line::Text(text) if text.trim().is_empty() => continue,
            lines::Line::Text(text) => match serde_json::from_str::<Value>(&text) {
                Ok(request) => session.dispatch(request),
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
