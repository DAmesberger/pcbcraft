//! Runs generator modules in a bounded wasmi sandbox.
//!
//! A module is checked before anything runs (size, a valid core module of
//! the allowed features, no imports, no start function, the ABI exports, a
//! memory within the limit), then every call gets a fresh instance with a
//! fuel budget and a memory cap, so a module can waste at most bounded work
//! and keeps no state between calls. Results are decoded with the canonical
//! codec and checked ([`validate`]) before anyone sees them. Every refusal is
//! a [`Failure`] in plain words; nothing outside the call changes.
//!
//! The crate builds for wasm32 too, so a browser build can run generators
//! the same way (see `crates/generators-probe`, which proves that build
//! gives the same result hashes).

pub mod params;
pub mod validate;

use pcbcraft_generator_abi::v1::{self, Description, ErrorBody, Generation, Request};
use pcbcraft_generator_abi::{CodecError, Failure, Generated, GeneratorRuntime, codec};
use wasmi::errors::{ErrorKind, MemoryError};
use wasmi::{
    CompilationMode, Config, Engine, ExternType, Instance, Linker, Memory, Module, Store,
    StoreLimits, StoreLimitsBuilder, TrapCode, TypedFunc,
};

pub use pcbcraft_generator_abi as abi;

/// Execution bounds of one call. The defaults are set from GP0's
/// measurements of the chip generator (`generators/README.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest module accepted.
    pub module_bytes: usize,
    /// Fuel per call (`describe` or `generate`, instantiation included).
    pub fuel: u64,
    /// Largest linear memory, initial size included.
    pub memory_bytes: usize,
    /// Largest table (function references).
    pub table_elements: usize,
    /// Largest request written into the module.
    pub request_bytes: usize,
    /// Largest description read.
    pub description_bytes: usize,
    /// Largest response read (status byte included).
    pub response_bytes: usize,
}

impl Limits {
    /// The limits generators run under, set from GP0's measurements
    /// (`generators/README.md`): the chip module is 274 KiB, describe takes
    /// 6.0 M fuel and generate at most 1.8 M, and memory ends at 1.1 MiB.
    /// Fuel grows by about 460 per response byte, so a response at the
    /// 1 MiB bound needs about 0.5 G: the fuel matches that bound, and at
    /// the measured 1.1 G fuel per second it ends a call within about 0.5 s
    /// natively (the 2 s wall-time target leaves room for slower hosts).
    pub const DEFAULT: Self = Self {
        module_bytes: 1024 * 1024,
        fuel: 500_000_000,
        memory_bytes: 32 * 1024 * 1024,
        table_elements: 10_000,
        request_bytes: v1::limits::REQUEST_BYTES,
        description_bytes: v1::limits::DESCRIPTION_BYTES,
        response_bytes: v1::limits::RESPONSE_BYTES,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The wasmi generator runtime.
pub struct WasmiRuntime {
    engine: Engine,
    limits: Limits,
}

/// A checked, compiled module. Each call instantiates it afresh.
pub struct Loaded<'r> {
    runtime: &'r WasmiRuntime,
    module: Module,
}

/// The raw reply of a `generate` call: its status byte and body, before
/// decoding, with the fuel the call took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raw {
    pub status: u8,
    pub body: Vec<u8>,
    pub fuel_used: u64,
    /// The module's linear memory at the end of the call.
    pub memory_bytes: usize,
}

fn fail(code: &str, message: impl Into<String>) -> Failure {
    Failure::one(code, message.into())
}

/// The engine configuration every generator runs under: fuel metering, no
/// start function, and only the deterministic scalar feature set that
/// rustc emits for `wasm32-unknown-unknown` (SIMD is not compiled into
/// this wasmi; threads, memory64 and GC are not supported by it either).
pub fn config() -> Config {
    let mut config = Config::default();
    config
        .consume_fuel(true)
        // Compile every function at load, so no call pays for compiling and
        // the fuel a call takes is the same on every call.
        .compilation_mode(CompilationMode::Eager)
        .allow_start_fn(false)
        .wasm_multi_memory(false)
        .wasm_tail_call(false)
        .wasm_extended_const(false)
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false);
    config
}

impl WasmiRuntime {
    pub fn new(limits: Limits) -> Self {
        Self {
            engine: Engine::new(&config()),
            limits,
        }
    }

    /// Like [`WasmiRuntime::new`], over an explicit engine configuration
    /// (measurement and tests). Fuel metering is always on.
    pub fn with_config(mut config: Config, limits: Limits) -> Self {
        config.consume_fuel(true);
        Self {
            engine: Engine::new(&config),
            limits,
        }
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Check and compile `bytes`.
    pub fn load(&self, bytes: &[u8]) -> Result<Loaded<'_>, Failure> {
        if bytes.len() > self.limits.module_bytes {
            return Err(fail(
                "generator.too_large",
                format!(
                    "The generator module is {} bytes, more than the {} allowed. It was not run.",
                    bytes.len(),
                    self.limits.module_bytes
                ),
            ));
        }
        let module = Module::new(&self.engine, bytes).map_err(|e| {
            let detail = e.to_string();
            // wasmi (pinned) words a disabled proposal as "… support is not
            // enabled", and SIMD, which this build cannot run at all, as an
            // "unexpected SIMD opcode"; the refusal tests pin both.
            if detail.contains("SIMD")
                || detail.contains("support is not enabled")
                || detail.contains("not supported")
            {
                fail(
                    "generator.unsupported_feature",
                    format!(
                        "The generator uses a WebAssembly feature generators may not use ({detail}). It was not run."
                    ),
                )
            } else {
                fail(
                    "generator.invalid_module",
                    format!("The generator is not a valid WebAssembly module ({detail}). It was not run."),
                )
            }
        })?;
        if let Some(import) = module.imports().next() {
            return Err(fail(
                "generator.has_imports",
                format!(
                    "This generator asks for host functions ({}.{}), which generators may not use. It was not run.",
                    import.module(),
                    import.name()
                ),
            ));
        }
        let mut memory = None;
        for name in [
            v1::exports::ABI,
            v1::exports::ALLOC,
            v1::exports::DESCRIBE,
            v1::exports::GENERATE,
            v1::exports::MEMORY,
        ] {
            let found = module.exports().find(|e| e.name() == name);
            let kind_ok = match (name, found.as_ref().map(|e| e.ty())) {
                (v1::exports::MEMORY, Some(ExternType::Memory(m))) => {
                    memory = Some(*m);
                    true
                }
                (v1::exports::MEMORY, _) => false,
                (_, Some(ExternType::Func(_))) => true,
                _ => false,
            };
            if !kind_ok {
                return Err(fail(
                    "generator.missing_export",
                    format!(
                        "The generator does not export \"{name}\", which ABI 1 requires. It was not run."
                    ),
                ));
            }
        }
        if let Some(memory) = memory {
            let initial = memory.minimum().saturating_mul(WASM_PAGE_BYTES);
            if initial > self.limits.memory_bytes as u64 {
                return Err(memory_refusal(self.limits.memory_bytes));
            }
        }
        Ok(Loaded {
            runtime: self,
            module,
        })
    }
}

/// The WebAssembly page size (custom page sizes are refused).
const WASM_PAGE_BYTES: u64 = 64 * 1024;

fn memory_refusal(limit: usize) -> Failure {
    fail(
        "generator.memory_limit",
        format!(
            "The generator needed more than its {} MiB of memory. Nothing was changed.",
            limit / (1024 * 1024)
        ),
    )
}

/// Map a wasmi error during a call to a refusal in plain words.
fn call_failure(error: &wasmi::Error, limits: &Limits) -> Failure {
    if matches!(
        error.kind(),
        ErrorKind::Memory(MemoryError::ResourceLimiterDeniedAllocation)
    ) {
        return memory_refusal(limits.memory_bytes);
    }
    match error.as_trap_code() {
        Some(TrapCode::OutOfFuel) => fail(
            "generator.out_of_fuel",
            "The generator did not finish within its work limit. Nothing was changed.",
        ),
        Some(TrapCode::GrowthOperationLimited) => memory_refusal(limits.memory_bytes),
        Some(code) => fail(
            "generator.trapped",
            format!(
                "The generator stopped with an internal error ({}). Nothing was changed.",
                code.trap_message()
            ),
        ),
        None => fail(
            "generator.trapped",
            format!("The generator stopped with an internal error ({error}). Nothing was changed."),
        ),
    }
}

fn invalid_output(what: &str, error: &CodecError) -> Failure {
    fail(
        "generator.invalid_output",
        format!("The generator returned an unreadable {what}: {error}. Nothing was changed."),
    )
}

/// A fresh instance and its memory.
struct Call {
    store: Store<StoreLimits>,
    instance: Instance,
    memory: Memory,
    limits: Limits,
}

impl Call {
    fn func<P: wasmi::WasmParams, R: wasmi::WasmResults>(
        &self,
        name: &str,
    ) -> Result<TypedFunc<P, R>, Failure> {
        self.instance
            .get_typed_func::<P, R>(&self.store, name)
            .map_err(|_| {
                fail(
                    "generator.missing_export",
                    format!("The generator's \"{name}\" export has the wrong signature for ABI 1. It was not run."),
                )
            })
    }

    fn check_abi(&mut self) -> Result<(), Failure> {
        let abi = self.func::<(), u32>(v1::exports::ABI)?;
        let version = abi
            .call(&mut self.store, ())
            .map_err(|e| call_failure(&e, &self.limits))?;
        if version != v1::ABI {
            return Err(fail(
                "generator.abi_mismatch",
                format!(
                    "The generator speaks ABI {version}; this application runs ABI {}. It was not run.",
                    v1::ABI
                ),
            ));
        }
        Ok(())
    }

    /// Read `(ptr << 32) | len` from memory, at most `limit` bytes.
    fn read(&self, packed: u64, limit: usize, what: &str) -> Result<Vec<u8>, Failure> {
        let len = usize::try_from(packed & 0xffff_ffff).unwrap_or(usize::MAX);
        let ptr = usize::try_from(packed >> 32).unwrap_or(usize::MAX);
        if len > limit {
            return Err(fail(
                "generator.output_too_large",
                format!(
                    "The generator returned a {what} of {len} bytes, more than the {limit} allowed. Nothing was changed."
                ),
            ));
        }
        let mut bytes = vec![0u8; len];
        self.memory
            .read(&self.store, ptr, &mut bytes)
            .map_err(|_| {
                fail(
                    "generator.invalid_output",
                    format!("The generator's {what} lies outside its memory. Nothing was changed."),
                )
            })?;
        Ok(bytes)
    }

    fn fuel_used(&self) -> u64 {
        self.limits
            .fuel
            .saturating_sub(self.store.get_fuel().unwrap_or(0))
    }
}

impl Loaded<'_> {
    fn start(&self) -> Result<Call, Failure> {
        let limits = self.runtime.limits;
        let store_limits = StoreLimitsBuilder::new()
            .memory_size(limits.memory_bytes)
            .table_elements(limits.table_elements)
            .instances(1)
            .memories(1)
            .tables(1)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(&self.runtime.engine, store_limits);
        store.limiter(|s| s);
        store
            .set_fuel(limits.fuel)
            .map_err(|e| call_failure(&e, &limits))?;
        let linker = Linker::<StoreLimits>::new(&self.runtime.engine);
        let instance = linker
            .instantiate_and_start(&mut store, &self.module)
            .map_err(|e| call_failure(&e, &limits))?;
        let memory = instance
            .get_memory(&store, v1::exports::MEMORY)
            .ok_or_else(|| {
                fail(
                    "generator.missing_export",
                    "The generator does not export its memory. It was not run.",
                )
            })?;
        let mut call = Call {
            store,
            instance,
            memory,
            limits,
        };
        call.check_abi()?;
        Ok(call)
    }

    /// The module's description, decoded and checked.
    pub fn describe(&self) -> Result<Description, Failure> {
        let mut call = self.start()?;
        let describe = call.func::<(), u64>(v1::exports::DESCRIBE)?;
        let packed = describe
            .call(&mut call.store, ())
            .map_err(|e| call_failure(&e, &call.limits))?;
        let bytes = call.read(packed, call.limits.description_bytes, "description")?;
        let description: Description = codec::decode(&bytes, call.limits.description_bytes)
            .map_err(|e| invalid_output("description", &e))?;
        validate::description(&description)?;
        Ok(description)
    }

    /// Run `generate` on raw request bytes and return the raw reply.
    pub fn generate_raw(&self, request: &[u8]) -> Result<Raw, Failure> {
        let limit = self.runtime.limits.request_bytes;
        if request.len() > limit {
            return Err(fail(
                "generator.request_too_large",
                format!(
                    "The request is {} bytes, more than the {limit} a generator accepts. It was not run.",
                    request.len()
                ),
            ));
        }
        let mut call = self.start()?;
        let len = u32::try_from(request.len()).map_err(|_| {
            fail(
                "generator.request_too_large",
                "The request is too large. It was not run.",
            )
        })?;
        let alloc = call.func::<u32, u32>(v1::exports::ALLOC)?;
        let ptr = alloc
            .call(&mut call.store, len)
            .map_err(|e| call_failure(&e, &call.limits))?;
        if ptr == 0 {
            return Err(fail(
                "generator.request_refused",
                "The generator could not take the request (it allocated no buffer). Nothing was changed.",
            ));
        }
        let at = usize::try_from(ptr).unwrap_or(usize::MAX);
        call.memory
            .write(&mut call.store, at, request)
            .map_err(|_| {
                fail(
                    "generator.invalid_output",
                    "The generator's request buffer lies outside its memory. Nothing was changed.",
                )
            })?;
        let generate = call.func::<(u32, u32), u64>(v1::exports::GENERATE)?;
        let packed = generate
            .call(&mut call.store, (ptr, len))
            .map_err(|e| call_failure(&e, &call.limits))?;
        let bytes = call.read(packed, call.limits.response_bytes, "response")?;
        let fuel_used = call.fuel_used();
        let memory_bytes = call.memory.data_size(&call.store);
        let Some((&status, body)) = bytes.split_first() else {
            return Err(fail(
                "generator.invalid_output",
                "The generator returned an empty response. Nothing was changed.",
            ));
        };
        Ok(Raw {
            status,
            body: body.to_vec(),
            fuel_used,
            memory_bytes,
        })
    }

    /// Run `request` and return the validated result.
    pub fn generate(&self, request: &Request) -> Result<Generated, Failure> {
        let bytes = codec::encode(request).map_err(|e| {
            fail(
                "generator.request_invalid",
                format!("The request could not be written: {e}. It was not run."),
            )
        })?;
        let raw = self.generate_raw(&bytes)?;
        let limit = self.runtime.limits.response_bytes;
        match raw.status {
            v1::STATUS_OK => {
                let generation: Generation =
                    codec::decode(&raw.body, limit).map_err(|e| invalid_output("part", &e))?;
                validate::generation(&generation, request.scope)?;
                Ok(Generated {
                    result_hash: pcbcraft_generator_abi::result_hash(&raw.body),
                    canonical: raw.body,
                    generation,
                    fuel_used: Some(raw.fuel_used),
                })
            }
            v1::STATUS_ERR => {
                let body: ErrorBody = codec::decode(&raw.body, limit)
                    .map_err(|e| invalid_output("error report", &e))?;
                if body.errors.is_empty() {
                    return Err(fail(
                        "generator.invalid_output",
                        "The generator refused the request without saying why. Nothing was changed.",
                    ));
                }
                Err(Failure {
                    problems: body.errors,
                })
            }
            other => Err(fail(
                "generator.invalid_output",
                format!("The generator returned an unknown status {other}. Nothing was changed."),
            )),
        }
    }
}

impl GeneratorRuntime for WasmiRuntime {
    fn describe(&self, module: &[u8]) -> Result<Description, Failure> {
        self.load(module)?.describe()
    }

    fn generate(&self, module: &[u8], request: &Request) -> Result<Generated, Failure> {
        if request.abi != v1::ABI {
            return Err(fail(
                "generator.abi_mismatch",
                format!(
                    "The request is for ABI {}; this application runs ABI {}.",
                    request.abi,
                    v1::ABI
                ),
            ));
        }
        self.load(module)?.generate(request)
    }
}
