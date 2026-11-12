//! Typed access to a request's parameters, checked against the description
//! with the ABI's own check ([`pcbcraft_generator_abi::params::check`]),
//! the same one the host runs before it calls.
use alloc::format;
use alloc::vec::Vec;
use pcbcraft_generator_abi::params;
use pcbcraft_generator_abi::v1::{Description, ParamValue, Params, Problem};

pub use pcbcraft_generator_abi::params::problem;

/// A request's parameters after [`checked`]: every declared parameter is
/// present, of its type and in its range, and nothing else is.
pub struct Args<'a> {
    params: &'a Params,
}

/// Check `params` against `description`'s parameters.
pub fn checked<'a>(
    description: &Description,
    params: &'a Params,
) -> Result<Args<'a>, Vec<Problem>> {
    params::check(description, params)?;
    Ok(Args { params })
}

impl Args<'_> {
    /// An integer parameter (length, angle or count).
    pub fn int(&self, key: &str) -> Result<i64, Problem> {
        match self.params.get(key) {
            Some(ParamValue::Int(v)) => Ok(*v),
            _ => Err(undeclared(key)),
        }
    }

    /// A text or enum parameter.
    pub fn text(&self, key: &str) -> Result<&str, Problem> {
        match self.params.get(key) {
            Some(ParamValue::Text(v)) => Ok(v),
            _ => Err(undeclared(key)),
        }
    }

    /// A boolean parameter.
    pub fn flag(&self, key: &str) -> Result<bool, Problem> {
        match self.params.get(key) {
            Some(ParamValue::Bool(v)) => Ok(*v),
            _ => Err(undeclared(key)),
        }
    }
}

/// A generator asked for a parameter its description does not declare with
/// that type: a bug in the generator, reported rather than guessed around.
fn undeclared(key: &str) -> Problem {
    problem(
        "generator.undeclared_param",
        key,
        format!(
            "The generator read parameter \"{key}\", which it does not declare with that type."
        ),
    )
}
