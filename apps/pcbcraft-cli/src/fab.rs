//! `fab FILE.pcbcraft OUT [--zip] [--profile ID] [--date ISO|none]`: write
//! the production files through the `project.export_fabrication` request.
use super::{Result, execute, open, print};
use serde_json::json;

/// The usage line(s).
pub const USAGE: &str = "fab FILE.pcbcraft OUT_DIR|OUT.zip [--zip] [--profile ID] [--date ISO|none]
                                   check the design and write Gerber X2, Excellon, job,
                                   pick-and-place, BOM and PDF drawings; --date defaults
                                   to SOURCE_DATE_EPOCH or now (none: no date, for
                                   byte-identical output)";

/// Run the subcommand with the arguments after its name.
pub fn run(args: &[String]) -> Result<()> {
    let [project, output, rest @ ..] = args else {
        return Err(format!("usage: pcbcraft-cli {USAGE}").into());
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
                return Err(format!("unknown option {other}; usage: pcbcraft-cli {USAGE}").into());
            }
        }
    }
    let mut session = open(Some(project))?;
    // Files are named after the project file (`board.pcbcraft` gives
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
    print(&execute(
        &mut session,
        json!({"command": "project.export_fabrication", "params": {
            "output": output,
            "zip": zip,
            "profile": profile,
            "options": options,
        }}),
    )?)
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
    use super::iso_utc;

    #[test]
    fn epoch_seconds_print_as_utc_dates() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso_utc(1_793_577_600), "2026-11-02T00:00:00Z");
        assert_eq!(iso_utc(-1), "1969-12-31T23:59:59Z");
    }
}
