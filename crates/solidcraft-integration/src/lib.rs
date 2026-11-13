//! Electronics → native Solidcraft. Builds the parametric `.solidcraft` model
//! of a Pcbcraft project; its geometry, evaluation and persistence belong to
//! Solidcraft's engine, reached only through [`native`]. Thin display-only
//! layers over it (soldermask, plated finish, terminations, solder) are
//! meshes in Solidcraft's mesh type ([`appearance`]), never saved.

pub mod appearance;
pub mod board;
pub mod enclosure;
pub mod footprint;
pub mod interface;
pub mod look_graph;
pub mod marking;
pub mod material;
pub mod models;
pub mod native;
pub mod pad_outline;
pub mod project;
pub mod scene;
pub mod silkscreen;
pub mod stack;
pub mod stackup_edge;
pub mod svg;
/// Solidcraft's span profiler: records only when this crate's `profile`
/// feature compiles it in (the build profile benchmark turns it on) and
/// it is switched on; otherwise every call is empty.
pub use solidcraft_engine::profile;
pub use solidcraft_engine::render;
/// Solidcraft's shared viewport renderer: camera, standard views, the CPU
/// picture and the shading models (surface, clear coat, material graph).
pub use solidcraft_viewport as viewport;

/// Solidcraft's geometry, with the plain vector and profile data its public
/// API uses, which live in solidcore's record crate, and its sRGB transfer
/// function, which lives in its colour crate.
pub mod geom {
    pub use solidcore_record::{Loop2, Plane, Seg2, Vec2, Vec3};
    pub use solidcraft_color::srgb;
    pub use solidcraft_engine::geom::*;
}

/// Solidcraft's sketch dimension layout (extension and dimension lines,
/// arrowheads, where the value sits), which the PCB 3D view draws part
/// dimensions with, as Solidcraft draws sketch dimensions.
pub mod dims {
    pub use solidcraft_engine::sketch::{
        ConstraintKind, DimFrame, DimLayout, default_text, default_text_offset, dim_layout_reaching,
    };
}

/// Native foundation fixtures (parametric probe and copper sample). They are
/// acceptance tests for Solidcraft capabilities, not product features.
#[cfg(test)]
mod fixtures;

/// Test values written in millimetres and degrees, as the document stores
/// them (whole nanometres and millidegrees); a value off the range reads
/// as zero.
#[cfg(test)]
pub(crate) mod test_units {
    use pcbcraft_core::library::ModelPlacement;
    use pcbcraft_core::units::{Angle, Nm, PointNm};

    pub fn nm(mm: f64) -> Nm {
        Nm::from_mm_f64_rounded(mm).unwrap_or_default()
    }

    pub fn nm2(mm: [f64; 2]) -> [Nm; 2] {
        mm.map(nm)
    }

    pub fn nm3(mm: [f64; 3]) -> [Nm; 3] {
        mm.map(nm)
    }

    pub fn bounds(mm: [[f64; 3]; 2]) -> [[Nm; 3]; 2] {
        mm.map(nm3)
    }

    pub fn pt(mm: [f64; 2]) -> PointNm {
        PointNm::from_mm_f64_rounded(mm).unwrap_or_default()
    }

    pub fn deg(degrees: f64) -> Angle {
        Angle::from_degrees_f64_rounded(degrees).unwrap_or_default()
    }

    pub fn placement(translation: [f64; 3], rotation_deg: [f64; 3]) -> ModelPlacement {
        ModelPlacement {
            translation_nm: nm3(translation),
            rotation_mdeg: rotation_deg.map(deg),
        }
    }
}

/// The Solidcraft revision Pcbcraft builds against: the `pcbcraft-smlib-open`
/// branch of the DAmesberger/solidcraft fork (upstream a34756830 plus the
/// opt-in product look, via scaling and a font-order fix).
pub const SOLIDCRAFT_REVISION: &str = "64bebfef0a77dab8c79666114c7601c9dda55d12";

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
