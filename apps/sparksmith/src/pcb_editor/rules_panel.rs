//! Design rules: the docked panel the Board card opens (docs/sidebar-ia.md,
//! "Dialogs and reviews"; docs/board-rules-and-shape.md).
//!
//! The panel shows the project's profile snapshot with its source, a
//! picker of the user's profiles and the built-in ones (read-only; a user
//! profile with a built-in's id takes its place), and every rule grouped
//! as the
//! core groups them (Copper, Holes & vias, Mask, Silkscreen, Paste, Fills,
//! Board edge, Placement, Lands) with the profile value beside the
//! effective value. A value commits live, per field, as one
//! `rules.set_override` (or `rules.clear_override` when it returns to the
//! profile value) and one undo step; Reset clears an override. Choosing
//! another profile first shows its impact (`rules.preview_fab_profile`): the
//! rules that change, the overrides dropped and the board issues that
//! appear or go; Switch then commits `rules.set_fab_profile` as one undo step.
//! A profile's manufacturing options (a mask colour, a drill process, its
//! recommended or minimum values) show under its source; choosing another
//! choice is reviewed the same way (`rules.preview_fab_options`) and Use
//! commits `rules.set_fab_options`, keeping the overrides.
//! A new project without a profile opens the panel at "Choose a fab
//! profile" ([`PcbEditor::ask_for_profile`]): the shipped library's
//! default profile is preselected, and Use profile sets it in one undo
//! step.
//!
//! The user profile library (`sparksmith_io::rule_profiles`) is file
//! effects, not document edits: saving, renaming, deleting, importing and
//! exporting a profile change files in the user config folder and are not
//! undone. A project never reads the library to evaluate its rules; it
//! keeps its own snapshot.
use super::{Inspected, PcbEditor};
use crate::kit::{
    self,
    card::{CardResponse, Inspector, Region, Subject},
    dialog::{DialogAction, DialogPanel},
    review::{ReviewAction, ReviewGroup, ReviewLine, ReviewPanel},
    severity::Level,
    toast::{self, ToastAction},
};
use crate::refusal::{self, Origin};
use crate::shell::Header;
use egui::{Align, RichText, Ui};
use sparksmith_core::rules::{
    self, CopperObject, MAX_RULE_MM, PadConnection, ProfileImpact, ProfileSource, ProjectRules,
    RuleEntry, RuleGroup, RuleKey, RuleProfile, RuleRow, RuleValue, RulesCommand,
};
use sparksmith_core::{Engine, Operation, Query, QueryResult};
use sparksmith_io::rule_profiles::{self, Listing, ProfileLibrary};
use sparksmith_ui::theme::Tokens;
use sparksmith_ui::ui_kit::{self, Button};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

/// The groups in panel order, with their captions.
pub const GROUPS: [(RuleGroup, &str); 9] = [
    (RuleGroup::Copper, "Copper"),
    (RuleGroup::HolesAndVias, "Holes & vias"),
    (RuleGroup::Mask, "Mask"),
    (RuleGroup::Silkscreen, "Silkscreen"),
    (RuleGroup::Paste, "Paste"),
    (RuleGroup::Fills, "Fills"),
    (RuleGroup::BoardEdge, "Board edge"),
    (RuleGroup::Placement, "Placement"),
    (RuleGroup::Lands, "Lands"),
];

/// The rule an issue's "Show rule" opens: its group, and the one rule
/// when the finding rests on exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuleTopic {
    Clearance,
    BoardEdge,
    ViaAnnularRing,
    PthAnnularRing,
    NpthAnnularRing,
    DrillToCopper,
    HoleSpacing,
    /// Pad holes to any hole: one of several hole-kind pairs.
    PadHoleSpacing,
    Courtyard,
}

impl RuleTopic {
    pub fn group(self) -> RuleGroup {
        match self {
            Self::Clearance => RuleGroup::Copper,
            Self::BoardEdge => RuleGroup::BoardEdge,
            Self::ViaAnnularRing
            | Self::PthAnnularRing
            | Self::NpthAnnularRing
            | Self::DrillToCopper
            | Self::HoleSpacing
            | Self::PadHoleSpacing => RuleGroup::HolesAndVias,
            Self::Courtyard => RuleGroup::Placement,
        }
    }

    pub fn key(self) -> Option<RuleKey> {
        match self {
            Self::Clearance | Self::BoardEdge | Self::DrillToCopper | Self::PadHoleSpacing => None,
            Self::ViaAnnularRing => Some(RuleKey::ViaAnnularRingMin),
            Self::PthAnnularRing => Some(RuleKey::PthAnnularRingMin),
            Self::NpthAnnularRing => Some(RuleKey::NpthAnnularRingMin),
            Self::HoleSpacing => Some(RuleKey::Clearance {
                a: CopperObject::ViaHole,
                b: CopperObject::ViaHole,
            }),
            Self::Courtyard => Some(RuleKey::CourtyardClearance),
        }
    }
}

/// Board finding codes whose limit is a design rule, and that rule.
const TOPICS: &[(&str, RuleTopic)] = &[
    ("board.clearance", RuleTopic::Clearance),
    ("board.outside_outline", RuleTopic::BoardEdge),
    ("board.edge_clearance", RuleTopic::BoardEdge),
    ("board.via_annular_ring", RuleTopic::ViaAnnularRing),
    ("board.via_drill_clearance", RuleTopic::DrillToCopper),
    ("board.hole_clearance", RuleTopic::DrillToCopper),
    ("board.via_hole_spacing", RuleTopic::HoleSpacing),
    ("board.pth_annular_ring", RuleTopic::PthAnnularRing),
    ("board.npth_annular_ring", RuleTopic::NpthAnnularRing),
    ("board.pad_hole_clearance", RuleTopic::DrillToCopper),
    ("board.pad_hole_spacing", RuleTopic::PadHoleSpacing),
    ("board.courtyard_overlap", RuleTopic::Courtyard),
    ("board.courtyard_clearance", RuleTopic::Courtyard),
    ("fill.clearance", RuleTopic::Clearance),
];

/// The rule behind a finding code, for the Issues row's "Show rule".
pub fn topic_for(code: &str) -> Option<RuleTopic> {
    TOPICS.iter().find(|(c, _)| *c == code).map(|(_, t)| *t)
}

fn field_id(key: RuleKey) -> egui::Id {
    egui::Id::unique(("pcb-rules-field", key.normalized()))
}
fn panel_id() -> egui::Id {
    egui::Id::unique("pcb-rules")
}
fn preview_id() -> egui::Id {
    egui::Id::unique("pcb-rules-preview")
}
fn save_id() -> egui::Id {
    egui::Id::unique("pcb-rules-save")
}
fn rename_id() -> egui::Id {
    egui::Id::unique("pcb-rules-rename")
}
fn delete_id() -> egui::Id {
    egui::Id::unique("pcb-rules-delete")
}
fn choose_id() -> egui::Id {
    egui::Id::unique("pcb-rules-choose")
}

/// The impact review of switching to `profile`.
struct Preview {
    profile: RuleProfile,
    keep_overrides: bool,
    /// Other choices of the board's profile options rather than another
    /// profile: what the review names ("Solder mask colour: Black or
    /// white"), the option's source, and the choices.
    options: Option<OptionChange>,
    /// The revision the impact was computed at.
    at: u64,
    impact: Result<ProfileImpact, String>,
    /// The revision Switch was sent at.
    sent: Option<u64>,
    refusal: Option<String>,
}

/// A change of the board's profile options under review.
#[derive(Clone)]
struct OptionChange {
    label: String,
    source: String,
    choices: BTreeMap<String, String>,
}

/// The "Save as fab profile" form: a name and where the numbers come from.
struct SaveForm {
    name: String,
    publisher: String,
    document: String,
    url: String,
    date: String,
    notes: String,
    /// Switch the board to the saved profile (its rules are the current
    /// effective rules, so nothing else changes).
    use_it: bool,
    refusal: Option<String>,
    opening: bool,
}

enum Mode {
    Table,
    /// A new board's profile: the chosen id, the revision Use profile was
    /// sent at, and the engine's refusal of it.
    Choose {
        id: String,
        sent: Option<u64>,
        refusal: Option<String>,
    },
    Preview(Box<Preview>),
    Save(Box<SaveForm>),
    Rename {
        id: String,
        name: String,
        refusal: Option<String>,
    },
    Delete {
        id: String,
        name: String,
    },
}

/// A file job's result, from a worker thread.
enum Job {
    /// The imported profile's name.
    Imported(Result<String, String>),
    Exported(Result<PathBuf, String>),
    Cancelled,
}

/// The Design rules panel's state while it is open.
pub(super) struct RulesPanel {
    library: Option<ProfileLibrary>,
    listing: Result<Listing, String>,
    /// Number fields being edited, committed when focus leaves.
    staged: BTreeMap<RuleKey, f64>,
    /// Refusals of a field's last commit, in shown wording.
    errors: BTreeMap<RuleKey, String>,
    /// The revision the field state belongs to.
    revision: Option<u64>,
    /// The rule an issue asked to show.
    focus: Option<RuleTopic>,
    /// Open the focused group and scroll its row into view once.
    reveal: bool,
    mode: Mode,
    job: Option<Receiver<Job>>,
    /// Every user profile instead of the first eight.
    all_profiles: bool,
    /// A field had keyboard focus last frame (Esc reverts it rather than
    /// closing the panel).
    typing: bool,
}

impl RulesPanel {
    pub(super) fn new(library: Option<ProfileLibrary>, focus: Option<RuleTopic>) -> Self {
        let mut panel = Self {
            library,
            listing: Ok(Listing::default()),
            staged: BTreeMap::new(),
            errors: BTreeMap::new(),
            revision: None,
            focus,
            reveal: focus.is_some(),
            mode: Mode::Table,
            job: None,
            all_profiles: false,
            typing: false,
        };
        panel.refresh();
        panel
    }

    /// Re-read the user profile library.
    fn refresh(&mut self) {
        self.listing = match &self.library {
            Some(library) => library.list().map_err(|e| e.to_string()),
            None => rule_profiles::shipped_from_env()
                .map(|builtins| Listing {
                    builtins,
                    ..Listing::default()
                })
                .map_err(|e| e.to_string()),
        };
    }

    fn user_profiles(&self) -> &[RuleProfile] {
        self.listing.as_ref().map_or(&[], |l| l.profiles.as_slice())
    }

    /// The picker's profiles: the user's, then the shipped ones none of
    /// them replaces (all shipped ones when the folder cannot be read).
    fn choices(&self) -> Vec<RuleProfile> {
        match &self.listing {
            Ok(listing) => listing
                .profiles
                .iter()
                .chain(&listing.builtins)
                .cloned()
                .collect(),
            Err(_) => rule_profiles::shipped_from_env().unwrap_or_default(),
        }
    }

    pub(super) fn subject(&self) -> Subject {
        match self.mode {
            Mode::Preview(_) => Subject::Review,
            _ => Subject::Dialog,
        }
    }

    /// The impact of switching to `profile`, computed now.
    fn preview(&mut self, engine: &Engine, profile: RuleProfile, keep_overrides: bool) {
        let impact = match engine.query(Query::PreviewProfile {
            profile: Box::new(profile.clone()),
            keep_overrides,
        }) {
            Ok(QueryResult::ProfileImpact(impact)) => Ok(*impact),
            Ok(_) => Err("The preview returned something else".to_owned()),
            Err(error) => Err(refusal::shown_error(&error)),
        };
        self.mode = Mode::Preview(Box::new(Preview {
            profile,
            keep_overrides,
            options: None,
            at: engine.revision(),
            impact,
            sent: None,
            refusal: None,
        }));
    }

    /// The impact of choosing `change`'s choices for the board's profile,
    /// computed now.
    fn preview_options(&mut self, engine: &Engine, change: OptionChange) {
        // A refused choice is reviewed with its reason (from the query).
        let Some(current) = engine.project().rules.profile.clone() else {
            return;
        };
        let profile =
            rules::with_options(&engine.project().rules, &change.choices).unwrap_or(current);
        let impact = match engine.query(Query::PreviewOptions {
            options: change.choices.clone(),
        }) {
            Ok(QueryResult::ProfileImpact(impact)) => Ok(*impact),
            Ok(_) => Err("The preview returned something else".to_owned()),
            Err(error) => Err(refusal::shown_error(&error)),
        };
        self.mode = Mode::Preview(Box::new(Preview {
            profile,
            keep_overrides: true,
            options: Some(change),
            at: engine.revision(),
            impact,
            sent: None,
            refusal: None,
        }));
    }

    /// Take the refusals handed back for the panel's fields and dialogs.
    fn claim(&mut self, ctx: &egui::Context) {
        for key in RuleKey::all() {
            if let Some(text) = refusal::claim(ctx, Origin::Field(field_id(key))) {
                self.errors.insert(key.normalized(), text);
            }
        }
        if let Some(text) = refusal::claim(ctx, Origin::Dialog(preview_id()))
            && let Mode::Preview(preview) = &mut self.mode
        {
            preview.refusal = Some(text);
            preview.sent = None;
        }
        if let Some(text) = refusal::claim(ctx, Origin::Dialog(choose_id()))
            && let Mode::Choose { sent, refusal, .. } = &mut self.mode
        {
            *refusal = Some(text);
            *sent = None;
        }
        if let Some(text) = refusal::claim(ctx, Origin::Dialog(save_id())) {
            toast::post(
                ctx,
                Level::Error,
                format!("Saved, but switching failed: {text}"),
                Vec::new(),
            );
        }
    }

    /// Take a finished file job.
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.job else {
            return;
        };
        let job = match receiver.try_recv() {
            Ok(job) => job,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Job::Exported(Err("The file picker closed unexpectedly".to_owned()))
            }
        };
        self.job = None;
        match job {
            Job::Imported(Ok(name)) => {
                self.refresh();
                toast::post(
                    ctx,
                    Level::Ok,
                    format!("Imported profile {name}"),
                    Vec::new(),
                );
            }
            Job::Exported(Ok(path)) => {
                let name = path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                toast::post(ctx, Level::Ok, format!("Exported {name}"), Vec::new());
            }
            Job::Imported(Err(error)) | Job::Exported(Err(error)) => {
                toast::post(ctx, Level::Error, error, Vec::new());
            }
            Job::Cancelled => {}
        }
    }

    /// Pick a profile file and import it into the library, off the UI
    /// thread.
    fn start_import(&mut self, ctx: &egui::Context) {
        let Some(library) = self.library.clone() else {
            return;
        };
        if self.job.is_some() {
            return;
        }
        let future = rfd::AsyncFileDialog::new()
            .set_title("Import a fab profile")
            .add_filter("Fab profile", &[rule_profiles::EXTENSION])
            .pick_file();
        let (sender, receiver) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let job = match pollster::block_on(future) {
                None => Job::Cancelled,
                Some(file) => Job::Imported(
                    library
                        .import(file.path())
                        .map(|p| p.name)
                        .map_err(|e| format!("Import failed: {e}")),
                ),
            };
            let _ = sender.send(job);
            ctx.request_repaint();
        });
        self.job = Some(receiver);
    }

    /// Pick a file name and write `profile` to it, off the UI thread. The
    /// native dialog asks before replacing a file.
    fn start_export(&mut self, ctx: &egui::Context, profile: RuleProfile) {
        if self.job.is_some() {
            return;
        }
        let future = rfd::AsyncFileDialog::new()
            .set_title("Export a fab profile")
            .add_filter("Fab profile", &[rule_profiles::EXTENSION])
            .set_file_name(format!("{}.{}", profile.id, rule_profiles::EXTENSION))
            .save_file();
        let (sender, receiver) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let job = match pollster::block_on(future) {
                None => Job::Cancelled,
                Some(file) => {
                    let path = file.path().to_path_buf();
                    Job::Exported(
                        rule_profiles::export_replacing(&profile, &path)
                            .map(|()| path)
                            .map_err(|e| format!("Export failed: {e}")),
                    )
                }
            };
            let _ = sender.send(job);
            ctx.request_repaint();
        });
        self.job = Some(receiver);
    }
}

/// "0.2 mm", "45°", "4", "10:1", "Not checked", "Thermal relief".
pub fn value_text(value: RuleValue) -> String {
    match value {
        RuleValue::Length(mm) => format!("{} mm", kit::number_text(mm)),
        RuleValue::Area(a) => format!("{} mm²", kit::number_text(a)),
        RuleValue::Angle(d) => format!("{}°", kit::number_text(d)),
        RuleValue::Fraction(f) => kit::number_text(f),
        RuleValue::Ratio(Some(r)) => format!("{}:1", kit::number_text(r)),
        RuleValue::Ratio(None) => "Not checked".into(),
        RuleValue::Count(n) => n.to_string(),
        RuleValue::Flag(true) => "Yes".into(),
        RuleValue::Flag(false) => "No".into(),
        RuleValue::Connection(c) => connection_label(c).into(),
    }
}

/// [`value_text`], and "Not checked" for a check rule at 0 (the etch
/// hazards' minimum angle and sliver width: nothing is under 0).
pub fn rule_text(key: RuleKey, value: RuleValue) -> String {
    let check = matches!(key, RuleKey::CopperAngleMin | RuleKey::SliverWidthMin);
    match value {
        RuleValue::Angle(v) | RuleValue::Length(v) if check && v == 0.0 => "Not checked".into(),
        _ => value_text(value),
    }
}

fn connection_label(c: PadConnection) -> &'static str {
    match c {
        PadConnection::Solid => "Solid",
        PadConnection::Thermal => "Thermal relief",
        PadConnection::None => "Not connected",
    }
}

/// A number-valued rule's number, unit and accepted range.
fn numeric(
    key: RuleKey,
    value: RuleValue,
) -> Option<(f64, &'static str, std::ops::RangeInclusive<f64>)> {
    match value {
        RuleValue::Length(mm) if key == RuleKey::MaskExpansion => {
            Some((mm, "mm", -1.0..=MAX_RULE_MM))
        }
        RuleValue::Length(mm) => Some((mm, "mm", 0.0..=MAX_RULE_MM)),
        RuleValue::Area(a) => Some((a, "mm²", 0.0..=1.0e6)),
        RuleValue::Angle(d) if key == RuleKey::CopperAngleMin => Some((d, "°", 0.0..=180.0)),
        RuleValue::Angle(d) => Some((d, "°", -360.0..=360.0)),
        RuleValue::Fraction(f) => Some((f, "", 0.0..=1.0)),
        RuleValue::Count(n) => Some((f64::from(n), "", 1.0..=8.0)),
        _ => None,
    }
}

/// `number` in the kind of `like`.
fn with_number(like: RuleValue, number: f64) -> RuleValue {
    match like {
        RuleValue::Area(_) => RuleValue::Area(number),
        RuleValue::Angle(_) => RuleValue::Angle(number),
        RuleValue::Fraction(_) => RuleValue::Fraction(number),
        RuleValue::Count(_) => {
            // The field's range is 1–8, so the rounded value fits.
            let n = number.round().clamp(0.0, f64::from(u32::MAX));
            RuleValue::Count(n as u32)
        }
        _ => RuleValue::Length(number),
    }
}

fn same(a: RuleValue, b: RuleValue) -> bool {
    match (a, b) {
        (RuleValue::Length(x), RuleValue::Length(y))
        | (RuleValue::Area(x), RuleValue::Area(y))
        | (RuleValue::Angle(x), RuleValue::Angle(y))
        | (RuleValue::Fraction(x), RuleValue::Fraction(y)) => (x - y).abs() < 1e-9,
        (RuleValue::Ratio(Some(x)), RuleValue::Ratio(Some(y))) => (x - y).abs() < 1e-9,
        (a, b) => a == b,
    }
}

/// The command that makes `key`'s effective value `value`: an override,
/// its removal when `value` is the profile's again, or nothing.
pub fn commit_for(rules: &ProjectRules, key: RuleKey, value: RuleValue) -> Option<RulesCommand> {
    let profile = key.get(rules.profile.as_ref()?.rules());
    let overridden = rules.override_of(key).is_some();
    if same(value, profile) {
        return overridden.then_some(RulesCommand::ClearOverride { key });
    }
    if same(value, key.get(&rules.effective())) {
        return None;
    }
    Some(RulesCommand::SetOverride { key, value })
}

/// "JLCPCB · PCB Capabilities · read 2026-11-11".
pub fn source_line(s: &ProfileSource) -> String {
    [&s.publisher, &s.document]
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().to_owned())
        .chain(std::iter::once(format!("read {}", s.date)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The Board card's Rules summary: "JLCPCB 2-layer · 2 overrides", or
/// "No fab profile".
pub fn rules_summary(rules: &ProjectRules) -> String {
    let Some(profile) = &rules.profile else {
        return "No fab profile".into();
    };
    match rules.overrides.len() {
        0 => profile.name.clone(),
        1 => format!("{} · 1 override", profile.name),
        n => format!("{} · {n} overrides", profile.name),
    }
}

/// The board's profile with every project override written in as a value
/// whose source says so: what Export and Save as profile write.
fn with_overrides(rules: &ProjectRules) -> Option<RuleProfile> {
    let mut profile = rules.profile.clone()?;
    for o in &rules.overrides {
        profile
            .set(o.key, RuleEntry::value(o.value, "Project override"))
            .ok()?;
    }
    Some(profile)
}

/// "Min track width" from "min track width".
fn sentence(label: &str) -> String {
    let mut chars = label.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// One entry of the profile picker.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pick {
    User(String),
    /// The board's own snapshot, matching no library entry.
    Board,
    /// The board has no profile yet.
    None,
}

/// The review's text: summary, groups and the copyable report.
pub fn impact_groups(impact: &ProfileImpact) -> Vec<ReviewGroup> {
    let mut groups = vec![ReviewGroup {
        title: "Rules that change".into(),
        summary: String::new(),
        lines: impact
            .changes
            .iter()
            .map(|c| {
                format!(
                    "{}: {} → {}",
                    sentence(&c.label),
                    value_text(c.from),
                    value_text(c.to)
                )
            })
            .collect(),
    }];
    if let Some((from, to)) = impact.outer_copper_um {
        groups.push(ReviewGroup {
            title: "Stackup".into(),
            summary: String::new(),
            lines: vec![format!("Top and bottom copper: {from} µm → {to} µm")],
        });
    }
    for (title, notes) in [
        ("Choices kept", &impact.kept_choices),
        (
            "Choices set to the profile's defaults",
            &impact.reset_choices,
        ),
    ] {
        if !notes.is_empty() {
            groups.push(ReviewGroup {
                title: title.into(),
                summary: String::new(),
                lines: notes.iter().map(|n| n.label.clone()).collect(),
            });
        }
    }
    if !impact.dropped_overrides.is_empty() {
        groups.push(ReviewGroup {
            title: "Overrides dropped".into(),
            summary: String::new(),
            lines: impact
                .dropped_overrides
                .iter()
                .map(|k| sentence(&k.label()))
                .collect(),
        });
    }
    for (title, lines) in [
        ("New issues", &impact.new_issues),
        ("Resolved issues", &impact.resolved_issues),
    ] {
        if !lines.is_empty() {
            groups.push(ReviewGroup {
                title: title.into(),
                summary: String::new(),
                lines: lines.clone(),
            });
        }
    }
    groups
}

/// "Fine 4/4 · 12 rules change · 3 new issues · 1 resolved".
pub fn impact_summary(name: &str, impact: &ProfileImpact) -> String {
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let mut parts = vec![
        name.to_owned(),
        plural(impact.changes.len(), "rule changes", "rules change"),
    ];
    if !impact.new_issues.is_empty() {
        parts.push(plural(impact.new_issues.len(), "new issue", "new issues"));
    }
    if !impact.resolved_issues.is_empty() {
        parts.push(format!("{} resolved", impact.resolved_issues.len()));
    }
    parts.join(" · ")
}

impl PcbEditor {
    /// Open the Design rules panel (the Board card's Rules…), at the rule
    /// of `topic` when an issue asked for it. The user profile library is
    /// the config folder's `fab-profiles/`.
    pub(super) fn open_rules(&mut self, topic: Option<RuleTopic>) {
        let library = sparksmith_io::settings::UserDirs::from_env()
            .ok()
            .and_then(|dirs| ProfileLibrary::from_dirs(&dirs).ok());
        self.open_rules_in(library, topic);
    }

    /// [`Self::open_rules`] over a given library folder (tests).
    pub(super) fn open_rules_in(
        &mut self,
        library: Option<ProfileLibrary>,
        topic: Option<RuleTopic>,
    ) {
        self.setup = None;
        self.stackup = None;
        self.net_classes = None;
        self.signals = None;
        match &mut self.rules {
            Some(panel) => {
                if topic.is_some() {
                    panel.focus = topic;
                    panel.reveal = true;
                    panel.mode = Mode::Table;
                }
            }
            None => self.rules = Some(RulesPanel::new(library, topic)),
        }
    }

    /// Ask for a new project's fab profile the next time the board's
    /// subject slot is drawn, if it has none then.
    pub fn ask_for_profile(&mut self) {
        self.ask_profile = true;
    }

    /// Open the panel at "Choose a fab profile" with the shipped library's
    /// default preselected (the user's own copy when it has that id).
    pub(super) fn choose_profile(&mut self) {
        if self.rules.is_none() {
            self.open_rules(None);
        }
        let Some(panel) = &mut self.rules else {
            return;
        };
        let choices = panel.choices();
        let default = match &panel.library {
            Some(library) => library.default_id().ok(),
            None => sparksmith_io::new_project::shipped()
                .ok()
                .map(|d| d.fab_profile),
        };
        let id = choices
            .iter()
            .find(|p| Some(&p.id) == default.as_ref())
            .or(choices.first())
            .map(|p| p.id.clone())
            .or(default)
            .unwrap_or_default();
        panel.mode = Mode::Choose {
            id,
            sent: None,
            refusal: None,
        };
    }

    /// An Issues row's "Show rule": the panel, at that rule.
    pub fn show_rule(&mut self, topic: RuleTopic) {
        self.open_rules(Some(topic));
    }

    pub(super) fn rules_subject(&self) -> Option<Subject> {
        self.rules.as_ref().map(RulesPanel::subject)
    }

    /// The whole subject slot while the panel is open.
    pub(super) fn rules_slot(&mut self, ui: &mut Ui, engine: &Engine) -> CardResponse<Operation> {
        let Some(mut panel) = self.rules.take() else {
            return CardResponse::output(None);
        };
        panel.claim(ui.ctx());
        panel.poll(ui.ctx());
        if panel.revision != Some(engine.revision()) {
            // Undo, redo or another edit: fields show the document again.
            panel.staged.clear();
            panel.errors.clear();
            panel.revision = Some(engine.revision());
        }
        // A switch the engine applied: say so and show the table again.
        if let Mode::Preview(preview) = &panel.mode
            && preview.sent.is_some_and(|sent| sent != engine.revision())
        {
            let text = match &preview.options {
                Some(change) => format!("Using {}", change.label),
                None => format!("Switched to {}", preview.profile.name),
            };
            toast::post(ui.ctx(), Level::Ok, text, vec![ToastAction::Undo]);
            panel.mode = Mode::Table;
        }
        let mut close = false;
        // A new board's choice: done once the board has a profile (the
        // engine took Use profile, or another file opened with one).
        if let Mode::Choose { sent, .. } = &panel.mode
            && let Some(profile) = &engine.project().rules.profile
        {
            if sent.is_some_and(|sent| sent != engine.revision()) {
                toast::post(
                    ui.ctx(),
                    Level::Ok,
                    format!("Using {}", profile.name),
                    vec![ToastAction::Undo],
                );
            }
            return CardResponse::output(None);
        }
        let response = match &mut panel.mode {
            Mode::Choose { .. } => CardResponse::output(choose_slot(&mut panel, ui, &mut close)),
            Mode::Preview(_) => CardResponse::output(preview_slot(&mut panel, ui, engine)),
            Mode::Save(_) => CardResponse::output(save_slot(&mut panel, ui, engine)),
            Mode::Rename { .. } => {
                rename_slot(&mut panel, ui);
                CardResponse::output(None)
            }
            Mode::Delete { id, name } => {
                let (id, name) = (id.clone(), name.clone());
                let text = format!(
                    "Delete {name} from your profiles? Boards that use it keep their copy."
                );
                match kit::dialog::confirm(ui.ctx(), delete_id(), "Delete profile", &text, "Delete")
                {
                    Some(true) => {
                        if let Some(library) = &panel.library {
                            match library.delete(&id) {
                                Ok(()) => toast::post(
                                    ui.ctx(),
                                    Level::Ok,
                                    format!("Deleted profile {name}"),
                                    Vec::new(),
                                ),
                                Err(error) => toast::post(
                                    ui.ctx(),
                                    Level::Error,
                                    format!("Deleting {name} failed: {error}"),
                                    Vec::new(),
                                ),
                            }
                        }
                        panel.refresh();
                        panel.mode = Mode::Table;
                    }
                    Some(false) => panel.mode = Mode::Table,
                    None => {}
                }
                table_card(self, &mut panel, ui, engine, &mut close)
            }
            Mode::Table => table_card(self, &mut panel, ui, engine, &mut close),
        };
        if !close {
            self.rules = Some(panel);
        }
        response
    }
}

/// The table card: header, profile, rule groups, your profiles, footer.
fn table_card(
    editor: &mut PcbEditor,
    panel: &mut RulesPanel,
    ui: &mut Ui,
    engine: &Engine,
    close: &mut bool,
) -> CardResponse<Operation> {
    let inspector = Inspector {
        header: Header::new("constraint", "Design rules", "Board"),
        subject: panel_id(),
        footer: true,
        issues: editor.issue_slot(engine, Inspected::Board),
    };
    let typing = panel.typing;
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let mut focused = false;
    let response = kit::card::card(ui, &inspector, |ui, region| match region {
        Region::Body => {
            let (op, any_focus) = body(panel, ui, engine);
            focused = any_focus;
            op
        }
        Region::Footer => {
            if kit::primary(ui, "Done", true).clicked() {
                *close = true;
            }
            let profiled = engine.project().rules.profile.is_some();
            if ui_kit::button(
                ui,
                "Save as fab profile…",
                Button::Enabled(panel.library.is_some() && profiled),
            )
            .on_disabled_hover_text(if profiled {
                "No user config folder: profiles cannot be saved"
            } else {
                "Choose a fab profile first"
            })
            .clicked()
            {
                panel.mode = Mode::Save(Box::new(save_form(&engine.project().rules)));
            }
            None
        }
    });
    if escape && !typing && !focused && matches!(panel.mode, Mode::Table) {
        *close = true;
    }
    panel.typing = focused;
    response
}

fn save_form(rules: &ProjectRules) -> SaveForm {
    let (name, s) = rules.profile.as_ref().map_or_else(Default::default, |p| {
        (format!("{} copy", p.name), p.source.clone())
    });
    SaveForm {
        name,
        publisher: s.publisher,
        document: s.document,
        url: s.url,
        date: s.date,
        notes: s.notes,
        use_it: true,
        refusal: None,
        opening: true,
    }
}

/// The picker's entries and the board's current one.
fn picks(
    panel: &RulesPanel,
    rules: &ProjectRules,
) -> (Vec<(Pick, String)>, Pick, Option<RuleProfile>) {
    let mut options = Vec::new();
    let choices = panel.choices();
    let Some(snapshot) = &rules.profile else {
        options.push((Pick::None, "Choose a profile…".to_owned()));
        for p in &choices {
            options.push((Pick::User(p.id.clone()), p.name.clone()));
        }
        return (options, Pick::None, None);
    };
    let mut current = Pick::Board;
    let mut differs = None;
    for p in &choices {
        let pick = Pick::User(p.id.clone());
        if p.id == snapshot.id {
            // The board's own choices of the profile's options are not a
            // difference from the library's.
            if snapshot.same_but_choices(p) {
                current = pick.clone();
            } else {
                differs = Some(p.clone());
            }
        }
        options.push((pick, p.name.clone()));
    }
    if current == Pick::Board {
        options.insert(0, (Pick::Board, format!("{} · this board", snapshot.name)));
    } else {
        differs = None;
    }
    (options, current, differs)
}

fn profile_of(panel: &RulesPanel, pick: &Pick) -> Option<RuleProfile> {
    match pick {
        Pick::User(id) => panel.choices().into_iter().find(|p| p.id == *id),
        Pick::Board | Pick::None => None,
    }
}

/// The table card's body; returns its operation and whether a field has
/// keyboard focus.
fn body(panel: &mut RulesPanel, ui: &mut Ui, engine: &Engine) -> (Option<Operation>, bool) {
    let rules = &engine.project().rules;
    let mut op: Option<(RulesCommand, Origin)> = None;
    // PROFILE: the picker, provenance, and the overrides with Reset all.
    kit::section(ui, "pcb-rules-profile", "Fab profile", true, None, |ui| {
        let (options, current, differs) = picks(panel, rules);
        let mut chosen = current.clone();
        kit::choice_row(ui, "Fab profile", &mut chosen, &options);
        if chosen != current
            && let Some(profile) = profile_of(panel, &chosen)
        {
            panel.preview(engine, profile, false);
        }
        match &rules.profile {
            Some(profile) => {
                kit::row(ui, "Source", |ui| {
                    kit::summary_row(ui, &source_line(&profile.source), None);
                });
                if let Some(change) = option_rows(ui, profile) {
                    panel.preview_options(engine, change);
                }
            }
            None => {
                kit::hint(
                    ui,
                    "This board has no fab profile, so nothing is checked",
                    concat!(
                        "Choose your fabricator's profile. ",
                        sparksmith_core::product_name!(),
                        " ships sourced profiles to choose from; import another \
                         fabricator's file (Your profiles › Import…) or write one from its \
                         capability page."
                    ),
                );
            }
        }
        if let Some(library) = differs
            && kit::summary_row(ui, "Differs from the library version", Some("Review…"))
        {
            panel.preview(engine, library, true);
        }
        match rules.overrides.len() {
            0 => {}
            n => {
                let text = if n == 1 {
                    "1 project override".to_owned()
                } else {
                    format!("{n} project overrides")
                };
                if kit::summary_row(ui, &text, Some("Reset all"))
                    && let Some(profile) = &rules.profile
                {
                    op = Some((
                        RulesCommand::SetProfile {
                            profile: Box::new(profile.clone()),
                            keep_overrides: false,
                        },
                        Origin::Command,
                    ));
                }
            }
        }
    });
    if let Some(profile) = &rules.profile {
        let s = &profile.source;
        kit::section(ui, "pcb-rules-provenance", "Source", false, None, |ui| {
            for (label, text) in [
                ("Publisher", &s.publisher),
                ("Document", &s.document),
                ("URL", &s.url),
                ("Read", &s.date),
            ] {
                if !text.trim().is_empty() {
                    kit::value_row(ui, label, text);
                }
            }
            kit::value_row(ui, "Drill files", &profile.drill_files.note());
            if !s.notes.trim().is_empty() {
                kit::note(ui, &s.notes);
            }
        });
    }
    let report = rules::report(rules);
    let mut focused = false;
    for (group, title) in GROUPS {
        let rows: Vec<&RuleRow> = report.rows.iter().filter(|r| r.group == group).collect();
        if rows.is_empty() {
            continue;
        }
        let section = ("pcb-rules-group", title);
        let reveal = panel.reveal && panel.focus.is_some_and(|t| t.group() == group);
        if reveal {
            let id = ui.make_persistent_id(("section", section));
            ui.ctx().data_mut(|d| d.insert_persisted(id, true));
        }
        let overridden = rows
            .iter()
            .filter(|r| rules.override_of(r.key).is_some())
            .count();
        let summary = match overridden {
            0 => format!("{} rules", rows.len()),
            n => format!("{} rules · {n} overridden", rows.len()),
        };
        let shown = kit::summary_section(
            ui,
            section,
            title,
            group == RuleGroup::Copper,
            &summary,
            |ui| group_table(panel, ui, rules, &rows, reveal),
        );
        if let Some((edit, any_focus)) = shown {
            focused |= any_focus;
            if op.is_none() {
                op = edit;
            }
        }
    }
    panel.reveal = false;
    super::impedance_view::impedance_section(ui, engine.project());
    let mut library_op = None;
    kit::section(
        ui,
        "pcb-rules-library",
        "Your fab profiles",
        true,
        Some(panel.user_profiles().len()),
        |ui| {
            library_op = library_section(panel, ui, engine);
        },
    );
    let op = match op {
        Some((command, origin)) => {
            refusal::mark(ui.ctx(), origin);
            Some(Operation::Rules(command))
        }
        None => library_op,
    };
    (op, focused)
}

/// The profile's manufacturing options, one choice row each; the change
/// a row asks for.
fn option_rows(ui: &mut Ui, profile: &RuleProfile) -> Option<OptionChange> {
    let mut change = None;
    for option in profile.options() {
        let current = profile
            .chosen()
            .get(&option.id)
            .cloned()
            .unwrap_or_default();
        let mut pick = current.clone();
        let list: Vec<(String, String)> = option
            .choices
            .iter()
            .map(|c| (c.id.clone(), c.name.clone()))
            .collect();
        kit::choice_row(ui, &option.name, &mut pick, &list);
        if pick != current {
            let name = option.choice(&pick).map_or("", |c| c.name.as_str());
            change = Some(OptionChange {
                label: format!("{}: {name}", option.name),
                source: option.source.clone(),
                choices: [(option.id.clone(), pick)].into_iter().collect(),
            });
        }
    }
    change
}

/// One group's table: Rule · Profile · Value · Reset. Returns the edit a
/// field or menu committed and whether a field has focus.
fn group_table(
    panel: &mut RulesPanel,
    ui: &mut Ui,
    rules: &ProjectRules,
    rows: &[&RuleRow],
    reveal: bool,
) -> (Option<(RulesCommand, Origin)>, bool) {
    let t = Tokens::get();
    let columns = [
        kit::TableColumn {
            title: "Rule",
            weight: 3.0,
        },
        kit::TableColumn {
            title: "Profile",
            weight: 1.6,
        },
        kit::TableColumn {
            title: "Value",
            weight: 1.8,
        },
        kit::TableColumn {
            title: "",
            weight: 0.9,
        },
    ];
    let focus_key = panel
        .focus
        .and_then(RuleTopic::key)
        .map(RuleKey::normalized);
    let mut edit: Option<(RulesCommand, Origin)> = None;
    let mut focused = false;
    let mut marked: Option<(egui::Response, String)> = None;
    let mut blurred: Vec<RuleKey> = Vec::new();
    let reveal_row = focus_key
        .filter(|_| reveal)
        .and_then(|k| rows.iter().position(|r| r.key.normalized() == k));
    kit::table_revealing(
        ui,
        &columns,
        rows.len(),
        reveal_row,
        |ui, index, column| {
            let Some(row) = rows.get(index) else {
                return;
            };
            let key = row.key.normalized();
            let overridden = rules.override_of(key).is_some();
            match column {
                0 => {
                    if Some(key) == focus_key {
                        ui.painter()
                            .rect_filled(ui.max_rect(), t.radius_sm, t.selected_bg);
                        if reveal {
                            ui.scroll_to_rect(ui.max_rect(), Some(Align::Center));
                        }
                    }
                    let ink = if overridden { t.form_ink } else { t.text };
                    let label = ui.add(
                        egui::Label::new(RichText::new(sentence(&row.label)).color(ink)).truncate(),
                    );
                    let unset = if rule_text(key, row.value) == "Not checked" {
                        " · 0: not checked"
                    } else {
                        ""
                    };
                    let mut tip = format!(
                        "{} · from {}{unset}",
                        sentence(&row.label),
                        row.source.describe()
                    );
                    if let Some(formula) = &row.formula {
                        tip.push_str(&format!("\nProfile formula: {formula}"));
                    }
                    if !row.value_source.is_empty() {
                        tip.push_str(&format!("\nSource: {}", row.value_source));
                    }
                    label.on_hover_text(tip);
                }
                1 => {
                    let text = rule_text(key, row.profile);
                    let text = if row.formula.is_some() {
                        format!("ƒ {text}")
                    } else {
                        text
                    };
                    ui.add(egui::Label::new(RichText::new(text).color(t.form_dim)).truncate());
                }
                2 => {
                    if let Some((number, unit, range)) = numeric(key, row.value) {
                        let mut value = panel.staged.get(&key).copied().unwrap_or(number);
                        let input =
                            kit::number_input(ui, &format!("{key:?}"), &mut value, unit, range);
                        let escaped = input.response.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Escape));
                        if input.response.has_focus() {
                            focused = true;
                            panel.staged.insert(key, value);
                        } else if escaped {
                            panel.staged.remove(&key);
                            panel.errors.remove(&key);
                        } else if panel.staged.contains_key(&key) {
                            blurred.push(key);
                        }
                        if let Some(message) = input.invalid {
                            marked = Some((input.response, message));
                        } else if let Some(message) = panel.errors.get(&key) {
                            marked = Some((input.response, message.clone()));
                        }
                    } else {
                        let mut value = row.value;
                        let changed = choice_cell(ui, key, &mut value);
                        if changed && let Some(command) = commit_for(rules, key, value) {
                            panel.errors.remove(&key);
                            edit = Some((command, Origin::Field(field_id(key))));
                        }
                    }
                }
                _ => {
                    if overridden
                        && ui_kit::button(ui, "Reset", Button::Small)
                            .on_hover_text(format!(
                                "Back to the profile value, {}",
                                value_text(row.profile)
                            ))
                            .clicked()
                    {
                        panel.staged.remove(&key);
                        panel.errors.remove(&key);
                        edit = Some((
                            RulesCommand::ClearOverride { key },
                            Origin::Field(field_id(key)),
                        ));
                    }
                }
            }
        },
        None,
    );
    if let Some((response, message)) = marked {
        kit::field_error(ui, &response, &message, true);
    }
    // A field that lost focus commits its staged value once.
    for key in blurred {
        let Some(number) = panel.staged.remove(&key) else {
            continue;
        };
        let like = key.get(&rules.effective());
        if edit.is_none()
            && let Some(command) = commit_for(rules, key, with_number(like, number))
        {
            panel.errors.remove(&key);
            edit = Some((command, Origin::Field(field_id(key))));
        }
    }
    (edit, focused)
}

/// A dropdown cell for a flag, ratio or pad-connection rule.
fn choice_cell(ui: &mut Ui, key: RuleKey, value: &mut RuleValue) -> bool {
    let id = ("pcb-rules-choice", key);
    match *value {
        RuleValue::Flag(_) => {
            let options =
                [true, false].map(|v| (RuleValue::Flag(v), value_text(RuleValue::Flag(v))));
            kit::dropdown(ui, id, value, &options)
        }
        RuleValue::Ratio(current) => {
            let mut options: Vec<(RuleValue, String)> = [
                None,
                Some(6.0),
                Some(8.0),
                Some(10.0),
                Some(12.0),
                Some(16.0),
                Some(20.0),
            ]
            .into_iter()
            .map(|r| (RuleValue::Ratio(r), value_text(RuleValue::Ratio(r))))
            .collect();
            if !options.iter().any(|(v, _)| same(*v, *value)) {
                options.push((RuleValue::Ratio(current), value_text(*value)));
            }
            kit::dropdown(ui, id, value, &options)
        }
        RuleValue::Connection(_) => {
            let options = [
                PadConnection::Solid,
                PadConnection::Thermal,
                PadConnection::None,
            ]
            .map(|c| (RuleValue::Connection(c), connection_label(c).to_owned()));
            kit::dropdown(ui, id, value, &options)
        }
        _ => false,
    }
}

/// YOUR PROFILES: the library's profiles, each with a menu, then Import…
/// and the files that could not be read.
fn library_section(panel: &mut RulesPanel, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
    let Some(library) = panel.library.clone() else {
        kit::hint(ui, "No user config folder: profiles cannot be saved", "");
        return None;
    };
    let profiles = panel.user_profiles().to_vec();
    if profiles.is_empty() {
        kit::hint(
            ui,
            "Import a fabricator's profile file to add one",
            &format!("Profiles are kept in {}", library.dir().display()),
        );
    }
    let max = if panel.all_profiles {
        profiles.len()
    } else {
        kit::LIST_MAX_ROWS
    };
    let mut chosen: Option<(usize, &'static str)> = None;
    let shown = kit::list_frame(ui, profiles.len(), max, |ui, index| {
        let Some(profile) = profiles.get(index) else {
            return;
        };
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("…", |ui| {
                    for action in ["Use for this board…", "Rename…", "Export…", "Delete…"] {
                        if ui.button(action).clicked() {
                            chosen = Some((index, action));
                            ui.close();
                        }
                    }
                });
                ui.add(
                    egui::Label::new(RichText::new(format!(
                        "{} · {}",
                        profile.name, profile.source.publisher
                    )))
                    .truncate(),
                )
                .on_hover_text(source_line(&profile.source));
            });
        });
    });
    if shown.show_all {
        panel.all_profiles = true;
    }
    if let Ok(listing) = &panel.listing
        && !listing.problems.is_empty()
    {
        let tip = listing
            .problems
            .iter()
            .map(|p| format!("{}: {}", p.file.display(), p.message))
            .collect::<Vec<_>>()
            .join("\n");
        let n = listing.problems.len();
        let text = if n == 1 {
            "1 profile file could not be read".to_owned()
        } else {
            format!("{n} profile files could not be read")
        };
        kit::hint(ui, &text, &tip);
    }
    if let Err(error) = &panel.listing {
        kit::hint(ui, "The profile folder could not be read", error);
    }
    let busy = panel.job.is_some();
    kit::actions(ui, |ui| {
        let board = with_overrides(&engine.project().rules);
        if ui_kit::button(ui, "Export…", Button::Enabled(!busy && board.is_some()))
            .on_hover_text("Write this board's rules, overrides included, to a profile file")
            .clicked()
            && let Some(profile) = board
        {
            panel.start_export(ui.ctx(), profile);
        }
        if ui_kit::button(ui, "Import…", Button::Enabled(!busy)).clicked() {
            panel.start_import(ui.ctx());
        }
    });
    let (index, action) = chosen?;
    let profile = profiles.get(index)?.clone();
    match action {
        "Use for this board…" => panel.preview(engine, profile, false),
        "Rename…" => {
            panel.mode = Mode::Rename {
                id: profile.id.clone(),
                name: profile.name.clone(),
                refusal: None,
            };
        }
        "Export…" => panel.start_export(ui.ctx(), profile),
        _ => {
            panel.mode = Mode::Delete {
                id: profile.id.clone(),
                name: profile.name.clone(),
            };
        }
    }
    None
}

/// The docked impact review of a profile switch.
/// What a review names: the option change, or the profile switched to.
fn preview_name(preview: &Preview) -> &str {
    preview
        .options
        .as_ref()
        .map_or(preview.profile.name.as_str(), |c| c.label.as_str())
}

fn preview_slot(panel: &mut RulesPanel, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
    let Mode::Preview(preview) = &mut panel.mode else {
        return None;
    };
    let stale = preview.at != engine.revision();
    let (summary, groups, blockers) = match &preview.impact {
        // A switch the engine would refuse is previewed with its reason
        // as the blocker, so Apply is not offered.
        Ok(impact) => (
            impact_summary(preview_name(preview), impact),
            impact_groups(impact),
            impact
                .refusal
                .iter()
                .map(|reason| ReviewLine {
                    level: Level::Error,
                    title: reason.clone(),
                    ack: None,
                    locate: false,
                })
                .collect(),
        ),
        Err(error) => (
            preview_name(preview).to_owned(),
            Vec::new(),
            vec![ReviewLine {
                level: Level::Error,
                title: error.clone(),
                ack: None,
                locate: false,
            }],
        ),
    };
    let (apply, title) = match &preview.options {
        Some(_) => ("Use these limits".to_owned(), "Change fab profile option"),
        None => (
            format!("Switch to {}", preview.profile.name),
            "Switch fab profile",
        ),
    };
    let review = ReviewPanel {
        id: preview_id(),
        icon: "constraint",
        title,
        summary: &summary,
        stale,
        blockers: &blockers,
        after: &[],
        groups: &groups,
        apply: &apply,
        destructive: false,
    };
    // An option change keeps the overrides; a switch asks.
    let overrides = !engine.project().rules.overrides.is_empty() && preview.options.is_none();
    let mut keep = preview.keep_overrides;
    let source = source_line(&preview.profile.source);
    let notes = match &preview.options {
        Some(change) => change.source.clone(),
        None => preview.profile.source.notes.clone(),
    };
    let action = review.show_with(ui, &mut [], preview.refusal.as_deref(), |ui| {
        kit::hint(ui, &source, &notes);
        if overrides {
            kit::check_row(ui, "Keep project overrides", &mut keep);
        }
    });
    let profile = preview.profile.clone();
    let options = preview.options.clone();
    if keep != preview.keep_overrides {
        panel.preview(engine, profile, keep);
        return None;
    }
    match action? {
        ReviewAction::Cancel => {
            panel.mode = Mode::Table;
            None
        }
        ReviewAction::Recompute => {
            match options {
                Some(change) => panel.preview_options(engine, change),
                None => panel.preview(engine, profile, keep),
            }
            None
        }
        ReviewAction::CopyReport => {
            let mut text = summary;
            for group in &groups {
                text.push_str(&format!("\n{}:", group.title));
                for line in &group.lines {
                    text.push_str(&format!("\n  {line}"));
                }
            }
            ui.ctx().copy_text(text);
            None
        }
        ReviewAction::Apply => {
            preview.refusal = None;
            preview.sent = Some(engine.revision());
            refusal::mark(ui.ctx(), Origin::Dialog(preview_id()));
            Some(Operation::Rules(match options {
                Some(change) => RulesCommand::SetOptions {
                    options: change.choices,
                },
                None => RulesCommand::SetProfile {
                    profile: Box::new(profile),
                    keep_overrides: keep,
                },
            }))
        }
        ReviewAction::Acknowledge(_) | ReviewAction::Locate(_) | ReviewAction::LocateAfter(_) => {
            None
        }
    }
}

/// The docked "Save as fab profile" dialog: name and source; Save writes the
/// board's profile with its overrides as values to the library and, when
/// asked, switches the board to the saved profile.
fn save_slot(panel: &mut RulesPanel, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
    let Mode::Save(form) = &mut panel.mode else {
        return None;
    };
    let blank = form.name.trim().is_empty();
    let mut dialog = DialogPanel::new(save_id(), "constraint", "Save as fab profile", "Save");
    dialog.kind = "Rules";
    dialog.body_rows = 10.0;
    dialog.reset_scroll = form.opening;
    dialog.refusal = form.refusal.as_deref();
    dialog.primary_disabled = if blank {
        Some("Enter a name")
    } else if form.publisher.trim().is_empty() || form.date.trim().is_empty() {
        Some("Name the publisher and the date the numbers were read")
    } else {
        None
    };
    form.opening = false;
    let action = dialog.show(ui, |ui| {
        kit::text_row(ui, "Name", &mut form.name);
        kit::text_row(ui, "Publisher", &mut form.publisher);
        kit::text_row(ui, "Document", &mut form.document);
        kit::text_row(ui, "URL", &mut form.url);
        kit::text_row(ui, "Read on", &mut form.date);
        kit::multiline_row(ui, "Notes", &mut form.notes, false);
        kit::check_row(ui, "Use it for this board", &mut form.use_it);
        kit::hint(
            ui,
            "Where the numbers come from: the fabricator's capability page and the date (YYYY-MM-DD) you read it",
            "",
        );
    });
    match action? {
        DialogAction::Cancel => {
            panel.mode = Mode::Table;
            None
        }
        DialogAction::Primary => {
            let library = panel.library.clone()?;
            let mut profile = with_overrides(&engine.project().rules)?;
            profile.name = form.name.trim().to_owned();
            profile.source = ProfileSource {
                publisher: form.publisher.trim().to_owned(),
                document: form.document.trim().to_owned(),
                url: form.url.trim().to_owned(),
                date: form.date.trim().to_owned(),
                notes: form.notes.trim().to_owned(),
            };
            match library.save_as(&profile, &profile.name) {
                Ok(stored) => {
                    let use_it = form.use_it;
                    toast::post(
                        ui.ctx(),
                        Level::Ok,
                        format!("Saved profile {}", stored.name),
                        Vec::new(),
                    );
                    panel.refresh();
                    panel.mode = Mode::Table;
                    use_it.then(|| {
                        refusal::mark(ui.ctx(), Origin::Dialog(save_id()));
                        Operation::Rules(RulesCommand::SetProfile {
                            profile: Box::new(stored),
                            keep_overrides: false,
                        })
                    })
                }
                Err(error) => {
                    form.refusal = Some(sentence(&error.to_string()));
                    None
                }
            }
        }
    }
}

/// The docked "Fab profile" dialog of a new board: one choice,
/// preselected; Use profile commits `rules.set_fab_profile`, Later closes
/// the panel and leaves the board without rules.
fn choose_slot(panel: &mut RulesPanel, ui: &mut Ui, close: &mut bool) -> Option<Operation> {
    let choices = panel.choices();
    let Mode::Choose { id, sent, refusal } = &mut panel.mode else {
        return None;
    };
    let options: Vec<(String, String)> = choices
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();
    let chosen = choices.iter().find(|p| p.id == *id).cloned();
    let mut dialog = DialogPanel::new(choose_id(), "constraint", "Fab profile", "Use profile");
    dialog.kind = "Rules";
    dialog.body_rows = 4.0;
    dialog.refusal = refusal.as_deref();
    dialog.primary_disabled = if sent.is_some() {
        Some("Setting the profile…")
    } else if chosen.is_none() {
        Some("Choose a profile")
    } else {
        None
    };
    let action = dialog.show(ui, |ui| {
        kit::hint(
            ui,
            "A board's design rules come from its fabricator's profile",
            "Change it later in Board › Rules.",
        );
        kit::choice_row(ui, "Fab profile", id, &options);
        if let Some(profile) = &chosen {
            kit::hint(ui, &source_line(&profile.source), &profile.source.notes);
        }
    });
    match action? {
        DialogAction::Cancel => {
            *close = true;
            None
        }
        DialogAction::Primary => {
            let profile = chosen?;
            *refusal = None;
            *sent = Some(panel.revision.unwrap_or_default());
            refusal::mark(ui.ctx(), Origin::Dialog(choose_id()));
            Some(Operation::Rules(RulesCommand::SetProfile {
                profile: Box::new(profile),
                keep_overrides: false,
            }))
        }
    }
}

/// The docked Rename dialog for one library profile.
fn rename_slot(panel: &mut RulesPanel, ui: &mut Ui) {
    let Mode::Rename { id, name, refusal } = &mut panel.mode else {
        return;
    };
    let blank = name.trim().is_empty();
    let mut dialog = DialogPanel::new(rename_id(), "constraint", "Rename profile", "Rename");
    dialog.kind = "Rules";
    dialog.body_rows = 2.0;
    dialog.refusal = refusal.as_deref();
    dialog.primary_disabled = blank.then_some("Enter a name");
    let action = dialog.show(ui, |ui| {
        kit::text_row(ui, "Name", name);
    });
    match action {
        Some(DialogAction::Cancel) => panel.mode = Mode::Table,
        Some(DialogAction::Primary) => {
            let result = panel
                .library
                .as_ref()
                .map(|library| library.rename(id, name));
            match result {
                Some(Ok(_)) | None => {
                    panel.refresh();
                    panel.mode = Mode::Table;
                }
                Some(Err(error)) => *refusal = Some(sentence(&error.to_string())),
            }
        }
        None => {}
    }
}

#[cfg(test)]
#[path = "rules_panel_tests.rs"]
mod tests;
