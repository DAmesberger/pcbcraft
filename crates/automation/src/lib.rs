//! Headless automation over the one Pcbcraft command engine.
//!
//! [`Session`] owns an [`pcbcraft_core::Engine`] plus the file effects
//! automation needs (open, save, native export). The CLI's JSON-lines
//! transport and the MCP server are thin adapters over [`Session::execute`],
//! so every transport reaches the same typed operations and error codes.
pub mod lines;
pub mod mcp;
pub mod session;

pub use session::{Request, Session, part_files};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Document(#[from] pcbcraft_core::Error),
    #[error(transparent)]
    File(#[from] pcbcraft_io::Error),
    #[error(transparent)]
    Native(#[from] pcbcraft_solidcraft::Error),
    #[error("invalid request: {0}")]
    Request(#[from] serde_json::Error),
}

impl Error {
    pub fn diagnostic(&self) -> pcbcraft_core::Diagnostic {
        match self {
            Self::Document(error) => error.diagnostic(),
            Self::File(error) => error.diagnostic(),
            Self::Native(error) => pcbcraft_core::Diagnostic {
                code: error.code().into(),
                rule: None,
                message: error.to_string(),
                expected_revision: None,
                actual_revision: None,
            },
            Self::Request(_) => pcbcraft_core::Diagnostic {
                code: "malformed_request".into(),
                rule: None,
                message: self.to_string(),
                expected_revision: None,
                actual_revision: None,
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
