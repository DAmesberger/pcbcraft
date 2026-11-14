//! Complete parameters: the host fills every default from the description
//! before it calls and before it hashes, so a default that changes in a
//! later module revision can never silently change a part's identity.
use sparksmith_generator_abi::Failure;
use sparksmith_generator_abi::v1::{self, Description, Params, Request, Scope};

pub use sparksmith_generator_abi::params::fits;

/// The request for `given` parameters in `scope`, with every default
/// filled. Refuses a scope the generator does not offer, unknown
/// parameters and values of the wrong type or out of range, all at once.
pub fn complete(
    description: &Description,
    scope: Scope,
    given: &Params,
) -> Result<Request, Failure> {
    if !description.scopes().contains(&scope) {
        let what = match scope {
            Scope::Component => "complete parts",
            Scope::Package => "packages on their own",
        };
        return Err(Failure::one(
            "param.scope",
            format!("The {} generator does not make {what}.", description.title),
        ));
    }
    let mut params = given.clone();
    for param in description.params() {
        params
            .entry(param.key.clone())
            .or_insert_with(|| param.default.clone());
    }
    sparksmith_generator_abi::params::check(description, &params)
        .map_err(|problems| Failure { problems })?;
    Ok(Request {
        abi: v1::ABI,
        scope,
        params,
    })
}
