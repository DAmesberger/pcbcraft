//! Fab profiles as files: every rule and default a board reads, each a
//! value or a formula ([`super::formula`]), with the profile's source (a
//! fabricator's capability page, its URL and the date it was read) and an
//! optional source per value. Sparksmith ships no estimated profiles: a
//! profile is a file someone wrote from a source they name.
//!
//! A profile file is versioned JSON ([`ProfileFile`], format
//! [`PROFILE_FILE_FORMAT`]); a project embeds the same profile object as a
//! snapshot (`ProjectRules::profile`), as library revisions are pinned, so
//! a board reopens with exactly the rules it was checked against. A file
//! must set every rule ([`RuleKey::all`]): a missing or unknown rule is
//! refused, never filled in.
//!
//! A profile may also offer manufacturing options ([`FabOption`]): order
//! choices the fabricator publishes different limits for (a mask colour,
//! a drill process, its recommended or its minimum values). A rule then
//! lists `cases`: the value and source that apply when the named options
//! take the named choices; the rule's own value applies otherwise, and
//! the first matching case wins. The profile keeps the choice of every
//! option (`chosen`; each option's `default` until one is chosen), and its
//! resolved rules are those of its choices: a board pins the profile with
//! its choices, and changing a choice is a profile change
//! (`rules.set_fab_options`).
//!
//! ```json
//! {"format": 1, "fab_profile": {
//!   "id": "jlcpcb-2-layer", "name": "JLCPCB 2-layer",
//!   "source": {"publisher": "JLCPCB", "url": "https://…", "date": "2026-11-11"},
//!   "drill_files": {"kind": "not_published", "source": "…"},
//!   "options": [{"id": "drill", "name": "Drilling", "default": "standard",
//!     "source": "…", "choices": [{"id": "standard", "name": "Standard"},
//!                                {"id": "microvia", "name": "0.1 mm microvias"}]}],
//!   "rules": {
//!     "drill_min": {"value": 0.15, "source": "Drilling: 2-layer 0.15 – 6.3 mm",
//!       "cases": [{"when": {"drill": "microvia"}, "value": 0.1, "source": "…"}]},
//!     "via_diameter_default": {"formula": "via_drill_default + 2.0 * via_annular_ring_min"},
//!     …}}}
//! ```
use super::{
    DesignRules, MAX_PROFILE_NOTES, MAX_PROFILE_TEXT, RuleKey, RuleOverride, RuleValue,
    connection_from_name, connection_name, formula,
};
use crate::diagnostics::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The profile file's format; any other version is refused.
pub const PROFILE_FILE_FORMAT: u32 = 1;

/// Where a profile's numbers come from.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileSource {
    /// Who published the numbers: a fabricator, or for a test fixture
    /// "Sparksmith test fixture".
    pub publisher: String,
    /// The document's title.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub document: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// The ISO date (YYYY-MM-DD) the numbers were read.
    pub date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// How the fabricator reads hole sizes in drill files. Sparksmith's hole
/// sizes are always finished sizes, as the industry states them; this
/// says what the drill file and the fabrication drawing write.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DrillFiles {
    /// Drill files list finished hole sizes.
    Finished { source: String },
    /// Drill files list tool sizes: a plated hole's finished size plus
    /// `pth_allowance` mm; unplated holes are drilled at their size.
    Drilled { pth_allowance: f64, source: String },
    /// The fabricator does not publish how it reads drill files. Sparksmith
    /// writes finished sizes and the drawing says so; ask the fabricator.
    NotPublished { source: String },
}

impl DrillFiles {
    pub fn source(&self) -> &str {
        match self {
            Self::Finished { source }
            | Self::Drilled { source, .. }
            | Self::NotPublished { source } => source,
        }
    }

    /// The size a drill file lists for a hole of `finished` mm.
    pub fn file_size(&self, finished: f64, plated: bool) -> f64 {
        match self {
            Self::Drilled { pth_allowance, .. } if plated => finished + pth_allowance,
            _ => finished,
        }
    }

    /// The fabrication drawing's sentence on hole sizes.
    pub fn note(&self) -> String {
        match self {
            Self::Finished { .. } => {
                "Hole sizes are finished sizes; the drill file lists finished sizes.".into()
            }
            Self::Drilled { pth_allowance, .. } => format!(
                "Hole sizes are finished sizes; the drill file lists tool sizes: plated holes at \
                 finished size + {} mm, unplated holes at finished size.",
                super::trim(*pth_allowance)
            ),
            Self::NotPublished { .. } => "Hole sizes are finished sizes; the drill file lists \
                 finished sizes. The fabricator does not publish how it reads drill files: \
                 confirm with it."
                .into(),
        }
    }
}

/// What a rule is set to.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    Value(RuleValue),
    /// A Rhai expression of other rules ([`super::formula`]).
    Formula(String),
}

/// One rule in a profile: its setting and where it comes from ("" means
/// the profile's source), and the values that replace it under some
/// manufacturing choices.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleEntry {
    pub setting: Setting,
    pub source: String,
    /// In order; the first whose `when` the profile's choices meet wins.
    pub cases: Vec<RuleCase>,
}

/// A rule's value under some manufacturing choices.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleCase {
    /// Option id → choice id; every pair must hold.
    pub when: BTreeMap<String, String>,
    pub value: RuleValue,
    pub source: String,
}

/// A manufacturing option a profile offers: a choice the fabricator
/// publishes different limits for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FabOption {
    /// Stable id that rule cases and a board's choices name.
    pub id: String,
    /// Shown name ("Solder mask colour").
    pub name: String,
    /// The choice a board takes until one is chosen: the profile author's
    /// reading of the fabricator's standard order.
    pub default: String,
    pub choices: Vec<FabChoice>,
    /// Where the option and its choices come from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// One choice of a [`FabOption`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FabChoice {
    pub id: String,
    /// Shown name ("Black or white").
    pub name: String,
}

/// Options a profile offers, and choices one option offers.
pub const MAX_OPTIONS: usize = 16;
/// Cases one rule lists.
pub const MAX_CASES: usize = 16;

impl FabOption {
    pub fn choice(&self, id: &str) -> Option<&FabChoice> {
        self.choices.iter().find(|c| c.id == id)
    }
}

impl RuleEntry {
    pub fn value(value: RuleValue, source: &str) -> Self {
        Self {
            setting: Setting::Value(value),
            source: source.into(),
            cases: Vec::new(),
        }
    }

    pub fn formula(text: &str, source: &str) -> Self {
        Self {
            setting: Setting::Formula(text.into()),
            source: source.into(),
            cases: Vec::new(),
        }
    }

    /// The value (or formula) and source under `chosen`: the first case
    /// whose every option takes its choice, else the entry's own.
    pub fn under(&self, chosen: &BTreeMap<String, String>) -> (Setting, &str) {
        match self
            .cases
            .iter()
            .find(|c| c.when.iter().all(|(o, v)| chosen.get(o) == Some(v)))
        {
            Some(case) => (Setting::Value(case.value), case.source.as_str()),
            None => (self.setting.clone(), self.source.as_str()),
        }
    }
}

/// A named, sourced set of every rule. Its resolved [`DesignRules`] are
/// computed when it is made or read and kept with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "ProfileData", into = "ProfileData")]
#[schemars(with = "ProfileData")]
pub struct RuleProfile {
    /// Stable identity in the profile library (a slug).
    pub id: String,
    /// Shown name ("JLCPCB 2-layer").
    pub name: String,
    pub source: ProfileSource,
    pub drill_files: DrillFiles,
    /// The manufacturing options it offers, in shown order.
    options: Vec<FabOption>,
    /// The choice of every option.
    chosen: BTreeMap<String, String>,
    entries: BTreeMap<RuleKey, RuleEntry>,
    /// The rules under `chosen`.
    rules: DesignRules,
    /// The rules its formulas name: an override of any other rule leaves
    /// every formula's value as resolved.
    formula_inputs: BTreeSet<RuleKey>,
}

impl RuleProfile {
    /// A profile from every rule's entry; a missing rule, a formula error
    /// or a value out of bounds is refused.
    pub fn new(
        id: &str,
        name: &str,
        source: ProfileSource,
        drill_files: DrillFiles,
        entries: BTreeMap<RuleKey, RuleEntry>,
    ) -> Result<Self> {
        let entries: BTreeMap<RuleKey, RuleEntry> = entries
            .into_iter()
            .map(|(k, e)| (k.normalized(), e))
            .collect();
        if let Some(missing) = RuleKey::all()
            .into_iter()
            .find(|k| !entries.contains_key(k))
        {
            return Err(Error::Invalid(format!(
                "rules.fab_profile_incomplete: the profile does not set {}",
                missing.name()
            )));
        }
        let mut profile = Self {
            id: id.into(),
            name: name.into(),
            source,
            drill_files,
            options: Vec::new(),
            chosen: BTreeMap::new(),
            entries,
            rules: DesignRules::unset(),
            formula_inputs: BTreeSet::new(),
        };
        profile.formula_inputs = profile.inputs();
        profile.rules = profile.resolve_with(&[])?;
        profile.validate()?;
        Ok(profile)
    }

    /// This profile offering `options`, each at its default choice, with
    /// `cases` for its rules (rule → cases, in order). The options and the
    /// cases must name each other ([`Self::validate`]).
    pub fn with_options(
        mut self,
        options: Vec<FabOption>,
        cases: BTreeMap<RuleKey, Vec<RuleCase>>,
    ) -> Result<Self> {
        for (key, list) in cases {
            let Some(entry) = self.entries.get_mut(&key.normalized()) else {
                return Err(Error::Invalid(format!(
                    "rules.fab_option: {} is not a rule of the profile",
                    key.name()
                )));
            };
            entry.cases = list;
        }
        self.chosen = options
            .iter()
            .map(|o| (o.id.clone(), o.default.clone()))
            .collect();
        self.options = options;
        self.check_options()?;
        self.rules = self.resolve_with(&[])?;
        self.validate()?;
        Ok(self)
    }

    /// The manufacturing options, in shown order.
    pub fn options(&self) -> &[FabOption] {
        &self.options
    }

    /// The choice of every option (option id → choice id).
    pub fn chosen(&self) -> &BTreeMap<String, String> {
        &self.chosen
    }

    /// This profile with `choices` (option id → choice id) in place of the
    /// current ones; options not named keep theirs. An unknown option or
    /// choice, or rules out of bounds under the choices, is refused.
    pub fn with_choices(&self, choices: &BTreeMap<String, String>) -> Result<Self> {
        let mut next = self.clone();
        for (option, choice) in choices {
            let Some(o) = next.options.iter().find(|o| o.id == *option) else {
                return Err(Error::Invalid(format!(
                    "rules.fab_option: {} offers no option {option}{}",
                    next.name,
                    offered(next.options.iter().map(|o| o.id.as_str()))
                )));
            };
            if o.choice(choice).is_none() {
                return Err(Error::Invalid(format!(
                    "rules.fab_option: {} offers no choice {choice}{}",
                    o.name,
                    offered(o.choices.iter().map(|c| c.id.as_str()))
                )));
            }
            next.chosen.insert(option.clone(), choice.clone());
        }
        next.rules = next.resolve_with(&[])?;
        next.validate()?;
        Ok(next)
    }

    /// The shown name of the choice `option` takes ("" when the profile
    /// offers no such option).
    pub fn choice_name(&self, option: &str) -> &str {
        self.options
            .iter()
            .find(|o| o.id == option)
            .and_then(|o| self.chosen.get(option).and_then(|c| o.choice(c)))
            .map_or("", |c| c.name.as_str())
    }

    /// Whether `other` is this profile with other choices.
    pub fn same_but_choices(&self, other: &Self) -> bool {
        other
            .with_choices(&self.chosen)
            .is_ok_and(|other| other == *self)
    }

    /// Options and cases name each other: unique, bounded ids; a default
    /// and a choice of every option among its choices; every case naming
    /// options and choices the profile offers, with a value of its rule's
    /// kind.
    fn check_options(&self) -> Result<()> {
        let bad = |what: String| Err(Error::Invalid(format!("rules.fab_option: {what}")));
        let printable = |t: &str| {
            !t.trim().is_empty() && t.len() <= MAX_PROFILE_TEXT && !t.chars().any(char::is_control)
        };
        let offers = |option: &str, choice: &str| {
            self.options
                .iter()
                .any(|o| o.id == option && o.choice(choice).is_some())
        };
        if self.options.len() > MAX_OPTIONS {
            return bad(format!("a profile offers at most {MAX_OPTIONS} options"));
        }
        let mut ids = BTreeSet::new();
        for o in &self.options {
            if !printable(&o.id) || !printable(&o.name) || o.source.len() > MAX_PROFILE_NOTES {
                return bad("an option needs an id and a name; texts are bounded".into());
            }
            if !ids.insert(o.id.as_str()) {
                return bad(format!("{} is offered twice", o.id));
            }
            if o.choices.is_empty() || o.choices.len() > MAX_OPTIONS {
                return bad(format!("{} offers 1–{MAX_OPTIONS} choices", o.name));
            }
            let mut choices = BTreeSet::new();
            for c in &o.choices {
                if !printable(&c.id) || !printable(&c.name) || !choices.insert(c.id.as_str()) {
                    return bad(format!("{}'s choices need distinct ids and names", o.name));
                }
            }
            if o.choice(&o.default).is_none() {
                return bad(format!(
                    "{}'s default {} is not a choice",
                    o.name, o.default
                ));
            }
        }
        if self.chosen.len() != self.options.len() || self.chosen.iter().any(|(o, c)| !offers(o, c))
        {
            return bad("every option needs one of its choices".into());
        }
        for (key, entry) in &self.entries {
            if entry.cases.len() > MAX_CASES {
                return bad(format!("{} lists more than {MAX_CASES} cases", key.name()));
            }
            for case in &entry.cases {
                if case.when.is_empty() || case.source.len() > MAX_PROFILE_NOTES {
                    return bad(format!(
                        "a case of {} names no option, or its source is too long",
                        key.name()
                    ));
                }
                if let Some((o, c)) = case.when.iter().find(|(o, c)| !offers(o, c)) {
                    return bad(format!(
                        "a case of {} names {o}: {c}, which the profile does not offer",
                        key.name()
                    ));
                }
                if std::mem::discriminant(&case.value) != std::mem::discriminant(&key.kind()) {
                    return bad(format!(
                        "a case of {} takes a different kind of value",
                        key.name()
                    ));
                }
            }
        }
        Ok(())
    }

    /// A profile of plain values: every rule of `rules`, each with
    /// `value_source` as its source.
    pub fn from_values(
        id: &str,
        name: &str,
        source: ProfileSource,
        drill_files: DrillFiles,
        rules: &DesignRules,
        value_source: &str,
    ) -> Result<Self> {
        let entries = RuleKey::all()
            .into_iter()
            .map(|k| (k, RuleEntry::value(k.get(rules), value_source)))
            .collect();
        Self::new(id, name, source, drill_files, entries)
    }

    /// The resolved rules (values and formulas under the chosen options,
    /// no project overrides).
    pub fn rules(&self) -> &DesignRules {
        &self.rules
    }

    /// One rule's entry, with its cases.
    pub fn entry(&self, key: RuleKey) -> Option<&RuleEntry> {
        self.entries.get(&key.normalized())
    }

    /// One rule's value (or formula) and source under the chosen options.
    pub fn active(&self, key: RuleKey) -> Option<(Setting, &str)> {
        self.entry(key).map(|e| e.under(&self.chosen))
    }

    /// Every rule's entry, in key order.
    pub fn entries(&self) -> impl Iterator<Item = (RuleKey, &RuleEntry)> {
        self.entries.iter().map(|(k, e)| (*k, e))
    }

    /// Whether any rule is a formula.
    pub fn has_formulas(&self) -> bool {
        self.entries
            .values()
            .any(|e| matches!(e.setting, Setting::Formula(_)))
    }

    /// Replace one rule's entry; on failure the profile is unchanged.
    pub fn set(&mut self, key: RuleKey, entry: RuleEntry) -> Result<()> {
        let mut next = self.clone();
        next.entries.insert(key.normalized(), entry);
        next.formula_inputs = next.inputs();
        next.rules = next.resolve_with(&[])?;
        next.validate()?;
        *self = next;
        Ok(())
    }

    fn inputs(&self) -> BTreeSet<RuleKey> {
        formula::inputs(self.entries.values().filter_map(|e| match &e.setting {
            Setting::Formula(text) => Some(text.as_str()),
            Setting::Value(_) => None,
        }))
    }

    /// The rules with `overrides` in place of their profile entries (an
    /// override replaces a formula too), formulas evaluated after. Overrides
    /// no formula names only replace their values in the resolved rules.
    pub fn resolve_with(&self, overrides: &[RuleOverride]) -> Result<DesignRules> {
        if !overrides.is_empty()
            && overrides.iter().all(|o| {
                let key = o.key.normalized();
                !self.formula_inputs.contains(&key)
                    && self
                        .entries
                        .get(&key)
                        .is_some_and(|e| matches!(e.under(&self.chosen).0, Setting::Value(_)))
            })
        {
            let mut rules = self.rules.clone();
            for o in overrides {
                o.key.set(&mut rules, o.value)?;
            }
            return Ok(rules);
        }
        let mut rules = DesignRules::unset();
        let mut formulas = Vec::new();
        for (key, entry) in &self.entries {
            if let Some(o) = overrides.iter().find(|o| o.key.normalized() == *key) {
                key.set(&mut rules, o.value)?;
                continue;
            }
            match entry.under(&self.chosen).0 {
                Setting::Value(v) => key.set(&mut rules, v)?,
                Setting::Formula(text) => formulas.push((*key, text)),
            }
        }
        let formulas: Vec<(RuleKey, &str)> =
            formulas.iter().map(|(k, t)| (*k, t.as_str())).collect();
        formula::evaluate(&mut rules, &formulas)?;
        Ok(rules)
    }

    /// Text bounds, the source's publisher and date, then the resolved
    /// rules' bounds.
    pub fn validate(&self) -> Result<()> {
        let s = &self.source;
        let short = [
            &self.id,
            &self.name,
            &s.publisher,
            &s.document,
            &s.url,
            &s.date,
        ];
        let long = std::iter::once(&s.notes)
            .chain(std::iter::once(&self.drill_files.source().to_owned()))
            .chain(self.entries.values().map(|e| &e.source))
            .cloned()
            .collect::<Vec<_>>();
        let printable = |t: &str| !t.chars().any(|c| c.is_control() && c != '\n');
        if self.id.trim().is_empty()
            || self.name.trim().is_empty()
            || short
                .iter()
                .any(|t| t.len() > MAX_PROFILE_TEXT || !printable(t))
            || long
                .iter()
                .any(|t| t.len() > MAX_PROFILE_NOTES || !printable(t))
        {
            return Err(Error::Invalid(
                "rules.fab_profile_text: a profile needs an id and a name; texts are bounded and printable"
                    .into(),
            ));
        }
        self.check_options()?;
        if s.publisher.trim().is_empty() || !valid_date(&s.date) {
            return Err(Error::Invalid(format!(
                "rules.fab_profile_source: {} must name who published its numbers and the date \
                 (YYYY-MM-DD) they were read",
                self.name
            )));
        }
        if let DrillFiles::Drilled { pth_allowance, .. } = self.drill_files
            && !(pth_allowance.is_finite() && (0.0..=1.0).contains(&pth_allowance))
        {
            return Err(Error::Invalid(
                "rules.bounds: the drill files' plating allowance must be 0–1 mm".into(),
            ));
        }
        self.rules.validate()
    }
}

/// A `YYYY-MM-DD` date.
pub fn valid_date(date: &str) -> bool {
    let number = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = date.split('-').collect();
    let [y, m, d] = parts.as_slice() else {
        return false;
    };
    if !(number(y, 4) && number(m, 2) && number(d, 2)) {
        return false;
    }
    let (m, d) = (m.parse::<u32>().unwrap_or(0), d.parse::<u32>().unwrap_or(0));
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// " (it offers a, b)" for an error, or "" when nothing is offered.
fn offered<'a>(ids: impl Iterator<Item = &'a str>) -> String {
    let ids: Vec<&str> = ids.collect();
    if ids.is_empty() {
        String::new()
    } else {
        format!(" (it offers {})", ids.join(", "))
    }
}

/// One fab profile file: [`PROFILE_FILE_FORMAT`] and the profile
/// (`fab_profile`).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfileFile {
    pub format: u32,
    #[serde(rename = "fab_profile")]
    pub profile: RuleProfile,
}

/// The stored form of a [`RuleProfile`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileData {
    pub id: String,
    pub name: String,
    pub source: ProfileSource,
    pub drill_files: DrillFiles,
    /// The manufacturing options it offers, in shown order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<FabOption>,
    /// The choice of options (option id → choice id); an option not named
    /// takes its `default`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub chosen: BTreeMap<String, String>,
    /// Every rule by name ([`RuleKey::name`]): `{"value": …}` or
    /// `{"formula": "…"}`, each with an optional `"source"` and `"cases"`.
    pub rules: BTreeMap<String, EntryData>,
}

/// One stored rule: exactly one of `value` and `formula`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntryData {
    /// The value: a number (mm, mm², degrees, a share or a ratio), `null`
    /// for an unchecked ratio, an integer count, a boolean, or a pad
    /// connection name (`solid`, `thermal`, `none`).
    #[serde(default, skip_serializing_if = "Missing::is_missing")]
    pub value: Missing,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Values that apply under some manufacturing choices, in order: the
    /// first whose `when` holds wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cases: Vec<CaseData>,
}

/// One stored rule case: `{"when": {"option": "choice"}, "value": …,
/// "source": "…"}`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseData {
    pub when: BTreeMap<String, String>,
    #[schemars(with = "Value")]
    pub value: Value,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// A JSON value that may be absent (absent differs from `null`).
#[derive(Clone, Debug, Default, PartialEq, JsonSchema)]
#[schemars(with = "Value")]
pub struct Missing(pub Option<Value>);

impl Missing {
    fn is_missing(&self) -> bool {
        self.0.is_none()
    }
}

impl Serialize for Missing {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.as_ref().unwrap_or(&Value::Null).serialize(s)
    }
}

impl<'de> Deserialize<'de> for Missing {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(Self(Some(Value::deserialize(d)?)))
    }
}

fn to_json(value: RuleValue) -> Value {
    match value {
        RuleValue::Length(v)
        | RuleValue::Area(v)
        | RuleValue::Angle(v)
        | RuleValue::Fraction(v)
        | RuleValue::Ratio(Some(v)) => Value::from(v),
        RuleValue::Ratio(None) => Value::Null,
        RuleValue::Count(n) => Value::from(n),
        RuleValue::Flag(f) => Value::from(f),
        RuleValue::Connection(c) => Value::from(connection_name(c)),
    }
}

fn from_json(key: RuleKey, value: &Value) -> Option<RuleValue> {
    Some(match key.kind() {
        RuleValue::Length(_) => RuleValue::Length(value.as_f64()?),
        RuleValue::Area(_) => RuleValue::Area(value.as_f64()?),
        RuleValue::Angle(_) => RuleValue::Angle(value.as_f64()?),
        RuleValue::Fraction(_) => RuleValue::Fraction(value.as_f64()?),
        RuleValue::Ratio(_) if value.is_null() => RuleValue::Ratio(None),
        RuleValue::Ratio(_) => RuleValue::Ratio(Some(value.as_f64()?)),
        RuleValue::Count(_) => RuleValue::Count(u32::try_from(value.as_u64()?).ok()?),
        RuleValue::Flag(_) => RuleValue::Flag(value.as_bool()?),
        RuleValue::Connection(_) => RuleValue::Connection(connection_from_name(value.as_str()?)?),
    })
}

impl From<RuleProfile> for ProfileData {
    fn from(p: RuleProfile) -> Self {
        let rules = p
            .entries
            .into_iter()
            .map(|(key, entry)| {
                let (value, formula) = match entry.setting {
                    Setting::Value(v) => (Missing(Some(to_json(v))), None),
                    Setting::Formula(f) => (Missing(None), Some(f)),
                };
                (
                    key.name(),
                    EntryData {
                        value,
                        formula,
                        source: entry.source,
                        cases: entry
                            .cases
                            .into_iter()
                            .map(|c| CaseData {
                                when: c.when,
                                value: to_json(c.value),
                                source: c.source,
                            })
                            .collect(),
                    },
                )
            })
            .collect();
        Self {
            id: p.id,
            name: p.name,
            source: p.source,
            drill_files: p.drill_files,
            options: p.options,
            chosen: p.chosen,
            rules,
        }
    }
}

impl TryFrom<ProfileData> for RuleProfile {
    type Error = Error;

    fn try_from(data: ProfileData) -> Result<Self> {
        let mut entries = BTreeMap::new();
        let mut cases = BTreeMap::new();
        for (name, entry) in data.rules {
            let key = RuleKey::from_name(&name).ok_or_else(|| {
                Error::Invalid(format!("rules.unknown_rule: {name} is not a rule"))
            })?;
            let setting = match (entry.value.0, entry.formula) {
                (Some(value), None) => Setting::Value(from_json(key, &value).ok_or_else(|| {
                    Error::Invalid(format!(
                        "rules.value_kind: {name} takes a different kind of value than {value}"
                    ))
                })?),
                (None, Some(text)) => {
                    formula::check_text(key, &text)?;
                    Setting::Formula(text)
                }
                _ => {
                    return Err(Error::Invalid(format!(
                        "rules.entry: {name} needs exactly one of value and formula"
                    )));
                }
            };
            if !entry.cases.is_empty() {
                let list = entry
                    .cases
                    .into_iter()
                    .map(|c| {
                        let value = from_json(key, &c.value).ok_or_else(|| {
                            Error::Invalid(format!(
                                "rules.value_kind: a case of {name} takes a different kind of \
                                 value than {}",
                                c.value
                            ))
                        })?;
                        Ok(RuleCase {
                            when: c.when,
                            value,
                            source: c.source,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                cases.insert(key, list);
            }
            if entries
                .insert(
                    key,
                    RuleEntry {
                        setting,
                        source: entry.source,
                        cases: Vec::new(),
                    },
                )
                .is_some()
            {
                return Err(Error::Invalid(format!(
                    "rules.duplicate_rule: {name} is set twice"
                )));
            }
        }
        // The rules without options first: they must be valid on their
        // own; then the options, their cases and the stored choices.
        let profile = Self::new(&data.id, &data.name, data.source, data.drill_files, entries)?;
        if data.options.is_empty() && cases.is_empty() && data.chosen.is_empty() {
            return Ok(profile);
        }
        profile
            .with_options(data.options, cases)?
            .with_choices(&data.chosen)
    }
}
