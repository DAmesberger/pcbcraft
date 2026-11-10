//! U2 footprints: pads, holes, mask and paste rules, the datum, pad-table
//! results and mechanical lands.
use crate::Result;
use serde_json::{Map, Value};

/// One definition body of `kind`; the number of values changed.
pub fn definition(_kind: &str, _body: &mut Map<String, Value>) -> Result<usize> {
    Ok(0)
}

/// Settings saved with a project's drafts.
pub fn drafts(_drafts: &mut Map<String, Value>) -> Result<usize> {
    Ok(0)
}

/// Data of this domain a project holds outside its library.
pub fn project(_project: &mut Map<String, Value>) -> Result<usize> {
    Ok(0)
}
