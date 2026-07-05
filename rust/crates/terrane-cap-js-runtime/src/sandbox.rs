use std::cell::RefCell;
use std::rc::Rc;

use rquickjs::function::Rest;
use rquickjs::{
    CatchResultExt, CaughtError, Context, Ctx, Function, IntoJs, Object, Runtime, Value,
};
use terrane_cap_interface::{Error, ReadValue, ResourceMethod, Result, RuntimeHostHandle};

use crate::bundle::JsRuntimeBundle;

/// The app-framework prelude, eval'd right after the backend. If the backend
/// declared an `actions` table instead of its own `handle`, it synthesizes
/// `handle` from it.
const APP_RUNTIME: &str = include_str!("runtime/app_runtime.js");

/// Default wall-clock budget for a single backend run; override with
/// `TERRANE_BACKEND_BUDGET_MS`.
const DEFAULT_BACKEND_BUDGET_MS: u64 = 5000;

/// Hand a resource read's result back to JS: a string|null, object, or list.
struct JsReadValue(ReadValue);

impl<'js> IntoJs<'js> for JsReadValue {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<Value<'js>> {
        match self.0 {
            ReadValue::OptString(opt) => opt.into_js(ctx),
            ReadValue::StringMap(map) => map.into_js(ctx),
            ReadValue::StringList(list) => list.into_js(ctx),
        }
    }
}

#[derive(Clone)]
struct InstallResourceCtx {
    host: RuntimeHostHandle,
    first_error: Rc<RefCell<Option<Error>>>,
}

pub fn run_js_bundle(
    app: &str,
    input: &[String],
    bundle: &JsRuntimeBundle,
    host: RuntimeHostHandle,
) -> Result<String> {
    let first_error = Rc::new(RefCell::new(None));
    let output = execute_js(
        &bundle.source,
        &bundle.resources,
        input,
        host.clone(),
        first_error.clone(),
        app,
        &bundle.name,
    );

    if let Some(e) = first_error.borrow_mut().take() {
        let _ = host.app_log(
            "error",
            &e.to_string(),
            &run_data(input),
            terrane_cap_telemetry::SOURCE_FIRST_ERROR,
            "",
            true,
        );
        return Err(e);
    }
    output
}

fn backend_budget() -> std::time::Duration {
    let ms = std::env::var("TERRANE_BACKEND_BUDGET_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_BACKEND_BUDGET_MS);
    std::time::Duration::from_millis(ms)
}

/// Build a QuickJS context, install declared resources, eval the backend script,
/// synthesize `handle` from an `actions` table if needed, then call
/// `handle(input)` and return its string result.
fn execute_js(
    backend_src: &str,
    resources: &[String],
    input: &[String],
    host: RuntimeHostHandle,
    first_error: Rc<RefCell<Option<Error>>>,
    app_id: &str,
    app_name: &str,
) -> Result<String> {
    let rt = Runtime::new().map_err(js_err)?;
    rt.set_max_stack_size(512 * 1024);
    rt.set_memory_limit(64 * 1024 * 1024);
    let deadline = std::time::Instant::now() + backend_budget();
    rt.set_interrupt_handler(Some(Box::new(move || {
        std::time::Instant::now() >= deadline
    })));
    let ctx = Context::full(&rt).map_err(js_err)?;

    ctx.with(|ctx| -> Result<String> {
        install_resources(&ctx, resources, host.clone(), first_error.clone())?;
        install_app_globals(&ctx, app_id, app_name)?;

        ctx.eval::<(), _>(backend_src.as_bytes())
            .catch(&ctx)
            .map_err(|e| caught_to_err(&host, input, e))?;
        ctx.eval::<(), _>(APP_RUNTIME.as_bytes())
            .catch(&ctx)
            .map_err(|e| caught_to_err(&host, input, e))?;

        let handle: Function = ctx.globals().get("handle").map_err(|_| {
            Error::Runtime(
                "backend defines neither a `handle` function nor an `actions` table".into(),
            )
        })?;
        let result: Value = handle
            .call((input,))
            .catch(&ctx)
            .map_err(|e| caught_to_err(&host, input, e))?;
        result
            .as_string()
            .and_then(|s| s.to_string().ok())
            .ok_or_else(|| {
                Error::Runtime(format!(
                    "handle() must return a string, got {}",
                    result.type_name()
                ))
            })
    })
}

/// Compile `source` as a plain-script function body without executing it.
/// Returns `Some(message)` on a syntax error, `None` when it parses — or when
/// the checker itself cannot run (absence of proof is not failure). Used by
/// validation layers to catch truncated/broken generated JS before install.
pub fn js_script_syntax_error(source: &str) -> Option<String> {
    let rt = Runtime::new().ok()?;
    let ctx = Context::full(&rt).ok()?;
    ctx.with(|ctx| {
        ctx.globals().set("__terrane_syntax_src", source).ok()?;
        let compiled: rquickjs::Result<Value> =
            ctx.eval("new Function(globalThis.__terrane_syntax_src)");
        match compiled.catch(&ctx) {
            Ok(_) => None,
            Err(caught) => Some(match caught {
                CaughtError::Exception(e) => {
                    e.message().unwrap_or_else(|| "syntax error".to_string())
                }
                other => format!("{other}"),
            }),
        }
    })
}

fn install_resources(
    ctx: &Ctx<'_>,
    resources: &[String],
    host: RuntimeHostHandle,
    first_error: Rc<RefCell<Option<Error>>>,
) -> Result<()> {
    let resource = Object::new(ctx.clone()).map_err(js_err)?;
    let install_ctx = InstallResourceCtx { host, first_error };
    let surface: Vec<(String, Vec<ResourceMethod>)> = resources
        .iter()
        .filter_map(|ns| {
            let api = install_ctx.host.resource_methods(ns).ok()?;
            (!api.is_empty()).then(|| (ns.clone(), api))
        })
        .collect();

    for (ns, methods) in surface {
        let obj = Object::new(ctx.clone()).map_err(js_err)?;
        for method in methods {
            let call = format!("{ns}.{}", method.name());
            let params = method.params();
            match method {
                ResourceMethod::Write { name, .. } => {
                    install_write(ctx, &obj, &ns, name, &call, params, install_ctx.clone())?;
                }
                ResourceMethod::Read { name, .. } => {
                    install_read(ctx, &obj, &ns, name, &call, params, install_ctx.clone())?;
                }
                ResourceMethod::Call { name, .. } => {
                    install_call(ctx, &obj, &ns, name, &call, params, install_ctx.clone())?;
                }
            }
        }
        resource.set(ns.as_str(), obj).map_err(js_err)?;
    }

    let ctx_obj = Object::new(ctx.clone()).map_err(js_err)?;
    ctx_obj.set("resource", resource).map_err(js_err)?;
    ctx.globals().set("ctx", ctx_obj).map_err(js_err)?;
    install_console(ctx, install_ctx.host)
}

fn install_console(ctx: &Ctx<'_>, host: RuntimeHostHandle) -> Result<()> {
    let console = Object::new(ctx.clone()).map_err(js_err)?;
    for (name, level) in [
        ("debug", "debug"),
        ("log", "info"),
        ("info", "info"),
        ("warn", "warn"),
        ("error", "error"),
    ] {
        let level = level.to_string();
        let host = host.clone();
        let f = Function::new(ctx.clone(), move |args: Rest<Value>| {
            let msg = console_message(&args.0);
            let record_error = level == "error";
            let _ = host.app_log(
                &level,
                &msg,
                "{}",
                terrane_cap_telemetry::SOURCE_EXPLICIT,
                "",
                record_error,
            );
        })
        .map_err(js_err)?;
        console.set(name, f).map_err(js_err)?;
    }
    ctx.globals().set("console", console).map_err(js_err)
}

fn install_write<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    namespace: &str,
    method_name: &'static str,
    call: &str,
    params: &'static [&'static str],
    install_ctx: InstallResourceCtx,
) -> Result<()> {
    let namespace = namespace.to_string();
    let call = call.to_string();
    let f = Function::new(ctx.clone(), move |args: Rest<Value>| {
        match string_args(&call, params, &args.0) {
            Ok(strs) => {
                if let Err(e) = install_ctx
                    .host
                    .write_resource(&namespace, method_name, &strs)
                {
                    capture(&install_ctx.first_error, e);
                }
            }
            Err(e) => capture(&install_ctx.first_error, e),
        }
    })
    .map_err(js_err)?;
    obj.set(method_name, f).map_err(js_err)
}

/// An effectful call: records events like a write and returns a value like a
/// read (e.g. `ctx.resource["local-model"].ask(prompt)`).
fn install_call<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    namespace: &str,
    method_name: &'static str,
    call: &str,
    params: &'static [&'static str],
    install_ctx: InstallResourceCtx,
) -> Result<()> {
    let namespace = namespace.to_string();
    let call = call.to_string();
    let f = Function::new(ctx.clone(), move |args: Rest<Value>| -> JsReadValue {
        match string_args(&call, params, &args.0) {
            Ok(strs) => match install_ctx
                .host
                .call_resource(&namespace, method_name, &strs)
            {
                Ok(value) => JsReadValue(value),
                Err(e) => {
                    capture(&install_ctx.first_error, e);
                    JsReadValue(ReadValue::OptString(None))
                }
            },
            Err(e) => {
                capture(&install_ctx.first_error, e);
                JsReadValue(ReadValue::OptString(None))
            }
        }
    })
    .map_err(js_err)?;
    obj.set(method_name, f).map_err(js_err)
}

fn install_read<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    namespace: &str,
    method_name: &'static str,
    call: &str,
    params: &'static [&'static str],
    install_ctx: InstallResourceCtx,
) -> Result<()> {
    let namespace = namespace.to_string();
    let call = call.to_string();
    let f = Function::new(ctx.clone(), move |args: Rest<Value>| -> JsReadValue {
        match string_args(&call, params, &args.0) {
            Ok(strs) => match install_ctx
                .host
                .read_resource(&namespace, method_name, &strs)
            {
                Ok(value) => JsReadValue(value),
                Err(e) => {
                    capture(&install_ctx.first_error, e);
                    JsReadValue(ReadValue::OptString(None))
                }
            },
            Err(e) => {
                capture(&install_ctx.first_error, e);
                JsReadValue(ReadValue::OptString(None))
            }
        }
    })
    .map_err(js_err)?;
    obj.set(method_name, f).map_err(js_err)
}

fn install_app_globals(ctx: &Ctx<'_>, app_id: &str, app_name: &str) -> Result<()> {
    ctx.globals()
        .set("__terrane_app_id", app_id)
        .map_err(js_err)?;
    ctx.globals()
        .set("__terrane_app_name", app_name)
        .map_err(js_err)?;
    ctx.globals()
        .set("eval", Value::new_undefined(ctx.clone()))
        .map_err(js_err)?;
    ctx.globals()
        .set("Function", Value::new_undefined(ctx.clone()))
        .map_err(js_err)
}

fn js_err(e: rquickjs::Error) -> Error {
    Error::Runtime(e.to_string())
}

fn capture(slot: &Rc<RefCell<Option<Error>>>, e: Error) {
    let mut slot = slot.borrow_mut();
    if slot.is_none() {
        *slot = Some(e);
    }
}

/// Strictly read a JS string argument — no coercion.
fn js_string_arg(v: &Value) -> std::result::Result<String, &'static str> {
    match v.as_string().and_then(|s| s.to_string().ok()) {
        Some(s) => Ok(s),
        None => Err(v.type_name()),
    }
}

/// Convert each JS argument to a string with no coercion, except payload-shaped
/// params where object/array literals are encoded as JSON for resource APIs.
fn string_args(call: &str, params: &[&str], vals: &[Value]) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(vals.len());
    for (i, v) in vals.iter().enumerate() {
        let param = params.get(i).copied().unwrap_or("arg");
        match js_resource_arg(v, param) {
            Ok(s) => out.push(s),
            Err(got) => {
                return Err(Error::InvalidInput(format!(
                    "{call}: expected string {param}, got {got}"
                )));
            }
        }
    }
    Ok(out)
}

fn js_resource_arg(v: &Value, param: &str) -> std::result::Result<String, &'static str> {
    if let Ok(s) = js_string_arg(v) {
        return Ok(s);
    }
    if matches!(param, "payload" | "payloadJson" | "dataJson")
        && (v.is_object() || v.is_array() || v.is_bool() || v.is_number() || v.is_null())
    {
        let ctx = v.ctx();
        let json: Object = ctx.globals().get("JSON").map_err(|_| v.type_name())?;
        let stringify: Function = json.get("stringify").map_err(|_| v.type_name())?;
        let encoded: Value = stringify.call((v.clone(),)).map_err(|_| v.type_name())?;
        return js_string_arg(&encoded);
    }
    Err(v.type_name())
}

/// Fold a caught JS exception/value into our typed Runtime error.
fn caught_to_err(host: &RuntimeHostHandle, input: &[String], e: CaughtError<'_>) -> Error {
    let message = e.to_string();
    let source = if message.to_ascii_lowercase().contains("interrupted") {
        terrane_cap_telemetry::SOURCE_TIMEOUT
    } else {
        terrane_cap_telemetry::SOURCE_EXCEPTION
    };
    let _ = host.app_log("error", &message, &run_data(input), source, &message, true);
    Error::Runtime(message)
}

fn console_message(vals: &[Value]) -> String {
    vals.iter()
        .map(js_log_arg)
        .collect::<Vec<_>>()
        .join(" ")
}

fn js_log_arg(v: &Value) -> String {
    if let Ok(s) = js_string_arg(v) {
        return s;
    }
    if v.is_object() || v.is_array() || v.is_bool() || v.is_number() || v.is_null() {
        let ctx = v.ctx();
        let encoded = ctx
            .globals()
            .get::<_, Object>("JSON")
            .and_then(|json| json.get::<_, Function>("stringify"))
            .and_then(|stringify| stringify.call::<_, Value>((v.clone(),)));
        if let Ok(encoded) = encoded {
            if let Ok(s) = js_string_arg(&encoded) {
                return s;
            }
        }
    }
    format!("[{}]", v.type_name())
}

fn run_data(input: &[String]) -> String {
    let args = input
        .iter()
        .map(|v| format!("\"{}\"", json_escape(v)))
        .collect::<Vec<_>>()
        .join(",");
    let verb = input.first().map(|v| v.as_str()).unwrap_or("");
    format!("{{\"verb\":\"{}\",\"input\":[{}]}}", json_escape(verb), args)
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
