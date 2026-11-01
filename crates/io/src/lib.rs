//! File effects for Pcbcraft documents: bounded reads, atomic writes and crash
//! recovery. Document meaning, versions and migrations stay in
//! `pcbcraft_core::format`; this crate only moves bytes safely.
pub mod artwork;
pub mod library_folder;
pub mod library_folder_index;
pub mod persistence;
pub mod recovery;
pub mod settings;

pub use persistence::{read_project, write_project};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Document(#[from] pcbcraft_core::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    TooLarge(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Document(error) => error.code(),
            Self::Invalid(_) => "invalid_input",
            Self::TooLarge(_) => "limit_exceeded",
            Self::Io(_) => "io_error",
            Self::Json(_) => "malformed_file",
        }
    }
    pub fn diagnostic(&self) -> pcbcraft_core::Diagnostic {
        match self {
            Self::Document(error) => error.diagnostic(),
            other => pcbcraft_core::Diagnostic {
                code: other.code().into(),
                rule: None,
                message: other.to_string(),
                expected_revision: None,
                actual_revision: None,
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
