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

use rquickjs::function::Rest;
use rquickjs::{CatchResultExt, CaughtError, Context, Ctx, Function, IntoJs, Object, Runtime, Value};
use terrane_domain::{Error, EventRecord, Result};

use super::{Capability, ReadValue, ResourceMethod};
use crate::{apply, default_registry, namespace_of, Decision, Registry, State};

/// Hand a resource read's result back to JS: a string|null, or an object.
impl<'js> IntoJs<'js> for ReadValue {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<Value<'js>> {
        match self {
            ReadValue::OptString(opt) => opt.into_js(ctx),
            ReadValue::StringMap(map) => map.into_js(ctx),
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
        let coalesce_key = name.starts_with("kv.").then(|| args.get(1).cloned()).flatten();
        let is_set = name == "kv.set";
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
                        let f = Function::new(ctx.clone(), move |args: Rest<Value>| {
                            match string_args(&call, params, &args.0) {
                                Ok(strs) => cell.borrow_mut().write(&call, strs),
                                Err(e) => cell.borrow_mut().capture(e),
                            }
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
        let fields = parse_manifest(&manifest)?;
        let backend = manifest_backend(&fields)?;
        let resources = manifest_resources(&fields);
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

/// Extract the top-level `"resources"` string array from a parsed manifest.
/// Absent (or not an array of strings) → empty (least privilege). Only top-level
/// keys count, and string escapes are already decoded by [`parse_manifest`].
fn manifest_resources(fields: &[(String, Json)]) -> Vec<String> {
    let Some(Json::Array(items)) = lookup(fields, "resources") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| match v {
            Json::Str(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// Extract the top-level `"backend"` string from a parsed manifest.json.
fn manifest_backend(fields: &[(String, Json)]) -> Result<String> {
    match lookup(fields, "backend") {
        Some(Json::Str(s)) => Ok(s.clone()),
        Some(_) => Err(Error::Runtime("manifest.json: \"backend\" is not a string".into())),
        None => Err(Error::Runtime("manifest.json missing \"backend\"".into())),
    }
}

/// First value bound to `key` among the manifest's top-level fields.
fn lookup<'a>(fields: &'a [(String, Json)], key: &str) -> Option<&'a Json> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// A manifest value we care about. Numbers, booleans, and null parse but are not
/// retained (`Other`) — the manifest only reads strings and string arrays.
enum Json {
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
    Other,
}

/// Parse a manifest into its top-level `(key, value)` fields.
///
/// We keep a hand-written JSON parser rather than pulling in `serde_json`:
/// terrane-core is deliberately serde-free (it serializes with borsh), and the
/// review confirmed serde is not in the workspace. This is the "harden the hand
/// parser" path of the deferred decision — a real (if minimal) JSON parse that
/// fixes the two edge cases the old positional scan got wrong: it matches keys
/// only at the *top level* (a nested `"backend"` in a sub-object no longer wins)
/// and it *decodes* string escapes (so `\"`, `\\`, `\uXXXX`, … in values are
/// read correctly). Revisit (and reconsider serde_json) only if manifests grow
/// genuinely rich/nested schemas.
fn parse_manifest(manifest: &str) -> Result<Vec<(String, Json)>> {
    let mut p = JsonParser {
        chars: manifest.chars().collect(),
        pos: 0,
    };
    match p.value()? {
        Json::Object(fields) => Ok(fields),
        _ => Err(Error::Runtime("manifest.json: root is not an object".into())),
    }
}

/// A tiny recursive-descent JSON reader over a `char` cursor. Correct for the
/// JSON grammar's structure (objects, arrays, strings with escapes); scalars
/// (numbers/bools/null) are consumed but discarded since manifests never read
/// them. Inputs are small, trusted, local bundle manifests.
struct JsonParser {
    chars: Vec<char>,
    pos: usize,
}

impl JsonParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<Json> {
        self.skip_ws();
        match self.peek() {
            Some('"') => Ok(Json::Str(self.string()?)),
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some(_) => {
                self.skip_scalar();
                Ok(Json::Other)
            }
            None => Err(Error::Runtime("manifest.json: unexpected end of input".into())),
        }
    }

    /// Read a string literal (cursor on the opening quote), decoding escapes.
    fn string(&mut self) -> Result<String> {
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(Error::Runtime("manifest.json: unterminated string".into())),
                Some('"') => return Ok(out),
                Some('\\') => match self.bump() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('b') => out.push('\u{0008}'),
                    Some('f') => out.push('\u{000C}'),
                    Some('u') => out.push(self.unicode_escape()?),
                    _ => return Err(Error::Runtime("manifest.json: invalid string escape".into())),
                },
                Some(c) => out.push(c),
            }
        }
    }

    /// Read the four hex digits of a `\uXXXX` escape (the `\u` is already eaten).
    fn unicode_escape(&mut self) -> Result<char> {
        let mut cp = 0u32;
        for _ in 0..4 {
            let d = self
                .bump()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| Error::Runtime("manifest.json: bad \\u escape".into()))?;
            cp = cp * 16 + d;
        }
        // Lone surrogates can't form a char; fall back to the replacement char.
        Ok(char::from_u32(cp).unwrap_or('\u{FFFD}'))
    }

    fn object(&mut self) -> Result<Json> {
        self.bump(); // '{'
        let mut fields = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some('}') => {
                    self.bump();
                    return Ok(Json::Object(fields));
                }
                Some('"') => {
                    let key = self.string()?;
                    self.skip_ws();
                    if self.bump() != Some(':') {
                        return Err(Error::Runtime("manifest.json: expected ':' after key".into()));
                    }
                    let value = self.value()?;
                    fields.push((key, value));
                    if !self.comma_or_end('}')? {
                        return Ok(Json::Object(fields));
                    }
                }
                _ => return Err(Error::Runtime("manifest.json: expected key or '}'".into())),
            }
        }
    }

    fn array(&mut self) -> Result<Json> {
        self.bump(); // '['
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(']') {
                self.bump();
                return Ok(Json::Array(items));
            }
            items.push(self.value()?);
            if !self.comma_or_end(']')? {
                return Ok(Json::Array(items));
            }
        }
    }

    /// After an element, consume a `,` (more follow → `true`) or the closing
    /// delimiter (`end` → `false`); anything else is malformed.
    fn comma_or_end(&mut self, end: char) -> Result<bool> {
        self.skip_ws();
        match self.bump() {
            Some(',') => Ok(true),
            Some(c) if c == end => Ok(false),
            _ => Err(Error::Runtime(format!(
                "manifest.json: expected ',' or '{end}'"
            ))),
        }
    }

    /// Consume a scalar token (number/true/false/null) we don't retain.
    fn skip_scalar(&mut self) {
        while let Some(c) = self.peek() {
            if c == ',' || c == '}' || c == ']' || c.is_whitespace() {
                break;
            }
            self.pos += 1;
        }
    }
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
