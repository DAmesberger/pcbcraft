//! `import-kicad DIR|FILE.kicad_pcb NEW_FILE.sparksmith [--profile ID]`:
//! import a KiCad board (footprints and pads, outline, stackup, nets,
//! tracks, vias and zones) through the one command engine, on the fab
//! profile library's profile (else the default) with SolidCraft's sketch
//! engines, and save it. The import report's counts are printed; its full
//! detail (designators, pads, net names) goes beside the new file as
//! `NEW_FILE.kicad-import.json`, for the user only. A board whose fill
//! copper would outgrow the file bound is saved with its fills defined but
//! unfilled (fill copper is derivable; the app refills it).
use super::{Result, execute, print};
use serde_json::json;
use sparksmith_automation::Session;
use sparksmith_core::product::SHORT_NAME as CLI;
use std::path::{Path, PathBuf};

/// The usage line(s).
pub const USAGE: &str = "import-kicad DIR|FILE.kicad_pcb NEW_FILE.sparksmith [--profile ID]
                                   import a KiCad board through the command engine
                                   (footprints, outline, stackup, nets, tracks, vias,
                                   zones filled by our engine) and save it; DIR names
                                   the folder of one board; the report's counts are
                                   printed, its detail written beside the new file";

/// Largest board file read, bytes (the reader's own bound).
const MAX_BOARD: u64 = sparksmith_kicad::sexpr::MAX_INPUT as u64;

/// The one board file of `source`: the file itself, or the only
/// `.kicad_pcb` directly in a folder (KiCad's backups and hidden files
/// aside).
fn board_file(source: &Path) -> Result<PathBuf> {
    if !source.is_dir() {
        return Ok(source.to_path_buf());
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(source)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_file() && name.ends_with(".kicad_pcb") && !name.starts_with(['.', '_']) {
            found.push(path);
        }
    }
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err("import-kicad: the folder holds no .kicad_pcb board".into()),
        more => Err(format!(
            "import-kicad: the folder holds {} boards; name the .kicad_pcb file",
            more.len()
        )
        .into()),
    }
}

/// Run the subcommand with the arguments after its name.
pub fn run(args: &[String]) -> Result<()> {
    let (source, target, choice) = match args {
        [source, target] => (source, target, None),
        [source, target, flag, id] if flag == "--profile" => (source, target, Some(id.as_str())),
        _ => return Err(format!("usage: {CLI} {USAGE}").into()),
    };
    let board = board_file(Path::new(source))?;
    if std::fs::metadata(&board)?.len() > MAX_BOARD {
        return Err("import-kicad: the board file is too large".into());
    }
    let bytes = std::fs::read(&board)?;
    let project = board.with_extension("kicad_pro");
    let project = project
        .is_file()
        .then(|| std::fs::read(&project))
        .transpose()?;
    let defaults = sparksmith_io::new_project::shipped()?;
    let profile = sparksmith_io::rule_profiles::for_new_board_from_env(choice)?;
    let imported = sparksmith_kicad::import(
        &bytes,
        &sparksmith_kicad::Setup {
            board: &defaults.board,
            presets: &defaults.presets,
            profile: Some(&profile),
            evaluators: sparksmith_automation::session::hosted(),
            project_file: project.as_deref(),
        },
    )?;
    let report = imported.report.clone();
    let mut engine = imported.engine;
    // Fill copper is derivable and refilled when missing; a board whose
    // fills outgrow the file bound is saved with its fill definitions only.
    let fits = sparksmith_core::format::encode_bounded(&engine.to_file()).is_ok();
    if !fits {
        let fills: Vec<_> = engine.project().fills.iter().cloned().collect();
        for mut fill in fills {
            fill.computed = None;
            engine.apply(
                engine.revision(),
                sparksmith_core::Operation::Fill(
                    sparksmith_core::layout::fill::FillCommand::Edit {
                        fill: Box::new(fill),
                    },
                ),
            )?;
        }
    }
    let mut session = Session::new(engine);
    let saved = execute(
        &mut session,
        json!({"command": "project.save", "params": {"path": target}}),
    )?;
    let detail = Path::new(target).with_extension("kicad-import.json");
    std::fs::write(&detail, serde_json::to_vec_pretty(&report.to_json())?)?;
    print(&json!({
        "saved": saved,
        "summary": report.summary(),
        "counts": report.counts,
        "rows": report.rows(),
        "detail": detail,
        "fills_saved_unfilled": !fits,
    }))
}
