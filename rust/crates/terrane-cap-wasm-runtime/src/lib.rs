//! The `wasm-runtime` capability — running app backends in Wasmtime.
//!
//! This runtime does not enable WASI or ambient host access. Guest modules get
//! only the imports declared here, and all persistent effects flow through
//! Terrane resources so replay remains event-log deterministic.

use std::path::Path;

use nanoserde::DeJson;
use terrane_cap_interface::{
    arg, ensure_app_exists, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error,
    EventRecord, ReadValue, Result, RuntimeCtx, RuntimeHostHandle, RuntimeOutput, RuntimeRequest,
    StateStore,
};
use wasmtime::{Caller, Config, Engine, Linker, Memory, Module, Store};
type AnyResult<T> = wasmtime::Result<T>;

pub struct WasmRuntimeCapability;

impl Capability for WasmRuntimeCapability {
    fn namespace(&self) -> &'static str {
        "wasm-runtime"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec {
                name: "wasm-runtime.run",
            }],
            events: Vec::new(),
            queries: Vec::new(),
            resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "wasm-runtime.run" => {
                let app = arg(args, 0, "app")?;
                ensure_app_exists(ctx.bus, &app)?;
                Ok(Decision::Runtime(RuntimeRequest {
                    app,
                    input: args.get(1..).unwrap_or_default().to_vec(),
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
        Ok(())
    }

    fn run_runtime(&self, ctx: RuntimeCtx, request: RuntimeRequest) -> Result<RuntimeOutput> {
        let bundle = load_bundle(&ctx.source)?;
        let output = run_wasm_bundle(&request.input, &bundle, ctx.host)?;
        Ok(RuntimeOutput { output })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmRuntimeBundle {
    pub module: Vec<u8>,
    pub name: String,
    pub entry: String,
    pub resources: Vec<String>,
}

/// Run a memory-backed WASM backend once.
pub fn run_wasm_bundle(
    input: &[String],
    bundle: &WasmRuntimeBundle,
    host: RuntimeHostHandle,
) -> Result<String> {
    execute_wasm(input, bundle, host)
}

#[derive(Debug, Clone, DeJson)]
pub struct BundleManifest {
    #[nserde(default)]
    pub id: String,
    #[nserde(default)]
    pub name: String,
    #[nserde(default)]
    pub runtime: String,
    #[nserde(default)]
    pub module: String,
    #[nserde(default)]
    pub entry: String,
    #[nserde(default)]
    pub ui: String,
    #[nserde(default)]
    pub resources: Vec<String>,
}

pub fn read_manifest(bundle_dir: &Path) -> Result<BundleManifest> {
    let text = std::fs::read_to_string(bundle_dir.join("manifest.json"))
        .map_err(|e| Error::Runtime(format!("read manifest.json: {e}")))?;
    BundleManifest::deserialize_json(&text)
        .map_err(|e| Error::Runtime(format!("manifest.json: {e}")))
}

fn load_bundle(source: &str) -> Result<WasmRuntimeBundle> {
    let path = Path::new(source);
    if path.is_dir() {
        let manifest = read_manifest(path)?;
        if manifest.runtime != "wasm" {
            return Err(Error::Runtime(format!(
                "manifest runtime {:?} is not wasm",
                manifest.runtime
            )));
        }
        if manifest.module.trim().is_empty() {
            return Err(Error::Runtime(
                "manifest.module is required for wasm".into(),
            ));
        }
        let module_path = path.join(&manifest.module);
        let module = std::fs::read(&module_path)
            .map_err(|e| Error::Runtime(format!("read module {}: {e}", module_path.display())))?;
        Ok(WasmRuntimeBundle {
            module,
            name: manifest.name,
            entry: non_empty_or(manifest.entry, "handle"),
            resources: manifest.resources,
        })
    } else {
        let module = std::fs::read(path)
            .map_err(|e| Error::Runtime(format!("read module {}: {e}", path.display())))?;
        Ok(WasmRuntimeBundle {
            module,
            name: String::new(),
            entry: "handle".to_string(),
            resources: vec!["kv".to_string()],
        })
    }
}

struct WasmState {
    host: RuntimeHostHandle,
    resources: Vec<String>,
}

fn execute_wasm(
    input: &[String],
    bundle: &WasmRuntimeBundle,
    host: RuntimeHostHandle,
) -> Result<String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config).map_err(wasm_err)?;
    let module = Module::from_binary(&engine, &bundle.module).map_err(wasm_err)?;
    let mut linker = Linker::new(&engine);
    define_imports(&mut linker).map_err(wasm_err)?;

    let mut store = Store::new(
        &engine,
        WasmState {
            host,
            resources: bundle.resources.clone(),
        },
    );
    store.set_fuel(wasm_fuel()).map_err(wasm_err)?;
    let instance = linker.instantiate(&mut store, &module).map_err(wasm_err)?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| Error::Runtime("wasm module must export memory".into()))?;
    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "alloc")
        .map_err(|e| Error::Runtime(format!("wasm module must export alloc(len)->ptr: {e}")))?;
    let handle = instance
        .get_typed_func::<(i32, i32), i64>(&mut store, &bundle.entry)
        .map_err(|e| {
            Error::Runtime(format!(
                "wasm module must export {}(ptr,len)->i64: {e}",
                bundle.entry
            ))
        })?;

    let input_json =
        serde_json::to_vec(input).map_err(|e| Error::Runtime(format!("encode wasm input: {e}")))?;
    let input_len = checked_i32(input_json.len(), "input length")?;
    let input_ptr = alloc.call(&mut store, input_len).map_err(wasm_err)?;
    memory
        .write(
            &mut store,
            checked_usize(input_ptr, "input pointer")?,
            &input_json,
        )
        .map_err(|_| Error::Runtime("wasm input pointer is out of bounds".into()))?;

    let packed = handle
        .call(&mut store, (input_ptr, input_len))
        .map_err(wasm_err)?;
    let (out_ptr, out_len) = unpack_ptr_len(packed)?;
    let mut output = vec![0u8; out_len];
    memory
        .read(&store, out_ptr, &mut output)
        .map_err(|_| Error::Runtime("wasm output pointer is out of bounds".into()))?;

    if let Ok(dealloc) = instance.get_typed_func::<(i32, i32), ()>(&mut store, "dealloc") {
        let _ = dealloc.call(&mut store, (input_ptr, input_len));
        let _ = dealloc.call(&mut store, (out_ptr as i32, out_len as i32));
    }

    String::from_utf8(output).map_err(|e| Error::Runtime(format!("wasm output is not UTF-8: {e}")))
}

fn define_imports(linker: &mut Linker<WasmState>) -> AnyResult<()> {
    linker.func_wrap(
        "terrane",
        "resource_write",
        |mut caller: Caller<'_, WasmState>,
         ns_pair: i64,
         method_pair: i64,
         args_pair: i64|
         -> AnyResult<i32> {
            let namespace = read_string_pair(&mut caller, ns_pair)?;
            ensure_resource_allowed(&caller, &namespace)?;
            let method = read_string_pair(&mut caller, method_pair)?;
            let args_json = read_string_pair(&mut caller, args_pair)?;
            let args: Vec<String> = serde_json::from_str(&args_json)?;
            caller
                .data_mut()
                .host
                .write_resource(&namespace, &method, &args)
                .map_err(|e| host_err(e.to_string()))?;
            Ok(0)
        },
    )?;

    linker.func_wrap(
        "terrane",
        "resource_read",
        |mut caller: Caller<'_, WasmState>,
         ns_pair: i64,
         method_pair: i64,
         args_pair: i64,
         out_pair: i64|
         -> AnyResult<i32> {
            let namespace = read_string_pair(&mut caller, ns_pair)?;
            ensure_resource_allowed(&caller, &namespace)?;
            let method = read_string_pair(&mut caller, method_pair)?;
            let args_json = read_string_pair(&mut caller, args_pair)?;
            let (out_ptr, out_cap) = unpack_host_ptr_len(out_pair)?;
            let args: Vec<String> = serde_json::from_str(&args_json)?;
            let value = caller
                .data_mut()
                .host
                .read_resource(&namespace, &method, &args)
                .map_err(|e| host_err(e.to_string()))?;
            let json = read_value_json(value)?;
            let bytes = json.as_bytes();
            let cap = out_cap;
            if bytes.len() > cap {
                return Ok(-(checked_host_i32(bytes.len(), "resource_read output length")?));
            }
            write_bytes(&mut caller, out_ptr as i32, bytes)?;
            Ok(checked_host_i32(
                bytes.len(),
                "resource_read output length",
            )?)
        },
    )?;
    Ok(())
}

fn memory(caller: &mut Caller<'_, WasmState>) -> AnyResult<Memory> {
    caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .ok_or_else(|| host_err("wasm module must export memory"))
}

fn read_string(caller: &mut Caller<'_, WasmState>, ptr: i32, len: i32) -> AnyResult<String> {
    let len = checked_host_usize(len, "string length")?;
    let ptr = checked_host_usize(ptr, "string pointer")?;
    let memory = memory(caller)?;
    let mut bytes = vec![0u8; len];
    memory
        .read(&mut *caller, ptr, &mut bytes)
        .map_err(|_| host_err("guest memory read out of bounds"))?;
    String::from_utf8(bytes).map_err(|e| host_err(format!("guest string is not UTF-8: {e}")))
}

fn read_string_pair(caller: &mut Caller<'_, WasmState>, pair: i64) -> AnyResult<String> {
    let (ptr, len) = unpack_host_ptr_len(pair)?;
    read_string(caller, ptr as i32, len as i32)
}

fn write_bytes(caller: &mut Caller<'_, WasmState>, ptr: i32, bytes: &[u8]) -> AnyResult<()> {
    let ptr = checked_host_usize(ptr, "output pointer")?;
    let memory = memory(caller)?;
    memory
        .write(&mut *caller, ptr, bytes)
        .map_err(|_| host_err("guest memory write out of bounds"))
}

fn read_value_json(value: ReadValue) -> AnyResult<String> {
    Ok(match value {
        ReadValue::OptString(value) => serde_json::to_string(&value)?,
        ReadValue::StringMap(value) => serde_json::to_string(&value)?,
        ReadValue::StringList(value) => serde_json::to_string(&value)?,
    })
}

fn ensure_resource_allowed(caller: &Caller<'_, WasmState>, namespace: &str) -> AnyResult<()> {
    if caller
        .data()
        .resources
        .iter()
        .any(|resource| resource == namespace)
    {
        Ok(())
    } else {
        Err(host_err(format!(
            "resource namespace {namespace:?} is not declared by this wasm app"
        )))
    }
}

fn unpack_ptr_len(packed: i64) -> Result<(usize, usize)> {
    let packed = packed as u64;
    let ptr = (packed >> 32) as u32;
    let len = (packed & 0xffff_ffff) as u32;
    Ok((ptr as usize, len as usize))
}

fn unpack_host_ptr_len(packed: i64) -> AnyResult<(usize, usize)> {
    let packed = packed as u64;
    let ptr = (packed >> 32) as u32;
    let len = (packed & 0xffff_ffff) as u32;
    Ok((ptr as usize, len as usize))
}

fn checked_i32(value: usize, label: &str) -> Result<i32> {
    i32::try_from(value).map_err(|_| Error::Runtime(format!("{label} does not fit in i32")))
}

fn checked_usize(value: i32, label: &str) -> Result<usize> {
    if value < 0 {
        return Err(Error::Runtime(format!("{label} must not be negative")));
    }
    Ok(value as usize)
}

fn checked_host_i32(value: usize, label: &str) -> AnyResult<i32> {
    i32::try_from(value).map_err(|_| host_err(format!("{label} does not fit in i32")))
}

fn checked_host_usize(value: i32, label: &str) -> AnyResult<usize> {
    if value < 0 {
        return Err(host_err(format!("{label} must not be negative")));
    }
    Ok(value as usize)
}

fn non_empty_or(value: String, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

fn wasm_fuel() -> u64 {
    std::env::var("TERRANE_WASM_FUEL")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|fuel| *fuel > 0)
        .unwrap_or(10_000_000)
}

fn wasm_err(e: impl std::fmt::Display) -> Error {
    Error::Runtime(e.to_string())
}

fn host_err(message: impl Into<String>) -> wasmtime::Error {
    wasmtime::Error::msg(message.into())
}
