use pcbcraft_solidcraft::{Command, Create, Empty, FilePath, Parameters, Probe, Update, schemas};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

#[derive(Default)]
struct CommandSession {
    project: pcbcraft_core::Engine,
    probe: Probe,
    board: Option<pcbcraft_solidcraft::copper::Board>,
}

impl CommandSession {
    fn dispatch(&mut self, request: Value) -> Value {
        let command = request.get("command").and_then(Value::as_str).unwrap_or("");
        if command.starts_with("project.") {
            return match self.project.dispatch(request) {
                Ok(result) => json!({"ok":true,"result":result}),
                Err(error) => {
                    let code = match &error {
                        pcbcraft_core::Error::Stale { .. } => "stale_revision",
                        pcbcraft_core::Error::Invalid(_) => "invalid_input",
                        pcbcraft_core::Error::Json(_) => "invalid_command",
                        pcbcraft_core::Error::Io(_) => "io_error",
                    };
                    json!({"ok":false,"error":{"code":code,"message":error.to_string()}})
                }
            };
        }
        let result = if command.starts_with("board.") {
            if self.board.is_none() {
                match pcbcraft_solidcraft::copper::Board::fixture() {
                    Ok(board) => self.board = Some(board),
                    Err(error) => return json!({"ok":false,"error":error.diagnostic()}),
                }
            }
            match self.board.as_mut() {
                Some(board) => board
                    .dispatch(request)
                    .and_then(|value| Ok(serde_json::to_value(value)?)),
                None => Err(pcbcraft_solidcraft::Error::Invalid(
                    "board unavailable".into(),
                )),
            }
        } else {
            self.probe
                .dispatch(request)
                .and_then(|value| Ok(serde_json::to_value(value)?))
        };
        match result {
            Ok(result) => json!({"ok":true,"result":result}),
            Err(error) => json!({"ok":false,"error":error.diagnostic()}),
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn print(value: &impl serde::Serialize) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd] if cmd == "commands" => print(&json!({"project":pcbcraft_core::schemas(),"probe":schemas(),"board":pcbcraft_solidcraft::copper::schemas()})),
        [cmd,path] if cmd == "project-demo" => {
            let path=std::path::Path::new(path);
            if path.try_exists()? {return Err("output exists; choose a new .pcbcraft filename".into());}
            let mut engine=pcbcraft_core::Engine::demo()?;
            engine.save(path)?;
            print(&json!({"saved_path":path,"result":engine.dispatch(json!({"command":"project.inspect","params":{}}))?}))
        },
        [cmd,source,target] if cmd == "project-export" => {
            let mut engine=pcbcraft_core::Engine::default();
            engine.open(std::path::Path::new(source))?;
            let model=pcbcraft_solidcraft::project::build(engine.project())?;
            model.save(target)?;
            print(&json!({"source":source,"saved_path":target,"geometry_count":model.geometry.len(),"sources":model.sources}))
        },
        [cmd,path] if cmd == "copper" => {
            let mut board=pcbcraft_solidcraft::copper::Board::fixture()?;
            print(&board.execute(pcbcraft_solidcraft::copper::Command::Save{path:path.clone()})?)
        },
        [cmd, path] if cmd == "probe" => {
            let mut probe = Probe::default();
            let initial = probe.execute(Command::Create(Create {
                parameters: Parameters::default(),
            }))?;
            let saved = probe.execute(Command::Save(FilePath { path: path.clone() }))?;
            let mut reopened = Probe::default();
            reopened.execute(Command::Open(FilePath { path: path.clone() }))?;
            let before = reopened.execute(Command::Inspect(Empty {}))?;
            let changed = Parameters {
                board_thickness: 2.4,
                hole_x: 6.0,
                package_length: 5.0,
                top_x: 17.0,
                top_y: 12.0,
                top_angle: 90.0,
                bottom_x: 28.0,
                bottom_angle: -45.0,
                ..Parameters::default()
            };
            let after = reopened.execute(Command::Update(Update {
                expected_revision: before.revision,
                parameters: changed,
            }))?;
            print(&json!({"created": initial, "saved": saved.saved_path,
                "reopened": before, "regenerated_after_reopen": after,
                "note": "The saved file contains the original parameters; regeneration checks run on the reopened in-memory document."}))
        }
        [cmd] if cmd == "stdio" => {
            let mut session = CommandSession::default();
            let mut input = io::stdin().lock();
            let mut line = String::new();
            loop {
                line.clear();
                // Bound line allocation before parsing external command input.
                let count = std::io::Read::take(&mut input, 65537).read_line(&mut line)?;
                if count == 0 {
                    break;
                }
                if count > 65536 {
                    return Err("request exceeds 64 KiB".into());
                }
                let reply = match serde_json::from_str::<Value>(&line) {
                    Ok(request) => session.dispatch(request),
                    Err(error) => {
                        json!({"ok": false, "error": {"code": "invalid_json", "message": error.to_string()}})
                    }
                };
                let mut stdout = io::stdout().lock();
                serde_json::to_writer(&mut stdout, &reply)?;
                writeln!(stdout)?;
                stdout.flush()?;
            }
            Ok(())
        }
        _ => Err("usage: pcbcraft-cli commands | project-demo NEW_FILE.pcbcraft | project-export FILE.pcbcraft NEW_FILE.solidcraft | probe NEW_FILE.solidcraft | copper NEW_FILE.solidcraft | stdio".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_transport_uses_typed_engine_and_keeps_session_after_failure() {
        let mut session = CommandSession::default();
        let before = session.dispatch(json!({"command":"project.inspect","params":{}}));
        assert_eq!(before["ok"], true);
        let bad = session.dispatch(json!({"command":"project.apply","params":{"expected_revision":99,"operation":{"editor":"Schematic","operation":{"command":"schematic.move","params":{"instance":999,"position":[0,0]}}}}}));
        assert_eq!(bad["ok"], false);
        assert_eq!(bad["error"]["code"], "stale_revision");
        assert_eq!(
            session.dispatch(json!({"command":"project.inspect","params":{}})),
            before
        );
    }
}
