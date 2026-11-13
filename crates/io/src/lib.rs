//! File effects for Sparksmith documents: bounded reads, atomic writes and crash
//! recovery. Document meaning, versions and migrations stay in
//! `sparksmith_core::format`; this crate only moves bytes safely.
pub mod artwork;
pub mod assembly_link;
pub mod fab_drawings;
pub mod fabrication;
mod former_name;
pub mod generated;
pub mod interface;
pub mod library_folder;
pub mod library_folder_index;
pub mod pdf;
pub mod persistence;
pub mod recovery;
pub mod rule_profiles;
pub mod schematic_pdf;
pub mod settings;
pub mod shipped;
pub mod standards;

pub use persistence::{read_project, write_project};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Document(#[from] sparksmith_core::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    TooLarge(String),
    /// The file was written and is in place, but synchronizing its folder
    /// failed, so it may not survive a power loss.
    #[error("{0}")]
    Unsynced(String),
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
            Self::Unsynced(_) => "written_unsynced",
            Self::Io(_) => "io_error",
            Self::Json(_) => "malformed_file",
        }
    }
    pub fn diagnostic(&self) -> sparksmith_core::Diagnostic {
        match self {
            Self::Document(error) => error.diagnostic(),
            other => sparksmith_core::Diagnostic {
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
