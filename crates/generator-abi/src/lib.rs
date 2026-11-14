//! The generator plugin ABI: what a generator module and the host exchange.
//!
//! A generator is a pure function in a plain core WebAssembly module: typed
//! parameters in, a complete part body out. The module imports nothing and
//! exports four functions ([`v1::exports`]). Everything crossing the boundary
//! is canonical JSON ([`codec`]): sorted keys, no insignificant whitespace,
//! integers only (lengths in nm, angles in millidegrees), so the same
//! request always gives the same bytes and therefore the same result hash.
//!
//! This crate holds types and the codec only, no generator logic, and is
//! `no_std` + `alloc` so every module can compile it in. The wire types live
//! in a versioned module ([`v1`]); a later, incompatible ABI is a new module
//! beside it, and the host keeps running every ABI it shipped.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod codec;
pub mod params;
pub mod runtime;
pub mod surface;
pub mod v1;

pub use codec::{CodecError, decode, encode, hex, result_hash, sha256};
pub use runtime::{Failure, Generated, GeneratorRuntime};
