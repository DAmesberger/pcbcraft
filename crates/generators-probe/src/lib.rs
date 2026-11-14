//! The probe: the generator host (`sparksmith-generators`, wasmi included)
//! built for `wasm32-unknown-unknown`, as a module the native host runs.
//! It answers the golden check "under wasmi inside the wasm32 build": the
//! native test hands it a generator module and a request, the probe runs
//! them with its own wasm32 copy of the host under the default limits, and
//! the native test compares result hash and fuel with its own run.
//!
//! It speaks generator ABI 1's calling convention so the native host can
//! load it, with its own request format:
//! - request: module length (u32, little-endian), module bytes, canonical
//!   request bytes;
//! - reply: `STATUS_OK`, the 32-byte result hash, the fuel used (u64,
//!   little-endian), the canonical generation; or `STATUS_ERR` and an
//!   `ErrorBody` with the inner host's refusal.
//!
//! Test code only: it is built by `cargo xtask generators check` and never
//! shipped. Buffers cross without pointer dereferences, as in the SDK's
//! `entry` module.
use sparksmith_generator_abi::v1::{self, Description, ErrorBody, Request, Scope};
use sparksmith_generator_abi::{Failure, codec};
use sparksmith_generators::{Limits, WasmiRuntime};
use std::sync::Mutex;

/// Largest input: a module of the default limit plus a request.
const INPUT_BYTES: usize = 4 * 1024 * 1024;

static INPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static OUTPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn reply(bytes: Vec<u8>) -> u64 {
    let address = u64::try_from(bytes.as_ptr() as usize).unwrap_or(0);
    let len = u64::try_from(bytes.len()).unwrap_or(0);
    if let Ok(mut output) = OUTPUT.lock() {
        *output = bytes;
    }
    (address << 32) | (len & 0xffff_ffff)
}

fn refuse(failure: Failure) -> Vec<u8> {
    let mut bytes = vec![v1::STATUS_ERR];
    if let Ok(body) = codec::encode(&ErrorBody {
        errors: failure.problems,
    }) {
        bytes.extend_from_slice(&body);
    }
    bytes
}

fn run(input: &[u8]) -> Vec<u8> {
    let Some((len, rest)) = input.split_first_chunk::<4>() else {
        return refuse(Failure::one("probe.input", "No module length.".into()));
    };
    let len = usize::try_from(u32::from_le_bytes(*len)).unwrap_or(usize::MAX);
    let Some((module, request)) = rest.split_at_checked(len) else {
        return refuse(Failure::one(
            "probe.input",
            "The module is cut short.".into(),
        ));
    };
    let request: Request = match codec::decode(request, v1::limits::REQUEST_BYTES) {
        Ok(request) => request,
        Err(e) => return refuse(Failure::one("probe.input", e.to_string())),
    };
    let runtime = WasmiRuntime::new(Limits::DEFAULT);
    let generated = runtime.load(module).and_then(|m| m.generate(&request));
    match generated {
        Ok(g) => {
            let mut bytes = vec![v1::STATUS_OK];
            bytes.extend_from_slice(&g.result_hash);
            bytes.extend_from_slice(&g.fuel_used.unwrap_or(0).to_le_bytes());
            bytes.extend_from_slice(&g.canonical);
            bytes
        }
        Err(failure) => refuse(failure),
    }
}

/// ABI export: the ABI version.
#[allow(unsafe_code)] // An exported symbol name: no safe spelling exists.
#[unsafe(no_mangle)]
pub extern "C" fn sparksmith_gen_abi() -> u32 {
    v1::ABI
}

/// ABI export: an input buffer.
#[allow(unsafe_code)] // An exported symbol name: no safe spelling exists.
#[unsafe(no_mangle)]
pub extern "C" fn sparksmith_gen_alloc(len: u32) -> u32 {
    let Ok(len) = usize::try_from(len) else {
        return 0;
    };
    if len == 0 || len > INPUT_BYTES {
        return 0;
    }
    let buffer = vec![0u8; len];
    let address = u32::try_from(buffer.as_ptr() as usize).unwrap_or(0);
    match INPUT.lock() {
        Ok(mut input) => {
            *input = buffer;
            address
        }
        Err(_) => 0,
    }
}

/// ABI export: a description of the probe (no parameters).
#[allow(unsafe_code)] // An exported symbol name: no safe spelling exists.
#[unsafe(no_mangle)]
pub extern "C" fn sparksmith_gen_describe() -> u64 {
    let description = Description {
        abi: v1::ABI,
        id_hint: "probe".into(),
        version: 1,
        name: "Generator host probe".into(),
        families: Vec::new(),
        scopes: vec![Scope::Package],
        params: Vec::new(),
        rules: Vec::new(),
        presets: Vec::new(),
        grammar: Vec::new(),
        instance_fields: Vec::new(),
    };
    reply(codec::encode(&description).unwrap_or_default())
}

/// ABI export: run the module and request in the input buffer.
#[allow(unsafe_code)] // An exported symbol name: no safe spelling exists.
#[unsafe(no_mangle)]
pub extern "C" fn sparksmith_gen_generate(ptr: u32, len: u32) -> u64 {
    let input = INPUT
        .lock()
        .map(|mut input| std::mem::take(&mut *input))
        .unwrap_or_default();
    let named = usize::try_from(ptr).ok() == Some(input.as_ptr() as usize)
        && usize::try_from(len).ok() == Some(input.len());
    if !named {
        return reply(refuse(Failure::one(
            "probe.input",
            "The input was not written to the probe's buffer.".into(),
        )));
    }
    reply(run(&input))
}
