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
        host,
        first_error.clone(),
        app,
        &bundle.name,
    );

    if let Some(e) = first_error.borrow_mut().take() {
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
        install_resources(&ctx, resources, host, first_error.clone())?;
        install_app_globals(&ctx, app_id, app_name)?;

        ctx.eval::<(), _>(backend_src.as_bytes())
            .catch(&ctx)
            .map_err(caught_to_err)?;
        ctx.eval::<(), _>(APP_RUNTIME.as_bytes())
            .catch(&ctx)
            .map_err(caught_to_err)?;

        let handle: Function = ctx.globals().get("handle").map_err(|_| {
            Error::Runtime(
                "backend defines neither a `handle` function nor an `actions` table".into(),
            )
        })?;
        let result: Value = handle.call((input,)).catch(&ctx).map_err(caught_to_err)?;
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
            }
        }
        resource.set(ns.as_str(), obj).map_err(js_err)?;
    }

    let ctx_obj = Object::new(ctx.clone()).map_err(js_err)?;
    ctx_obj.set("resource", resource).map_err(js_err)?;
    ctx.globals().set("ctx", ctx_obj).map_err(js_err)
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

/// Convert each JS argument to a string with no coercion, attributing a
/// non-string to its resource call and parameter name.
fn string_args(call: &str, params: &[&str], vals: &[Value]) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(vals.len());
    for (i, v) in vals.iter().enumerate() {
        match js_string_arg(v) {
            Ok(s) => out.push(s),
            Err(got) => {
                let param = params.get(i).copied().unwrap_or("arg");
                return Err(Error::InvalidInput(format!(
                    "{call}: expected string {param}, got {got}"
                )));
            }
        }
    }
    Ok(out)
}

/// Fold a caught JS exception/value into our typed Runtime error.
fn caught_to_err(e: CaughtError<'_>) -> Error {
    Error::Runtime(e.to_string())
}
