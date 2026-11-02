//! Electronics → native Solidcraft. Builds the parametric `.solidcraft` model
//! of a Pcbcraft project; its geometry, evaluation and persistence belong to
//! Solidcraft's engine, reached only through [`native`]. Thin display-only
//! layers over it (soldermask, plated finish, terminations, solder) are
//! meshes in Solidcraft's mesh type ([`appearance`]), never saved.

pub mod appearance;
pub mod board;
pub mod footprint;
pub mod marking;
pub mod material;
pub mod models;
pub mod native;
pub mod pad_outline;
pub mod project;
pub mod scene;
pub mod silkscreen;
pub mod stackup_edge;
pub mod svg;
pub use solidcraft_engine::{geom, render};

/// Native foundation fixtures (parametric probe and copper sample). They are
/// acceptance tests for Solidcraft capabilities, not product features.
#[cfg(test)]
mod fixtures;

/// The Solidcraft revision Pcbcraft builds against: the `pcbcraft-via2`
/// branch of the DAmesberger/solidcraft fork (upstream 4f3ac07 plus the
/// opt-in product look and via scaling).
pub const SOLIDCRAFT_REVISION: &str = "d53b131e3d1cf0b9489e9c048c60af787b14cc47";

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
    /// A newer request replaced this build; nothing was produced.
    #[error("the build was cancelled")]
    Cancelled,
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) | Self::Json(_) => "invalid_input",
            Self::Engine { .. } => "native_engine_error",
            Self::Verification(_) => "native_verification_failed",
            Self::Io(_) => "io_error",
            Self::Cancelled => "cancelled",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
