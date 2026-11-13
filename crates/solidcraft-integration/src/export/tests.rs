use super::*;
use crate::material::BodyKind;
use crate::native::Native;
use pcbcraft_core::Engine;

fn showcase() -> Result<Project> {
    Ok(Engine::showcase()
        .map_err(|error| Error::Verification(error.to_string()))?
        .project()
        .clone())
}

fn path_in(dir: &tempfile::TempDir, name: &str) -> String {
    dir.path().join(name).to_string_lossy().into_owned()
}

/// The files in `dir`, hidden ones too.
fn files(dir: &tempfile::TempDir) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir.path())? {
        out.push(entry?.file_name().to_string_lossy().into_owned());
    }
    out.sort();
    Ok(out)
}

fn kinds(model: &NativeModel) -> Result<Vec<BodyKind>> {
    Ok(model.native_geometry()?.iter().map(|g| g.kind).collect())
}

/// The gate of slice A5: a full export whose built bodies differ from the
/// board plan reports each of them, in the user's words, and still writes
/// the model. Here the plan read is the board with one track 0.1 mm
/// wider than the one built: the track's copper is named, nothing else.
#[test]
fn a_full_export_reports_a_body_that_differs_from_the_plan() -> Result<()> {
    let project = showcase()?;
    let watch = Watch::new();
    let model = project::build_watched(&project, Arc::clone(&watch))?;
    let mut planned = project.clone();
    let track = planned
        .tracks
        .iter_mut()
        .find(|t| t.layer == pcbcraft_core::CopperLayer::Top)
        .ok_or_else(|| Error::Verification("the showcase has no track".into()))?;
    track.width += 0.1;
    let id = track.id;
    let dir = tempfile::tempdir()?;
    let path = path_in(&dir, "board.solidcraft");
    let report = write(&planned, &model, Tier::Full, &path, &watch, None)?;
    assert!(report.differs(), "{report:#?}");
    let check = report
        .check
        .as_ref()
        .ok_or_else(|| Error::Verification("a full export checks".into()))?;
    // The wider track planned, not built; the narrower one built, not planned.
    assert_eq!(check.differences.len(), 2, "{:#?}", check.differences);
    assert!(
        check
            .differences
            .iter()
            .all(|d| d.source == Source::Track { track: id } && d.kind == BodyKind::CopperTrack),
        "{:#?}",
        check.differences
    );
    let lines = report.lines(&planned);
    assert!(
        lines.iter().any(|l| l.starts_with(&format!(
            "Track {id} on the top layer: its track copper was built in another shape"
        ))),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|l| l
            == &format!(
                "Track {id} on the top layer: the model has a track copper the board does not"
            )),
        "{lines:#?}"
    );
    // Reported, and written all the same.
    assert_eq!(files(&dir)?, vec!["board.solidcraft".to_owned()]);
    Ok(())
}

/// A full export of the showcase: every flat body the plan holds is built
/// as planned, the file is written whole (no temporary file left) and
/// opens again, and the watch counted the commands and ended done.
#[test]
fn a_full_export_matches_its_plan_and_writes_the_file() -> Result<()> {
    let project = showcase()?;
    let dir = tempfile::tempdir()?;
    let path = path_in(&dir, "board.solidcraft");
    let watch = Watch::new();
    let report = board(&project, Tier::Full, &path, &watch)?;
    let check = report
        .check
        .as_ref()
        .ok_or_else(|| Error::Verification("a full export checks".into()))?;
    assert!(check.differences.is_empty(), "{:#?}", check.differences);
    assert!(!report.differs());
    let planned = plan::of(&project)?.bodies().count();
    assert_eq!(check.planned, planned);
    assert!(check.built > 0 && check.built <= planned);
    assert!(report.bodies >= check.built);
    assert_eq!(files(&dir)?, vec!["board.solidcraft".to_owned()]);
    let reopened = Native::open(&path)?;
    let sources = project::saved_sources(&reopened)?;
    let raw = reopened.world_state()?.bodies.len();
    let again = project::finish(reopened, sources)?;
    assert_eq!(
        again.native_geometry()?.len(),
        report.bodies,
        "the file holds the bodies the report counts (raw world {raw})"
    );
    let replanned = plan::check(&plan::of(&project)?, &again, &mut || Ok(()))?;
    assert!(
        replanned.differences.is_empty(),
        "{:#?}",
        replanned.differences
    );
    let progress = watch.progress();
    assert_eq!(progress.stage, Stage::Done);
    assert!(progress.commands > 0, "{progress:?}");
    assert_eq!(progress.checked, progress.to_check);
    assert!(progress.to_check >= u64::try_from(check.built).unwrap_or(u64::MAX));
    Ok(())
}

/// The mechanical tier: the laminate, the pads and the parts, no track,
/// via or fill copper, and checked against the plan of that board.
#[test]
fn the_mechanical_tier_has_no_tracks_vias_or_fills() -> Result<()> {
    let project = showcase()?;
    assert!(!project.tracks.is_empty() && !project.vias.is_empty() && !project.fills.is_empty());
    let board = section(&project, Tier::Mechanical)
        .ok_or_else(|| Error::Verification("the mechanical tier builds a board".into()))?;
    let watch = Watch::new();
    let model = project::build_watched(&board, Arc::clone(&watch))?;
    let kinds = kinds(&model)?;
    for copper in [
        BodyKind::CopperTrack,
        BodyKind::ViaCopper,
        BodyKind::FillCopper,
    ] {
        assert!(
            !kinds.contains(&copper),
            "{copper:?} in the mechanical tier"
        );
    }
    assert!(kinds.contains(&BodyKind::PadCopper));
    assert!(kinds.contains(&BodyKind::Substrate));
    let checked = check(&board, &model, &watch)?;
    assert!(checked.differences.is_empty(), "{:#?}", checked.differences);
    assert!(checked.planned < plan::of(&project)?.bodies().count());
    assert!(section(&project, Tier::Interface).is_none());
    Ok(())
}

/// The plain export refuses the interface tier, and a file that is there,
/// before building anything.
#[test]
fn the_export_refuses_the_interface_tier_and_an_existing_file() -> Result<()> {
    let project = showcase()?;
    let dir = tempfile::tempdir()?;
    let path = path_in(&dir, "board.solidcraft");
    let watch = Watch::new();
    let Err(Error::Invalid(message)) = board(&project, Tier::Interface, &path, &watch) else {
        return Err(Error::Verification(
            "the interface tier was exported".into(),
        ));
    };
    assert!(message.contains("interface.publish"), "{message}");
    std::fs::write(&path, b"kept")?;
    let Err(Error::Invalid(message)) = board(&project, Tier::Full, &path, &watch) else {
        return Err(Error::Verification("an existing file was replaced".into()));
    };
    assert!(message.contains("output exists"), "{message}");
    assert_eq!(watch.progress().commands, 0, "nothing was built");
    assert_eq!(std::fs::read(&path)?, b"kept");
    Ok(())
}

/// Cancelled before it starts, or while it builds (from another thread,
/// once 50 commands ran), an export stops with `Cancelled` and writes
/// nothing, no temporary file either.
#[test]
fn a_cancelled_export_writes_nothing() -> Result<()> {
    let project = showcase()?;
    let dir = tempfile::tempdir()?;
    let path = path_in(&dir, "board.solidcraft");
    let watch = Watch::new();
    watch.cancel();
    assert!(matches!(
        board(&project, Tier::Full, &path, &watch),
        Err(Error::Cancelled)
    ));
    assert_eq!(watch.progress().commands, 0);
    assert!(files(&dir)?.is_empty());

    let watch = Watch::new();
    let cancelling = Arc::clone(&watch);
    let canceller = std::thread::spawn(move || {
        while cancelling.progress().commands < 50 && cancelling.progress().stage != Stage::Done {
            std::thread::yield_now();
        }
        cancelling.cancel();
    });
    let result = board(&project, Tier::Full, &path, &watch);
    canceller
        .join()
        .map_err(|_| Error::Verification("the cancelling thread failed".into()))?;
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    let progress = watch.progress();
    assert!(progress.commands >= 50, "{progress:?}");
    assert_eq!(progress.stage, Stage::Done);
    assert!(progress.cancelled);
    assert!(files(&dir)?.is_empty(), "{:?}", files(&dir)?);
    Ok(())
}

/// A cancel during the check stops it between bodies.
#[test]
fn a_cancelled_check_stops() -> Result<()> {
    let project = showcase()?;
    let watch = Watch::new();
    let model = project::build_watched(&project, Arc::clone(&watch))?;
    watch.cancel();
    assert!(matches!(
        check(&project, &model, &watch),
        Err(Error::Cancelled)
    ));
    Ok(())
}

/// The progress line the app shows.
#[test]
fn progress_reads_as_the_user_reads_it() {
    let progress = Progress {
        stage: Stage::Checking,
        commands: 120,
        checked: 3,
        to_check: 40,
        cancelled: false,
    };
    assert_eq!(
        progress.text(),
        "Checking bodies against the board · 3 of 40"
    );
    let stopping = Progress {
        stage: Stage::Building,
        cancelled: true,
        ..progress
    };
    assert_eq!(
        stopping.text(),
        "Building the model · 120 commands · stopping"
    );
}
