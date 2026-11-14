//! `fab FILE.sparksmith OUT [--zip] [--profile ID] [--date ISO|none]`: write
//! the production files through the `project.export_fabrication` request.
//! A blocked export lists the findings that block it.
use super::{Result, execute, open, print};
use serde_json::{Value, json};
use sparksmith_automation::session::Session;
use sparksmith_core::product::SHORT_NAME as CLI;

/// The usage line(s).
pub const USAGE: &str =
    "fab FILE.sparksmith OUT_DIR|OUT.zip [--zip] [--profile ID] [--date ISO|none]
                                   check the design and write Gerber X2, Excellon, job,
                                   pick-and-place, BOM and PDF drawings; --date defaults
                                   to SOURCE_DATE_EPOCH or now (none: no date, for
                                   byte-identical output); a blocked export
                                   lists the errors that block it";

/// Blocking findings printed by one refused export; the rest are counted.
const MAX_LISTED: usize = 100;

/// Run the subcommand with the arguments after its name.
pub fn run(args: &[String]) -> Result<()> {
    let [project, output, rest @ ..] = args else {
        return Err(format!("usage: {CLI} {USAGE}").into());
    };
    let mut zip = output.ends_with(".zip");
    let mut profile = None;
    let mut date = Some(now()?);
    let mut flags = rest.iter();
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--zip" => zip = true,
            "--profile" => profile = flags.next().cloned(),
            "--date" => {
                date = match flags.next().map(String::as_str) {
                    Some("none") => None,
                    Some(d) => Some(d.to_owned()),
                    None => return Err("--date needs an ISO 8601 time or none".into()),
                }
            }
            other => {
                return Err(format!("unknown option {other}; usage: {CLI} {USAGE}").into());
            }
        }
    }
    let mut session = open(Some(project))?;
    // Files are named after the project file (`board.sparksmith` gives
    // `board-copper-l1-top.gbr`, …).
    let stem: String = std::path::Path::new(project)
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let mut options = if stem.trim_matches('_').is_empty() {
        json!({})
    } else {
        json!({ "base_name": stem })
    };
    if let Some(date) = date {
        options["creation_date"] = json!(date);
    }
    let reply = session.dispatch(json!({"command": "project.export_fabrication", "params": {
        "output": output,
        "zip": zip,
        "profile": profile,
        "options": options,
    }}));
    if reply["ok"] == true {
        return print(&reply["result"]);
    }
    let error = reply["error"].to_string();
    if !error.contains("fabrication.blocked") {
        return Err(error.into());
    }
    let report = blocking_report(&mut session, profile.as_deref())?;
    Err(format!("{error}\n{}", blocking_titles(&report)).into())
}

/// The production check the refused export ran.
fn blocking_report(session: &mut Session, profile: Option<&str>) -> Result<Value> {
    execute(
        session,
        json!({"command": "project.query", "params": {
            "query": "fabrication.preflight",
            "params": {"profile": profile},
        }}),
    )
}

/// One line per unwaived error of a `fabrication.preflight` result, each
/// the finding's title, at most [`MAX_LISTED`] and then how many remain.
fn blocking_titles(report: &Value) -> String {
    let items = report["result"]["items"].as_array();
    let titles: Vec<&str> = items
        .into_iter()
        .flatten()
        .filter(|i| i["severity"] == "error" && i.get("waiver").is_none())
        .filter_map(|i| i["title"].as_str())
        .collect();
    let mut lines: Vec<String> = titles
        .iter()
        .take(MAX_LISTED)
        .map(|title| format!("  {title}"))
        .collect();
    if let Some(more) = titles.len().checked_sub(MAX_LISTED).filter(|n| *n > 0) {
        lines.push(format!("  and {more} more"));
    }
    lines.join("\n")
}

/// `SOURCE_DATE_EPOCH` when set (reproducible builds), else the current
/// time, as `YYYY-MM-DDThh:mm:ssZ`.
fn now() -> Result<String> {
    let seconds = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(v) => v
            .trim()
            .parse::<i64>()
            .map_err(|_| "SOURCE_DATE_EPOCH is not a whole number of seconds")?,
        Err(_) => i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        )?,
    };
    Ok(iso_utc(seconds))
}

/// Seconds since 1970 as a UTC ISO 8601 time (proleptic Gregorian days
/// from the epoch to a civil date).
fn iso_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_LISTED, Value, blocking_titles, iso_utc, json, run};

    /// The showcase switched to a finer profile: its fills were computed
    /// under other rules, so they are stale errors and the export is
    /// refused, naming each one.
    #[test]
    fn a_blocked_export_lists_the_error_titles() -> super::Result<()> {
        let _context = sparksmith_core::library::standards::Context::fixture().map(|c| c.enter());
        let dir = tempfile::tempdir()?;
        let project = dir.path().join("board.sparksmith");
        let out = dir.path().join("out");
        let arg = |p: &std::path::Path| p.display().to_string();
        use sparksmith_core::rules::fixtures;
        let mut session =
            sparksmith_automation::session::Session::new(sparksmith_automation::session::showcase(
                sparksmith_core::library::standards::Context::fixture()?,
                &sparksmith_core::StackupPresets::fixture()?,
                fixtures::prototype()?,
            )?);
        let revision = session.engine().revision();
        super::execute(
            &mut session,
            json!({"command": "project.apply", "params": {
                "expected_revision": revision,
                "operation": {"editor": "Rules", "operation": {
                    "command": "rules.set_fab_profile",
                    "params": {"profile": fixtures::fine()?}}},
            }}),
        )?;
        super::execute(
            &mut session,
            json!({"command": "project.save", "params": {"path": arg(&project)}}),
        )?;
        let report = super::blocking_report(&mut session, None)?;
        let error = run(&[arg(&project), arg(&out)])
            .err()
            .ok_or("the export should be refused")?
            .to_string();
        assert!(error.contains("fabrication.blocked"), "{error}");
        let blocking: Vec<&Value> = report["result"]["items"]
            .as_array()
            .ok_or("items")?
            .iter()
            .filter(|i| i["severity"] == "error")
            .collect();
        assert!(!blocking.is_empty());
        for item in &blocking {
            let title = item["title"].as_str().ok_or("title")?;
            assert!(error.contains(title), "{title} in {error}");
        }
        assert!(!out.exists(), "nothing is written");
        Ok(())
    }

    /// Warnings and waived errors do not block; a long list says how many
    /// it left out.
    #[test]
    fn only_unwaived_errors_are_listed_and_a_long_list_is_counted() {
        let report = json!({"result": {"items": [
            {"severity": "error", "title": "Fill GND is stale"},
            {"severity": "error", "title": "Waived", "waiver": {"key": "k", "reason": "r"}},
            {"severity": "warning", "title": "A warning"},
        ]}});
        assert_eq!(blocking_titles(&report), "  Fill GND is stale");
        let many: Vec<Value> = (0..MAX_LISTED + 3)
            .map(|k| json!({"severity": "error", "title": format!("Finding {k}")}))
            .collect();
        let long = blocking_titles(&json!({"result": {"items": many}}));
        let lines: Vec<&str> = long.lines().collect();
        assert_eq!(lines.len(), MAX_LISTED + 1);
        assert_eq!(lines.last().copied(), Some("  and 3 more"));
    }

    #[test]
    fn epoch_seconds_print_as_utc_dates() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_793_577_600), "2026-11-02T00:00:00Z");
        assert_eq!(iso_utc(-1), "1969-12-31T23:59:59Z");
    }
}
