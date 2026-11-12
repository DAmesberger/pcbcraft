//! Fabrication output read back, for verification only: a Gerber X2 reader
//! ([`gerber`]) and an Excellon reader ([`excellon`]) written from Ucamco's
//! public Gerber Layer Format Specification and the public Excellon format
//! description, a bitmap rasteriser ([`raster`]), exact polygon rendering in
//! integer nanometres ([`polygons`]), a layer comparison reporting the
//! difference area and its widest piece ([`compare`]) and drill-hit pairing
//! ([`drills`]), whole sets paired by file function ([`sets`]), and
//! reference files from an installed `kicad-cli` run as a black-box oracle
//! on a temporary copy ([`reference`]).
//!
//! No other tool's reader or writer was consulted. Nothing here ships: the
//! workspace uses this crate as a dev-dependency and from `xtask`.
pub mod compare;
pub mod drills;
pub mod excellon;
pub mod gerber;
pub mod polygons;
pub mod raster;
pub mod reference;
pub mod sets;

/// The error type of this crate's fallible functions.
pub type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A point in millimetres.
pub type Point = [f64; 2];
