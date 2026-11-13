//! Bounded reads of artwork files to import into library drawings. The
//! text is converted elsewhere (`sparksmith_solidcraft::svg`); this module
//! only reads it safely.
use crate::{Error, Result};
use std::{io::Read, path::Path};

/// Largest SVG file read, in bytes (the converter's own limit).
pub const MAX_SVG_BYTES: usize = 1 << 20;

/// Read a `.svg` file of at most [`MAX_SVG_BYTES`] as UTF-8 text.
pub fn read_svg(path: &Path) -> Result<String> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_none_or(|e| !e.eq_ignore_ascii_case("svg"))
    {
        return Err(Error::Invalid("choose a .svg file".into()));
    }
    if path.as_os_str().len() > 4096 {
        return Err(Error::Invalid("the SVG path is too long".into()));
    }
    let too_large = || Error::TooLarge(format!("the SVG file exceeds {} KiB", MAX_SVG_BYTES >> 10));
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_SVG_BYTES as u64 {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(MAX_SVG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_SVG_BYTES {
        return Err(too_large());
    }
    String::from_utf8(bytes).map_err(|_| Error::Invalid("the SVG file is not UTF-8 text".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_bounded_svg_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let ok = dir.path().join("mark.SVG");
        std::fs::write(&ok, "<svg/>")?;
        assert_eq!(read_svg(&ok)?, "<svg/>");
        let wrong = dir.path().join("mark.png");
        std::fs::write(&wrong, "<svg/>")?;
        assert!(matches!(read_svg(&wrong), Err(Error::Invalid(_))));
        let big = dir.path().join("big.svg");
        std::fs::write(&big, vec![b' '; MAX_SVG_BYTES + 1])?;
        assert!(matches!(read_svg(&big), Err(Error::TooLarge(_))));
        let binary = dir.path().join("bin.svg");
        std::fs::write(&binary, [0xff, 0xfe, 0x00])?;
        assert!(matches!(read_svg(&binary), Err(Error::Invalid(_))));
        Ok(())
    }
}
