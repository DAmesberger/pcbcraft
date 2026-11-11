//! Rule profiles as files: every rule and default a board reads, each a
//! value or a formula ([`super::formula`]), with the profile's source (a
//! fabricator's capability page, its URL and the date it was read) and an
//! optional source per value. Pcbcraft ships no estimated profiles: a
//! profile is a file someone wrote from a source they name.
//!
//! A profile file is versioned JSON ([`ProfileFile`], format
//! [`PROFILE_FILE_FORMAT`]); a project embeds the same profile object as a
//! snapshot (`ProjectRules::profile`), as library revisions are pinned, so
//! a board reopens with exactly the rules it was checked against. A file
//! must set every rule ([`RuleKey::all`]): a missing or unknown rule is
//! refused, never filled in.
//!
//! ```json
//! {"format": 1, "profile": {
//!   "id": "jlcpcb-2-layer", "name": "JLCPCB 2-layer",
//!   "source": {"publisher": "JLCPCB", "url": "https://…", "date": "2026-11-11"},
//!   "drill_files": {"kind": "not_published", "source": "…"},
//!   "rules": {
//!     "drill_min": {"value": 0.15, "source": "Drilling: 2-layer 0.15 – 6.3 mm"},
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
    /// "Pcbcraft test fixture".
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

/// How the fabricator reads hole sizes in drill files. Pcbcraft's hole
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
    /// The fabricator does not publish how it reads drill files. Pcbcraft
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
/// the profile's source).
#[derive(Clone, Debug, PartialEq)]
pub struct RuleEntry {
    pub setting: Setting,
    pub source: String,
}

impl RuleEntry {
    pub fn value(value: RuleValue, source: &str) -> Self {
        Self {
            setting: Setting::Value(value),
            source: source.into(),
        }
    }

    pub fn formula(text: &str, source: &str) -> Self {
        Self {
            setting: Setting::Formula(text.into()),
            source: source.into(),
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
    entries: BTreeMap<RuleKey, RuleEntry>,
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
                "rules.profile_incomplete: the profile does not set {}",
                missing.name()
            )));
        }
        let mut profile = Self {
            id: id.into(),
            name: name.into(),
            source,
            drill_files,
            entries,
            rules: DesignRules::unset(),
            formula_inputs: BTreeSet::new(),
        };
        profile.formula_inputs = profile.inputs();
        profile.rules = profile.resolve_with(&[])?;
        profile.validate()?;
        Ok(profile)
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

    /// The resolved rules (values and formulas, no project overrides).
    pub fn rules(&self) -> &DesignRules {
        &self.rules
    }

    pub fn entry(&self, key: RuleKey) -> Option<&RuleEntry> {
        self.entries.get(&key.normalized())
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
                        .is_some_and(|e| matches!(e.setting, Setting::Value(_)))
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
            match &entry.setting {
                Setting::Value(v) => key.set(&mut rules, *v)?,
                Setting::Formula(text) => formulas.push((*key, text.as_str())),
            }
        }
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
                "rules.profile_text: a profile needs an id and a name; texts are bounded and printable"
                    .into(),
            ));
        }
        if s.publisher.trim().is_empty() || !valid_date(&s.date) {
            return Err(Error::Invalid(format!(
                "rules.profile_source: {} must name who published its numbers and the date \
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

/// One profile file: [`PROFILE_FILE_FORMAT`] and the profile.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfileFile {
    pub format: u32,
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
    /// Every rule by name ([`RuleKey::name`]): `{"value": …}` or
    /// `{"formula": "…"}`, each with an optional `"source"`.
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
                    },
                )
            })
            .collect();
        Self {
            id: p.id,
            name: p.name,
            source: p.source,
            drill_files: p.drill_files,
            rules,
        }
    }
}

impl TryFrom<ProfileData> for RuleProfile {
    type Error = Error;

    fn try_from(data: ProfileData) -> Result<Self> {
        let mut entries = BTreeMap::new();
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
            if entries
                .insert(
                    key,
                    RuleEntry {
                        setting,
                        source: entry.source,
                    },
                )
                .is_some()
            {
                return Err(Error::Invalid(format!(
                    "rules.duplicate_rule: {name} is set twice"
                )));
            }
        }
        Self::new(&data.id, &data.name, data.source, data.drill_files, entries)
    }
}
