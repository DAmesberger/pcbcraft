//! Electronics → native Solidcraft. Builds the parametric `.solidcraft` model
//! of a Pcbcraft project; all geometry, evaluation and persistence belong to
//! Solidcraft's engine, reached only through [`native`].

pub mod material;
pub mod native;
pub mod project;
pub mod scene;
pub use solidcraft_engine::{geom, render};

/// Native foundation fixtures (parametric probe and copper sample). They are
/// acceptance tests for Solidcraft capabilities, not product features.
#[cfg(test)]
mod fixtures;

pub const SOLIDCRAFT_REVISION: &str = "4f3ac07aec69489183185071f07d7fbcf4f51e64";

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
