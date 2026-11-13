//! Complete parameters: the host fills every default from the description
//! before it calls and before it hashes, so a default that changes in a
//! later module revision can never silently change a part's identity.
//! The fill lives in the ABI crate (`sparksmith_generator_abi::params`),
//! so the document layer completes requests the same way.
pub use sparksmith_generator_abi::params::{complete, fits};
