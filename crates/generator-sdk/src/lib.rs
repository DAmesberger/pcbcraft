//! The toolkit generator modules are written with.
//!
//! A generator crate implements [`Generator`] and calls
//! [`export_generator!`]; the SDK supplies the ABI exports, the request and
//! response buffers, the allocator and panic handler of the module, typed
//! parameter access ([`args`]), symbol drawing ([`draw`]), land and
//! footprint drawing helpers ([`land`]) and the native golden-test harness
//! ([`golden`]).
//!
//! # Unsafe code
//!
//! The SDK itself has none. A WebAssembly export needs `#[unsafe(no_mangle)]`
//! (Rust has no safe way to name an exported symbol), so
//! [`export_generator!`] expands to four exported functions that allow
//! `unsafe_code` for that attribute alone; a generator crate therefore sets
//! `unsafe_code = "deny"` rather than `forbid` and writes no unsafe code of
//! its own. Buffers cross the boundary without pointer dereferences: the
//! host writes into a buffer the module allocated and handed out (see
//! [`entry`]).
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod args;
pub mod draw;
pub mod entry;
pub mod golden;
pub mod land;
pub mod units;

pub use pcbcraft_generator_abi as abi;
pub use pcbcraft_generator_abi::v1;

use alloc::vec::Vec;
use v1::{Description, Generation, Problem, Request};

/// A generator: a pure function from complete parameters to a part body.
pub trait Generator {
    /// What the generator offers. Must be the same on every call.
    fn describe() -> Description;
    /// The part body for `request`, or the parameter problems in plain words.
    fn generate(request: &Request) -> Result<Generation, Vec<Problem>>;
}

/// Parse a generator's preset table: a JSON array of [`v1::Preset`]s,
/// usually `include_str!`'d from the crate's `presets.json`.
pub fn presets(json: &str) -> Result<Vec<v1::Preset>, alloc::string::String> {
    serde_json::from_str(json).map_err(|e| alloc::format!("presets.json: {e}"))
}

/// Export `$generator` (a type implementing [`Generator`]) as the module's
/// ABI functions. Use once, in the generator crate's root.
#[macro_export]
macro_rules! export_generator {
    ($generator:ty) => {
        /// ABI export: the ABI version.
        #[allow(unsafe_code)] // An exported symbol name; see the SDK's "Unsafe code".
        #[unsafe(no_mangle)]
        pub extern "C" fn pcbcraft_gen_abi() -> u32 {
            $crate::entry::abi()
        }

        /// ABI export: a request buffer.
        #[allow(unsafe_code)] // An exported symbol name; see the SDK's "Unsafe code".
        #[unsafe(no_mangle)]
        pub extern "C" fn pcbcraft_gen_alloc(len: u32) -> u32 {
            $crate::entry::alloc(len)
        }

        /// ABI export: the canonical description.
        #[allow(unsafe_code)] // An exported symbol name; see the SDK's "Unsafe code".
        #[unsafe(no_mangle)]
        pub extern "C" fn pcbcraft_gen_describe() -> u64 {
            $crate::entry::describe::<$generator>()
        }

        /// ABI export: generate from the request in the buffer.
        #[allow(unsafe_code)] // An exported symbol name; see the SDK's "Unsafe code".
        #[unsafe(no_mangle)]
        pub extern "C" fn pcbcraft_gen_generate(ptr: u32, len: u32) -> u64 {
            $crate::entry::generate::<$generator>(ptr, len)
        }
    };
}

/// The module runtime on the generator target: the allocator `std` uses
/// there, and a panic handler that traps (the host reports
/// `generator.trapped`). A panic in a generator is a bug; generators return
/// [`Problem`]s instead.
#[cfg(all(target_arch = "wasm32", not(test)))]
mod module_runtime {
    #[global_allocator]
    static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

    #[panic_handler]
    fn on_panic(_: &core::panic::PanicInfo<'_>) -> ! {
        core::arch::wasm32::unreachable()
    }
}
