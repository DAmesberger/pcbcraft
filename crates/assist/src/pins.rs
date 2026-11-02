//! Reviewer views of a run's drafts: one row per package pin (number,
//! name, class, default and every function with its mux table and code,
//! peripheral and role, source page, review flags), and a diff of two
//! projects of drafts by pin number, pin rule, pad and package value, so
//! two runs over the same datasheets (or a run and a reference) can be
//! compared field by field instead of by eye.
//!
//! Both read the document snapshot (`Session::snapshot`), the same JSON
//! the MCP server and CLI return.
use pcbcraft_core::functions::mux_text;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// One package pin as a reviewer reads it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PinRow {
    /// The package designation ("12", "95"), through the binding.
    pub number: String,
    pub name: String,
    /// electrical, device_nc or reserved.
    pub class: String,
    /// The design-default function's label, or the only function's.
    #[serde(default)]
    pub default: String,
    /// Each function as `label [table code] PERIPHERAL.role direction`.
    #[serde(default)]
    pub functions: Vec<String>,
    /// Source pages of the row's functions ("p.14 Table 3-1").
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub unreviewed: bool,
    #[serde(default)]
    pub uncertain: bool,
}

fn kind_of(draft: &Value) -> &str {
    draft["draft"]["definition"]["kind"].as_str().unwrap_or("")
}

fn body(draft: &Value) -> &Value {
    &draft["draft"]["definition"]["body"]
}

/// The drafts of `kind` in a snapshot.
fn of_kind<'a>(snapshot: &'a Value, kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
    snapshot["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(move |d| kind_of(d) == kind)
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_owned()
}

/// The first component draft's terminals as pin rows, numbered through the
/// first binding draft of it (terminals without a package number are
/// listed by name with an empty number), sorted by number.
pub fn rows(snapshot: &Value) -> Vec<PinRow> {
    let Some(component) = of_kind(snapshot, "component").next() else {
        return Vec::new();
    };
    let id = &component["draft"]["id"];
    let c = body(component);
    let numbers: BTreeMap<u64, String> = of_kind(snapshot, "binding")
        .find(|b| body(b)["component"] == *id)
        .map(|b| {
            let package = of_kind(snapshot, "package")
                .find(|p| p["draft"]["id"] == body(b)["package"]["id"])
                .map(body);
            let designation = |pt: &Value| {
                package
                    .and_then(|p| p["terminals"].as_array())
                    .into_iter()
                    .flatten()
                    .find(|t| t["id"] == *pt)
                    .map(|t| text(&t["designation"]))
                    .unwrap_or_default()
            };
            body(b)["mappings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| {
                    Some((m["terminal"].as_u64()?, designation(&m["package_terminal"])))
                })
                .collect()
        })
        .unwrap_or_default();
    let resources: BTreeMap<u64, (String, BTreeMap<u64, String>)> = c["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let roles = r["roles"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| Some((x["id"].as_u64()?, text(&x["label"]))))
                .collect();
            Some((r["id"].as_u64()?, (text(&r["label"]), roles)))
        })
        .collect();
    let flagged = |key: &str| -> BTreeSet<u64> {
        c["review"][key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .collect()
    };
    let (unreviewed, uncertain) = (flagged("unreviewed"), flagged("uncertain"));
    let mut out: Vec<PinRow> = c["terminals"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let tid = t["id"].as_u64().unwrap_or(0);
            let mut row = PinRow {
                number: numbers.get(&tid).cloned().unwrap_or_default(),
                name: text(&t["name"]),
                unreviewed: unreviewed.contains(&tid),
                uncertain: uncertain.contains(&tid),
                ..PinRow::default()
            };
            match &t["class"] {
                Value::String(s) => row.class = s.clone(),
                Value::Object(map) => {
                    row.class = map.keys().next().cloned().unwrap_or_default();
                    let e = &t["class"]["electrical"];
                    let functions = e["functions"].as_array().cloned().unwrap_or_default();
                    let mut pages = BTreeSet::new();
                    for f in &functions {
                        let mut s = text(&f["label"]);
                        if let Some(mux) = mux_text(f["mux_table"].as_str(), f["mux"].as_str()) {
                            let _ = write!(s, " [{mux}]");
                        }
                        let rr = &f["resource_role"];
                        if let (Some(r), Some(role)) =
                            (rr["resource"].as_u64(), rr["role"].as_u64())
                            && let Some((label, roles)) = resources.get(&r)
                        {
                            let _ = write!(
                                s,
                                " {label}.{}",
                                roles.get(&role).map_or("?", String::as_str)
                            );
                        }
                        let _ = write!(s, " {}", text(&f["direction"]));
                        row.functions.push(s);
                        let src = &f["source"];
                        if let Some(page) = src["page"].as_u64() {
                            let table = src["table"]
                                .as_str()
                                .map(|t| format!(" {t}"))
                                .unwrap_or_default();
                            pages.insert(format!("p.{page}{table}"));
                        }
                        if e["design_default"] == f["id"] || functions.len() == 1 {
                            row.default = text(&f["label"]);
                        }
                    }
                    row.source = pages.into_iter().collect::<Vec<_>>().join("; ");
                }
                _ => {}
            }
            row
        })
        .collect();
    out.sort_by_key(|r| {
        (
            r.number.parse::<u64>().unwrap_or(u64::MAX),
            r.number.clone(),
            r.name.clone(),
        )
    });
    out
}

fn cell(t: &str) -> String {
    t.replace('|', "\\|").replace('\n', " ")
}

/// The pin rows as a Markdown table.
pub fn markdown(rows: &[PinRow]) -> String {
    let mut s = String::from(
        "| Pin | Name | Default | Functions [mux] peripheral.role | Source | Flags |\n|---|---|---|---|---|---|\n",
    );
    for r in rows {
        let mut flags = Vec::new();
        if r.class != "electrical" {
            flags.push(r.class.as_str());
        }
        if r.uncertain {
            flags.push("UNCERTAIN");
        }
        if r.unreviewed {
            flags.push("unreviewed");
        }
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} |",
            cell(&r.number),
            cell(&r.name),
            cell(&r.default),
            cell(&r.functions.join("; ")),
            cell(&r.source),
            flags.join(", ")
        );
    }
    s
}

/// The rules of the first component draft, label → script source.
fn rules(snapshot: &Value) -> BTreeMap<String, String> {
    of_kind(snapshot, "component")
        .next()
        .map(|c| {
            body(c)["pin_rules"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| (text(&r["label"]), text(&r["script"]["source"])))
                .collect()
        })
        .unwrap_or_default()
}

/// The first footprint draft's pads by number: each pad's position,
/// rotation and shape as text, sorted.
fn pads(snapshot: &Value) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(f) = of_kind(snapshot, "footprint").next() {
        for p in body(f)["pads"].as_array().into_iter().flatten() {
            let pos = &p["position"];
            let at = format!(
                "({:.3}, {:.3}) {}° {}",
                pos[0].as_f64().unwrap_or(f64::NAN),
                pos[1].as_f64().unwrap_or(f64::NAN),
                p["rotation_deg"].as_f64().unwrap_or(0.0),
                p["shape"]
            );
            out.entry(text(&p["number"])).or_default().push(at);
        }
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

/// Top-level values of one draft kind's body (the package's `form`, the
/// footprint's keep-outs, the component's marking) for a key-by-key diff.
fn values(snapshot: &Value, kind: &str, key: &str) -> BTreeMap<String, Value> {
    of_kind(snapshot, kind)
        .next()
        .and_then(|d| body(d)[key].as_object().cloned())
        .map(|m| m.into_iter().collect())
        .unwrap_or_default()
}

/// The differences between two projects of drafts (`a` and `b`, named
/// `names`) as Markdown: per pin (name, class, default, functions added
/// and removed), pin rules, pads by number, package form values,
/// footprint keep-outs and marking. Identical sections say so.
pub fn diff(a: &Value, b: &Value, names: (&str, &str)) -> String {
    let (na, nb) = names;
    let mut s = String::new();
    let (ra, rb) = (rows(a), rows(b));
    let index = |rows: &[PinRow]| -> BTreeMap<String, PinRow> {
        rows.iter()
            .map(|r| {
                (
                    format!(
                        "{}:{}",
                        r.number,
                        if r.number.is_empty() { &r.name } else { "" }
                    ),
                    r.clone(),
                )
            })
            .collect()
    };
    let (ia, ib) = (index(&ra), index(&rb));
    let keys: BTreeSet<&String> = ia.keys().chain(ib.keys()).collect();
    let mut pins = String::new();
    let mut differing = 0;
    let mut keys: Vec<&String> = keys.into_iter().collect();
    keys.sort_by_key(|k| {
        (
            k.split(':')
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(u64::MAX),
            (*k).clone(),
        )
    });
    for key in keys {
        let label = key.trim_end_matches(':');
        match (ia.get(key), ib.get(key)) {
            (Some(x), None) => {
                differing += 1;
                let _ = writeln!(pins, "- pin {label} {}: only in {na}", x.name);
            }
            (None, Some(y)) => {
                differing += 1;
                let _ = writeln!(pins, "- pin {label} {}: only in {nb}", y.name);
            }
            (Some(x), Some(y)) => {
                let mut lines = Vec::new();
                for (what, p, q) in [
                    ("name", &x.name, &y.name),
                    ("class", &x.class, &y.class),
                    ("default", &x.default, &y.default),
                ] {
                    if p != q {
                        lines.push(format!("{what}: {na} `{p}`, {nb} `{q}`"));
                    }
                }
                let fa: BTreeSet<&String> = x.functions.iter().collect();
                let fb: BTreeSet<&String> = y.functions.iter().collect();
                let only_a: Vec<&str> = fa.difference(&fb).map(|f| f.as_str()).collect();
                let only_b: Vec<&str> = fb.difference(&fa).map(|f| f.as_str()).collect();
                if !only_a.is_empty() {
                    lines.push(format!("only {na}: {}", only_a.join("; ")));
                }
                if !only_b.is_empty() {
                    lines.push(format!("only {nb}: {}", only_b.join("; ")));
                }
                if x.uncertain != y.uncertain {
                    lines.push(format!(
                        "uncertain: {na} {}, {nb} {}",
                        x.uncertain, y.uncertain
                    ));
                }
                if !lines.is_empty() {
                    differing += 1;
                    let _ = writeln!(pins, "- pin {label} {}:", x.name);
                    for l in lines {
                        let _ = writeln!(pins, "  - {l}");
                    }
                }
            }
            (None, None) => {}
        }
    }
    let _ = writeln!(
        s,
        "## Pins\n\n{} pins in {na}, {} in {nb}; {differing} differ.\n",
        ra.len(),
        rb.len()
    );
    s.push_str(&pins);
    let (qa, qb) = (rules(a), rules(b));
    let _ = writeln!(
        s,
        "\n## Pin rules\n\n{} in {na}, {} in {nb}.\n",
        qa.len(),
        qb.len()
    );
    for label in qa.keys().chain(qb.keys()).collect::<BTreeSet<_>>() {
        match (qa.get(label), qb.get(label)) {
            (Some(_), None) => {
                let _ = writeln!(s, "- `{label}`: only in {na}");
            }
            (None, Some(_)) => {
                let _ = writeln!(s, "- `{label}`: only in {nb}");
            }
            (Some(x), Some(y)) if x != y => {
                let _ = writeln!(s, "- `{label}`: scripts differ");
            }
            _ => {}
        }
    }
    let (pa, pb) = (pads(a), pads(b));
    let mut pad_lines = Vec::new();
    for number in pa.keys().chain(pb.keys()).collect::<BTreeSet<_>>() {
        if pa.get(number) != pb.get(number) {
            pad_lines.push(format!(
                "- pad {number}: {na} {:?}, {nb} {:?}",
                pa.get(number),
                pb.get(number)
            ));
        }
    }
    let _ = writeln!(
        s,
        "\n## Pads\n\n{} pads in {na}, {} in {nb}; {} numbers differ.\n",
        pa.values().map(Vec::len).sum::<usize>(),
        pb.values().map(Vec::len).sum::<usize>(),
        pad_lines.len()
    );
    for l in pad_lines {
        let _ = writeln!(s, "{l}");
    }
    for (title, kind, key) in [
        ("Package form", "package", "form"),
        ("Component marking", "component", "marking"),
    ] {
        let (va, vb) = (values(a, kind, key), values(b, kind, key));
        let mut lines = Vec::new();
        for k in va.keys().chain(vb.keys()).collect::<BTreeSet<_>>() {
            if va.get(k) != vb.get(k) {
                lines.push(format!(
                    "- `{k}`: {na} `{}`, {nb} `{}`",
                    va.get(k).map_or("—".into(), Value::to_string),
                    vb.get(k).map_or("—".into(), Value::to_string)
                ));
            }
        }
        let _ = writeln!(
            s,
            "\n## {title}\n\n{}",
            if lines.is_empty() {
                "Identical.".to_owned()
            } else {
                lines.join("\n")
            }
        );
    }
    let keepouts = |v: &Value| {
        of_kind(v, "footprint")
            .next()
            .map(|f| body(f)["keepouts"].clone())
            .unwrap_or(Value::Null)
    };
    let _ = writeln!(
        s,
        "\n## Footprint keep-outs\n\n{}",
        if keepouts(a) == keepouts(b) {
            "Identical.".to_owned()
        } else {
            format!("{na} `{}`\n\n{nb} `{}`", keepouts(a), keepouts(b))
        }
    );
    s
}
