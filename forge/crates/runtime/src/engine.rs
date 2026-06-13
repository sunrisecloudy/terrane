//! `QuickJsEngine`: the native rquickjs implementation of [`JsEngine`].
//!
//! prd-merged/01 CR-1 (zero ambient capability), CR-2 (engine trait), CR-3
//! (`ctx` namespaces), CR-4 (call-time checks), CR-5 (resource limits), CR-8
//! (deterministic mode), CR-11 (injected clock/RNG), CR-13 (two-layer defense).
//! prd-merged/07 SC-1/SC-2.
//!
//! **Native only**: rquickjs ships native C (QuickJS) and does not build for
//! `wasm32-unknown-unknown`, so this whole module is `#[cfg(not(target_arch =
//! "wasm32"))]` (gated at the `mod engine;` site in lib.rs and via the
//! Cargo.toml target-specific dependency).
//!
//! Containment model:
//!
//! - The realm is created with the standard-library intrinsics
//!   (`intrinsic::All`) and *no* host globals beyond standard JS. We install
//!   exactly one host object, `ctx`. There is no
//!   `fetch`/`process`/`require`/`XMLHttpRequest` (QuickJS doesn't add them; a
//!   test asserts they are `undefined`).
//! - CPU/wall-clock are bounded by an interrupt handler that trips a fuel budget
//!   and a wall-clock deadline → `ResourceLimitExceeded`.
//! - Memory is bounded by `Runtime::set_memory_limit`; stack by
//!   `set_max_stack_size` (deep recursion → `RuntimeError`, never a host stack
//!   overflow / FFI panic).
//! - Dynamic code evaluation is **poisoned** at the engine level (review 009 P1
//!   / 019 P1, CR-13): after the realm is built we (1) overwrite the
//!   `constructor` on every function-kind prototype with `undefined` so the
//!   `Function` constructor is unreachable through the prototype chain
//!   (`(() => {}).constructor` etc. are `undefined`, closing the bypass review
//!   019 found), and (2) null the `eval`/`Function` globals so `typeof eval ===
//!   'undefined'` and `typeof Function === 'undefined'`. The static policy scan
//!   (forge-pipeline, layer one) is still the first line, but the engine no
//!   longer *relies* on it — dynamic code evaluation is simply unavailable in the
//!   realm. (The Rust-side `Context::eval` used to load the program is `JS_Eval`,
//!   not the JS global, so loading + promise driving are unaffected. We keep the
//!   QuickJS `Eval` intrinsic precisely because `JS_Eval` shares its hook; see
//!   `disable_dynamic_eval`.)

use crate::host::HostContext;
use crate::{EngineOutcome, JsEngine, Program};
use forge_domain::{AppResult, CoreError, Limits};
use rquickjs::promise::PromiseState;
use rquickjs::{
    function::Rest, CatchResultExt, Context, Ctx, Function, Object, Promise, Runtime, Value,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

/// The native QuickJS-backed engine. Stateless: each [`run`](JsEngine::run)
/// builds a fresh realm so runs cannot leak state into one another.
#[derive(Debug, Default)]
pub struct QuickJsEngine {
    _private: (),
}

impl QuickJsEngine {
    pub fn new() -> Self {
        QuickJsEngine { _private: () }
    }
}

/// A `'static`, `Copy` handle to the borrowed [`HostContext`] for the duration
/// of one realm. See the long safety note at the call site in `run_inner`: the
/// pointee outlives the realm, all access is synchronous and single-threaded,
/// and the `&mut` is never aliased (one host call in flight at a time).
#[derive(Clone, Copy)]
struct HostPtr(*mut HostContext<'static>);

impl HostPtr {
    /// Erase the bridge lifetime so the handle is `'static`. Sound only because
    /// the realm (and every closure that holds this handle) is confined to the
    /// `with` block, which is strictly inside the host's borrow.
    fn new(host: &mut HostContext<'_>) -> Self {
        let ptr = host as *mut HostContext<'_>;
        // The lifetime parameter of HostContext only bounds the &mut dyn
        // HostBridge it holds; we never observe a value of that lifetime through
        // this pointer beyond the realm's synchronous lifetime, so erasing it to
        // 'static is sound under the invariant documented at the call site.
        // (clippy only sees "same type" because it ignores lifetimes in pointer
        // casts; the cast is load-bearing.)
        #[allow(clippy::unnecessary_cast)]
        HostPtr(ptr as *mut HostContext<'static>)
    }

    /// Reborrow the hub for one host call. Single-threaded, non-reentrant: each
    /// `ctx.*` forwarder takes this exclusive borrow, completes synchronously,
    /// and releases it before returning to JS.
    ///
    /// # Safety
    /// Caller guarantees no other live `&mut` to the same `HostContext` exists
    /// (upheld: host calls do not re-enter one another in M0a).
    #[allow(clippy::mut_from_ref, clippy::unnecessary_cast)]
    unsafe fn get<'a>(&self) -> &'a mut HostContext<'a> {
        &mut *(self.0 as *mut HostContext<'a>)
    }
}

/// Shared interrupt budget consulted by the QuickJS interrupt handler. The
/// handler is called periodically by the engine; returning `true` aborts
/// execution. We trip on either a fuel (tick) budget or a wall-clock deadline so
/// a hot loop or a slow pathological run cannot hang the host.
struct InterruptBudget {
    ticks_remaining: u64,
    deadline: Instant,
    /// Set once when the budget trips, so the engine can distinguish a
    /// limit-induced interrupt from an ordinary JS exception.
    tripped: bool,
}

/// What stopped the run, used to map a raw QuickJS failure to the right
/// `CoreError` (a tripped budget is `ResourceLimitExceeded`; an
/// uncaught/stack error is `RuntimeError`; a recorded host-call CoreError takes
/// precedence over both).
enum Stop {
    Completed(AppResult),
    HostError(CoreError),
    Limit(String),
    Runtime(String),
}

impl QuickJsEngine {
    /// Convert a JS value to canonical JSON via the realm's `JSON.stringify`.
    /// `undefined` (no JSON form) maps to `null`.
    fn js_to_json<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<serde_json::Value, CoreError> {
        match ctx.json_stringify(value) {
            Ok(Some(s)) => {
                let text = s.to_string().map_err(|e| {
                    CoreError::RuntimeError(format!("host could not read JS string: {e}"))
                })?;
                serde_json::from_str(&text).map_err(|e| {
                    CoreError::RuntimeError(format!("host could not parse JS JSON: {e}"))
                })
            }
            Ok(None) => Ok(serde_json::Value::Null),
            Err(e) => Err(CoreError::RuntimeError(format!(
                "JSON.stringify failed: {e}"
            ))),
        }
    }

    /// Convert canonical JSON into a JS value via the realm's `JSON.parse`.
    fn json_to_js<'js>(
        ctx: &Ctx<'js>,
        value: &serde_json::Value,
    ) -> Result<Value<'js>, rquickjs::Error> {
        let text = value.to_string();
        ctx.json_parse(text)
    }
}

impl JsEngine for QuickJsEngine {
    fn run(
        &self,
        program: &Program,
        input: &serde_json::Value,
        host: &mut HostContext<'_>,
        limits: &Limits,
    ) -> EngineOutcome {
        match run_inner(program, input, host, limits) {
            Ok(result) => EngineOutcome {
                result: Ok(result),
                logs: Vec::new(),
            },
            Err(e) => EngineOutcome {
                result: Err(e),
                logs: Vec::new(),
            },
        }
    }
}

/// Build the runtime/realm, wire the budget and `ctx`, drive `main`, and map the
/// stop reason to a `CoreError`. Logs are carried by the [`HostContext`] (the
/// caller drains them), so the returned outcome's `logs` is filled by the
/// record/replay layer below — `run` itself leaves it empty.
fn run_inner(
    program: &Program,
    input: &serde_json::Value,
    host: &mut HostContext<'_>,
    limits: &Limits,
) -> Result<AppResult, CoreError> {
    let runtime = Runtime::new()
        .map_err(|e| CoreError::RuntimeError(format!("failed to create JS runtime: {e}")))?;
    // Memory ceiling (CR-5). 0 means "unlimited" to QuickJS, so a positive
    // limit is always set from the manifest.
    runtime.set_memory_limit(limits.memory_bytes as usize);
    // Bound the C stack so deep/mutual recursion throws a catchable RangeError
    // (→ RuntimeError) instead of overflowing the host stack across FFI.
    runtime.set_max_stack_size(256 * 1024);

    // Interrupt budget: fuel ticks + wall-clock deadline (CR-5). The handler is
    // `FnMut() -> bool`; returning true aborts. We share the budget cell so the
    // engine can read `tripped` after the run to classify the failure.
    let budget = Rc::new(RefCell::new(InterruptBudget {
        ticks_remaining: limits.fuel,
        deadline: Instant::now() + std::time::Duration::from_millis(limits.wall_ms),
        tripped: false,
    }));
    {
        let budget = budget.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let mut b = budget.borrow_mut();
            if b.ticks_remaining == 0 || Instant::now() >= b.deadline {
                b.tripped = true;
                true // interrupt
            } else {
                b.ticks_remaining -= 1;
                false
            }
        })));
    }

    // Standard-library realm (`intrinsic::All`): Date, Eval, RegExp, JSON,
    // Proxy, Map/Set, TypedArrays, Promise — everything an applet's JS needs,
    // and nothing host-specific. Critically, this adds NO ambient capability
    // globals (`fetch`/`process`/`require`/`XMLHttpRequest` do not exist —
    // asserted by a test). The standard library would normally expose `eval` and
    // the `Function` constructor; we poison both right after building the realm
    // (see `disable_dynamic_eval` in `install_ctx`) so dynamic code evaluation is
    // unavailable at the engine level, not merely rejected by the static scan
    // (review 009 P1, CR-13).
    let context = Context::custom::<rquickjs::context::intrinsic::All>(&runtime)
        .map_err(|e| CoreError::RuntimeError(format!("failed to create JS context: {e}")))?;

    // The single shared host error slot: a host call that fails stores its
    // CoreError here and throws into JS to unwind; we read it back afterwards so
    // a PermissionDenied/ResourceLimitExceeded surfaces as the run outcome
    // rather than a generic JS exception.
    let host_error: Rc<RefCell<Option<CoreError>>> = Rc::new(RefCell::new(None));

    // Shared handle to the (borrowed) HostContext for the `ctx.*` forwarders.
    //
    // `Context::with` exposes its realm lifetime `'js` existentially (HRTB), so
    // the borrow checker cannot prove our `&mut HostContext` (a concrete,
    // non-`'static` borrow) outlives it — even though it factually does. We
    // therefore share the hub through a raw pointer (see [`HostPtr`]). This is
    // sound because: (1) `host` is borrowed for the *entire* `run_inner` call,
    // which strictly contains this `with` block; (2) the `ctx.*` closures only
    // ever run *synchronously inside* `main.call`/`execute_pending_job`, all of
    // which happen within this `with`; and (3) no closure escapes the realm
    // (the realm and all its functions are dropped at the end of `with`). The
    // `&mut` is never aliased: only one host call is in flight at a time.
    let host_ptr = HostPtr::new(host);

    let stop = context.with(|ctx| -> Stop {
        if let Err(e) = install_ctx(&ctx, host_ptr, &host_error) {
            return Stop::Runtime(format!("failed to install ctx host object: {e}"));
        }

        // Wrap the program so `main` is reachable as a global without relying on
        // ES-module namespace plumbing: strip a leading `export` from the
        // entrypoint declaration and assign the function to a known global.
        let wrapped = wrap_program(&program.source);
        if let Err(e) = ctx.eval::<(), _>(wrapped).catch(&ctx) {
            // A compile/eval error in user code is a runtime error (not a limit
            // hit) unless the budget already tripped.
            if budget.borrow().tripped {
                return Stop::Limit("CPU/wall-clock budget exceeded during program load".into());
            }
            return Stop::Runtime(format!("program failed to load: {e}"));
        }

        // Fetch the captured `main`.
        let main: Function = match ctx.globals().get("__forge_main") {
            Ok(f) => f,
            Err(_) => {
                return Stop::Runtime(
                    "program does not export an async function main(ctx, input)".into(),
                )
            }
        };

        // Marshal input and the ctx object, then call main(ctx, input).
        let ctx_obj: Object = match ctx.globals().get("ctx") {
            Ok(o) => o,
            Err(e) => return Stop::Runtime(format!("ctx object missing: {e}")),
        };
        let input_js = match QuickJsEngine::json_to_js(&ctx, input) {
            Ok(v) => v,
            Err(e) => return Stop::Runtime(format!("failed to marshal input: {e}")),
        };

        let value = match main.call::<_, Value>((ctx_obj, input_js)) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("main threw: {}", exception_message(&ctx, e));
                return classify_failure(&budget, &host_error, msg);
            }
        };

        // `main` is async → it returns a Promise. Pump the job queue until it
        // settles, honoring the interrupt budget (the handler fires inside the
        // jobs). Non-promise returns are accepted too (await of a plain value).
        match value.clone().into_promise() {
            Some(promise) => drive_promise(&ctx, &budget, &host_error, promise),
            None => Stop::from_json_result(QuickJsEngine::js_to_json(&ctx, value)),
        }
    });

    match stop {
        Stop::Completed(result) => Ok(result),
        Stop::HostError(e) => Err(e),
        Stop::Limit(msg) => Err(CoreError::ResourceLimitExceeded(msg)),
        Stop::Runtime(msg) => Err(CoreError::RuntimeError(msg)),
    }
}

impl Stop {
    /// Build a completed outcome from the JSON `main` resolved to. The script
    /// contract is `{ ok: bool, value: any }`; a non-object resolution is
    /// wrapped as `{ ok: true, value }` so simple scripts (returning a string,
    /// etc.) still produce a well-formed `AppResult`.
    fn from_json(value: serde_json::Value) -> Stop {
        match serde_json::from_value::<AppResult>(value.clone()) {
            Ok(app) => Stop::Completed(app),
            Err(_) => Stop::Completed(AppResult { ok: true, value }),
        }
    }

    /// Map a `js_to_json` conversion result into a [`Stop`]: success becomes a
    /// completed `AppResult`; a conversion `CoreError` becomes a runtime stop.
    fn from_json_result(result: Result<serde_json::Value, CoreError>) -> Stop {
        match result {
            Ok(value) => Stop::from_json(value),
            Err(e) => Stop::Runtime(e.to_string()),
        }
    }
}

/// Pump the QuickJS job queue until the run's promise settles or the budget
/// trips. The interrupt handler aborts a runaway job; we additionally guard the
/// pump loop with the wall-clock deadline so a job queue that never drains can't
/// spin forever.
fn drive_promise<'js>(
    ctx: &Ctx<'js>,
    budget: &Rc<RefCell<InterruptBudget>>,
    host_error: &Rc<RefCell<Option<CoreError>>>,
    promise: Promise<'js>,
) -> Stop {
    loop {
        match promise.state() {
            PromiseState::Pending => {
                // Run one pending job. If the interrupt tripped inside it, the
                // job throws; the promise may stay pending, so check the budget.
                let made_progress = ctx.execute_pending_job();
                if budget.borrow().tripped {
                    return Stop::Limit("CPU/wall-clock budget exceeded".into());
                }
                if let Some(e) = host_error.borrow_mut().take() {
                    return Stop::HostError(e);
                }
                if !made_progress {
                    // No more jobs and still pending: the promise can never
                    // settle (e.g. awaiting something that never resolves).
                    return Stop::Runtime(
                        "program awaited a value that never resolved (dead promise)".into(),
                    );
                }
            }
            PromiseState::Resolved => {
                let value: Result<Value, _> =
                    promise.result().expect("resolved promise has result");
                return match value {
                    Ok(v) => Stop::from_json_result(QuickJsEngine::js_to_json(ctx, v)),
                    Err(e) => classify_failure(budget, host_error, exception_message(ctx, e)),
                };
            }
            PromiseState::Rejected => {
                // Surface the rejection; `result()` re-throws the rejected value
                // into the realm, so we recover its real message via the ctx.
                let rejected: Result<Value, _> = promise.result().expect("rejected has result");
                let msg = match rejected {
                    Err(e) => exception_message(ctx, e),
                    Ok(_) => "promise rejected".to_string(),
                };
                return classify_failure(budget, host_error, msg);
            }
        }
    }
}

/// Recover the human-readable message of a QuickJS failure. For
/// `Error::Exception` the thrown value sits on the realm's exception slot, so we
/// catch it and read its `.toString()`/message; other errors stringify directly.
///
/// A memory exhaustion can leave QuickJS unable to even allocate an `Error`
/// object, so it throws a bare `null`/`undefined` with no message. We surface
/// that as the sentinel `"<oom: null throw>"` so the classifier can treat it as
/// a memory-limit suspension rather than a mysterious runtime error.
fn exception_message<'js>(ctx: &Ctx<'js>, error: rquickjs::Error) -> String {
    use rquickjs::CaughtError;
    match CaughtError::from_error(ctx, error) {
        CaughtError::Exception(ex) => ex
            .message()
            .filter(|m| !m.is_empty())
            .or_else(|| {
                let s = ex.to_string();
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            })
            .unwrap_or_else(|| "uncaught exception".to_string()),
        // A thrown `null`/`undefined` (no Error object) is the OOM signature.
        CaughtError::Value(v) if v.is_null() || v.is_undefined() => "<oom: null throw>".to_string(),
        other => other.to_string(),
    }
}

/// True if `msg` is an allocation-bound failure that should map to the memory
/// resource limit rather than a generic runtime error (CR-5). Covers QuickJS's
/// explicit OOM, the bare-`null` OOM throw, and the allocation-cap `RangeError`s
/// a doubling string / unbounded array hits right at the ceiling.
fn is_memory_exhaustion(msg: &str) -> bool {
    let lowered = msg.to_ascii_lowercase();
    lowered.contains("out of memory")
        || lowered.contains("oom: null throw")
        || lowered.contains("string too long")
        || lowered.contains("invalid array length")
        || lowered.contains("array too long")
        || lowered.contains("allocation")
}

/// Map a raw QuickJS failure to a [`Stop`]. Precedence:
///   1. a recorded host `CoreError` (policy/limit/divergence) wins;
///   2. a tripped fuel/wall-clock budget is `ResourceLimitExceeded`;
///   3. a memory/allocation-exhaustion signature is also `ResourceLimitExceeded`
///      (the memory ceiling, CR-5);
///   4. everything else (including stack-overflow errors from deep recursion) is
///      an ordinary `RuntimeError`.
fn classify_failure(
    budget: &Rc<RefCell<InterruptBudget>>,
    host_error: &Rc<RefCell<Option<CoreError>>>,
    msg: String,
) -> Stop {
    if let Some(e) = host_error.borrow_mut().take() {
        return Stop::HostError(e);
    }
    if budget.borrow().tripped {
        return Stop::Limit(format!("CPU/wall-clock budget exceeded ({msg})"));
    }
    if is_memory_exhaustion(&msg) {
        return Stop::Limit(format!("memory budget exceeded ({msg})"));
    }
    Stop::Runtime(msg)
}

/// Transform user source so its `export async function main(...)` (or a plain
/// `async function main(...)`) is captured as the global `__forge_main`, then
/// re-exported through a name the engine reads. We do not rely on ES-module
/// evaluation: the program runs as a global script and assigns `main`.
fn wrap_program(source: &str) -> String {
    // The transpiler emits `export async function main` / `export function main`
    // / `export const main = ...`. For the spine we strip a leading `export `
    // (module syntax is invalid in a global script) and capture the binding.
    let stripped = source.replace("export async function main", "async function main");
    let stripped = stripped.replace("export function main", "function main");
    let stripped = stripped.replace("export const main", "const main");
    let stripped = stripped.replace("export default async function main", "async function main");
    format!("{stripped}\n;globalThis.__forge_main = main;\n")
}

/// Install the single host object `ctx` into the realm globals. Every method
/// forwards to the shared [`HostContext`] (policy + recorder + budgets). A
/// forwarded host error is stored in `host_error` and re-thrown into JS to
/// unwind; the engine reads the slot after the run so the real `CoreError`
/// (not a generic JS exception) becomes the outcome.
fn install_ctx<'js>(
    ctx: &Ctx<'js>,
    host: HostPtr,
    host_error: &Rc<RefCell<Option<CoreError>>>,
) -> Result<(), rquickjs::Error> {
    let globals = ctx.globals();
    let ctx_obj = Object::new(ctx.clone())?;

    let storage = Object::new(ctx.clone())?;
    let db = Object::new(ctx.clone())?;
    let ui = Object::new(ctx.clone())?;
    let time = Object::new(ctx.clone())?;
    let random = Object::new(ctx.clone())?;

    // --- storage.get(key) -> value | null --------------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>, key: Value<'js>| -> rquickjs::Result<Value<'js>> {
                let key = value_to_string(&cx, &key)?;
                let r = unsafe { host.get() }.storage_get(&key);
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        storage.set("get", f)?;
    }
    // --- storage.set(key, value) -> null ---------------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>, key: Value<'js>, val: Value<'js>| -> rquickjs::Result<Value<'js>> {
                let key = value_to_string(&cx, &key)?;
                let json = QuickJsEngine::js_to_json(&cx, val)
                    .map_err(|e| store_and_throw(&cx, &host_error, e))?;
                let r = unsafe { host.get() }
                    .storage_set(&key, json)
                    .map(|()| serde_json::Value::Null);
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        storage.set("set", f)?;
    }
    // --- storage.delete(key) -> null -------------------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>, key: Value<'js>| -> rquickjs::Result<Value<'js>> {
                let key = value_to_string(&cx, &key)?;
                let r = unsafe { host.get() }
                    .storage_delete(&key)
                    .map(|()| serde_json::Value::Null);
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        storage.set("delete", f)?;
    }
    // --- storage.list(prefix) -> string[] --------------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>, prefix: Value<'js>| -> rquickjs::Result<Value<'js>> {
                let prefix = value_to_string(&cx, &prefix)?;
                let r = unsafe { host.get() }
                    .storage_list(&prefix)
                    .map(|keys| serde_json::json!(keys));
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        storage.set("list", f)?;
    }

    // --- db.insert(collection, record) -> id -----------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>,
                  coll: Value<'js>,
                  rec: Value<'js>|
                  -> rquickjs::Result<Value<'js>> {
                let coll = value_to_string(&cx, &coll)?;
                let json = QuickJsEngine::js_to_json(&cx, rec)
                    .map_err(|e| store_and_throw(&cx, &host_error, e))?;
                let r = unsafe { host.get() }
                    .db_insert(&coll, json)
                    .map(|id| serde_json::json!(id));
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        db.set("insert", f)?;
    }
    // --- db.get(collection, id) -> record | null -------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>, coll: Value<'js>, id: Value<'js>| -> rquickjs::Result<Value<'js>> {
                let coll = value_to_string(&cx, &coll)?;
                let id = value_to_string(&cx, &id)?;
                let r = unsafe { host.get() }.db_get(&coll, &id);
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        db.set("get", f)?;
    }
    // --- db.list(collection) -> record[] ---------------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>, coll: Value<'js>| -> rquickjs::Result<Value<'js>> {
                let coll = value_to_string(&cx, &coll)?;
                let r = unsafe { host.get() }
                    .db_list(&coll)
                    .map(|rows| serde_json::json!(rows));
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        db.set("list", f)?;
    }
    // --- db.query(query) / db.query(collection, query) -> record[] --------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>, args: Rest<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                let (coll, query_json) = match args.as_slice() {
                    [query] => {
                        let query_json = QuickJsEngine::js_to_json(&cx, query.clone())
                            .map_err(|e| store_and_throw(&cx, &host_error, e))?;
                        let coll = query_json
                            .get("from")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                store_and_throw(
                                    &cx,
                                    &host_error,
                                    CoreError::QueryError(
                                        "ctx.db.query(query) requires a string 'from' collection"
                                            .into(),
                                    ),
                                )
                            })?
                            .to_string();
                        (coll, query_json)
                    }
                    [collection, query] => {
                        let coll = value_to_string(&cx, collection)?;
                        let query_json = QuickJsEngine::js_to_json(&cx, query.clone())
                            .map_err(|e| store_and_throw(&cx, &host_error, e))?;
                        (coll, query_json)
                    }
                    _ => {
                        return Err(store_and_throw(
                            &cx,
                            &host_error,
                            CoreError::QueryError(
                                "ctx.db.query expects query or collection, query".into(),
                            ),
                        ))
                    }
                };
                let r = unsafe { host.get() }.db_query(&coll, query_json);
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        db.set("query", f)?;
    }

    // --- ui.render(tree) -> null -----------------------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>, tree: Value<'js>| -> rquickjs::Result<Value<'js>> {
                let json = QuickJsEngine::js_to_json(&cx, tree)
                    .map_err(|e| store_and_throw(&cx, &host_error, e))?;
                let r = unsafe { host.get() }
                    .ui_render(json)
                    .map(|()| serde_json::Value::Null);
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        ui.set("render", f)?;
    }

    // --- time.now() -> i64 (logical clock seam) --------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                let r = unsafe { host.get() }.now().map(|n| serde_json::json!(n));
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        time.set("now", f)?;
    }
    // --- random.next() -> f64 (seeded RNG seam) --------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                let r = unsafe { host.get() }
                    .random_next()
                    .map(|x| serde_json::json!(x));
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        random.set("next", f)?;
    }

    // --- log(line) -> null (top-level ctx.log) ---------------------------
    {
        let host_error = host_error.clone();
        let f = Function::new(
            ctx.clone(),
            move |cx: Ctx<'js>, line: Value<'js>| -> rquickjs::Result<Value<'js>> {
                let line = value_to_string(&cx, &line)?;
                let r = unsafe { host.get() }
                    .log(&line)
                    .map(|()| serde_json::Value::Null);
                host_result_to_js(&cx, &host_error, r)
            },
        )?;
        ctx_obj.set("log", f)?;
    }

    ctx_obj.set("storage", storage)?;
    ctx_obj.set("db", db)?;
    ctx_obj.set("ui", ui)?;
    ctx_obj.set("time", time)?;
    ctx_obj.set("random", random)?;
    globals.set("ctx", ctx_obj)?;

    // Poison dynamic code evaluation at the engine level (review 009 P1, CR-13).
    disable_dynamic_eval(ctx)?;
    Ok(())
}

/// Poison dynamic code evaluation in the realm (review 009 P1 / 019 P1, CR-13).
///
/// Nulling only `globalThis.eval` and `globalThis.Function` is **not** enough:
/// the `Function` constructor is reachable through any function object's
/// prototype chain — `(() => {}).constructor`, `(function(){}).constructor`,
/// `(async function(){}).constructor`, `(function*(){}).constructor`,
/// `(async function*(){}).constructor` all yield a live constructor that
/// compiles a string into runnable code. Review 019 confirmed `(() =>
/// {}).constructor('return 1+1')()` returned `2` against the global-only
/// version, so dynamic evaluation was still reachable.
///
/// We cannot simply drop the QuickJS `Eval` intrinsic at realm construction:
/// the host-side `Context::eval` we use to *load* the program is `JS_Eval`,
/// which requires the same `eval_internal` hook the intrinsic installs — dropping
/// it would make program load fail with "eval is not supported". Instead we keep
/// the intrinsic (so `Context::eval` and `async`/Promise machinery work) and:
///
/// 1. Walk each function-kind prototype (`Function.prototype` and the
///    Async/Generator/AsyncGenerator function prototypes, reached via literals
///    so we never touch the global `Function`) and overwrite its `constructor`
///    with `undefined`. After this, `(<any function>).constructor` is
///    `undefined` — not callable — so the constructor chain cannot reach
///    `js_function_constructor` (which internally does an indirect `eval`).
/// 2. Null the `eval` and `Function` globals so `typeof eval === 'undefined'`
///    and `typeof Function === 'undefined'` (the assertable no-ambient-capability
///    shape).
///
/// QuickJS's internal function machinery (used by the host `Context::eval` that
/// loads the program, and by `async`/Promise) does not route through these JS
/// bindings, so program load + promise driving stay unaffected.
fn disable_dynamic_eval<'js>(ctx: &Ctx<'js>) -> Result<(), rquickjs::Error> {
    // Poison the constructor reachable through every function-kind prototype.
    // Done in JS so we walk real prototype objects (including async/generator
    // kinds) without naming the global `Function`. `Reflect.getPrototypeOf` of a
    // function literal gives us each `*.prototype`; we redefine its `constructor`
    // to `undefined` as a non-writable, non-configurable property so it cannot be
    // restored or reassigned. Wrapped in try/catch per prototype so a missing
    // kind (e.g. if an intrinsic is absent) is non-fatal.
    const POISON_CONSTRUCTOR_CHAIN: &str = r#"
        (function () {
            "use strict";
            var protos = [
                Reflect.getPrototypeOf(function () {}),
                Reflect.getPrototypeOf(function* () {}),
                Reflect.getPrototypeOf(async function () {}),
                Reflect.getPrototypeOf(async function* () {})
            ];
            for (var i = 0; i < protos.length; i++) {
                var p = protos[i];
                if (!p) { continue; }
                try {
                    Object.defineProperty(p, "constructor", {
                        value: undefined,
                        writable: false,
                        enumerable: false,
                        configurable: false
                    });
                } catch (e) { /* already non-configurable: best effort */ }
            }
        })();
    "#;
    ctx.eval::<(), _>(POISON_CONSTRUCTOR_CHAIN)?;

    // Null the global bindings so `typeof eval`/`typeof Function` === 'undefined'.
    let globals = ctx.globals();
    let undefined = Value::new_undefined(ctx.clone());
    globals.set("eval", undefined.clone())?;
    globals.set("Function", undefined)?;
    Ok(())
}

/// Coerce a JS argument to a Rust `String` for keys/ids/prefixes. Strings pass
/// through; everything else is JSON-stringified so numeric/other keys degrade
/// predictably rather than panicking.
fn value_to_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> rquickjs::Result<String> {
    if let Some(s) = value.as_string() {
        return s.to_string();
    }
    match ctx.json_stringify(value.clone())? {
        Some(s) => s.to_string(),
        None => Ok(String::new()),
    }
}

/// Convert a host call `Result<serde_json::Value>` into a JS value, or — on a
/// `CoreError` — store it in the shared slot and throw into JS to unwind the
/// run. The engine reads the slot afterward and surfaces the real CoreError.
fn host_result_to_js<'js>(
    ctx: &Ctx<'js>,
    host_error: &Rc<RefCell<Option<CoreError>>>,
    result: Result<serde_json::Value, CoreError>,
) -> rquickjs::Result<Value<'js>> {
    match result {
        Ok(json) => QuickJsEngine::json_to_js(ctx, &json),
        Err(e) => Err(store_and_throw(ctx, host_error, e)),
    }
}

/// Stash a `CoreError` in the shared slot and throw a JS exception carrying its
/// message (so JS unwinds and the engine can recover the real error).
fn store_and_throw<'js>(
    ctx: &Ctx<'js>,
    host_error: &Rc<RefCell<Option<CoreError>>>,
    e: CoreError,
) -> rquickjs::Error {
    let msg = e.to_string();
    // Only the first host error matters (the one that unwinds the run).
    if host_error.borrow().is_none() {
        *host_error.borrow_mut() = Some(e);
    }
    ctx.throw(
        rquickjs::String::from_str(ctx.clone(), &msg)
            .map(Value::from_string)
            .unwrap_or_else(|_| Value::new_undefined(ctx.clone())),
    )
}
