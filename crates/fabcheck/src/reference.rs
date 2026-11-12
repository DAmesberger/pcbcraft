//! Reference Gerber and drill files from an installed `kicad-cli`, used as
//! a black-box oracle: it is run, never read.
//!
//! `kicad-cli` rewrites project-local files next to the board it plots, so
//! it never runs on a design folder: the board (and its project file, when
//! there is one) is copied into `<run>/ref-src/`, plotted from there into
//! `<run>/ref/`, and the copy is deleted afterwards, whether the plot
//! succeeded or not (decision D6). Callers keep `<run>` under an ignored
//! folder (`target/kicad-compare/`).
use crate::Fallible;
use crate::sets::File;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The program run; `PCBCRAFT_KICAD_CLI` overrides it.
pub fn program() -> PathBuf {
    std::env::var_os("PCBCRAFT_KICAD_CLI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("kicad-cli"))
}

/// The installed `kicad-cli` version, or `None` when it cannot be run.
pub fn version() -> Option<String> {
    let out = Command::new(program()).arg("version").output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The layers plotted: copper, mask, paste and the outline.
fn layer_list(copper: usize) -> String {
    let mut layers = vec!["F.Cu".to_owned()];
    layers.extend((1..copper.saturating_sub(1)).map(|k| format!("In{k}.Cu")));
    layers.push("B.Cu".to_owned());
    layers.extend(
        ["F.Mask", "B.Mask", "F.Paste", "B.Paste", "Edge.Cuts"]
            .iter()
            .map(|s| (*s).to_owned()),
    );
    layers.join(",")
}

fn run(args: &[&str]) -> Fallible<()> {
    let out = Command::new(program()).args(args).output()?;
    if !out.status.success() {
        return Err(format!("kicad-cli failed ({})", out.status).into());
    }
    Ok(())
}

/// Copy `board` (and `project`) into `<run>/ref-src`, plot Gerbers and
/// drills into `<run>/ref` relative to the drill/place origin, delete the
/// copy, and return the plotted files. `copper` is the board's copper
/// layer count.
pub fn plot(
    board: &Path,
    project: Option<&Path>,
    copper: usize,
    run_dir: &Path,
) -> Fallible<Vec<File>> {
    let source = run_dir.join("ref-src");
    let output = run_dir.join("ref");
    std::fs::create_dir_all(&source)?;
    std::fs::create_dir_all(&output)?;
    let result = plot_copy(board, project, copper, &source, &output);
    // The copy goes whatever happened.
    let removed = std::fs::remove_dir_all(&source);
    let files = result?;
    removed?;
    Ok(files)
}

fn plot_copy(
    board: &Path,
    project: Option<&Path>,
    copper: usize,
    source: &Path,
    output: &Path,
) -> Fallible<Vec<File>> {
    let copy = source.join("board.kicad_pcb");
    std::fs::copy(board, &copy)?;
    if let Some(project) = project {
        std::fs::copy(project, source.join("board.kicad_pro"))?;
    }
    let copy = copy.to_str().ok_or("a path that is not UTF-8")?;
    let out = output.to_str().ok_or("a path that is not UTF-8")?;
    let out = format!("{out}/");
    run(&[
        "pcb",
        "export",
        "gerbers",
        "--output",
        &out,
        "--layers",
        &layer_list(copper),
        "--use-drill-file-origin",
        "--precision",
        "6",
        "--no-protel-ext",
        copy,
    ])?;
    run(&[
        "pcb",
        "export",
        "drill",
        "--output",
        &out,
        "--format",
        "excellon",
        "--drill-origin",
        "plot",
        "--excellon-units",
        "mm",
        "--excellon-zeros-format",
        "decimal",
        "--excellon-oval-format",
        "alternate",
        "--excellon-separate-th",
        copy,
    ])?;
    read_dir(output)
}

/// Every file of a folder, sorted by name.
pub fn read_dir(dir: &Path) -> Fallible<Vec<File>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() {
            files.push(File {
                name: path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned(),
                bytes: std::fs::read(&path)?,
            });
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}
