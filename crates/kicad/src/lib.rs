//! KiCad board import (slice K1 of the importer design): footprints and
//! pads, with their mask, paste and drill settings, placed where the
//! board has them, built into a new project through the command engine.
//! It reads the bytes it is given; file effects stay with the host.
//!
//! The KiCad format is read from the founder's files, from `kicad-cli`
//! plots of synthetic boards (the oracle) and from KiCad's file-format
//! documentation for facts only; no KiCad source or library was read.
//! Tests build their boards in Rust (no `.kicad_*` fixture files).
pub mod board;
pub mod import;
pub mod items;
pub mod project;
pub mod report;
mod routing;
pub mod sexpr;
pub mod shape;
pub mod translate;
mod zones;

pub use import::{ImportError, Imported, PadRecord, Setup, import};
pub use report::Report;
