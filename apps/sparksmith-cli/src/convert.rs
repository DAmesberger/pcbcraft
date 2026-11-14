//! `convert`: files of an older format version to the current one, in
//! place, keeping each original beside it (`sparksmith_io::convert`). Until
//! the first release the app opens only the current version and names this
//! command when it refuses a file.
use super::Result;
use sparksmith_core::product::SHORT_NAME as CLI;
use std::path::Path;

pub const USAGE: &str =
    "convert [--profile PROFILE.json] FILE.sparksmith|LIBRARY_DIR|REPLAY.jsonl|OLD_PROFILE.json...
                                   convert projects of an older format version to the
                                   current one in place (the original is kept as
                                   FILE.v<N>); library folders, replays and old profile
                                   files take the units conversion (original kept as
                                   .pre-nm). --profile names the fab profile an old
                                   project's rule snapshot becomes";

pub fn run(args: &[String]) -> Result<()> {
    let mut options = sparksmith_io::convert::Options::default();
    let paths = match args {
        [flag, profile, rest @ ..] if flag == "--profile" => {
            options.profile = Some(
                sparksmith_io::convert::v1::read_profile(Path::new(profile))
                    .map_err(|e| e.to_string())?,
            );
            rest
        }
        _ => args,
    };
    if paths.is_empty() || paths.iter().any(|p| p.starts_with("--")) {
        return Err(format!("usage: {CLI} {USAGE}").into());
    }
    for path in paths {
        let lines = sparksmith_io::convert::convert_path(Path::new(path), &options)
            .map_err(|e| e.to_string())?;
        for line in lines {
            println!("{line}");
        }
    }
    Ok(())
}
