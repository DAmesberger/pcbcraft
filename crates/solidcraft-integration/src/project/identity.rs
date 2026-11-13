//! Which Sparksmith object each native body came from, and each body's kind,
//! saved compactly in the native file's metadata so identity survives
//! reopening.
//!
//! Solidcraft bounds a design's metadata (256 keys, 64 KiB a value, 256 KiB
//! in all), and a board has tens of thousands of pads, tracks and vias, so
//! nothing is listed one object at a time. The metadata holds
//! `org.sparksmith.electronics` (`{"version": 1}`) and records under
//! `org.sparksmith.electronics.identity.0`, `.1`, … (each `{"records": […]}`,
//! at most [`CHUNK_BYTES`]):
//!
//! - **Sources** are columns over the sources in pick order: each number
//!   column is a [`Column`] (differences from the row before, repeats
//!   run-length coded, periodic runs as repeated groups), so the
//!   consecutive picks, occurrences and ids of a batch take a few bytes in
//!   all. Nets are a table and an index per row; labels are templates like
//!   body names' below (`Via {}`, `R{} pad {}`), an index per row and
//!   number columns.
//! - **Bodies** group body names by kind and template: every whole number
//!   in a name becomes `{}` (a literal `{` is written `{{`), and the numbers
//!   of each name are columns, so `pad_12_3`, `pad_12_4`, … are one
//!   template and two short columns.
use super::Source;
use crate::material::BodyKind;
use crate::native::Native;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sparksmith_core::{CopperLayer, Id, PadId};
use std::collections::BTreeMap;
use std::ops::Range;

pub(super) const METADATA_KEY: &str = "org.sparksmith.electronics";
const RECORDS_KEY: &str = "org.sparksmith.electronics.identity.";
const VERSION: u64 = 1;
/// Largest record value, below Solidcraft's 64 KiB a value.
pub(super) const CHUNK_BYTES: usize = 48 * 1024;
/// Most record values; Solidcraft allows 256 keys, the models take others.
const MAX_VALUES: usize = 64;
/// Most rows one decoded column may hold (a bound on a file's claims).
const MAX_ROWS: usize = 1 << 20;
/// Most numbers in one body name a template takes; longer runs stay text.
const MAX_NAME_NUMBERS: usize = 16;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Record {
    Sources(Box<Sources>),
    Bodies(Bodies),
}

/// A run of sources in pick order, by column.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sources {
    picks: Column,
    components: Column,
    /// `None` is 0 and `Some(v)` is `v + 1` in these.
    occurrences: Column,
    instances: Column,
    pads: Column,
    tracks: Column,
    vias: Column,
    fills: Column,
    /// The nets the rows name, and each row's index into them.
    net_table: Vec<Option<Id>>,
    nets: Column,
    /// 0 none, 1 top, 2 bottom, `n + 3` inner `n`.
    layers: Column,
    /// The label templates the rows use (see [`template`]), each row's
    /// index into them, and the labels' numbers: column `k` holds the `k`th
    /// number of every row whose label has more than `k`.
    label_table: Vec<String>,
    labels: Column,
    label_numbers: Vec<Column>,
}

/// Body names of one kind and template.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bodies {
    kind: BodyKind,
    template: String,
    count: usize,
    /// One column per `{}` in the template.
    columns: Vec<Column>,
}

/// Numbers as text: comma-separated tokens, each a value with an optional
/// `*n` repeat, or a group of space-separated such tokens repeated, as
/// `(3 0*3)*1024`; a delta column holds each value less the one before (the
/// first less 0). Groups make a periodic run (a pad number column 1, 2, 3,
/// 4, 1, 2, …) as short as a constant one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
struct Column(String);

/// Longest group a column repeats, in tokens.
const MAX_PERIOD: usize = 8;

impl Column {
    fn values(values: impl IntoIterator<Item = i128>) -> Self {
        // Runs of one value first, then repeated groups of runs.
        let mut runs: Vec<(i128, usize)> = Vec::new();
        for value in values {
            match runs.last_mut() {
                Some((last, count)) if *last == value => *count += 1,
                _ => runs.push((value, 1)),
            }
        }
        let token = |(value, count): (i128, usize)| {
            if count > 1 {
                format!("{value}*{count}")
            } else {
                value.to_string()
            }
        };
        let mut tokens: Vec<String> = Vec::new();
        let mut at = 0;
        while at < runs.len() {
            // The period covering the most runs from here, repeated twice
            // or more.
            let mut best = (1, 1);
            for period in 2..=MAX_PERIOD {
                let Some(group) = runs.get(at..at + period) else {
                    break;
                };
                let mut repeats = 1;
                while runs.get(at + repeats * period..at + (repeats + 1) * period) == Some(group) {
                    repeats += 1;
                }
                if repeats > 1 && period * repeats > best.0 * best.1 {
                    best = (period, repeats);
                }
            }
            let (period, repeats) = best;
            if repeats > 1 {
                let group: Vec<String> = runs
                    .get(at..at + period)
                    .unwrap_or_default()
                    .iter()
                    .map(|r| token(*r))
                    .collect();
                tokens.push(format!("({})*{repeats}", group.join(" ")));
            } else if let Some(run) = runs.get(at) {
                tokens.push(token(*run));
            }
            at += period * repeats;
        }
        Self(tokens.join(","))
    }

    fn deltas(values: impl IntoIterator<Item = i128>) -> Self {
        let mut last = 0i128;
        Self::values(values.into_iter().map(|v| {
            let d = v - last;
            last = v;
            d
        }))
    }

    /// The values [`Self::values`] wrote, at most `limit` of them (a
    /// column claiming more is broken, refused before it is expanded).
    fn read_values(&self, limit: usize) -> Result<Vec<i128>> {
        let mut out = Vec::new();
        if self.0.is_empty() {
            return Ok(out);
        }
        let run = |token: &str| -> Result<(i128, usize)> {
            let (value, count) = match token.split_once('*') {
                Some((value, count)) => (value, count.parse::<usize>().map_err(|_| broken())?),
                None => (token, 1),
            };
            Ok((value.parse::<i128>().map_err(|_| broken())?, count))
        };
        let push = |out: &mut Vec<i128>, (value, count): (i128, usize)| {
            if count == 0 || count > limit.min(MAX_ROWS).saturating_sub(out.len()) {
                return Err(broken());
            }
            out.extend(std::iter::repeat_n(value, count));
            Ok(())
        };
        for token in self.0.split(',') {
            let Some(group) = token.strip_prefix('(') else {
                push(&mut out, run(token)?)?;
                continue;
            };
            let (group, repeats) = group.split_once(")*").ok_or_else(broken)?;
            let repeats = repeats.parse::<usize>().map_err(|_| broken())?;
            let runs = group.split(' ').map(run).collect::<Result<Vec<_>>>()?;
            if runs.len() > MAX_PERIOD || repeats == 0 {
                return Err(broken());
            }
            for _ in 0..repeats {
                for r in &runs {
                    push(&mut out, *r)?;
                }
            }
        }
        Ok(out)
    }

    fn read_deltas(&self, limit: usize) -> Result<Vec<i128>> {
        let mut last = 0i128;
        self.read_values(limit)?
            .into_iter()
            .map(|d| {
                last = last.checked_add(d).ok_or_else(broken)?;
                Ok(last)
            })
            .collect()
    }
}

fn broken() -> Error {
    Error::Verification("the native model's Sparksmith identity records are inconsistent".into())
}

fn opt(value: Option<u64>) -> i128 {
    value.map_or(0, |v| i128::from(v) + 1)
}

fn unopt(value: i128) -> Result<Option<u64>> {
    if value == 0 {
        return Ok(None);
    }
    u64::try_from(value - 1).map(Some).map_err(|_| broken())
}

fn exact(value: i128) -> Result<u64> {
    u64::try_from(value).map_err(|_| broken())
}

fn layer_code(layer: Option<CopperLayer>) -> i128 {
    match layer {
        None => 0,
        Some(CopperLayer::Top) => 1,
        Some(CopperLayer::Bottom) => 2,
        Some(CopperLayer::Inner(n)) => i128::from(n) + 3,
    }
}

fn layer_of(code: i128) -> Result<Option<CopperLayer>> {
    Ok(match code {
        0 => None,
        1 => Some(CopperLayer::Top),
        2 => Some(CopperLayer::Bottom),
        n => Some(CopperLayer::Inner(
            u8::try_from(n - 3).map_err(|_| broken())?,
        )),
    })
}

impl Sources {
    fn of(rows: &[&Source]) -> Self {
        let mut net_table: Vec<Option<Id>> = Vec::new();
        let mut net_index: BTreeMap<Option<Id>, usize> = BTreeMap::new();
        let nets: Vec<i128> = rows
            .iter()
            .map(|s| {
                let next = net_index.len();
                let index = *net_index.entry(s.net).or_insert_with(|| {
                    net_table.push(s.net);
                    next
                });
                i128::try_from(index).unwrap_or(i128::MAX)
            })
            .collect();
        let mut label_table: Vec<String> = Vec::new();
        let mut label_index: BTreeMap<String, usize> = BTreeMap::new();
        let mut labels = Vec::with_capacity(rows.len());
        let mut numbers: Vec<Vec<i128>> = Vec::new();
        for s in rows {
            let (form, values) = template(&s.label);
            let index = match label_index.get(&form) {
                Some(index) => *index,
                None => {
                    label_table.push(form.clone());
                    label_index.insert(form, label_table.len() - 1);
                    label_table.len() - 1
                }
            };
            labels.push(i128::try_from(index).unwrap_or(i128::MAX));
            for (k, value) in values.into_iter().enumerate() {
                if numbers.len() <= k {
                    numbers.push(Vec::new());
                }
                if let Some(column) = numbers.get_mut(k) {
                    column.push(i128::from(value));
                }
            }
        }
        let column = |f: &dyn Fn(&Source) -> i128| Column::deltas(rows.iter().map(|s| f(s)));
        Self {
            picks: column(&|s| i128::from(s.pick_id)),
            components: column(&|s| i128::from(s.component)),
            occurrences: column(&|s| opt(s.occurrence)),
            instances: column(&|s| opt(s.instance)),
            pads: column(&|s| opt(s.pad.map(|p| u64::from(p.0)))),
            tracks: column(&|s| opt(s.track)),
            vias: column(&|s| opt(s.via)),
            fills: column(&|s| opt(s.fill)),
            net_table,
            nets: Column::values(nets),
            layers: Column::values(rows.iter().map(|s| layer_code(s.layer))),
            label_table,
            labels: Column::values(labels),
            label_numbers: numbers.into_iter().map(Column::deltas).collect(),
        }
    }

    /// The sources, at most `budget` of them: every other column holds
    /// as many values as there are picks, so a record decodes to a bounded
    /// size whatever it claims.
    fn read(&self, budget: usize) -> Result<Vec<Source>> {
        let picks = self.picks.read_deltas(budget)?;
        let rows = picks.len();
        let column = |c: &Column| -> Result<Vec<i128>> {
            let values = c.read_deltas(rows)?;
            if values.len() != rows {
                return Err(broken());
            }
            Ok(values)
        };
        let components = column(&self.components)?;
        let occurrences = column(&self.occurrences)?;
        let instances = column(&self.instances)?;
        let pads = column(&self.pads)?;
        let tracks = column(&self.tracks)?;
        let vias = column(&self.vias)?;
        let fills = column(&self.fills)?;
        let nets = self.nets.read_values(rows)?;
        let layers = self.layers.read_values(rows)?;
        if nets.len() != rows || layers.len() != rows {
            return Err(broken());
        }
        let labels = self.labels.read_values(rows)?;
        if labels.len() != rows {
            return Err(broken());
        }
        let templates = self
            .label_table
            .iter()
            .map(|t| Ok((t.as_str(), placeholders(t)?)))
            .collect::<Result<Vec<_>>>()?;
        if self.label_numbers.len() > MAX_NAME_NUMBERS {
            return Err(broken());
        }
        let numbers = self
            .label_numbers
            .iter()
            .map(|c| {
                c.read_deltas(rows)?
                    .into_iter()
                    .map(exact)
                    .collect::<Result<Vec<u64>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        let mut taken = vec![0usize; numbers.len()];
        let mut out = Vec::with_capacity(rows);
        for row in 0..rows {
            let at = |values: &[i128]| values.get(row).copied().ok_or_else(broken);
            let via = unopt(at(&vias)?)?;
            let track = unopt(at(&tracks)?)?;
            let fill_id = unopt(at(&fills)?)?;
            let index = usize::try_from(at(&labels)?).map_err(|_| broken())?;
            let (template, count) = templates.get(index).copied().ok_or_else(broken)?;
            let mut values = Vec::with_capacity(count);
            for (column, taken) in numbers.iter().zip(&mut taken).take(count) {
                values.push(*column.get(*taken).ok_or_else(broken)?);
                *taken += 1;
            }
            let label = fill(template, &values)?;
            let net = usize::try_from(at(&nets)?).map_err(|_| broken())?;
            let pad = unopt(at(&pads)?)?
                .map(|p| u32::try_from(p).map(PadId).map_err(|_| broken()))
                .transpose()?;
            out.push(Source {
                pick_id: exact(at(&picks)?)?,
                instance: unopt(at(&instances)?)?,
                pad,
                track,
                via,
                fill: fill_id,
                net: *self.net_table.get(net).ok_or_else(broken)?,
                layer: layer_of(at(&layers)?)?,
                label,
                component: exact(at(&components)?)?,
                occurrence: unopt(at(&occurrences)?)?,
            });
        }
        if numbers.iter().zip(&taken).any(|(c, t)| c.len() != *t) {
            return Err(broken());
        }
        Ok(out)
    }
}

/// A body name as a template and its numbers: each maximal run of digits
/// that is a whole number as written (no leading zero, at most 18 digits)
/// becomes `{}`; a literal `{` becomes `{{`.
fn template(name: &str) -> (String, Vec<u64>) {
    let mut template = String::with_capacity(name.len());
    let mut numbers = Vec::new();
    let mut rest = name;
    while let Some(c) = rest.chars().next() {
        if c.is_ascii_digit() {
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let (digits, tail) = rest.split_at(end);
            let number = (digits.len() <= 18 && (digits == "0" || !digits.starts_with('0')))
                .then(|| digits.parse::<u64>().ok())
                .flatten()
                .filter(|_| numbers.len() < MAX_NAME_NUMBERS);
            match number {
                Some(n) => {
                    numbers.push(n);
                    template.push_str("{}");
                }
                None => template.push_str(digits),
            }
            rest = tail;
            continue;
        }
        if c == '{' {
            template.push_str("{{");
        } else {
            template.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    (template, numbers)
}

/// The name [`template`] took apart.
fn fill(template: &str, numbers: &[u64]) -> Result<String> {
    let mut out = String::with_capacity(template.len() + numbers.len() * 4);
    let mut numbers = numbers.iter();
    let mut rest = template;
    while let Some(c) = rest.chars().next() {
        if c == '{' {
            match rest.get(1..2) {
                Some("{") => out.push('{'),
                Some("}") => out.push_str(&numbers.next().ok_or_else(broken)?.to_string()),
                _ => return Err(broken()),
            }
            rest = &rest[2..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    if numbers.next().is_some() {
        return Err(broken());
    }
    Ok(out)
}

/// How many numbers a template takes.
fn placeholders(template: &str) -> Result<usize> {
    let mut count = 0;
    let mut rest = template;
    while let Some(at) = rest.find('{') {
        match rest.get(at + 1..at + 2) {
            Some("{") => {}
            Some("}") => count += 1,
            _ => return Err(broken()),
        }
        rest = rest.get(at + 2..).unwrap_or_default();
    }
    Ok(count)
}

impl Bodies {
    fn of(kind: BodyKind, template: &str, rows: &[Vec<u64>]) -> Self {
        let width = rows.first().map_or(0, Vec::len);
        let columns = (0..width)
            .map(|k| {
                Column::deltas(
                    rows.iter()
                        .map(|row| row.get(k).copied().map_or(0, i128::from)),
                )
            })
            .collect();
        Self {
            kind,
            template: template.to_owned(),
            count: rows.len(),
            columns,
        }
    }

    /// The body names, at most `budget` of them.
    fn read(&self, into: &mut BTreeMap<String, BodyKind>, budget: usize) -> Result<()> {
        if self.count > budget.min(MAX_ROWS) || self.columns.len() > MAX_NAME_NUMBERS {
            return Err(broken());
        }
        let columns = self
            .columns
            .iter()
            .map(|c| {
                let values = c.read_deltas(self.count)?;
                if values.len() != self.count {
                    return Err(broken());
                }
                values.into_iter().map(exact).collect::<Result<Vec<u64>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        for row in 0..self.count {
            let numbers: Vec<u64> = columns
                .iter()
                .map(|c| c.get(row).copied().ok_or_else(broken))
                .collect::<Result<_>>()?;
            let name = fill(&self.template, &numbers)?;
            if into.insert(name, self.kind).is_some() {
                return Err(broken());
            }
        }
        Ok(())
    }
}

/// Records of `rows` items, each made by `make` from a range of them and
/// at most [`CHUNK_BYTES`] serialized: the whole range if it fits, else
/// halves until each part does. A single item too large is refused.
fn chunked(
    rows: usize,
    make: &dyn Fn(Range<usize>) -> Record,
    what: &dyn Fn(usize) -> String,
) -> Result<Vec<(usize, Record)>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut length = rows.max(1);
    while start < rows {
        let end = start.saturating_add(length).min(rows);
        let record = make(start..end);
        let bytes = serde_json::to_string(&record)?.len();
        if bytes <= CHUNK_BYTES {
            out.push((bytes, record));
            start = end;
            continue;
        }
        if end - start == 1 {
            return Err(Error::Invalid(format!(
                "{} is too large to record",
                what(start)
            )));
        }
        length = (end - start).div_ceil(2);
    }
    Ok(out)
}

/// Every record of `sources` and of the body → kind map `bodies`.
fn records(
    sources: &[Source],
    bodies: &BTreeMap<String, BodyKind>,
) -> Result<Vec<(usize, Record)>> {
    let mut sorted: Vec<&Source> = sources.iter().collect();
    sorted.sort_by_key(|s| s.pick_id);
    let mut out = chunked(
        sorted.len(),
        &|range| Record::Sources(Box::new(Sources::of(sorted.get(range).unwrap_or_default()))),
        &|k| sorted.get(k).map_or_else(String::new, |s| s.label.clone()),
    )?;
    let mut groups: BTreeMap<(BodyKind, String), Vec<Vec<u64>>> = BTreeMap::new();
    for (name, kind) in bodies {
        let (template, numbers) = template(name);
        groups.entry((*kind, template)).or_default().push(numbers);
    }
    for ((kind, template), mut rows) in groups {
        rows.sort_unstable();
        out.extend(chunked(
            rows.len(),
            &|range| {
                Record::Bodies(Bodies::of(
                    kind,
                    &template,
                    rows.get(range).unwrap_or_default(),
                ))
            },
            &|k| {
                rows.get(k)
                    .and_then(|numbers| fill(&template, numbers).ok())
                    .map_or_else(String::new, |name| format!("body name {name}"))
            },
        )?);
    }
    Ok(out)
}

/// The metadata values recording `sources` and `bodies`: the version key
/// and the records packed into values of at most [`CHUNK_BYTES`].
fn values(sources: &[Source], bodies: &BTreeMap<String, BodyKind>) -> Result<Map<String, Value>> {
    let mut packed: Vec<Vec<Record>> = Vec::new();
    let mut size = CHUNK_BYTES;
    for (bytes, record) in records(sources, bodies)? {
        // `{"records":[` … `]}` and a comma per record.
        let entry = bytes + 1;
        if size + entry > CHUNK_BYTES {
            packed.push(Vec::new());
            size = 14;
        }
        if let Some(value) = packed.last_mut() {
            value.push(record);
            size += entry;
        }
    }
    if packed.len() > MAX_VALUES {
        return Err(Error::Invalid(
            "the board has too many objects to record their identity in the native file".into(),
        ));
    }
    let mut map = Map::new();
    map.insert(METADATA_KEY.into(), json!({ "version": VERSION }));
    for (index, records) in packed.into_iter().enumerate() {
        map.insert(
            format!("{RECORDS_KEY}{index}"),
            json!({ "records": records }),
        );
    }
    Ok(map)
}

/// Record every source and body kind in `native`'s metadata, in one
/// command.
pub(super) fn write(
    native: &mut Native,
    sources: &[Source],
    bodies: &BTreeMap<String, BodyKind>,
) -> Result<()> {
    let values = values(sources, bodies)?;
    native.call("document.metadata.set", json!({ "values": values }))?;
    Ok(())
}

/// The saved sources (in pick order) and body → kind map of `native`.
pub(super) fn read(native: &Native) -> Result<(Vec<Source>, BTreeMap<String, BodyKind>)> {
    read_values(&native.session().doc.metadata.0)
}

fn read_values(metadata: &Map<String, Value>) -> Result<(Vec<Source>, BTreeMap<String, BodyKind>)> {
    let version = metadata
        .get(METADATA_KEY)
        .and_then(|m| m.get("version"))
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Verification("native model has no Sparksmith identity".into()))?;
    if version != VERSION {
        return Err(Error::Verification(format!(
            "native model's Sparksmith identity is version {version}; this build reads {VERSION}"
        )));
    }
    let mut sources = Vec::new();
    let mut bodies = BTreeMap::new();
    if !metadata.contains_key(&format!("{RECORDS_KEY}0")) {
        return Err(Error::Verification(
            "native model's Sparksmith identity was saved by an older pre-release build; \
             build its model again from the board"
                .into(),
        ));
    }
    for index in 0..MAX_VALUES {
        let Some(value) = metadata.get(&format!("{RECORDS_KEY}{index}")) else {
            break;
        };
        let records: Vec<Record> =
            serde_json::from_value(value.get("records").cloned().ok_or_else(broken)?)?;
        for record in records {
            match record {
                Record::Sources(columns) => {
                    sources.extend(columns.read(MAX_ROWS.saturating_sub(sources.len()))?)
                }
                Record::Bodies(names) => {
                    let budget = MAX_ROWS.saturating_sub(bodies.len());
                    names.read(&mut bodies, budget)?
                }
            }
            if sources.len() > MAX_ROWS || bodies.len() > MAX_ROWS {
                return Err(broken());
            }
        }
    }
    sources.sort_by_key(|s| s.pick_id);
    if bodies.is_empty() {
        return Err(Error::Verification(
            "native model has no Sparksmith body kinds".into(),
        ));
    }
    Ok((sources, bodies))
}

#[cfg(test)]
mod tests;
