//! The SHA-256 of each part file in `parts/`, as constants the module names
//! its 3D models by (`src/lib.rs`, `mod parts`). The module carries only
//! the hashes; the files travel in the generator's bundle
//! (`cargo xtask generators build`).
use std::error::Error;
use std::fmt::Write as _;
use std::path::PathBuf;

/// The part files, by file stem; each becomes `CHIP_<STEM>` in upper case.
const PARTS: [&str; 4] = [
    "chip-resistor",
    "chip-capacitor",
    "chip-ferrite",
    "chip-led",
];

fn main() -> Result<(), Box<dyn Error>> {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("parts");
    println!("cargo:rerun-if-changed=parts");
    let mut out = String::from("// The bundle's part files by SHA-256 (build.rs).\n");
    for stem in PARTS {
        let file = format!("{stem}.solidcraft");
        println!("cargo:rerun-if-changed=parts/{file}");
        let bytes = std::fs::read(dir.join(&file))?;
        let hash = sparksmith_generator_abi::hex(&sparksmith_generator_abi::sha256(&bytes));
        let name = stem.replace('-', "_").to_uppercase();
        writeln!(
            out,
            "/// `parts/{file}`.\npub const {name}: &str = \"{hash}\";"
        )?;
    }
    std::fs::write(
        PathBuf::from(std::env::var("OUT_DIR")?).join("parts.rs"),
        out,
    )?;
    Ok(())
}
