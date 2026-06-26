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
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;

use rquickjs::{CatchResultExt, CaughtError, Context, Function, Object, Runtime, Value};
use terrane_domain::{Error, EventRecord, Result};

use super::Capability;
use crate::{apply, default_registry, namespace_of, Decision, Registry, State};

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

/// The outcome of a backend run: the `kv.*` records it produced (to commit) and
/// the string it returned (for the host to print).
pub(crate) struct RunResult {
    pub(crate) records: Vec<EventRecord>,
    pub(crate) output: String,
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

    let cell = Rc::new(RefCell::new(RunAccum {
        app: app.to_string(),
        state: base_state,
        registry: default_registry(),
        recorded: Vec::new(),
        first_error: None,
    }));

    let output = execute_js(&bundle.source, &bundle.resources, input, cell.clone());

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
        records: accum.recorded,
        output,
    })
}

/// One run's owned, `'static` working surface, shared with the JS host closures
/// via `Rc<RefCell<_>>`.
struct RunAccum {
    app: String,
    state: State,
    registry: Registry,
    recorded: Vec<EventRecord>,
    first_error: Option<Error>,
}

impl RunAccum {
    /// A resource WRITE: app-scope it (force arg0 = the running app), decide via
    /// the owning capability against the working State, fold so later reads see
    /// it, and stash the records. Errors are captured (first wins), not thrown.
    fn write(&mut self, name: &str, mut args: Vec<String>) {
        args.insert(0, self.app.clone());
        let result = (|| -> Result<()> {
            let decision = self
                .registry
                .get(namespace_of(name)?)?
                .decide(&self.state, name, &args)?;
            let records = match decision {
                Decision::Commit(records) => records,
                Decision::Effect(_) => {
                    return Err(Error::Runtime(format!("{name}: effects not allowed in a backend")))
                }
            };
            for record in &records {
                apply(&self.registry, &mut self.state, record)?;
            }
            self.recorded.extend(records);
            Ok(())
        })();
        if let Err(e) = result {
            self.capture(e);
        }
    }

    fn kv_all(&self) -> BTreeMap<String, String> {
        self.state.kv.data.get(&self.app).cloned().unwrap_or_default()
    }

    fn kv_get(&self, key: &str) -> Option<String> {
        self.state
            .kv
            .data
            .get(&self.app)
            .and_then(|m| m.get(key).cloned())
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
/// from `ctx.resource`), eval the backend script (which defines globals `ctx`
/// and `handle`), then call `handle(input)` and return its String. A wall-clock
/// interrupt handler bounds CPU so a runaway loop can't wedge the host.
fn execute_js(
    backend_src: &str,
    resources: &[String],
    input: &[String],
    cell: Rc<RefCell<RunAccum>>,
) -> Result<String> {
    let rt = Runtime::new().map_err(js_err)?;
    rt.set_max_stack_size(512 * 1024);
    rt.set_memory_limit(64 * 1024 * 1024);
    let deadline = std::time::Instant::now() + backend_budget();
    rt.set_interrupt_handler(Some(Box::new(move || std::time::Instant::now() >= deadline)));
    let ctx = Context::full(&rt).map_err(js_err)?;

    ctx.with(|ctx| -> Result<String> {
        let resource = Object::new(ctx.clone()).map_err(js_err)?;

        // kv — installed only if the manifest declared it (capability sandbox).
        if resources.iter().any(|r| r == "kv") {
            let kv = Object::new(ctx.clone()).map_err(js_err)?;
            {
                let cell = cell.clone();
                let set = Function::new(ctx.clone(), move |key: String, value: String| {
                    cell.borrow_mut().write("kv.set", vec![key, value]);
                })
                .map_err(js_err)?;
                kv.set("set", set).map_err(js_err)?;
            }
            {
                let cell = cell.clone();
                let rm = Function::new(ctx.clone(), move |key: String| {
                    cell.borrow_mut().write("kv.rm", vec![key]);
                })
                .map_err(js_err)?;
                kv.set("rm", rm).map_err(js_err)?;
            }
            {
                let cell = cell.clone();
                let get = Function::new(ctx.clone(), move |key: String| -> Option<String> {
                    cell.borrow().kv_get(&key)
                })
                .map_err(js_err)?;
                kv.set("get", get).map_err(js_err)?;
            }
            {
                let cell = cell.clone();
                let all = Function::new(ctx.clone(), move || -> BTreeMap<String, String> {
                    cell.borrow().kv_all()
                })
                .map_err(js_err)?;
                kv.set("all", all).map_err(js_err)?;
            }
            resource.set("kv", kv).map_err(js_err)?;
        }

        let ctx_obj = Object::new(ctx.clone()).map_err(js_err)?;
        ctx_obj.set("resource", resource).map_err(js_err)?;
        ctx.globals().set("ctx", ctx_obj).map_err(js_err)?;

        // Eval the backend as a script (defines globals; reads `ctx`).
        ctx.eval::<(), _>(backend_src.as_bytes())
            .catch(&ctx)
            .map_err(caught_to_err)?;

        // Call global handle(input); it must return a string.
        let handle: Function = ctx
            .globals()
            .get("handle")
            .map_err(|_| Error::Runtime("backend defines no callable `handle`".into()))?;
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

/// A loaded app bundle: the backend JS source + the resource namespaces it is
/// allowed to reach (its declared sandbox surface).
struct Bundle {
    source: String,
    resources: Vec<String>,
}

/// Load the bundle for an app whose `source` is either the bundle directory
/// (containing manifest.json + the backend file) or a direct `.js` path. A
/// directory's resources come from `manifest.resources` (absent → none, least
/// privilege); a direct `.js` has no manifest, so it gets the dev default of all
/// currently-known resources.
fn load_bundle(source: &str) -> Result<Bundle> {
    let path = Path::new(source);
    if path.is_dir() {
        let manifest = std::fs::read_to_string(path.join("manifest.json"))
            .map_err(|e| Error::Runtime(format!("read manifest.json: {e}")))?;
        let backend = manifest_backend(&manifest)?;
        let resources = manifest_resources(&manifest);
        let js_path = path.join(&backend);
        let source = std::fs::read_to_string(&js_path)
            .map_err(|e| Error::Runtime(format!("read backend {}: {e}", js_path.display())))?;
        Ok(Bundle { source, resources })
    } else {
        let source = std::fs::read_to_string(path)
            .map_err(|e| Error::Runtime(format!("read backend {}: {e}", path.display())))?;
        Ok(Bundle {
            source,
            resources: vec!["kv".to_string()],
        })
    }
}

/// Extract the `"resources"` string array from a (trusted, local) manifest.
/// Absent → empty (least privilege). Best-effort hand parse (see
/// [`manifest_backend`] for the dependency-surface rationale).
fn manifest_resources(manifest: &str) -> Vec<String> {
    let Some(key) = manifest.find("\"resources\"") else {
        return Vec::new();
    };
    let after = &manifest[key..];
    let Some(open) = after.find('[') else {
        return Vec::new();
    };
    let Some(close) = after[open..].find(']') else {
        return Vec::new();
    };
    let mut list = &after[open + 1..open + close];
    let mut out = Vec::new();
    while let Some(q1) = list.find('"') {
        let rest = &list[q1 + 1..];
        let Some(q2) = rest.find('"') else { break };
        out.push(rest[..q2].to_string());
        list = &rest[q2 + 1..];
    }
    out
}

/// Extract the `"backend"` string from a (trusted, local) manifest.json. A tiny
/// hand parse keeps the crate's dependency surface unchanged; swap in serde_json
/// when manifests grow richer fields.
fn manifest_backend(manifest: &str) -> Result<String> {
    let key = manifest
        .find("\"backend\"")
        .ok_or_else(|| Error::Runtime("manifest.json missing \"backend\"".into()))?;
    let after = &manifest[key + "\"backend\"".len()..];
    let colon = after
        .find(':')
        .ok_or_else(|| Error::Runtime("manifest.json: malformed backend entry".into()))?;
    let rest = &after[colon + 1..];
    let start = rest
        .find('"')
        .ok_or_else(|| Error::Runtime("manifest.json: backend value not a string".into()))?;
    let value = &rest[start + 1..];
    let end = value
        .find('"')
        .ok_or_else(|| Error::Runtime("manifest.json: unterminated backend value".into()))?;
    Ok(value[..end].to_string())
}

/// Map an rquickjs error into our typed Runtime error.
fn js_err(e: rquickjs::Error) -> Error {
    Error::Runtime(e.to_string())
}

/// Fold a caught JS exception/value into our typed Runtime error (owned String,
/// so nothing `'js` escapes the `with` closure).
fn caught_to_err(e: CaughtError<'_>) -> Error {
    Error::Runtime(e.to_string())
}
