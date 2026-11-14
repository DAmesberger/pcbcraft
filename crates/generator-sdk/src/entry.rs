//! The bodies of the ABI exports (called by [`crate::export_generator!`]).
//!
//! Buffers cross the boundary without unsafe code. `alloc` creates a zeroed
//! buffer, keeps it in [`INPUT`] and hands its address to the host, which
//! writes the request into the module's memory there. `invoke` takes the
//! buffer back only when the host names that same address and length, then
//! reads it as an ordinary `Vec`. The address escaped to the host through
//! an export, so the compiler must assume outside writes to it; nothing
//! borrows the buffer while the host writes. Replies are kept in [`OUTPUT`]
//! until the next call, and the host reads them at the address returned.
use crate::Generator;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use sparksmith_generator_abi::{codec, v1};
use spin::Mutex;
use v1::{ErrorBody, Invoke, Problem};

static INPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static OUTPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// `sparksmith_abi`.
pub fn abi() -> u32 {
    v1::ABI
}

/// `sparksmith_alloc`: a zeroed buffer of `len` bytes for the request, or
/// 0 when `len` is 0 or exceeds [`v1::limits::REQUEST_BYTES`].
pub fn alloc(len: u32) -> u32 {
    let Ok(len) = usize::try_from(len) else {
        return 0;
    };
    if len == 0 || len > v1::limits::REQUEST_BYTES {
        return 0;
    }
    let buffer = vec![0u8; len];
    let Ok(address) = u32::try_from(buffer.as_ptr() as usize) else {
        return 0;
    };
    *INPUT.lock() = buffer;
    address
}

/// The buffer the host filled at `address` with `len` bytes (a reply to
/// `call`, [`crate::host`]), taken back; `None` when the host named
/// another buffer.
#[cfg_attr(not(all(target_arch = "wasm32", not(test))), allow(dead_code))]
pub(crate) fn take_reply(address: usize, len: usize) -> Option<Vec<u8>> {
    let buffer = core::mem::take(&mut *INPUT.lock());
    (buffer.as_ptr() as usize == address && buffer.len() == len).then_some(buffer)
}

/// `sparksmith_describe`.
pub fn describe<G: Generator>() -> u64 {
    match codec::encode(&G::describe()) {
        Ok(bytes) => reply(bytes),
        // An undescribable generator is a bug: report nothing, which the
        // host refuses as an invalid description.
        Err(_) => reply(Vec::new()),
    }
}

/// `sparksmith_invoke`: a generator offers one capability,
/// [`v1::GENERATE_PART`].
pub fn invoke<G: Generator>(ptr: u32, len: u32) -> u64 {
    let request = core::mem::take(&mut *INPUT.lock());
    let named = usize::try_from(ptr).ok() == Some(request.as_ptr() as usize)
        && usize::try_from(len).ok() == Some(request.len());
    if !named {
        return error(Problem {
            code: "request.buffer".into(),
            field: None,
            message: "The request was not written to the buffer the generator allocated.".into(),
        });
    }
    let invoke: Invoke = match codec::decode(&request, v1::limits::REQUEST_BYTES) {
        Ok(invoke) => invoke,
        Err(e) => {
            return error(Problem {
                code: "request.invalid".into(),
                field: None,
                message: alloc::format!("The generator could not read the request: {e}."),
            });
        }
    };
    let Some(request) = invoke.request() else {
        return error(Problem {
            code: "capability.unknown".into(),
            field: None,
            message: alloc::format!(
                "This plugin has no capability \"{}\" for that request; it offers \"{}\" with a scope.",
                invoke.capability,
                v1::GENERATE_PART
            ),
        });
    };
    if request.abi != v1::ABI {
        return error(Problem {
            code: "request.abi".into(),
            field: None,
            message: alloc::format!(
                "The request is for generator ABI {}; this generator speaks ABI {}.",
                request.abi,
                v1::ABI
            ),
        });
    }
    match G::generate(&request) {
        Ok(generation) => match codec::encode(&generation) {
            Ok(body) => {
                let mut bytes = Vec::with_capacity(body.len() + 1);
                bytes.push(v1::STATUS_OK);
                bytes.extend_from_slice(&body);
                reply(bytes)
            }
            Err(e) => error(Problem {
                code: "generator.encode".into(),
                field: None,
                message: e.to_string(),
            }),
        },
        Err(errors) => errors_reply(errors),
    }
}

fn error(problem: Problem) -> u64 {
    errors_reply(vec![problem])
}

fn errors_reply(errors: Vec<Problem>) -> u64 {
    let mut bytes = vec![v1::STATUS_ERR];
    // Unencodable errors leave the body empty: the host refuses that as
    // invalid output.
    if let Ok(body) = codec::encode(&ErrorBody { errors }) {
        bytes.extend_from_slice(&body);
    }
    reply(bytes)
}

/// Keep `bytes` until the next call and return `(ptr << 32) | len`.
fn reply(bytes: Vec<u8>) -> u64 {
    let address = u64::try_from(bytes.as_ptr() as usize).unwrap_or(0);
    let len = u64::try_from(bytes.len()).unwrap_or(0);
    *OUTPUT.lock() = bytes;
    (address << 32) | (len & 0xffff_ffff)
}
