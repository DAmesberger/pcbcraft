//! Electronics → native Solidcraft. Builds the parametric `.solidcraft` model
//! of a Pcbcraft project; its geometry, evaluation and persistence belong to
//! Solidcraft's engine, reached only through [`native`]. Thin display-only
//! layers over it (soldermask, plated finish, terminations, solder) are
//! meshes in Solidcraft's mesh type ([`appearance`]), never saved.

pub mod appearance;
pub mod footprint;
pub mod material;
pub mod native;
pub mod project;
pub mod scene;
pub use solidcraft_engine::{geom, render};

/// Native foundation fixtures (parametric probe and copper sample). They are
/// acceptance tests for Solidcraft capabilities, not product features.
#[cfg(test)]
mod fixtures;

/// Spike F0: headless parametric-footprint evaluator prototype (tests only,
/// not a product API; see `docs/footprint-spike.md`).
#[cfg(test)]
mod footprint_spike;

/// The Solidcraft revision Pcbcraft builds against: the `pcbcraft-product-look`
/// branch of the DAmesberger/solidcraft fork (upstream 4f3ac07 plus the
/// opt-in product look).
pub const SOLIDCRAFT_REVISION: &str = "87b1009b029a1b381ad1799fe72bfec44a4b1a66";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("Solidcraft command {command} failed: {message}")]
    Engine { command: String, message: String },
    #[error("model verification failed: {0}")]
    Verification(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) | Self::Json(_) => "invalid_input",
            Self::Engine { .. } => "native_engine_error",
            Self::Verification(_) => "native_verification_failed",
            Self::Io(_) => "io_error",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
