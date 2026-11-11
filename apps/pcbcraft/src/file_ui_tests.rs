//! Quitting with unsaved work, in headless egui frames with the real file
//! worker: the dialog, each answer, a quit during a save, and an exit that
//! nobody answered. Recovery snapshots go to a temporary folder.
use super::*;
use pcbcraft_core::Operation;
use pcbcraft_core::schematic::SchematicCommand;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const TITLE: &str = "Save changes before quitting?";

struct Harness {
    ctx: egui::Context,
    files: FileUi,
    engine: Engine,
    dir: tempfile::TempDir,
    now: f64,
    effects: Vec<Effect>,
}

impl Harness {
    fn new() -> Fallible<Self> {
        Self::in_dir(tempfile::tempdir()?)
    }

    /// A session whose recovery folder is `dir`'s, as left by earlier ones.
    fn in_dir(dir: tempfile::TempDir) -> Fallible<Self> {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let files = FileUi::with_directory(
            &ctx,
            rfd::AsyncFileDialog::new(),
            &engine,
            dir.path().join("recovery"),
        );
        let mut harness = Self {
            ctx,
            files,
            engine,
            dir,
            now: 0.0,
            effects: Vec::new(),
        };
        // The worker's startup listing (an empty folder) arrives first.
        harness.frame(vec![]);
        Ok(harness)
    }

    /// One frame as the app runs it: file results, then the document, then
    /// the dialogs. Returns what it painted.
    fn frame(&mut self, events: Vec<egui::Event>) -> Vec<(String, egui::Pos2)> {
        let Self {
            ctx,
            files,
            engine,
            now,
            effects,
            ..
        } = self;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            time: Some(*now),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            effects.extend(files.tick(ui.ctx()));
            files.observe(engine, *now);
            files.dialogs(ui.ctx());
        });
        output.textures_delta.clear();
        let mut painted = Vec::new();
        for shape in &output.shapes {
            texts(&shape.shape, &mut painted);
        }
        painted
    }

    /// Frames until `done` holds, waiting for the worker between them.
    fn until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) -> Fallible {
        for _ in 0..500 {
            if done(self) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(5));
            self.frame(vec![]);
        }
        Err(format!("timed out waiting for {what}").into())
    }

    /// Change the project, as an edit in any workspace would.
    fn edit(&mut self, name: &str) -> Fallible {
        self.engine.apply(
            self.engine.revision(),
            Operation::Schematic(SchematicCommand::RenameProject { name: name.into() }),
        )?;
        self.frame(vec![]);
        assert!(self.files.dirty());
        Ok(())
    }

    /// The painted texts once a newly opened dialog shows: egui lays a
    /// new window out invisibly in its first frame.
    fn shown(&mut self) -> Vec<(String, egui::Pos2)> {
        self.frame(vec![]);
        self.frame(vec![])
    }

    /// Click the dialog button labelled `label`.
    fn click(&mut self, label: &str) -> Fallible {
        let painted = self.shown();
        let at = painted
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, at)| *at)
            .ok_or_else(|| format!("no {label} button among {painted:?}"))?;
        self.frame(vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        self.frame(vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        Ok(())
    }

    fn closed(&self) -> bool {
        self.effects.iter().any(|e| matches!(e, Effect::Close))
    }

    /// The project names this session's recovery folder holds.
    fn recoverable(&self) -> Fallible<Vec<String>> {
        Ok(recovery::list(&self.dir.path().join("recovery"))?
            .entries
            .into_iter()
            .map(|e| e.project_name)
            .collect())
    }

    /// Every file in the recovery folder (none before it exists), empty
    /// session reservations too.
    fn recovery_files(&self) -> Fallible<Vec<PathBuf>> {
        let mut files = Vec::new();
        let entries = match std::fs::read_dir(self.dir.path().join("recovery")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(files),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            files.push(entry?.path());
        }
        Ok(files)
    }

    /// Wait for the background snapshot of the current edit (taken two
    /// seconds after the last change).
    fn snapshot(&mut self, name: &str) -> Fallible {
        self.now += 3.0;
        self.until("the recovery snapshot", |h| {
            h.recoverable().is_ok_and(|names| names == [name])
        })
    }
}

fn texts(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
    match shape {
        egui::Shape::Text(text) => out.push((
            text.galley.job.text.clone(),
            text.pos + text.galley.size() * 0.5,
        )),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
        _ => {}
    }
}

fn shows(painted: &[(String, egui::Pos2)], text: &str) -> bool {
    painted.iter().any(|(t, _)| t == text)
}

/// A restored session has no file of its own yet: Save proposes the name
/// and folder of the file it was restored from, not "untitled" at home.
#[test]
fn save_after_a_restore_proposes_the_original_file() -> Fallible {
    let dir = tempfile::tempdir()?;
    let original = dir.path().join("prec").join("board.pcbcraft");
    let engine = Engine::demo()?;
    RecoveryStore::new(&dir.path().join("recovery"))?.write(
        &EditorFile::from_document(engine.document()),
        Some(&original),
    )?;
    let mut h = Harness::in_dir(dir)?;
    h.until("the recovery question", |h| !h.files.recoveries.is_empty())?;
    h.click("Restore")?;
    h.until("the restored document", |h| {
        h.effects.iter().any(|e| {
            matches!(
                e,
                Effect::Replace {
                    recovered: true,
                    ..
                }
            )
        })
    })?;
    // The app adopts it as an unsaved document without a path.
    h.files.adopt(&h.engine, None, true)?;
    let (folder, name) = h.files.save_proposal();
    assert_eq!(Some(folder), original.parent());
    assert_eq!(name, "board.pcbcraft");
    // Once the project has a file of its own, that one is proposed.
    let own = h.dir.path().join("own.pcbcraft");
    h.files.adopt(&h.engine, Some(own), false)?;
    assert_eq!(h.files.save_proposal().1, "own.pcbcraft");
    Ok(())
}

/// Nothing unsaved: quitting closes the window at once, with no dialog.
#[test]
fn quitting_a_saved_project_closes_without_asking() -> Fallible {
    let mut h = Harness::new()?;
    h.files.request(Action::Close);
    let painted = h.shown();
    assert!(!shows(&painted, TITLE));
    h.until("the close", Harness::closed)
}

/// Unsaved work asks Save, Don't Save or Cancel. Cancel leaves everything
/// as it was: the window stays, the work stays unsaved and its recovery
/// snapshot stays on disk.
#[test]
fn cancel_keeps_the_window_and_the_work() -> Fallible {
    let mut h = Harness::new()?;
    h.edit("Sensor board")?;
    h.snapshot("Sensor board")?;
    let revision = h.engine.revision();
    h.files.request(Action::Close);
    let painted = h.shown();
    for text in [TITLE, "Save", DONT_SAVE, "Cancel"] {
        assert!(shows(&painted, text), "{text} missing: {painted:?}");
    }
    h.click("Cancel")?;
    for _ in 0..5 {
        h.frame(vec![]);
    }
    assert!(!h.closed(), "Cancel closed the window");
    assert!(!shows(&h.shown(), TITLE), "the dialog stayed");
    assert!(h.files.dirty());
    assert_eq!(h.engine.revision(), revision);
    assert_eq!(h.engine.project().name, "Sensor board");
    assert_eq!(h.recoverable()?, ["Sensor board"]);
    // Asking again asks again.
    h.files.request(Action::Close);
    assert!(shows(&h.shown(), TITLE));
    Ok(())
}

/// Don't Save closes without saving and keeps no recovery copy, as in
/// Solidcraft: the next launch offers nothing to restore.
#[test]
fn dont_save_closes_and_removes_the_recovery_copy() -> Fallible {
    let mut h = Harness::new()?;
    h.edit("Sensor board")?;
    h.snapshot("Sensor board")?;
    h.files.request(Action::Close);
    h.click(DONT_SAVE)?;
    h.until("the close", Harness::closed)?;
    // Frames still run while the window closes; none brings a snapshot back.
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(5));
        h.frame(vec![]);
    }
    assert!(h.recoverable()?.is_empty());
    // The exit that follows keeps nothing either.
    assert!(!h.files.keep_for_recovery(Duration::from_secs(1)));
    assert!(h.recoverable()?.is_empty());
    Ok(())
}

/// Save writes the project to its file, then closes.
#[test]
fn save_writes_the_file_then_closes() -> Fallible {
    let mut h = Harness::new()?;
    let path = h.dir.path().join("board.pcbcraft");
    h.files.adopt(&h.engine, Some(path.clone()), false)?;
    h.edit("Saved on quit")?;
    h.files.request(Action::Close);
    h.click("Save")?;
    h.until("the close", Harness::closed)?;
    let saved = pcbcraft_io::read_project(&path)?;
    assert_eq!(saved.project.name, "Saved on quit");
    assert!(!h.files.dirty());
    Ok(())
}

/// Quitting while a save is being written waits for it, then quits; it is
/// neither lost nor answered twice.
#[test]
fn quitting_during_a_save_quits_once_it_is_written() -> Fallible {
    let mut h = Harness::new()?;
    let path = h.dir.path().join("board.pcbcraft");
    h.files.adopt(&h.engine, Some(path.clone()), false)?;
    h.edit("Written first")?;
    h.files.request(Action::Save);
    assert!(h.files.busy());
    h.files.request(Action::Close);
    h.until("the close", Harness::closed)?;
    assert_eq!(
        pcbcraft_io::read_project(&path)?.project.name,
        "Written first"
    );
    Ok(())
}

/// R7-4: a save says it runs from the request until its file is written,
/// so one the system holds (a permission prompt) is not silent.
#[test]
fn a_save_says_it_runs_until_it_is_written() -> Fallible {
    let mut h = Harness::new()?;
    let path = h.dir.path().join("board.pcbcraft");
    h.files.adopt(&h.engine, Some(path.clone()), false)?;
    assert!(!h.files.saving());
    h.edit("Saving")?;
    h.files.request(Action::Save);
    assert!(h.files.saving());
    h.until("the write", |h| !h.files.saving())?;
    assert!(!h.files.dirty());
    assert_eq!(pcbcraft_io::read_project(&path)?.project.name, "Saving");
    Ok(())
}

/// The system ending the app without asking (Dock › Quit on macOS, a
/// logout) keeps the latest edit for recovery, even one made less than
/// the snapshot delay before.
#[test]
fn an_unanswered_exit_keeps_the_latest_edit_for_recovery() -> Fallible {
    let mut h = Harness::new()?;
    h.edit("First")?;
    h.snapshot("First")?;
    h.edit("Latest")?;
    assert!(h.files.keep_for_recovery(Duration::from_secs(5)));
    assert_eq!(h.recoverable()?, ["Latest"]);
    Ok(())
}

/// Undo back to the saved content clears the unsaved state; redo away from
/// it sets it again. Nothing is asked on quit at the saved content.
#[test]
fn undo_to_the_saved_content_is_not_unsaved() -> Fallible {
    let mut h = Harness::new()?;
    let path = h.dir.path().join("board.pcbcraft");
    h.files.adopt(&h.engine, Some(path), false)?;
    h.edit("Edited")?;
    h.engine.undo()?;
    h.frame(vec![]);
    assert!(!h.files.dirty(), "undo to the saved content");
    h.engine.redo()?;
    h.frame(vec![]);
    assert!(h.files.dirty(), "redo away from it");
    h.engine.undo()?;
    h.frame(vec![]);
    h.files.request(Action::Close);
    assert!(!shows(&h.shown(), TITLE));
    h.until("the close", Harness::closed)
}

/// Precision run finding 3: opening a published component for editing
/// drafts it, but the project is not unsaved until the draft is edited.
#[test]
fn an_unedited_draft_is_not_unsaved() -> Fallible {
    let mut h = Harness::new()?;
    // Start from a saved project without drafts.
    let ids: Vec<_> = h.engine.drafts().items.iter().map(|d| d.id).collect();
    for id in ids {
        h.engine.apply(
            h.engine.revision(),
            Operation::Library(pcbcraft_core::LibraryCommand::DiscardDraft { id }),
        )?;
    }
    let path = h.dir.path().join("board.pcbcraft");
    h.files.adopt(&h.engine, Some(path), false)?;
    h.frame(vec![]);
    assert!(!h.files.dirty());
    let published = h
        .engine
        .project()
        .library
        .latest_components()
        .into_iter()
        .find(|c| h.engine.drafts().get(c.id).is_none())
        .map(|c| pcbcraft_core::ComponentRef::new(c.id, c.revision))
        .ok_or("a published component without a draft")?;
    let before = h.engine.revision();
    crate::part_editor::run(&mut h.engine, crate::part_editor::Request::Draft(published))?;
    assert!(h.engine.revision() > before, "the draft was created");
    h.frame(vec![]);
    assert!(!h.files.dirty(), "an unedited draft is not unsaved work");
    let mut draft = h
        .engine
        .drafts()
        .get(published.id)
        .cloned()
        .ok_or("no draft")?;
    draft.meta.name.push_str(" edited");
    h.engine.apply(
        h.engine.revision(),
        Operation::Library(pcbcraft_core::LibraryCommand::Edit {
            edits: vec![pcbcraft_core::library::DraftEdit {
                id: draft.id,
                meta: draft.meta,
                definition: draft.definition,
            }],
        }),
    )?;
    h.frame(vec![]);
    assert!(h.files.dirty(), "an edited draft is");
    Ok(())
}

/// A clean quit leaves no file of this session behind, not even the empty
/// reservation made at launch: after a project was opened, with Don't
/// Save, and with nothing to save.
#[test]
fn a_clean_quit_leaves_no_recovery_file() -> Fallible {
    for edit in [false, true] {
        let mut h = Harness::new()?;
        // The worker's store is ready (it makes the folder) at launch.
        h.until("the recovery folder", |h| {
            h.dir.path().join("recovery").is_dir()
        })?;
        assert_eq!(h.recovery_files()?, Vec::<PathBuf>::new(), "at launch");
        let path = h.dir.path().join("board.pcbcraft");
        h.files.adopt(&h.engine, Some(path), false)?;
        h.frame(vec![]);
        if edit {
            h.edit("Discarded")?;
        }
        h.files.request(Action::Close);
        if edit {
            h.click(DONT_SAVE)?;
        }
        h.until("the close", Harness::closed)?;
        assert!(!h.files.keep_for_recovery(Duration::from_secs(1)));
        assert_eq!(h.recovery_files()?, Vec::<PathBuf>::new(), "edit: {edit}");
    }
    Ok(())
}

/// Without a recovery folder (no home, a relative override) the app says
/// at startup that recovery is off, rather than keeping snapshots in a
/// temporary folder the system may empty.
#[test]
fn a_missing_recovery_folder_is_reported_at_startup() -> Fallible {
    let ctx = egui::Context::default();
    let engine = Engine::demo()?;
    let reason = pcbcraft_io::settings::UserDirs::recovery_from_vars(|_| None)
        .err()
        .map(|e| e.to_string())
        .ok_or("no variables still found a recovery folder")?;
    let files = FileUi::with_recovery(&ctx, rfd::AsyncFileDialog::new(), &engine, Err(reason));
    let mut h = Harness {
        ctx,
        files,
        engine,
        dir: tempfile::tempdir()?,
        now: 0.0,
        effects: Vec::new(),
    };
    h.until("the startup report", |h| h.files.status.is_some())?;
    let (level, text) = h.files.take_status().ok_or("no status")?;
    assert_eq!(level, Level::Error);
    assert!(
        text.starts_with("Recovery unavailable: no recovery directory")
            && text.contains("not kept"),
        "{text}"
    );
    assert!(h.files.recoveries.is_empty());
    Ok(())
}

/// More sessions than a listing reads: the newest are offered, the
/// question says how many older ones it left out, and one button discards
/// exactly those.
#[test]
fn older_sessions_past_the_limit_are_named_and_discardable() -> Fallible {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("recovery");
    let store = RecoveryStore::new(&folder)?;
    store.write(&EditorFile::from_document(Engine::demo()?.document()), None)?;
    let first = store.path().ok_or("no snapshot")?.to_path_buf();
    let base = std::time::SystemTime::now() - Duration::from_secs(100_000);
    let mut paths = Vec::new();
    for n in 0..recovery::MAX_ENTRIES + 3 {
        let path = folder.join(format!("session-n{n:04}.pcbcraft-recovery"));
        std::fs::copy(&first, &path)?;
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(base + Duration::from_secs(n as u64 * 10))?;
        paths.push(path);
    }
    recovery::remove_entry(&folder, &first)?;
    let mut h = Harness::in_dir(dir)?;
    h.until("the recovery question", |h| !h.files.recoveries.is_empty())?;
    assert_eq!(h.files.recoveries.len(), recovery::MAX_ENTRIES);
    assert_eq!(h.files.older_recoveries.len(), 3);
    assert!(shows(&h.shown(), &older_sessions_note(3)));
    h.click(DISCARD_OLDER)?;
    h.until("the older sessions discarded", |h| {
        h.files.older_recoveries.is_empty() && !h.files.recovery_choice_pending
    })?;
    for path in &paths[..3] {
        assert!(!path.exists(), "{}", path.display());
    }
    assert!(paths[3..].iter().all(|p| p.exists()));
    assert_eq!(
        h.files.recoveries.len(),
        recovery::MAX_ENTRIES,
        "still offered"
    );
    Ok(())
}
