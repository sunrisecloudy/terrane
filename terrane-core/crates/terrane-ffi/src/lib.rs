//! terrane-ffi — a C ABI into terrane-core for non-Rust hosts.
//!
//! The contract every non-Rust host (macOS/Swift, iOS, Android/JNI, Windows,
//! Linux) uses. Deliberately tiny and **serde-free**: commands cross as a name
//! plus a C array of string args; results come back as an opaque output string
//! (e.g. an app's `items` JSON, which Rust never parses) or an error string.
//!
//! Safety contract for callers:
//! - Every returned `char*` (in `out_output` / `out_error`) is Rust-allocated and
//!   must be freed exactly once with [`terrane_string_free`]; never `free(3)`.
//! - The [`TerraneHandle`] from [`terrane_open`] must be closed with
//!   [`terrane_close`] exactly once. Free/close calls with null pointers are safe
//!   no-ops.
//! - No Rust panic ever crosses the boundary: every entry point is wrapped in
//!   `catch_unwind` and reports [`TERRANE_ERR_PANIC`] instead.

use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::Mutex;

use terrane_domain::Request;

pub const TERRANE_OK: c_int = 0;
pub const TERRANE_ERR_NULL_ARG: c_int = 1;
pub const TERRANE_ERR_UTF8: c_int = 2;
pub const TERRANE_ERR_DISPATCH: c_int = 3;
pub const TERRANE_ERR_PANIC: c_int = 4;
pub const TERRANE_ERR_INTERNAL: c_int = 5;

/// Opaque handle to an open workspace. Only ever crossed as a pointer.
pub struct TerraneHandle {
    inner: Mutex<terrane_host::HostCore>,
}

/// Open (or create) a workspace at `home` (the dir holding `log.bin`); an empty
/// or null `home` uses the default (`$TERRANE_HOME` / `./.terrane`). Returns a
/// handle to close with [`terrane_close`], or null on failure.
///
/// # Safety
/// `home` must be null or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn terrane_open(home: *const c_char) -> *mut TerraneHandle {
    let result = catch_unwind(AssertUnwindSafe(|| -> Option<*mut TerraneHandle> {
        let log_path = if home.is_null() {
            return terrane_host::open().ok().map(|core| {
                Box::into_raw(Box::new(TerraneHandle {
                    inner: Mutex::new(core),
                }))
            });
        } else {
            let s = CStr::from_ptr(home).to_str().ok()?; // bad UTF-8 → fail
            if s.is_empty() {
                return terrane_host::open().ok().map(|core| {
                    Box::into_raw(Box::new(TerraneHandle {
                        inner: Mutex::new(core),
                    }))
                });
            } else {
                s
            }
        };
        let core = terrane_host::open_at_home(log_path).ok()?;
        Some(Box::into_raw(Box::new(TerraneHandle {
            inner: Mutex::new(core),
        })))
    }));
    result.ok().flatten().unwrap_or(ptr::null_mut())
}

/// Run an app's JS backend: `host.run app [args…]`. On success writes the
/// backend's printed string to `out_output` and returns [`TERRANE_OK`]; on
/// failure writes a message to `out_error` and returns a non-zero code.
///
/// # Safety
/// `app` and each `argv[i]` must be valid C strings; `argv` must point to `argc`
/// elements (or be null when `argc == 0`). `out_output`/`out_error` must be
/// valid pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_host_run(
    h: *mut TerraneHandle,
    app: *const c_char,
    argc: usize,
    argv: *const *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let app = match read_str(app) {
            Ok(a) => a,
            Err(code) => return code,
        };
        let mut args = match read_argv(argc, argv) {
            Ok(a) => a,
            Err(code) => return code,
        };
        let mut full = Vec::with_capacity(1 + args.len());
        full.push(app);
        full.append(&mut args);
        dispatch_request(
            h,
            Request::new("host.run", full),
            true,
            out_output,
            out_error,
        )
    }));
    finish(code, out_error)
}

/// Dispatch any command: `name [args…]`. On success writes the committed event
/// kinds (one per line) to `out_output`; on failure writes a message to
/// `out_error`. For non-`host.run` commands (e.g. `app.add`, `kv.set`).
///
/// # Safety
/// Same as [`terrane_host_run`], with `name` in place of `app`.
#[no_mangle]
pub unsafe extern "C" fn terrane_dispatch(
    h: *mut TerraneHandle,
    name: *const c_char,
    argc: usize,
    argv: *const *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let name = match read_str(name) {
            Ok(n) => n,
            Err(code) => return code,
        };
        let args = match read_argv(argc, argv) {
            Ok(a) => a,
            Err(code) => return code,
        };
        dispatch_request(h, Request::new(name, args), false, out_output, out_error)
    }));
    finish(code, out_error)
}

/// Free a string returned by terrane-ffi. Null-safe; non-null pointers are
/// single-use.
///
/// # Safety
/// `s` must be null or a pointer previously returned by this library that has
/// not already been freed.
#[no_mangle]
pub unsafe extern "C" fn terrane_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| drop(CString::from_raw(s))));
}

/// Close a handle from [`terrane_open`]. Null-safe; non-null handles are
/// single-use.
///
/// # Safety
/// `h` must be null or a pointer previously returned by [`terrane_open`], not
/// already closed.
#[no_mangle]
pub unsafe extern "C" fn terrane_close(h: *mut TerraneHandle) {
    if h.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| drop(Box::from_raw(h))));
}

// ---- internals ----

/// Lock the core, dispatch, and write the output (backend string for `host.run`,
/// else the committed event kinds) or the error.
unsafe fn dispatch_request(
    h: *mut TerraneHandle,
    request: Request,
    use_last_output: bool,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    let handle = match h.as_ref() {
        Some(handle) => handle,
        None => return TERRANE_ERR_NULL_ARG,
    };
    // Recover from a poisoned lock (a prior panic) rather than wedging the handle.
    let mut core = handle.inner.lock().unwrap_or_else(|e| e.into_inner());
    let name = request.name.clone();
    let args = request.args.clone();
    match terrane_host::dispatch_on_core(&mut core, &name, &args) {
        Ok(outcome) => {
            let output = if use_last_output {
                outcome.output.unwrap_or_default()
            } else {
                outcome
                    .records
                    .iter()
                    .map(|r| r.kind.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            write_out(out_output, output);
            TERRANE_OK
        }
        Err(e) => {
            write_out(out_error, e.to_string());
            TERRANE_ERR_DISPATCH
        }
    }
}

/// Convert a `catch_unwind` result into a code, reporting a panic via `out_error`.
unsafe fn finish(code: std::thread::Result<c_int>, out_error: *mut *mut c_char) -> c_int {
    match code {
        Ok(code) => code,
        Err(_) => {
            write_out(out_error, "panic in terrane-ffi".to_string());
            TERRANE_ERR_PANIC
        }
    }
}

unsafe fn null_out(out: *mut *mut c_char) {
    if !out.is_null() {
        *out = ptr::null_mut();
    }
}

unsafe fn read_str(p: *const c_char) -> Result<String, c_int> {
    if p.is_null() {
        return Err(TERRANE_ERR_NULL_ARG);
    }
    CStr::from_ptr(p)
        .to_str()
        .map(|s| s.to_string())
        .map_err(|_| TERRANE_ERR_UTF8)
}

unsafe fn read_argv(argc: usize, argv: *const *const c_char) -> Result<Vec<String>, c_int> {
    if argc == 0 {
        return Ok(Vec::new());
    }
    if argv.is_null() {
        return Err(TERRANE_ERR_NULL_ARG);
    }
    let mut out = Vec::with_capacity(argc);
    for i in 0..argc {
        out.push(read_str(*argv.add(i))?);
    }
    Ok(out)
}

/// Write an owned String into an out-pointer as a fresh C string (callee frees
/// with [`terrane_string_free`]). A string with an interior NUL writes null.
unsafe fn write_out(out: *mut *mut c_char, s: String) {
    if out.is_null() {
        return;
    }
    *out = match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    };
}
