//! The part-file chooser (docs/mechanical-parts.md M5): one Solidcraft-
//! styled sheet for every place that picks a SolidCraft part file. The
//! source list on the left offers the built-in part files
//! (`part_file::BUILTIN`), the project's Models of format `solidcraft`, the
//! recent files (at most eight, kept per user) and Open file… (the native
//! picker). The detail pane on the right shows the check report of the
//! highlighted source at the edited values: findings (blocking in red,
//! advisory in amber), the user parameters as a form with units, the
//! bodies and their materials, and the evaluation time.
//!
//! Files are read and evaluated on a worker thread ([`Evaluator`]); the
//! pane keeps the last result up while a newer one is evaluated, and the
//! primary button waits for the check of exactly what is shown. The sheet
//! knows nothing of packages or boards: the primary returns a
//! [`ChooserPick`] (the source, the values and the checked bytes), and the
//! caller builds the one core operation with the automation builders
//! (`sparksmith_automation::part_files`), as `library.use_part_file` and the
//! placement commands do.
use crate::kit::{self, severity::Level};
use egui::{Align, Layout, Ui, pos2, vec2};
use sparksmith_automation::part_files::{self, PartFileBytes};
use sparksmith_core::ids::ContentHash;
use sparksmith_core::library::Definition;
use sparksmith_core::library::model3d::ModelFormat;
use sparksmith_core::library::package_model::PartValue;
use sparksmith_core::{Engine, ModelRef};
use sparksmith_solidcraft::models::part_file::{self, PartParameter, PartReport, PartValues};
use sparksmith_ui::theme::Tokens;
use sparksmith_ui::ui_kit::{self, Button};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

/// Recent part files kept per user.
pub const MAX_RECENT: usize = sparksmith_io::settings::MAX_RECENT_PART_FILES;
/// The id of the sheet.
const SHEET: &str = "part-chooser";
const KEYS: &str = "Click a source · Esc close";
/// Licence of Sparksmith's built-in part files.
const BUILTIN_LICENCE: &str = "MIT OR Apache-2.0";
/// Bytes of a stated licence.
const MAX_LICENCE: usize = 128;
/// The value range of a length, angle or number (`PartValue::finite`).
const VALUE_RANGE: std::ops::RangeInclusive<f64> = -10000.0..=10000.0;
/// The value range of a count.
const COUNT_RANGE: std::ops::RangeInclusive<f64> = 0.0..=10000.0;

/// Where a part file comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PartSource {
    /// A `.solidcraft` file on disk.
    Path(PathBuf),
    /// A built-in part file, by name (`esp32-s31-wroom-3`).
    Builtin(String),
    /// A Model revision of format `solidcraft` the project holds.
    Model(ModelRef),
}

/// What the primary button returns: the source, the values to use and the
/// bytes and report the check ran on (so the caller builds its operation
/// from exactly what was checked, and the evaluation is cached).
#[derive(Clone, Debug)]
pub struct ChooserPick {
    #[allow(
        dead_code,
        reason = "Place 3D part (PCB Layout) reuses the chooser; M5"
    )]
    pub source: PartSource,
    pub values: PartValues,
    pub file: Arc<PartFileBytes>,
    pub report: PartReport,
    /// The licence stated for a file the project does not hold yet; `None`
    /// for built-ins and project models.
    pub licence: Option<String>,
}

/// What the chooser is opened for: the title and the primary button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChooserMode {
    /// A package uses the part file (Component 3D, Body).
    Use,
    /// A part is placed on the board (PCB Layout, Place 3D part…).
    #[allow(
        dead_code,
        reason = "Place 3D part (PCB Layout) reuses the chooser; M5"
    )]
    Place,
}

impl ChooserMode {
    pub fn title(self) -> &'static str {
        match self {
            Self::Use => "Part file",
            Self::Place => "Place 3D part",
        }
    }

    pub fn primary(self) -> &'static str {
        match self {
            Self::Use => "Use",
            Self::Place => "Place",
        }
    }
}

/// A checked file: its bytes, their asset hash and the report.
#[derive(Clone, Debug)]
pub struct Checked {
    pub file: Arc<PartFileBytes>,
    pub hash: ContentHash,
    pub report: PartReport,
}

/// One finished evaluation of a source at some values.
#[derive(Clone, Debug)]
pub struct Evaluation {
    pub source: PartSource,
    pub values: PartValues,
    pub result: Result<Checked, String>,
}

enum Input {
    /// Read the source (a path or a built-in) on the worker.
    Read,
    /// Bytes already in hand (a project model, or the last read).
    Bytes(Arc<PartFileBytes>),
}

struct Job {
    ticket: u64,
    source: PartSource,
    input: Input,
    values: PartValues,
}

/// A message without its leading `code.name: ` (the check's codes are for
/// agents; people read the sentence).
pub fn shown_message(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.split_once(": ") {
        Some((code, rest))
            if !code.is_empty()
                && code.contains('.')
                && code
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_') =>
        {
            rest.to_owned()
        }
        _ => line.to_owned(),
    }
}

fn read(source: &PartSource) -> Result<PartFileBytes, String> {
    let read = match source {
        PartSource::Path(path) => match path.to_str() {
            Some(path) => part_files::read_part_file(Some(path), None),
            None => return Err("The file's path is not valid text".into()),
        },
        PartSource::Builtin(name) => part_files::read_part_file(None, Some(name)),
        PartSource::Model(_) => return Err("The project model was not handed over".into()),
    };
    read.map_err(|e| shown_message(&e.diagnostic().message))
}

/// Read (when asked) and evaluate one job. The defaults are evaluated too
/// for a file the project may not hold, so a later use of it (whose model
/// records the file's own bounds) finds both in the cache.
fn evaluate(job: Job) -> (u64, Evaluation) {
    let file = match job.input {
        Input::Bytes(file) => Ok(file),
        Input::Read => read(&job.source).map(Arc::new),
    };
    let result = file.and_then(|file| {
        let part = part_file::evaluate(&file.bytes, &job.values).map_err(|e| e.to_string())?;
        if !matches!(job.source, PartSource::Model(_)) && !job.values.is_empty() {
            // Only warms the cache; a failure shows with the values.
            let _ = part_file::evaluate(&file.bytes, &PartValues::new());
        }
        Ok(Checked {
            hash: sparksmith_core::library::bundle::asset_hash(&file.bytes),
            file,
            report: part.report(),
        })
    });
    (
        job.ticket,
        Evaluation {
            source: job.source,
            values: job.values,
            result: result.map_err(|e| shown_message(&e)),
        },
    )
}

/// Evaluates part files on one worker thread: only the newest request is
/// evaluated, and the last finished result stays available while a newer
/// one runs.
#[derive(Default)]
pub struct Evaluator {
    jobs: Option<Sender<Job>>,
    results: Option<Receiver<(u64, Evaluation)>>,
    ticket: u64,
    asked: Option<(PartSource, PartValues)>,
    last: Option<(u64, Evaluation)>,
}

impl Evaluator {
    fn start(&mut self, ctx: &egui::Context) {
        if self.jobs.is_some() {
            return;
        }
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, results) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            while let Ok(mut job) = inbox.recv() {
                while let Ok(newer) = inbox.try_recv() {
                    job = newer;
                }
                if outbox.send(evaluate(job)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        self.jobs = Some(jobs);
        self.results = Some(results);
    }

    /// Ask for `source` at `values`, unless that is what was asked last.
    /// A project model's bytes are taken from `engine`; a path or built-in
    /// is read on the worker, once per source.
    pub fn request(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        source: &PartSource,
        values: &PartValues,
    ) {
        if self
            .asked
            .as_ref()
            .is_some_and(|(s, v)| s == source && v == values)
        {
            return;
        }
        self.start(ctx);
        self.ticket = self.ticket.wrapping_add(1);
        self.asked = Some((source.clone(), values.clone()));
        let reused = self.last.as_ref().and_then(|(_, e)| match &e.result {
            Ok(checked) if e.source == *source => Some(Arc::clone(&checked.file)),
            _ => None,
        });
        let input = match (reused, source) {
            (Some(file), _) => Input::Bytes(file),
            (None, PartSource::Model(model)) => match part_files::model_part_file(engine, *model) {
                Ok(file) => Input::Bytes(Arc::new(file)),
                Err(error) => {
                    self.last = Some((
                        self.ticket,
                        Evaluation {
                            source: source.clone(),
                            values: values.clone(),
                            result: Err(shown_message(&error.diagnostic().message)),
                        },
                    ));
                    return;
                }
            },
            (None, _) => Input::Read,
        };
        let job = Job {
            ticket: self.ticket,
            source: source.clone(),
            input,
            values: values.clone(),
        };
        if self.jobs.as_ref().is_none_or(|j| j.send(job).is_err()) {
            self.jobs = None;
            self.last = Some((
                self.ticket,
                Evaluation {
                    source: source.clone(),
                    values: values.clone(),
                    result: Err("The part-file check stopped; reopen the chooser".into()),
                },
            ));
        }
    }

    /// Take finished results; returns whether one arrived.
    pub fn poll(&mut self) -> bool {
        let mut arrived = false;
        let Some(results) = &self.results else {
            return false;
        };
        loop {
            match results.try_recv() {
                Ok((ticket, evaluation)) => {
                    if self.last.as_ref().is_none_or(|(t, _)| *t < ticket) {
                        self.last = Some((ticket, evaluation));
                        arrived = true;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.jobs = None;
                    self.results = None;
                    break;
                }
            }
        }
        arrived
    }

    /// Block until the newest request is evaluated (tests).
    #[cfg(test)]
    pub fn wait(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while self.busy() && std::time::Instant::now() < deadline {
            if !self.poll() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    /// The last finished evaluation, whatever it was for.
    pub fn last(&self) -> Option<&Evaluation> {
        self.last.as_ref().map(|(_, e)| e)
    }

    /// The evaluation of exactly what was asked last, once it is done.
    pub fn current(&self) -> Option<&Evaluation> {
        let (source, values) = self.asked.as_ref()?;
        self.last()
            .filter(|e| e.source == *source && e.values == *values)
    }

    /// Whether the last request is still being evaluated.
    pub fn busy(&self) -> bool {
        self.asked.is_some() && self.current().is_none()
    }
}

/// How a parameter's value is typed: by its unit, and for a plain number
/// by the value already given or, failing that, whether it evaluates to a
/// whole number (a count).
fn value_kind(parameter: &PartParameter, given: Option<PartValue>) -> PartValue {
    match (parameter.unit.as_str(), given) {
        ("mm", _) => PartValue::Mm(0.0),
        ("deg", _) => PartValue::Deg(0.0),
        (_, Some(PartValue::Count(_))) => PartValue::Count(0),
        (_, Some(PartValue::Number(_))) => PartValue::Number(0.0),
        _ => match parameter.value {
            Some(v) if v >= 0.0 && v.fract() == 0.0 && v <= *COUNT_RANGE.end() => {
                PartValue::Count(0)
            }
            _ => PartValue::Number(0.0),
        },
    }
}

fn number(value: PartValue) -> f64 {
    match value {
        PartValue::Mm(v) | PartValue::Deg(v) | PartValue::Number(v) => v,
        PartValue::Count(n) => f64::from(n),
    }
}

/// `v` typed as `kind`, or `None` for a count that is not whole.
fn typed(kind: PartValue, v: f64) -> Option<PartValue> {
    match kind {
        PartValue::Mm(_) => Some(PartValue::Mm(v)),
        PartValue::Deg(_) => Some(PartValue::Deg(v)),
        PartValue::Number(_) => Some(PartValue::Number(v)),
        PartValue::Count(_) => (v.fract() == 0.0 && COUNT_RANGE.contains(&v))
            // Bounded and whole by the check above.
            .then_some(PartValue::Count(v as u32)),
    }
}

/// The unit a field shows for `kind`.
fn unit(kind: PartValue) -> &'static str {
    match kind {
        PartValue::Mm(_) => "mm",
        PartValue::Deg(_) => "°",
        PartValue::Count(_) | PartValue::Number(_) => "",
    }
}

/// The part's user parameters as one form, a number row each with its
/// unit (mm, °, a count or a plain number). Edits go to `editing`; when a
/// field is left (Enter or focus loss) and `editing` differs from
/// `committed`, the new values are returned: one commit per edit.
pub fn values_form(
    ui: &mut Ui,
    salt: &str,
    parameters: &[PartParameter],
    editing: &mut PartValues,
    committed: &PartValues,
) -> Option<PartValues> {
    let mut left = false;
    ui.push_id(("part-values", salt), |ui| {
        for parameter in parameters {
            let given = editing.get(&parameter.name).copied();
            let kind = value_kind(parameter, given);
            let mut v = given.map(number).or(parameter.value).unwrap_or(0.0);
            let range = match kind {
                PartValue::Count(_) => COUNT_RANGE,
                _ => VALUE_RANGE,
            };
            let input = kit::row(ui, &parameter.name, |ui| {
                kit::number_input(ui, &parameter.name, &mut v, unit(kind), range)
            });
            if input.changed
                && let Some(value) = typed(kind, v)
            {
                editing.insert(parameter.name.clone(), value);
            }
            if let Some(message) = &input.invalid {
                kit::field_error(ui, &input.response, message, true);
            }
            let response = input
                .response
                .on_hover_text(format!("In the file: {}", parameter.expression));
            left |= response.lost_focus();
        }
    });
    (left && editing != committed).then(|| editing.clone())
}

/// One finding row: the severity glyph (blocking red, advisory amber) and
/// the check's sentence.
fn finding_row(ui: &mut Ui, blocking: bool, message: &str) {
    let t = Tokens::get();
    let level = if blocking {
        Level::Error
    } else {
        Level::Warning
    };
    let font = egui::FontId::proportional(t.font_label);
    let galley = ui.painter().layout(
        message.to_owned(),
        font,
        t.form_ink,
        (ui.available_width() - t.space_24).max(t.space_24),
    );
    let height = galley.size().y.max(t.space_16) + t.space_6;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::hover());
    kit::severity_glyph(
        ui.painter(),
        pos2(rect.left() + t.space_8, rect.top() + t.space_3 + t.space_6),
        t.space_12,
        level,
    );
    ui.painter().galley(
        pos2(rect.left() + t.space_24, rect.top() + t.space_3),
        galley,
        t.form_ink,
    );
    let label = format!("{} {message}", level.marker());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &label));
}

/// The check report's findings, then its bodies, materials and timing.
pub fn report_rows(ui: &mut Ui, report: &PartReport) {
    if report.findings.is_empty() {
        kit::note(ui, "The check found nothing to fix");
    }
    for finding in &report.findings {
        finding_row(ui, finding.blocking, &finding.message);
    }
    kit::value_row(ui, "Bodies", &report.bodies.len().to_string());
    kit::value_row(ui, "Materials", &materials_line(report));
    let [lo, hi] = report.bounds_mm;
    kit::value_row(
        ui,
        "Size",
        &format!(
            "{} × {} × {} mm",
            kit::number_text(hi[0] - lo[0]),
            kit::number_text(hi[1] - lo[1]),
            kit::number_text(hi[2] - lo[2])
        ),
    );
    kit::value_row(ui, "Marking areas", &report.surfaces.len().to_string());
    kit::value_row(ui, "Evaluated in", &format!("{:.0} ms", report.evaluate_ms));
}

/// "gold ×107, nickel-silver ×1, 2 unmapped": the bodies per material.
pub fn materials_line(report: &PartReport) -> String {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut unmapped = 0usize;
    for body in &report.bodies {
        match body.material {
            Some(material) => {
                *counts
                    .entry(part_file::appearance_name(material))
                    .or_default() += 1;
            }
            None => unmapped += 1,
        }
    }
    let mut parts: Vec<String> = counts
        .into_iter()
        .map(|(name, n)| format!("{name} ×{n}"))
        .collect();
    if unmapped > 0 {
        parts.push(format!("{unmapped} unmapped"));
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(", ")
    }
}

/// A file size as people read it: "152 KiB".
pub fn size_label(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KiB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// One row of the source list.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRow {
    pub source: PartSource,
    pub label: String,
    pub detail: String,
}

/// The source list's groups in order: Built-in, This project, Recent files.
pub fn source_rows(engine: &Engine, recent: &[PathBuf]) -> [(&'static str, Vec<SourceRow>); 3] {
    let mut project: Vec<SourceRow> = Vec::new();
    let library = &engine.project().library;
    let mut newest: std::collections::BTreeMap<sparksmith_core::DefinitionId, ModelRef> =
        Default::default();
    for model in &library.models {
        if model.definition.format == ModelFormat::Solidcraft {
            let candidate = ModelRef::new(model.id, model.revision);
            newest
                .entry(model.id)
                .and_modify(|r| {
                    if r.revision < candidate.revision {
                        *r = candidate;
                    }
                })
                .or_insert(candidate);
        }
    }
    for draft in &engine.drafts().items {
        if let Definition::Model(m) = &draft.definition
            && m.format == ModelFormat::Solidcraft
            && let Some(revision) = draft.pending()
        {
            newest.insert(draft.id, ModelRef::new(draft.id, revision));
        }
    }
    for model in newest.into_values() {
        if let Some((name, definition)) = crate::model_library::model_definition(engine, model) {
            let published = library.model(model).is_some();
            project.push(SourceRow {
                source: PartSource::Model(model),
                label: if name.trim().is_empty() {
                    definition.asset.file_name.clone()
                } else {
                    name.to_owned()
                },
                detail: format!(
                    "{} · {} · {}",
                    definition.asset.file_name,
                    if published {
                        format!("r{}", model.revision)
                    } else {
                        "draft".into()
                    },
                    size_label(definition.asset.bytes)
                ),
            });
        }
    }
    let recent = recent
        .iter()
        .map(|path| SourceRow {
            source: PartSource::Path(path.clone()),
            label: path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into(),
            ),
            detail: path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        })
        .collect();
    [
        ("Built-in", builtin_rows()),
        ("This project", project),
        ("Recent files", recent),
    ]
}

fn builtin_rows() -> Vec<SourceRow> {
    part_file::BUILTIN
        .iter()
        .map(|(name, bytes)| SourceRow {
            source: PartSource::Builtin((*name).to_owned()),
            label: (*name).to_owned(),
            detail: format!(
                "Built-in · {} · {}",
                BUILTIN_LICENCE,
                size_label(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            ),
        })
        .collect()
}

/// The chooser's state; closed by default.
#[derive(Default)]
pub struct PartChooser {
    mode: Option<ChooserMode>,
    selected: Option<PartSource>,
    /// The values the check last ran with (committed fields).
    values: PartValues,
    /// The values as edited.
    editing: PartValues,
    licence: String,
    recent: Vec<PathBuf>,
    recent_changed: bool,
    evaluator: Evaluator,
    picking: Option<Receiver<Option<PathBuf>>>,
    refusal: Option<String>,
}

impl PartChooser {
    /// Open the sheet for `mode`, starting at `source` when given (the
    /// part file in use), else at the first built-in.
    pub fn open(&mut self, mode: ChooserMode, source: Option<PartSource>, values: PartValues) {
        self.mode = Some(mode);
        self.refusal = None;
        let source = source.or_else(|| {
            part_file::BUILTIN
                .first()
                .map(|(n, _)| PartSource::Builtin((*n).to_owned()))
        });
        match source {
            Some(source) => self.select_with(source, values),
            None => self.selected = None,
        }
    }

    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "Place 3D part (PCB Layout) reuses the chooser; M5"
        )
    )]
    pub fn is_open(&self) -> bool {
        self.mode.is_some()
    }

    pub fn close(&mut self) {
        self.mode = None;
        self.refusal = None;
    }

    /// The caller could not apply the pick: the reason shows as the
    /// sheet's refusal line, and the sheet stays open.
    pub fn refused(&mut self, message: String) {
        self.refusal = Some(message);
    }

    /// Highlight `source` with its default values.
    pub fn select(&mut self, source: PartSource) {
        self.select_with(source, PartValues::new());
    }

    fn select_with(&mut self, source: PartSource, values: PartValues) {
        if self.selected.as_ref() != Some(&source) {
            self.licence.clear();
        }
        self.selected = Some(source);
        self.values = values.clone();
        self.editing = values;
        self.refusal = None;
    }

    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "Place 3D part (PCB Layout) reuses the chooser; M5"
        )
    )]
    pub fn selected(&self) -> Option<&PartSource> {
        self.selected.as_ref()
    }

    /// The recent part files kept per user, most recent first.
    pub fn set_recent(&mut self, recent: &[PathBuf]) {
        if !self.recent_changed && self.recent != recent {
            self.recent = recent.iter().take(MAX_RECENT).cloned().collect();
        }
    }

    /// The recent files after Open file… changed them, once, to keep.
    pub fn take_recent_changed(&mut self) -> Option<Vec<PathBuf>> {
        std::mem::take(&mut self.recent_changed).then(|| self.recent.clone())
    }

    /// `path` first among the recent files.
    pub fn remember(&mut self, path: PathBuf) {
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
        self.recent_changed = true;
    }

    /// The evaluation shown (the last one finished).
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "Place 3D part (PCB Layout) reuses the chooser; M5"
        )
    )]
    pub fn evaluation(&self) -> Option<&Evaluation> {
        self.evaluator.last()
    }

    #[cfg(test)]
    pub fn wait(&mut self) {
        self.evaluator.wait();
    }

    fn start_picking(&mut self, ctx: &egui::Context) {
        if self.picking.is_some() {
            return;
        }
        let future = rfd::AsyncFileDialog::new()
            .set_title("Open a part file")
            .add_filter("SolidCraft part file", &["solidcraft"])
            .pick_file();
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let picked = pollster::block_on(future).map(|h| h.path().to_path_buf());
            let _ = sender.send(picked);
            ctx.request_repaint();
        });
        self.picking = Some(receiver);
    }

    fn poll_picking(&mut self) {
        let Some(picking) = &self.picking else {
            return;
        };
        match picking.try_recv() {
            Ok(picked) => {
                self.picking = None;
                if let Some(path) = picked {
                    self.remember(path.clone());
                    self.select(PartSource::Path(path));
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.picking = None,
        }
    }

    /// Whether the project already holds a file of `hash` as a part-file
    /// Model (then no licence needs stating).
    fn held(engine: &Engine, hash: ContentHash) -> bool {
        let library = &engine.project().library;
        library.models.iter().any(|m| {
            m.definition.asset.hash == hash && m.definition.format == ModelFormat::Solidcraft
        }) || engine.drafts().items.iter().any(|d| {
            matches!(&d.definition, Definition::Model(m)
                    if m.asset.hash == hash && m.format == ModelFormat::Solidcraft)
        })
    }

    /// Show the sheet when open. Returns the pick when the primary button
    /// is pressed; the sheet stays open until the caller closes it (or
    /// reports a refusal with [`Self::refused`]).
    pub fn show(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<ChooserPick> {
        let mode = self.mode?;
        self.poll_picking();
        self.evaluator.poll();
        // The sheet owns the search keys while it is open: Esc closes it,
        // and Enter leaves the edited field, which commits it.
        let keys = kit::search_keys(ctx);
        if keys.escape {
            self.close();
            return None;
        }
        if keys.enter {
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
        }
        if let Some(source) = self.selected.clone() {
            self.evaluator.request(ctx, engine, &source, &self.values);
        }
        let t = Tokens::get();
        let area = ctx.content_rect();
        let width = (t.form_card_width * 2.0 + t.space_24).min(area.width() - t.space_16);
        let top = pos2(area.center().x - width / 2.0, area.top() + t.space_24 * 2.0);
        let height = (area.height() - t.space_24 * 7.0).max(t.space_24 * 8.0);
        let groups = source_rows(engine, &self.recent);
        let current = self.evaluator.current().cloned();
        let shown = self.evaluator.last().cloned();
        let busy = self.evaluator.busy();
        let needs_licence = matches!(self.selected, Some(PartSource::Path(_)))
            && current
                .as_ref()
                .and_then(|e| e.result.as_ref().ok())
                .is_some_and(|c| !Self::held(engine, c.hash));
        let mut clicked = None;
        let mut open_file = false;
        let mut cancel = false;
        let mut primary = false;
        let mut committed = None;
        let selected = self.selected.clone();
        let picking = self.picking.is_some();
        kit::search_sheet(ctx, SHEET, top, width, KEYS, |ui| {
            kit::search_heading(ui, mode.title(), groups.iter().map(|(_, r)| r.len()).sum());
            let columns = ui.available_width();
            let left = (columns * 0.42).max(0.0);
            let right = (columns - left - t.space_12).max(0.0);
            ui.allocate_ui_with_layout(
                vec2(columns, height),
                Layout::left_to_right(Align::Min),
                |ui| {
                    ui.allocate_ui_with_layout(
                        vec2(left, height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(left);
                            egui::ScrollArea::vertical()
                                .id_salt("part-chooser-sources")
                                .max_height(height - t.control_height - t.space_8)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    for (group, rows) in &groups {
                                        kit::search_heading(ui, group, rows.len());
                                        if rows.is_empty() {
                                            kit::note(
                                                ui,
                                                match *group {
                                                    "This project" => {
                                                        "No part file in this project yet"
                                                    }
                                                    "Recent files" => "Files you open show here",
                                                    _ => "None",
                                                },
                                            );
                                        }
                                        for (index, row) in rows.iter().enumerate() {
                                            let search = kit::SearchRow {
                                                icon: match row.source {
                                                    PartSource::Path(_) => "folder",
                                                    PartSource::Builtin(_) => "component",
                                                    PartSource::Model(_) => "library",
                                                },
                                                label: &row.label,
                                                query: "",
                                                detail: &row.detail,
                                                badges: &[],
                                                highlighted: selected.as_ref() == Some(&row.source),
                                            };
                                            let response = kit::search_row(
                                                ui,
                                                egui::Id::unique(SHEET).with((*group, index)),
                                                &search,
                                            );
                                            if response.clicked() {
                                                clicked = Some(row.source.clone());
                                            }
                                        }
                                    }
                                });
                            kit::actions(ui, |ui| {
                                let response =
                                    ui_kit::button(ui, "Open file…", Button::Enabled(!picking));
                                if response
                                    .on_hover_text("Open a .solidcraft part file from disk")
                                    .clicked()
                                {
                                    open_file = true;
                                }
                            });
                        },
                    );
                    ui.add_space(t.space_12);
                    ui.allocate_ui_with_layout(
                        vec2(right, height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(right);
                            egui::ScrollArea::vertical()
                                .id_salt("part-chooser-detail")
                                .max_height(height - t.control_height - t.space_8)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    committed = self.detail(
                                        ui,
                                        engine,
                                        shown.as_ref(),
                                        busy,
                                        needs_licence,
                                    );
                                });
                            let ready = current.as_ref().and_then(|e| e.result.as_ref().ok());
                            let why_not = match (ready, busy) {
                                (_, true) => Some("Checking the part file…"),
                                (None, false) if selected.is_none() => Some("Choose a part file"),
                                (None, false) => Some("The part file could not be read"),
                                (Some(c), false) if !c.report.ok => {
                                    Some("Fix the blocking findings first")
                                }
                                (Some(_), false)
                                    if needs_licence && self.licence.trim().is_empty() =>
                                {
                                    Some("State the file's licence first")
                                }
                                _ => None,
                            };
                            kit::actions(ui, |ui| {
                                let response = kit::primary(ui, mode.primary(), why_not.is_none());
                                let response = match why_not {
                                    Some(why) => response.on_disabled_hover_text(why),
                                    None => response,
                                };
                                if response.clicked() {
                                    primary = true;
                                }
                                if ui_kit::button(ui, "Cancel", Button::Ghost).clicked() {
                                    cancel = true;
                                }
                            });
                        },
                    );
                },
            );
        });
        if let Some(values) = committed {
            self.values = values;
            self.refusal = None;
        }
        if open_file {
            self.start_picking(ctx);
        }
        if cancel {
            self.close();
            return None;
        }
        if let Some(source) = clicked
            && self.selected.as_ref() != Some(&source)
        {
            self.select(source);
            return None;
        }
        if !primary {
            return None;
        }
        let evaluation = self.evaluator.current()?;
        let checked = evaluation.result.as_ref().ok()?;
        Some(ChooserPick {
            source: evaluation.source.clone(),
            values: evaluation.values.clone(),
            file: Arc::clone(&checked.file),
            report: checked.report.clone(),
            licence: needs_licence.then(|| self.licence.trim().to_owned()),
        })
    }

    /// The detail pane: the file, the check, the parameters form and the
    /// refusal line. Returns newly committed values.
    fn detail(
        &mut self,
        ui: &mut Ui,
        engine: &Engine,
        shown: Option<&Evaluation>,
        busy: bool,
        needs_licence: bool,
    ) -> Option<PartValues> {
        let Some(selected) = self.selected.clone() else {
            kit::note(ui, "Choose a part file on the left");
            return None;
        };
        let shown = shown.filter(|e| e.source == selected);
        let file = shown.and_then(|e| e.result.as_ref().ok());
        let (name, licence) = match &selected {
            PartSource::Builtin(name) => (
                format!("{name}.solidcraft"),
                Some(BUILTIN_LICENCE.to_owned()),
            ),
            PartSource::Model(model) => {
                match crate::model_library::model_definition(engine, *model) {
                    Some((_, d)) => (
                        d.asset.file_name.clone(),
                        Some(
                            d.provenance
                                .record
                                .licence
                                .clone()
                                .unwrap_or_else(|| "not recorded".into()),
                        ),
                    ),
                    None => ("Missing model".into(), None),
                }
            }
            PartSource::Path(path) => (
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into(),
                ),
                None,
            ),
        };
        kit::value_row(ui, "File", &name);
        if let Some(file) = file {
            kit::value_row(
                ui,
                "File size",
                &size_label(u64::try_from(file.file.bytes.len()).unwrap_or(u64::MAX)),
            );
        }
        match licence {
            Some(licence) => kit::value_row(ui, "Licence", &licence),
            None if needs_licence => {
                kit::text_row(ui, "Licence", &mut self.licence).on_hover_text(
                    "The file's licence as SPDX (MIT, CC-BY-4.0, …); recorded on the project's model",
                );
                if self.licence.len() > MAX_LICENCE {
                    let mut end = MAX_LICENCE;
                    while !self.licence.is_char_boundary(end) {
                        end -= 1;
                    }
                    self.licence.truncate(end);
                }
            }
            None => {}
        }
        if busy {
            kit::note(ui, "Checking…");
        }
        let mut committed = None;
        match shown.map(|e| &e.result) {
            None => {}
            Some(Err(error)) => {
                kit::refusal(ui, error);
            }
            Some(Ok(checked)) => {
                let report = &checked.report;
                kit::section(
                    ui,
                    "part-chooser-check",
                    "Check",
                    true,
                    Some(report.findings.len()),
                    |ui| {
                        report_rows(ui, report);
                    },
                );
                let parameters = report.parameters.clone();
                kit::section(
                    ui,
                    "part-chooser-parameters",
                    "Parameters",
                    true,
                    Some(parameters.len()),
                    |ui| {
                        if parameters.is_empty() {
                            kit::note(ui, "This part file has no user parameters");
                        }
                        committed = values_form(
                            ui,
                            "chooser",
                            &parameters,
                            &mut self.editing,
                            &self.values,
                        );
                    },
                );
            }
        }
        if let Some(refusal) = &self.refusal {
            kit::refusal(ui, refusal);
        }
        committed
    }
}

#[cfg(test)]
#[path = "part_chooser_tests.rs"]
mod tests;
