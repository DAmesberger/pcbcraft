//! The board's exact export (slice A5 of docs/architecture/
//! board-plan-and-interface.md): the tiers, progress and cancel, and the
//! per-export volume check.
//!
//! **Tiers** ([`Tier`]). The *interface* is the slots alone and is only
//! published (`interface.publish`, [`crate::interface`]). *Mechanical*, the
//! default, is the laminate, the pads and the parts; *full* adds every
//! copper body (tracks, vias and fills). [`section`] is the board each tier
//! builds, for a publish's section 2 and for [`board`] alike.
//!
//! **Progress and cancel** ([`Watch`]). A build counts each Solidcraft
//! command on its watch and says which stage it is in; another thread reads
//! [`Watch::progress`] and may [`Watch::cancel`]. A cancelled export stops
//! at its next command or checked body ([`Error::Cancelled`]) and writes
//! nothing: one command (the board's evaluation, the save) runs to its end
//! first, and a save that ends after the cancel is removed.
//!
//! **The volume check** ([`check`]). After every mechanical or full build,
//! before anything is written, the built flat bodies are compared with the
//! board plan they were built from ([`plan::check`]): each planned body
//! built once, of its kind, source and layer, its box within
//! [`plan::PARITY_MM`] and its volume within [`plan::PARITY_VOLUME`], and no
//! flat body built that the plan lacks. The report names every body that
//! differs ([`Report::lines`]); the file is written all the same, so the
//! user has the model and knows where it is wrong.
use crate::project::{self, NativeModel, plan};
use crate::{Error, Result};
use pcbcraft_core::model::Project;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

pub use crate::project::plan::{Check, Difference, Problem, Source};
pub use pcbcraft_core::model::interface::Tier;

/// Differences a report keeps whole; the rest are counted.
pub const MAX_REPORTED: usize = 200;

/// Where an export is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Not started.
    Waiting,
    /// Issuing the model's commands.
    Building,
    /// Evaluating the board's design (one command; it cannot stop inside).
    Evaluating,
    /// Comparing the built bodies with the board plan.
    Checking,
    /// Writing the file (one command; it cannot stop inside).
    Saving,
    /// Finished, or stopped.
    Done,
}

impl Stage {
    const ALL: [Self; 6] = [
        Self::Waiting,
        Self::Building,
        Self::Evaluating,
        Self::Checking,
        Self::Saving,
        Self::Done,
    ];

    fn index(self) -> u8 {
        match self {
            Self::Waiting => 0,
            Self::Building => 1,
            Self::Evaluating => 2,
            Self::Checking => 3,
            Self::Saving => 4,
            Self::Done => 5,
        }
    }

    fn of(index: u8) -> Self {
        Self::ALL
            .get(usize::from(index))
            .copied()
            .unwrap_or(Self::Done)
    }
}

/// An export's progress and cancel, shared between the thread that builds
/// and the one that shows it.
#[derive(Debug, Default)]
pub struct Watch {
    cancelled: AtomicBool,
    stage: AtomicU8,
    commands: AtomicU64,
    checked: AtomicU64,
    to_check: AtomicU64,
}

/// What [`Watch::progress`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub stage: Stage,
    /// Solidcraft commands run so far.
    pub commands: u64,
    /// Bodies compared with the plan, of `to_check`.
    pub checked: u64,
    pub to_check: u64,
    pub cancelled: bool,
}

impl Progress {
    /// The progress as the user reads it.
    pub fn text(&self) -> String {
        let doing = match self.stage {
            Stage::Waiting => "Starting".to_owned(),
            Stage::Building => format!("Building the model · {} commands", self.commands),
            Stage::Evaluating => "Evaluating the board".to_owned(),
            Stage::Checking => format!(
                "Checking bodies against the board · {} of {}",
                self.checked, self.to_check
            ),
            Stage::Saving => "Writing the file".to_owned(),
            Stage::Done => "Finished".to_owned(),
        };
        if self.cancelled && self.stage != Stage::Done {
            format!("{doing} · stopping")
        } else {
            doing
        }
    }
}

impl Watch {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Stop the export at its next command or checked body.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn progress(&self) -> Progress {
        Progress {
            stage: Stage::of(self.stage.load(Ordering::Relaxed)),
            commands: self.commands.load(Ordering::Relaxed),
            checked: self.checked.load(Ordering::Relaxed),
            to_check: self.to_check.load(Ordering::Relaxed),
            cancelled: self.is_cancelled(),
        }
    }

    fn stopped(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Before each Solidcraft command ([`crate::native::Native`]).
    pub(crate) fn command(&self) -> Result<()> {
        self.stopped()?;
        self.commands.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Enter `stage`; refused once cancelled.
    pub(crate) fn enter(&self, stage: Stage) -> Result<()> {
        self.stopped()?;
        self.stage.store(stage.index(), Ordering::Relaxed);
        Ok(())
    }

    fn finish(&self) {
        self.stage.store(Stage::Done.index(), Ordering::Relaxed);
    }
}

/// The board a tier builds after its slots: none for the interface; the
/// board without tracks, vias and fills for the mechanical tier; all of it
/// for the full tier.
pub fn section(project: &Project, tier: Tier) -> Option<Project> {
    match tier {
        Tier::Interface => None,
        Tier::Full => Some(project.clone()),
        Tier::Mechanical => {
            let mut board = project.clone();
            board.tracks = Default::default();
            board.vias = Default::default();
            board.fills = Default::default();
            Some(board)
        }
    }
}

/// Check `model`, built from `board`, against `board`'s plan, counting on
/// `watch` and stopping when it is cancelled.
pub fn check(board: &Project, model: &NativeModel, watch: &Watch) -> Result<Check> {
    watch.enter(Stage::Checking)?;
    let plan = plan::of(board)?;
    let to_check = model
        .native_geometry()?
        .iter()
        .filter(|g| plan::is_flat(g.kind))
        .count();
    watch.to_check.store(
        u64::try_from(to_check).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    watch.checked.store(0, Ordering::Relaxed);
    let mut step = || {
        watch.stopped()?;
        watch.checked.fetch_add(1, Ordering::Relaxed);
        Ok(())
    };
    plan::check(&plan, model, &mut step)
}

/// What an export wrote and found.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub tier: Tier,
    /// Bodies in the written design (the board's; an interface's
    /// reference bodies not counted).
    pub bodies: usize,
    /// The volume check; `None` for the interface tier, which builds no
    /// board.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
    /// Differences past [`MAX_REPORTED`], left out of `check`.
    #[serde(skip_serializing_if = "is_zero")]
    pub more_differences: usize,
    /// Milliseconds spent building, checking and saving (`None` where the
    /// platform has no clock).
    pub ms: Timings,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Timings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save: Option<f64>,
}

impl Report {
    /// Whether any built body differs from the plan.
    pub fn differs(&self) -> bool {
        self.more_differences > 0
            || self
                .check
                .as_ref()
                .is_some_and(|c| !c.differences.is_empty())
    }

    /// How many bodies differ from the plan.
    pub fn difference_count(&self) -> usize {
        self.check
            .as_ref()
            .map_or(0, |c| c.differences.len())
            .saturating_add(self.more_differences)
    }

    /// Each differing body in the user's words, naming its object as the
    /// document does ("Track 12 on the top layer: built, but not as
    /// planned").
    pub fn lines(&self, project: &Project) -> Vec<String> {
        self.check
            .iter()
            .flat_map(|c| &c.differences)
            .map(|d| describe(project, d))
            .collect()
    }
}

/// One difference in the user's words.
pub fn describe(project: &Project, difference: &Difference) -> String {
    let object = match difference.source {
        Source::Board => "The laminate".to_owned(),
        Source::Pad { instance, pad } => match project.instance(instance) {
            Some(i) => format!("Pad {pad} of {}", i.designator),
            None => format!("Pad {pad} of part {instance}"),
        },
        Source::Track { track } => format!("Track {track}"),
        Source::Via { via } => format!("Via {via}"),
        Source::Fill { fill } => format!("Fill {fill}"),
    };
    let layer = match difference.layer {
        Some(pcbcraft_core::CopperLayer::Top) => " on the top layer".to_owned(),
        Some(pcbcraft_core::CopperLayer::Bottom) => " on the bottom layer".to_owned(),
        Some(pcbcraft_core::CopperLayer::Inner(n)) => format!(" on inner layer {n}"),
        None => String::new(),
    };
    let what = kind_name(difference.kind);
    let how = match (difference.problem, difference.instead.first()) {
        (Problem::PlannedNotBuilt, Some(built)) => {
            let planned = &difference.measure;
            let off = planned
                .min
                .iter()
                .chain(&planned.max)
                .zip(built.min.iter().chain(&built.max))
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f64::max);
            let volume = match (planned.volume, built.volume) {
                (Some(a), Some(b)) if a.abs() > 0.0 => {
                    format!(", volume {:+.3} %", (b - a) / a * 100.0)
                }
                _ => String::new(),
            };
            format!("its {what} was built in another shape (box off by up to {off:.4} mm{volume})")
        }
        (Problem::PlannedNotBuilt, None) => format!("its {what} is missing from the model"),
        (Problem::BuiltNotPlanned, _) => format!("the model has a {what} the board does not"),
    };
    format!("{object}{layer}: {how}")
}

fn kind_name(kind: crate::material::BodyKind) -> &'static str {
    use crate::material::BodyKind as K;
    match kind {
        K::Substrate => "laminate slab",
        K::PadCopper => "pad copper",
        K::CopperTrack => "track copper",
        K::ViaCopper => "via copper",
        K::FillCopper => "fill copper",
        _ => "body",
    }
}

#[cfg(not(target_arch = "wasm32"))]
type Clock = std::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
fn clock() -> Option<Clock> {
    Some(std::time::Instant::now())
}

#[cfg(target_arch = "wasm32")]
type Clock = ();

#[cfg(target_arch = "wasm32")]
fn clock() -> Option<Clock> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn since(start: Option<Clock>) -> Option<f64> {
    start.map(|s| s.elapsed().as_secs_f64() * 1e3)
}

#[cfg(target_arch = "wasm32")]
fn since(_: Option<Clock>) -> Option<f64> {
    None
}

/// Export `project`'s board at `tier` as a new `.solidcraft` file at
/// `path`: build the exact model, check it against the board plan, then
/// write it (to a hidden file beside `path`, moved into place when it is
/// whole). Refuses an existing file before building, and the interface
/// tier, which is published with `interface.publish`. Stops with
/// [`Error::Cancelled`], writing nothing, once `watch` is cancelled.
pub fn board(project: &Project, tier: Tier, path: &str, watch: &Arc<Watch>) -> Result<Report> {
    let result = board_inner(project, tier, path, watch);
    watch.finish();
    result
}

fn board_inner(project: &Project, tier: Tier, path: &str, watch: &Arc<Watch>) -> Result<Report> {
    crate::native::validate_path(path)?;
    if Path::new(path).try_exists()? {
        return Err(Error::Invalid(
            "output exists; choose a new .solidcraft filename".into(),
        ));
    }
    let board = section(project, tier).ok_or_else(|| {
        Error::Invalid(
            "export.tier: the interface tier is published with interface.publish; \
             export mechanical or full"
                .into(),
        )
    })?;
    watch.enter(Stage::Building)?;
    let started = clock();
    let model = project::build_watched(&board, Arc::clone(watch))?;
    let build = since(started);
    write(&board, &model, tier, path, watch, build)
}

/// Check `model` against the plan of `board` (the board it was built
/// from), then save it whole at `path`; the report says both.
fn write(
    board: &Project,
    model: &NativeModel,
    tier: Tier,
    path: &str,
    watch: &Watch,
    build: Option<f64>,
) -> Result<Report> {
    let started = clock();
    let checked = check(board, model, watch)?;
    let check_ms = since(started);
    watch.enter(Stage::Saving)?;
    let started = clock();
    save_whole(model, path, watch)?;
    let save = since(started);
    let bodies = model.native_geometry()?.len();
    Ok(report(
        tier,
        bodies,
        Some(checked),
        Timings {
            build,
            check: check_ms,
            save,
        },
    ))
}

/// A report of `check`, its differences past [`MAX_REPORTED`] counted.
pub fn report(tier: Tier, bodies: usize, check: Option<Check>, ms: Timings) -> Report {
    let mut more = 0;
    let check = check.map(|mut c| {
        more = c.differences.len().saturating_sub(MAX_REPORTED);
        c.differences.truncate(MAX_REPORTED);
        c
    });
    Report {
        tier,
        bodies,
        check,
        more_differences: more,
        ms,
    }
}

/// Save `model` whole at `path`, which must not exist: written to a
/// hidden sibling, then linked into place, which refuses a file that
/// appeared meanwhile. A save that ends after `watch` was cancelled is
/// removed.
fn save_whole(model: &NativeModel, path: &str, watch: &Watch) -> Result<()> {
    let target = Path::new(path);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| Error::Invalid("the export path has no file name".into()))?;
    let pid = std::process::id();
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    let temporary = target.with_file_name(format!(".{name}.exporting-{pid}-{serial}.solidcraft"));
    let temporary_text = temporary
        .to_str()
        .ok_or_else(|| Error::Invalid("the export path must be UTF-8".into()))?;
    let _ = std::fs::remove_file(&temporary);
    if let Err(error) = model.save(temporary_text) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if watch.is_cancelled() {
        let _ = std::fs::remove_file(&temporary);
        return Err(Error::Cancelled);
    }
    // A hard link never replaces a file; where links are not supported,
    // a rename after one more look.
    let placed = match std::fs::hard_link(&temporary, target) {
        Ok(()) => {
            let _ = std::fs::remove_file(&temporary);
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(Error::Invalid(
            "output exists; choose a new .solidcraft filename".into(),
        )),
        Err(_) if !target.try_exists()? => std::fs::rename(&temporary, target).map_err(Error::from),
        Err(_) => Err(Error::Invalid(
            "output exists; choose a new .solidcraft filename".into(),
        )),
    };
    if placed.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    placed
}

/// Tells this process's temporary export files apart.
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests;
