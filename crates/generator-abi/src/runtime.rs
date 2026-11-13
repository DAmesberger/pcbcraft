//! The interface the document layer uses to run generator modules.
//!
//! It lives here, beside the wire types, so that `sparksmith-core` can depend
//! on it without depending on an interpreter: `sparksmith-generators`
//! implements it with wasmi, and a browser build may implement it with the
//! browser's own `WebAssembly`.
use crate::v1::{Description, Generation, Problem, Request};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Runs generator modules. Every call starts a fresh instance, so no state
/// carries over, and any failure leaves the caller's state untouched.
pub trait GeneratorRuntime {
    /// The module's [`Description`], checked and decoded.
    fn describe(&self, module: &[u8]) -> Result<Description, Failure>;
    /// Run `request` (with complete parameters) and return the validated
    /// result with its canonical bytes and result hash.
    fn generate(&self, module: &[u8], request: &Request) -> Result<Generated, Failure>;
}

/// A validated generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generated {
    pub generation: Generation,
    /// The canonical bytes the result hash covers.
    pub canonical: Vec<u8>,
    /// SHA-256 of `canonical`.
    pub result_hash: [u8; 32],
    /// Work the call took, in the runtime's fuel units, when it meters fuel.
    pub fuel_used: Option<u64>,
}

/// Why a module did not give a result: the module's own parameter problems,
/// or the host's refusal. Every message is a sentence a person can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub problems: Vec<Problem>,
}

impl Failure {
    /// A failure with one problem.
    pub fn one(code: &str, message: String) -> Self {
        Self {
            problems: alloc::vec![Problem {
                code: code.into(),
                field: None,
                message,
            }],
        }
    }

    /// The first problem's code, for matching.
    pub fn code(&self) -> Option<&str> {
        self.problems.first().map(|p| p.code.as_str())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (k, problem) in self.problems.iter().enumerate() {
            if k > 0 {
                f.write_str(" ")?;
            }
            f.write_str(&problem.message)?;
        }
        Ok(())
    }
}
