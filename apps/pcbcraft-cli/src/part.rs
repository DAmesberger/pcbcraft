//! `part check`: a Solidcraft part file evaluated at literal values and
//! checked against Pcbcraft's naming conventions (docs/part-templates.md
//! §11), through `library.check_part_file`. Read-only; the exit status is a
//! failure on a blocking finding.
use super::{Result, execute, open, print};
use serde_json::{Map, Value, json};

pub const USAGE: &str = "part check FILE.solidcraft|--builtin NAME [--set NAME=VALUE[mm|deg]]... [--json] [--materials]
                                   check a part file's conventions at values (exit 1 on a blocking finding)";

/// A `--set` value: `2.465mm`, `45deg`, `25` (a count) or `0.5` (a number).
fn value(text: &str) -> Result<Value> {
    let text = text.trim();
    let number = |t: &str| -> Result<f64> {
        let v: f64 = t
            .trim()
            .parse()
            .map_err(|_| format!("`{text}` is not a number with mm, deg or no unit"))?;
        Ok(v)
    };
    if let Some(v) = text.strip_suffix("mm") {
        return Ok(json!({"mm": number(v)?}));
    }
    if let Some(v) = text.strip_suffix("deg") {
        return Ok(json!({"deg": number(v)?}));
    }
    if let Ok(n) = text.parse::<u32>() {
        return Ok(json!({"count": n}));
    }
    Ok(json!({"number": number(text)?}))
}

pub fn run(args: &[String]) -> Result<()> {
    let usage = || format!("usage: pcbcraft-cli {USAGE}");
    let Some((command, rest)) = args.split_first() else {
        return Err(usage().into());
    };
    if command != "check" {
        return Err(usage().into());
    }
    let mut params = Map::new();
    let mut values = Map::new();
    let (mut json_out, mut materials) = (false, false);
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--json" => json_out = true,
            "--materials" => materials = true,
            "--builtin" => {
                let name = rest.next().ok_or_else(usage)?;
                params.insert("builtin".into(), json!(name));
            }
            "--set" => {
                let pair = rest.next().ok_or_else(usage)?;
                let (name, v) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("--set takes NAME=VALUE, not {pair}"))?;
                values.insert(name.trim().to_owned(), value(v)?);
            }
            path if !path.starts_with("--") && !params.contains_key("path") => {
                params.insert("path".into(), json!(path));
            }
            _ => return Err(usage().into()),
        }
    }
    params.insert("values".into(), Value::Object(values));
    let mut session = open(None)?;
    let reply = execute(
        &mut session,
        json!({"command": "library.check_part_file", "params": params}),
    )?;
    let report = &reply["report"];
    if json_out {
        print(&reply)?;
    } else {
        println!("{}", text(&reply, materials));
    }
    if report["ok"] == true {
        Ok(())
    } else {
        Err("the part file has blocking findings".into())
    }
}

/// The report as text for a person.
fn text(reply: &Value, materials: bool) -> String {
    let r = &reply["report"];
    let mut out = format!(
        "{}: {} bodies, {} parameters, {} marking surfaces, evaluated in {:.1} ms\n",
        reply["file"].as_str().unwrap_or("part file"),
        r["bodies"].as_array().map_or(0, Vec::len),
        r["parameters"].as_array().map_or(0, Vec::len),
        r["surfaces"].as_array().map_or(0, Vec::len),
        r["evaluate_ms"].as_f64().unwrap_or(0.0),
    );
    if let Some(b) = r["bounds_mm"].as_array() {
        out.push_str(&format!(
            "bounds (mm): {} .. {}\n",
            b[0],
            b.get(1).unwrap_or(&Value::Null)
        ));
    }
    out.push_str("parameters:\n");
    for p in r["parameters"].as_array().into_iter().flatten() {
        let given = if p["given"] == true { " (given)" } else { "" };
        out.push_str(&format!(
            "  {} = {}{given}\n",
            p["name"].as_str().unwrap_or_default(),
            p["expression"].as_str().unwrap_or_default()
        ));
    }
    out.push_str("marking surfaces:\n");
    for s in r["surfaces"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  {} {}\n",
            s["name"].as_str().unwrap_or_default(),
            s["geometry"]["size_mm"]
        ));
    }
    if !r["pin1"].is_null() {
        out.push_str(&format!("pin1: {}\n", r["pin1"]));
    }
    let findings = r["findings"].as_array().cloned().unwrap_or_default();
    if findings.is_empty() {
        out.push_str("no findings\n");
    }
    for f in findings {
        out.push_str(&format!(
            "{} {}: {}\n",
            if f["blocking"] == true {
                "error"
            } else {
                "note"
            },
            f["code"].as_str().unwrap_or_default(),
            f["message"].as_str().unwrap_or_default()
        ));
    }
    if materials {
        out.push_str("material appearance names:\n");
        for m in reply["materials"].as_array().into_iter().flatten() {
            out.push_str(&format!("  {}\n", m.as_str().unwrap_or_default()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_values_parse_as_typed_literals() -> Result<()> {
        assert_eq!(value("2.465mm")?, json!({"mm": 2.465}));
        assert_eq!(value("45 deg")?, json!({"deg": 45.0}));
        assert_eq!(value("25")?, json!({"count": 25}));
        assert_eq!(value("0.5")?, json!({"number": 0.5}));
        assert!(value("can_height").is_err());
        Ok(())
    }

    #[test]
    fn the_builtin_part_checks_clean() -> Result<()> {
        run(&[
            "check".into(),
            "--builtin".into(),
            "esp32-s31-wroom-3".into(),
        ])?;
        assert!(
            run(&[
                "check".into(),
                "--builtin".into(),
                "esp32-s31-wroom-3".into(),
                "--set".into(),
                "nope=1mm".into()
            ])
            .is_err()
        );
        Ok(())
    }
}
