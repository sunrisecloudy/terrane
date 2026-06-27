//! The `host` capability — running a bundled JS backend in QuickJS.
//!
//! `host.run` is special-cased in [`Core::dispatch`](crate::Core::dispatch): it
//! needs `&mut self` to re-dispatch the backend's `kv.*` writes, which a pure
//! `decide` (`&State`) cannot do. The [`HostCapability`] below exists for the
//! registry / namespace contract and to reject any *direct* `host.*` command. It
//! owns no State slice and folds nothing — a run's only records are ordinary
//! `kv.*` events, so Option-A replay rebuilds state without ever re-running JS.
//!
//! ## How a run works (the load-bearing detail)
//!
//! The backend calls `ctx.resource.kv.{set,rm,get,all}` synchronously. Writes
//! must be visible to later reads *within the run*, and each must become a real
//! `kv.*` record in the global log. rquickjs host closures must be `'static`, so
//! we cannot hand them `&mut Core`. Instead, one run owns a [`RunAccum`] — a
//! working copy of State + a fresh registry — behind `Rc<RefCell<_>>`, captured
//! by `Fn` closures. Writes go through the *same* `kv` capability `decide`/fold
//! as a CLI `kv set`, applied to the working State (so reads see them) and
//! stashed in `recorded`. After JS returns, [`Core::run_backend`] commits the
//! collected records once, through the normal path. JS runs exactly here, never
//! on replay.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use nanoserde::DeJson;
use rquickjs::function::Rest;
use rquickjs::{
    CatchResultExt, CaughtError, Context, Ctx, Function, IntoJs, Object, Runtime, Value,
};
use terrane_domain::{Error, EventRecord, Result};

use super::{Capability, ReadValue, ResourceMethod};
use crate::{apply, default_registry, namespace_of, Decision, Registry, State};

/// Hand a resource read's result back to JS: a string|null, or an object.
impl<'js> IntoJs<'js> for ReadValue {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<Value<'js>> {
        match self {
            ReadValue::OptString(opt) => opt.into_js(ctx),
            ReadValue::StringMap(map) => map.into_js(ctx),
            ReadValue::StringList(list) => list.into_js(ctx),
        }
    }
}

pub struct HostCapability;

impl Capability for HostCapability {
    fn namespace(&self) -> &'static str {
        "host"
    }

    fn decide(&self, _state: &State, name: &str, _args: &[String]) -> Result<Decision> {
        // host.run is executed by Core::dispatch directly and never reaches here.
        Err(Error::InvalidInput(format!(
            "{name}: host commands are executed by the core, not decided"
        )))
    }

    fn fold(&self, _state: &mut State, _record: &EventRecord) -> Result<()> {
        Ok(())
    }
}

/// The outcome of a backend run: the resource-write records it produced and the
/// string it returned. Callers choose whether to commit the records to the real
/// log or fold them into a private in-memory [`State`](crate::State).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub records: Vec<EventRecord>,
    pub output: String,
}

/// A memory-backed backend bundle: the JS source, display name, and declared
/// resource namespaces. This is the preview/test twin of an on-disk bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryBackendBundle {
    pub source: String,
    pub name: String,
    pub resources: Vec<String>,
}

/// Run an app's JS backend once. `source` is the app's bundle dir (or a direct
/// `.js` path); `input` is the verb argument array passed to `handle`. The
/// returned records are NOT yet committed — the caller commits them through
/// [`Core::commit`](crate::Core) so they land in the global log.
pub(crate) fn run(
    app: &str,
    input: &[String],
    source: &str,
    base_state: State,
) -> Result<RunResult> {
    let bundle = load_bundle(source)?;
    run_memory_backend(app, input, &bundle, base_state)
}

/// Run a memory-backed backend once. No disk reads are performed and no event
/// log is appended; the returned records remain the caller's responsibility.
pub fn run_memory_backend(
    app: &str,
    input: &[String],
    bundle: &MemoryBackendBundle,
    base_state: State,
) -> Result<RunResult> {
    let cell = Rc::new(RefCell::new(RunAccum {
        app: app.to_string(),
        state: base_state,
        registry: default_registry(),
        recorded: Vec::new(),
        first_error: None,
    }));

    let output = execute_js(
        &bundle.source,
        &bundle.resources,
        input,
        cell.clone(),
        app,
        &bundle.name,
    );

    // Sole ownership again now that the Context/Runtime (and their closures) dropped.
    let accum = Rc::try_unwrap(cell)
        .map_err(|_| Error::Runtime("dangling JS handle after run".into()))?
        .into_inner();

    // A typed write error (e.g. KeyNotFound) wins over the JS result; nothing is
    // committed in that case (run-level all-or-nothing on error).
    if let Some(e) = accum.first_error {
        return Err(e);
    }
    let output = output?;

    Ok(RunResult {
        records: coalesce(accum.recorded),
        output,
    })
}

/// One run's owned, `'static` working surface, shared with the JS host closures
/// via `Rc<RefCell<_>>`.
struct RunAccum {
    app: String,
    state: State,
    registry: Registry,
    recorded: Vec<RecordedWrite>,
    first_error: Option<Error>,
}

/// A write stashed during a run, carrying just enough to coalesce redundant
/// same-key `kv.set`s before committing (see [`coalesce`]).
struct RecordedWrite {
    record: EventRecord,
    /// The app-scoped key this write targets, set only for `kv.*` writes (the
    /// only ones that participate in coalescing); `None` for anything else.
    coalesce_key: Option<String>,
    /// True only for a `kv.set` — the kind we drop when a later write supersedes
    /// it. A `kv.rm` (or any other record) is always kept.
    is_set: bool,
}

/// Coalesce redundant same-key `kv.set` records produced within a single run: a
/// `kv.set` is dropped when a *later* write targets the same key (a newer
/// `kv.set`, or a `kv.rm` that removes it). The last set of each key and every
/// `kv.rm` survive, in their original relative order — so the committed records
/// fold to the exact same State, just without the intra-run churn. Replay stays
/// identical because only the net effect is logged.
fn coalesce(writes: Vec<RecordedWrite>) -> Vec<EventRecord> {
    let mut keep = vec![true; writes.len()];
    for i in 0..writes.len() {
        if !writes[i].is_set {
            continue;
        }
        let Some(key) = writes[i].coalesce_key.as_deref() else {
            continue;
        };
        if writes[i + 1..]
            .iter()
            .any(|w| w.coalesce_key.as_deref() == Some(key))
        {
            keep[i] = false;
        }
    }
    writes
        .into_iter()
        .zip(keep)
        .filter_map(|(w, keep)| keep.then_some(w.record))
        .collect()
}

impl RunAccum {
    /// A resource WRITE: app-scope it (force arg0 = the running app), decide via
    /// the owning capability against the working State, fold so later reads see
    /// it, and stash the records. Errors are captured (first wins), not thrown.
    fn write(&mut self, name: &str, mut args: Vec<String>) {
        args.insert(0, self.app.clone());
        // The kv key (args[1] after app-scoping) drives coalescing; non-kv writes
        // never coalesce.
        let coalesce_key = name
            .starts_with("kv.")
            .then(|| args.get(1).cloned())
            .flatten();
        let is_set = name == "kv.set";
        let result = (|| -> Result<()> {
            let decision =
                self.registry
                    .get(namespace_of(name)?)?
                    .decide(&self.state, name, &args)?;
            let records = match decision {
                Decision::Commit(records) => records,
                Decision::Effect(_) => {
                    return Err(Error::Runtime(format!(
                        "{name}: effects not allowed in a backend"
                    )))
                }
            };
            for record in &records {
                apply(&self.registry, &mut self.state, record)?;
            }
            for record in records {
                self.recorded.push(RecordedWrite {
                    record,
                    coalesce_key: coalesce_key.clone(),
                    is_set,
                });
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.capture(e);
        }
    }

    fn capture(&mut self, e: Error) {
        if self.first_error.is_none() {
            self.first_error = Some(e);
        }
    }
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
    cell: Rc<RefCell<RunAccum>>,
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
        let surface: Vec<(&'static str, Vec<ResourceMethod>)> = {
            let accum = cell.borrow();
            resources
                .iter()
                .filter_map(|ns| {
                    let cap = accum.registry.get(ns).ok()?;
                    let api = cap.resource_api();
                    (!api.is_empty()).then(|| (cap.namespace(), api))
                })
                .collect()
        };
        for (ns, methods) in surface {
            let obj = Object::new(ctx.clone()).map_err(js_err)?;
            for method in methods {
                let call = format!("{ns}.{}", method.name());
                let params = method.params();
                let cell = cell.clone();
                match method {
                    // A non-string arg is NOT coerced (that would change app-visible
                    // semantics); it captures a typed, attributable error so the run
                    // aborts naming the offending call and parameter.
                    ResourceMethod::Write { name, .. } => {
                        let f =
                            Function::new(ctx.clone(), move |args: Rest<Value>| match string_args(
                                &call, params, &args.0,
                            ) {
                                Ok(strs) => cell.borrow_mut().write(&call, strs),
                                Err(e) => cell.borrow_mut().capture(e),
                            })
                            .map_err(js_err)?;
                        obj.set(name, f).map_err(js_err)?;
                    }
                    ResourceMethod::Read { name, read, .. } => {
                        let f = Function::new(ctx.clone(), move |args: Rest<Value>| -> ReadValue {
                            match string_args(&call, params, &args.0) {
                                Ok(strs) => {
                                    let accum = cell.borrow();
                                    read(&accum.state, &accum.app, &strs)
                                }
                                Err(e) => {
                                    cell.borrow_mut().capture(e);
                                    ReadValue::OptString(None)
                                }
                            }
                        })
                        .map_err(js_err)?;
                        obj.set(name, f).map_err(js_err)?;
                    }
                }
            }
            resource.set(ns, obj).map_err(js_err)?;
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
fn load_bundle(source: &str) -> Result<MemoryBackendBundle> {
    let path = Path::new(source);
    if path.is_dir() {
        let manifest = read_manifest(path)?;
        let js_path = path.join(&manifest.backend);
        let source = std::fs::read_to_string(&js_path)
            .map_err(|e| Error::Runtime(format!("read backend {}: {e}", js_path.display())))?;
        Ok(MemoryBackendBundle {
            source,
            name: manifest.name,
            resources: manifest.resources,
        })
    } else {
        let source = std::fs::read_to_string(path)
            .map_err(|e| Error::Runtime(format!("read backend {}: {e}", path.display())))?;
        Ok(MemoryBackendBundle {
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
