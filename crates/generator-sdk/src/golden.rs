//! Golden cases: `(request → result hash)` pairs each generator crate keeps
//! in `golden.json`. The same file is checked three ways: natively here,
//! under wasmi natively, and under wasmi built for wasm32
//! (`cargo xtask generators check`); all three must give these hashes.
use crate::Generator;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde::Deserialize;
use sparksmith_generator_abi::v1::Request;
use sparksmith_generator_abi::{codec, hex, result_hash};

/// A generator crate's `golden.json`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenFile {
    pub cases: Vec<GoldenCase>,
}

/// One case: a request with complete parameters and the hex SHA-256 of its
/// canonical generation.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenCase {
    pub name: String,
    pub request: Request,
    pub result: String,
}

/// Parse a `golden.json`.
pub fn parse(json: &str) -> Result<GoldenFile, String> {
    serde_json::from_str(json).map_err(|e| format!("golden.json: {e}"))
}

/// Run every case natively and compare result hashes. Returns the number of
/// cases checked, or every mismatch with the hash actually produced.
pub fn check<G: Generator>(json: &str) -> Result<usize, String> {
    let file = parse(json)?;
    if file.cases.is_empty() {
        return Err("golden.json has no cases".into());
    }
    // The description must encode canonically on every call.
    let first = codec::encode(&G::describe()).map_err(|e| format!("describe: {e}"))?;
    let again = codec::encode(&G::describe()).map_err(|e| format!("describe: {e}"))?;
    if first != again {
        return Err("describe gave different bytes on a second call".into());
    }
    let mut problems = Vec::new();
    for case in &file.cases {
        match G::generate(&case.request) {
            Ok(generation) => match codec::encode(&generation) {
                Ok(bytes) => {
                    let actual = hex(&result_hash(&bytes));
                    if actual != case.result {
                        problems.push(format!(
                            "{}: result {actual}, golden.json has {}",
                            case.name, case.result
                        ));
                    }
                }
                Err(e) => problems.push(format!("{}: {e}", case.name)),
            },
            Err(errors) => problems.push(format!("{}: refused: {errors:?}", case.name)),
        }
    }
    if problems.is_empty() {
        Ok(file.cases.len())
    } else {
        Err(problems.join("\n"))
    }
}
