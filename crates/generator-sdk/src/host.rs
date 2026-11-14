//! The three host functions a plugin may import (`docs/design/
//! extension-abi-design.md` §3.5): `call`, `log` and `cancelled`, all
//! optional. A plugin that uses none of these functions imports nothing,
//! which is what a plugin whose capabilities are all of effect `none` must
//! do; only a plugin that also offers a `read` or `edit` capability may use
//! them. During an invoke of effect `none` the host answers `call` with
//! the fixed refusal (`v1::pure_refusal`), drops `log` and answers
//! `cancelled` with 0, so a module that calls them still gives one result.
//!
//! Natively (the golden tests) the functions answer exactly as the host
//! does during an invoke of effect `none`, so the native result hash is
//! the one wasmi gives.
//!
//! # Unsafe code
//!
//! Importing a host function needs an `unsafe extern` block, the only
//! spelling Rust has; the imports are declared `safe` (they take and
//! return integers), and this module allows `unsafe_code` for that block
//! alone. Replies cross without pointer dereferences: the host writes a
//! reply into a buffer the module allocated through `sparksmith_alloc`
//! ([`crate::entry`]), which [`call`] takes back by its address.
use alloc::vec::Vec;

#[cfg(all(target_arch = "wasm32", not(test)))]
#[allow(unsafe_code)] // Importing host functions; see the module's "Unsafe code".
#[link(wasm_import_module = "sparksmith")]
unsafe extern "C" {
    #[link_name = "call"]
    safe fn host_call(ptr: u32, len: u32) -> u64;
    #[link_name = "log"]
    safe fn host_log(level: u32, ptr: u32, len: u32);
    #[link_name = "cancelled"]
    safe fn host_cancelled() -> u32;
}

/// `call`: one envelope (`{entry, params}`, canonical) to the host. The
/// reply: a status byte (`v1::STATUS_OK` or `v1::STATUS_ERR`) and the
/// reply or error bytes; empty when the host gave no reply.
pub fn call(envelope: &[u8]) -> Vec<u8> {
    #[cfg(all(target_arch = "wasm32", not(test)))]
    {
        let (Ok(ptr), Ok(len)) = (
            u32::try_from(envelope.as_ptr() as usize),
            u32::try_from(envelope.len()),
        ) else {
            return Vec::new();
        };
        let packed = host_call(ptr, len);
        let (Ok(at), Ok(n)) = (
            usize::try_from(packed >> 32),
            usize::try_from(packed & 0xffff_ffff),
        ) else {
            return Vec::new();
        };
        crate::entry::take_reply(at, n).unwrap_or_default()
    }
    #[cfg(not(all(target_arch = "wasm32", not(test))))]
    {
        let _ = envelope;
        use sparksmith_generator_abi::v1;
        let mut bytes = alloc::vec![v1::STATUS_ERR];
        if let Ok(body) = sparksmith_generator_abi::codec::encode(&v1::pure_refusal()) {
            bytes.extend_from_slice(&body);
        }
        bytes
    }
}

/// `log`: a line for the run's log at `level` (dropped in an invoke of
/// effect `none`).
pub fn log(level: u32, line: &[u8]) {
    #[cfg(all(target_arch = "wasm32", not(test)))]
    if let (Ok(ptr), Ok(len)) = (
        u32::try_from(line.as_ptr() as usize),
        u32::try_from(line.len()),
    ) {
        host_log(level, ptr, len);
    }
    #[cfg(not(all(target_arch = "wasm32", not(test))))]
    let _ = (level, line);
}

/// `cancelled`: whether the run should stop (never in an invoke of effect
/// `none`).
pub fn cancelled() -> bool {
    #[cfg(all(target_arch = "wasm32", not(test)))]
    {
        host_cancelled() != 0
    }
    #[cfg(not(all(target_arch = "wasm32", not(test))))]
    {
        false
    }
}
