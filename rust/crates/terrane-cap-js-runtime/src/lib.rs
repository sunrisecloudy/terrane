//! The `js-runtime` capability — running a bundled JS backend in QuickJS.
//!
//! The engine stays deterministic by letting this capability execute JS exactly
//! once and by recording only the ordinary resource-write events produced during
//! the run. Replay folds those recorded events and never re-runs JavaScript.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use nanoserde::DeJson;
use rquickjs::function::Rest;
use rquickjs::{
    CatchResultExt, CaughtError, Context, Ctx, Function, IntoJs, Object, Runtime, Value,
};
use terrane_cap_interface::{
    arg, ensure_app_exists, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error,
    EventRecord, ReadValue, ResourceMethod, Result, RuntimeCtx, RuntimeHostHandle, RuntimeOutput,
    RuntimeRequest, StateStore,
};

/// Hand a resource read's result back to JS: a string|null, or an object.
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

pub struct JsRuntimeCapability;

impl Capability for JsRuntimeCapability {
    fn namespace(&self) -> &'static str {
        "js-runtime"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec {
                name: "js-runtime.run",
            }],
            events: Vec::new(),
            queries: Vec::new(),
            resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "js-runtime.run" => {
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
        let output = run_js_bundle(
            &request.app,
            &request.input,
            &JsRuntimeBundle {
                source: bundle.source,
                name: if bundle.name.is_empty() {
                    ctx.app_name
                } else {
                    bundle.name
                },
                resources: bundle.resources,
            },
            ctx.host,
        )?;
        Ok(RuntimeOutput { output })
    }
}

/// A memory-backed JS backend bundle: source, display name, and granted resource
/// namespaces. Preview and tests use this without disk I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsRuntimeBundle {
    pub source: String,
    pub name: String,
    pub resources: Vec<String>,
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

/// Default wall-clock budget for a single backend run; override with
/// `TERRANE_BACKEND_BUDGET_MS`. Bounds runaway scripts (e.g. `while(true){}`)
/// that neither the stack nor the memory limit would catch.
const DEFAULT_BACKEND_BUDGET_MS: u64 = 5000;

fn backend_budget() -> std::time::Duration {
    let ms = std::env::var("TERRANE_BACKEND_BUDGET_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_BACKEND_BUDGET_MS);
    std::time::Duration::from_millis(ms)
}

/// Build a QuickJS context, install the sandboxed app-scoped resources the
/// bundle *declared* (only those — undeclared capabilities are simply absent
/// from `ctx.resource`), eval the backend script, synthesize `handle` from an
/// `actions` table if the backend declared one, then call `handle(input)` and
/// return its String. `app_id`/`app_name` (from the manifest) feed an
/// `actions`-style backend's self-description. A wall-clock interrupt handler
/// bounds CPU so a runaway loop can't wedge the host.
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
        let resource = Object::new(ctx.clone()).map_err(js_err)?;

        // Install ctx.resource.<ns> for each granted namespace, built from each
        // capability's declared resource_api() — the SAME declaration the docs are
        // generated from, so the bridge and the reference cannot drift. Only
        // namespaces the manifest lists are installed (capability sandbox).
        let surface: Vec<(String, Vec<ResourceMethod>)> = resources
            .iter()
            .filter_map(|ns| {
                let api = host.resource_methods(ns).ok()?;
                (!api.is_empty()).then(|| (ns.clone(), api))
            })
            .collect();
        for (ns, methods) in surface {
            let obj = Object::new(ctx.clone()).map_err(js_err)?;
            for method in methods {
                let call = format!("{ns}.{}", method.name());
                let params = method.params();
                match method {
                    // A non-string arg is NOT coerced (that would change app-visible
                    // semantics); it captures a typed, attributable error so the run
                    // aborts naming the offending call and parameter.
                    ResourceMethod::Write { name, .. } => {
                        let method_name = name;
                        let namespace = ns.clone();
                        let host = host.clone();
                        let first_error = first_error.clone();
                        let f =
                            Function::new(ctx.clone(), move |args: Rest<Value>| match string_args(
                                &call, params, &args.0,
                            ) {
                                Ok(strs) => {
                                    if let Err(e) =
                                        host.write_resource(&namespace, method_name, &strs)
                                    {
                                        capture(&first_error, e);
                                    }
                                }
                                Err(e) => capture(&first_error, e),
                            })
                            .map_err(js_err)?;
                        obj.set(name, f).map_err(js_err)?;
                    }
                    ResourceMethod::Read { name, .. } => {
                        let method_name = name;
                        let namespace = ns.clone();
                        let host = host.clone();
                        let first_error = first_error.clone();
                        let f =
                            Function::new(ctx.clone(), move |args: Rest<Value>| -> JsReadValue {
                                match string_args(&call, params, &args.0) {
                                    Ok(strs) => {
                                        match host.read_resource(&namespace, method_name, &strs) {
                                            Ok(value) => JsReadValue(value),
                                            Err(e) => {
                                                capture(&first_error, e);
                                                JsReadValue(ReadValue::OptString(None))
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        capture(&first_error, e);
                                        JsReadValue(ReadValue::OptString(None))
                                    }
                                }
                            })
                            .map_err(js_err)?;
                        obj.set(name, f).map_err(js_err)?;
                    }
                }
            }
            resource.set(ns.as_str(), obj).map_err(js_err)?;
        }

        let ctx_obj = Object::new(ctx.clone()).map_err(js_err)?;
        ctx_obj.set("resource", resource).map_err(js_err)?;
        ctx.globals().set("ctx", ctx_obj).map_err(js_err)?;
        // The app's id/name (from the manifest), so an `actions`-style backend can
        // self-describe without repeating them.
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
            .map_err(js_err)?;

        // Eval the backend as a script (defines globals; reads `ctx`).
        ctx.eval::<(), _>(backend_src.as_bytes())
            .catch(&ctx)
            .map_err(caught_to_err)?;
        // Synthesize `handle` from an `actions` table if the backend didn't define
        // its own (a no-op when it did).
        ctx.eval::<(), _>(APP_RUNTIME.as_bytes())
            .catch(&ctx)
            .map_err(caught_to_err)?;

        // Call global handle(input); it must return a string.
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

/// The app-framework prelude, eval'd right after the backend. If the backend
/// declared an `actions` table instead of its own `handle`, it synthesizes
/// `handle` from it: verb dispatch, the `__actions__` self-description (merging
/// the app id/name from the manifest), per-action `usage()`, and the unknown-verb
/// help — all derived from the one table, so they can't drift. A backend that
/// defines its own `handle` is left untouched (full control).
const APP_RUNTIME: &str = r#"
(function () {
  if (typeof handle === 'function') return;
  if (typeof actions !== 'object' || actions === null) return;
  var ID = (typeof __terrane_app_id === 'string') ? __terrane_app_id : '';
  var NAME = (typeof __terrane_app_name === 'string') ? __terrane_app_name : '';
  var DESC = (typeof description === 'string') ? description : '';
  function usageFor(verb) {
    var a = actions[verb];
    var slots = (a && a.args ? a.args : []).map(function (x) {
      return x.required ? '<' + x.name + '>' : '[' + x.name + ']';
    });
    return 'usage: ' + verb + (slots.length ? ' ' + slots.join(' ') : '');
  }
  function runnerFor(a) {
    if (typeof a === 'function') return a;
    if (a && typeof a.run === 'function') return function (args, usage) { return a.run(args, usage); };
    return null;
  }
  function describe() {
    var list = Object.keys(actions).map(function (verb) {
      var a = actions[verb];
      return { verb: verb, summary: a.summary || '', args: a.args || [], returns: a.returns || '' };
    });
    return JSON.stringify({ app: ID, title: NAME, description: DESC, actions: list });
  }
  globalThis.handle = function (input) {
    var argv = input || [];
    var verb = argv[0] || '';
    if (verb === '__actions__') return describe();
    var a = actions[verb];
    var run = runnerFor(a);
    if (!run) {
      return 'unknown verb: ' + verb + ' (try ' + Object.keys(actions).join(' | ') + ')';
    }
    return run(argv.slice(1), function () { return usageFor(verb); });
  };
})();
"#;

/// A loaded app bundle: the backend JS source, the resource namespaces it is
/// allowed to reach (its declared sandbox surface), and its display name.
/// Load the bundle for an app whose `source` is either the bundle directory
/// (containing manifest.json + the backend file) or a direct `.js` path. A
/// directory's resources come from `manifest.resources` (absent → none, least
/// privilege); a direct `.js` has no manifest, so it gets the dev default of all
/// currently-known resources.
fn load_bundle(source: &str) -> Result<JsRuntimeBundle> {
    let path = Path::new(source);
    if path.is_dir() {
        let manifest = read_manifest(path)?;
        if !manifest.runtime.is_empty() && manifest.runtime != "js" {
            return Err(Error::Runtime(format!(
                "manifest runtime {:?} is not js",
                manifest.runtime
            )));
        }
        let js_path = path.join(&manifest.backend);
        let source = std::fs::read_to_string(&js_path)
            .map_err(|e| Error::Runtime(format!("read backend {}: {e}", js_path.display())))?;
        Ok(JsRuntimeBundle {
            source,
            name: manifest.name,
            resources: manifest.resources,
        })
    } else {
        let source = std::fs::read_to_string(path)
            .map_err(|e| Error::Runtime(format!("read backend {}: {e}", path.display())))?;
        Ok(JsRuntimeBundle {
            source,
            name: String::new(),
            resources: vec!["kv".to_string()],
        })
    }
}

/// The fields of `manifest.json` terrane reads. Public so the CLI (`app install`)
/// and the edge hosts can read a bundle's catalog metadata (id/name/ui) without
/// re-implementing the parse.
///
/// Parsed with nanoserde's JSON path (`DeJson`) — a zero-dependency, serde-free
/// reader, so terrane-core stays serde-free and borsh remains the sole event-log
/// format. Unknown keys are ignored (forward-compatible). `resources` defaults to
/// empty when absent: least privilege. Manifests are small, trusted, local files
/// read once at the edge (off the replay path), so allocating owned strings here
/// costs nothing.
#[derive(Debug, Clone, DeJson)]
pub struct BundleManifest {
    /// Stable app id (matches the catalog entry). Empty if the manifest omits it.
    #[nserde(default)]
    pub id: String,
    /// Display name.
    #[nserde(default)]
    pub name: String,
    /// The backend JS file, e.g. `"main.js"`.
    pub backend: String,
    /// Runtime engine. Empty means JS for source-only developer use.
    #[nserde(default)]
    pub runtime: String,
    /// The UI entry file (e.g. `"index.html"`); empty for CLI-only apps.
    #[nserde(default)]
    pub ui: String,
    /// Resource namespaces the backend may reach (least privilege; empty default).
    #[nserde(default)]
    pub resources: Vec<String>,
}

/// Read and parse `<bundle_dir>/manifest.json`.
pub fn read_manifest(bundle_dir: &Path) -> Result<BundleManifest> {
    let text = std::fs::read_to_string(bundle_dir.join("manifest.json"))
        .map_err(|e| Error::Runtime(format!("read manifest.json: {e}")))?;
    BundleManifest::deserialize_json(&text)
        .map_err(|e| Error::Runtime(format!("manifest.json: {e}")))
}

/// Map an rquickjs error into our typed Runtime error.
fn js_err(e: rquickjs::Error) -> Error {
    Error::Runtime(e.to_string())
}

fn capture(slot: &Rc<RefCell<Option<Error>>>, e: Error) {
    let mut slot = slot.borrow_mut();
    if slot.is_none() {
        *slot = Some(e);
    }
}

/// Strictly read a JS string argument — no coercion. Returns the owned string, or
/// the argument's JS type name (so the caller can report which kv call and which
/// parameter got the wrong type).
fn js_string_arg(v: &Value) -> std::result::Result<String, &'static str> {
    match v.as_string().and_then(|s| s.to_string().ok()) {
        Some(s) => Ok(s),
        None => Err(v.type_name()),
    }
}

/// Convert each JS argument to a string with NO coercion, attributing a
/// non-string to its resource call and parameter name (so the run aborts with a
/// clear "kv.set: expected string key, got int").
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

/// Fold a caught JS exception/value into our typed Runtime error (owned String,
/// so nothing `'js` escapes the `with` closure).
fn caught_to_err(e: CaughtError<'_>) -> Error {
    Error::Runtime(e.to_string())
}
