//! GP2 through the session: `library.generate_part` makes a part whose
//! package uses the part file the module names, from the generator's
//! bundle, at the module's values; saved, reopened with the generator
//! folder gone, the project is identical and its packed part file builds
//! the same 3D model through Solidcraft.
use super::super::Session;
use serde_json::{Value, json};
use sparksmith_core::Engine;
use sparksmith_core::library::PackageModel;
use sparksmith_io::shipped::{MARKER, ShippedLibrary};
use sparksmith_solidcraft::models::part_file;
use std::path::{Path, PathBuf};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The repository's chip generator bundle (`cargo xtask generators build`
/// keeps it equal to the crate's module and part files).
const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../library/generators/chip");

/// A shipped library in `dir` holding only the chip generator's bundle.
fn library(dir: &Path) -> TestResult<PathBuf> {
    let root = dir.join("library");
    std::fs::create_dir_all(root.join("generators"))?;
    std::fs::write(root.join(MARKER), "{\"format\":1}\n")?;
    copy_dir(Path::new(BUNDLE), &root.join("generators/chip"))?;
    Ok(root)
}

fn copy_dir(from: &Path, to: &Path) -> TestResult {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let path = entry?.path();
        let target = to.join(path.file_name().ok_or("no file name")?);
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn session(root: &Path) -> TestResult<Session> {
    let mut session = Session::new(Engine::default());
    session.set_shipped(ShippedLibrary::at(root)?);
    Ok(session)
}

fn call(session: &mut Session, command: &str, mut params: Value) -> Value {
    if let Some(map) = params.as_object_mut()
        && command != "project.save"
    {
        map.insert(
            "expected_revision".into(),
            json!(session.engine().revision()),
        );
    }
    session.dispatch(json!({"command": command, "params": params}))
}

/// The package's 3D model as built: its part file (the project's own
/// packed copy) evaluated at the package's values, each body's name and
/// material with the bounds, and the stored derived data equal to it.
fn model(session: &Session) -> TestResult<Vec<String>> {
    let library = &session.engine().project().library;
    let package = library.packages.first().ok_or("no package")?;
    let PackageModel::PartFile(part) = &package.definition.model else {
        return Err("the package has no part file".into());
    };
    let model = library.model(part.model).ok_or("no model")?;
    let bytes = library
        .assets
        .iter()
        .find(|a| a.hash == model.definition.asset.hash)
        .ok_or("the part file is not packed")?;
    let evaluated = part_file::evaluate(bytes.data.as_slice(), &part.values)?;
    assert!(part_file::matches(
        &part.derived,
        &part_file::derived(&evaluated)?
    ));
    Ok(evaluated
        .bodies
        .iter()
        .map(|b| {
            let [lo, hi] = b.bounds_mm;
            format!("{} {:?} {:.3}..{:.3}", b.name, b.material, lo[0], hi[0])
        })
        .collect())
}

#[test]
fn a_generated_part_keeps_its_model_offline() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = library(temp.path())?;
    let mut made = session(&root)?;
    let reply = call(
        &mut made,
        "library.generate_part",
        json!({"generator": "chip", "scope": "component", "preset": "capacitor-0805", "params": {}}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let built = model(&made)?;
    assert_eq!(
        built,
        [
            "body Some(BariumTitanate) -0.500..0.500",
            "termination 1 Some(TinMatte) -1.000..-0.500",
            "termination 2 Some(TinMatte) 0.500..1.000",
        ]
    );
    let path = temp.path().join("model.sparksmith");
    let saved = call(
        &mut made,
        "project.save",
        json!({"path": path.display().to_string()}),
    );
    assert_eq!(saved["ok"], true, "{saved}");

    std::fs::remove_dir_all(root.join("generators"))?;
    let mut reopened = session(&root)?;
    let opened = call(
        &mut reopened,
        "project.open",
        json!({"path": path.display().to_string()}),
    );
    assert_eq!(opened["ok"], true, "{opened}");
    assert_eq!(reopened.engine().to_file(), made.engine().to_file());
    assert_eq!(model(&reopened)?, built);
    Ok(())
}
