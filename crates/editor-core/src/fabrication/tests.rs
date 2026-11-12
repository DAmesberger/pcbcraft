//! Fabrication output tests: an original Gerber/Excellon reader and
//! rasteriser, format details, and raster comparisons of the written
//! layers against the core's geometry.
//!
//! The reader (`reader`, `excellon_reader`) and rasteriser (`raster`) are
//! `pcbcraft-fabcheck`'s, written from Ucamco's public Gerber Layer Format
//! Specification and the public Excellon format description, for
//! verification only. `expected`
//! rasterises the core's own geometry (pads, tracks, vias, fill regions,
//! mask openings, the board outline and drills) without the writer, so a
//! comparison checks the written files against an independent rendering.
#![allow(dead_code)]

mod compare;
mod expected;
mod units;

use pcbcraft_fabcheck::excellon as excellon_reader;
use pcbcraft_fabcheck::gerber as reader;
use pcbcraft_fabcheck::raster;

pub(crate) type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;
