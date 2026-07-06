//! C ABI into the Terrane host for non-Rust hosts.
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
use std::path::PathBuf;
use std::ptr;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use terrane_core::{local_owner_principal, Request};

pub const TERRANE_OK: c_int = 0;
pub const TERRANE_ERR_NULL_ARG: c_int = 1;
pub const TERRANE_ERR_UTF8: c_int = 2;
pub const TERRANE_ERR_DISPATCH: c_int = 3;
pub const TERRANE_ERR_PANIC: c_int = 4;
pub const TERRANE_ERR_INTERNAL: c_int = 5;

/// Opaque handle to an open workspace. Only ever crossed as a pointer.
pub struct TerraneHandle {
    pub(crate) inner: Mutex<crate::HostCore>,
    previews: Mutex<crate::PreviewStore>,
    home: PathBuf,
}

/// Dispatch a trusted host command against an open handle. Used by the native
/// STT worker thread so PCM enqueue stays on the audio thread.
pub(crate) unsafe fn dispatch_on_terrane_handle(
    handle: *mut TerraneHandle,
    command: &str,
    args: &[String],
) -> Result<(), String> {
    let inner = handle
        .as_ref()
        .ok_or_else(|| "terrane handle is null".to_string())?;
    let mut core = inner
        .inner
        .lock()
        .map_err(|_| "terrane core lock poisoned".to_string())?;
    crate::dispatch_on_core(&mut core, command, args).map(|_| ())
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
        let open_home = if home.is_null() {
            let home = crate::home_dir();
            return crate::open_at_home(&home).ok().map(|core| {
                Box::into_raw(Box::new(TerraneHandle {
                    inner: Mutex::new(core),
                    previews: Mutex::new(crate::PreviewStore::new()),
                    home,
                }))
            });
        } else {
            let s = CStr::from_ptr(home).to_str().ok()?; // bad UTF-8 → fail
            if s.is_empty() {
                let home = crate::home_dir();
                return crate::open_at_home(&home).ok().map(|core| {
                    Box::into_raw(Box::new(TerraneHandle {
                        inner: Mutex::new(core),
                        previews: Mutex::new(crate::PreviewStore::new()),
                        home,
                    }))
                });
            } else {
                PathBuf::from(s)
            }
        };
        let core = crate::open_at_home(&open_home).ok()?;
        Some(Box::into_raw(Box::new(TerraneHandle {
            inner: Mutex::new(core),
            previews: Mutex::new(crate::PreviewStore::new()),
            home: open_home,
        })))
    }));
    result.ok().flatten().unwrap_or(ptr::null_mut())
}

/// Run an app backend through its cataloged runtime. On success writes the
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
        let args = match read_argv(argc, argv) {
            Ok(a) => a,
            Err(code) => return code,
        };
        let handle = match h.as_ref() {
            Some(handle) => handle,
            None => return TERRANE_ERR_NULL_ARG,
        };
        let mut core = handle.inner.lock().unwrap_or_else(|e| e.into_inner());
        match crate::invoke_app_input(&mut core, &app, &args) {
            Ok(output) => {
                write_out(out_output, output);
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e);
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Dispatch any command: `name [args…]`. On success writes the committed event
/// kinds (one per line) to `out_output`; on failure writes a message to
/// `out_error`. For non-runtime commands (e.g. `app.add`, `kv.set`).
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
        dispatch_request(
            h,
            Request::trusted_host(name, args),
            false,
            out_output,
            out_error,
        )
    }));
    finish(code, out_error)
}

/// Route a Terrane URL or file path through the deep-link host edge. On success
/// writes a short human summary.
///
/// # Safety
/// `target` must be a valid C string. `out_output`/`out_error` must be valid
/// pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_open_target(
    h: *mut TerraneHandle,
    target: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let target = match read_str(target) {
            Ok(target) => target,
            Err(code) => return code,
        };
        let handle = match h.as_ref() {
            Some(handle) => handle,
            None => return TERRANE_ERR_NULL_ARG,
        };
        let mut core = handle.inner.lock().unwrap_or_else(|e| e.into_inner());
        match crate::deep_links::open_target_on_core(&mut core, &target) {
            Ok(outcome) => {
                write_out(out_output, outcome.message());
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e);
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Create an in-memory App Builder preview from a JSON files payload. On success
/// writes `{"id":"...","frameUrl":"terrane-preview://<id>/frame/"}`.
///
/// # Safety
/// `files_json` must be a valid C string. `out_output`/`out_error` must be
/// valid pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_preview_create(
    h: *mut TerraneHandle,
    files_json: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let files_json = match read_str(files_json) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let handle = match h.as_ref() {
            Some(handle) => handle,
            None => return TERRANE_ERR_NULL_ARG,
        };
        let base_state = {
            let core = handle.inner.lock().unwrap_or_else(|e| e.into_inner());
            core.state().clone()
        };
        let mut previews = handle.previews.lock().unwrap_or_else(|e| e.into_inner());
        match previews.create_preview_json_from_json(&files_json, &base_state) {
            Ok(json) => {
                write_out(out_output, json);
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e);
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Read an in-memory preview asset by preview id and frame-relative path. An
/// empty `path` returns `manifest.ui`. On success writes JSON with `content` and
/// `contentType`.
///
/// # Safety
/// `preview_id` and `path` must be valid C strings. `out_output`/`out_error`
/// must be valid pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_preview_read_asset(
    h: *mut TerraneHandle,
    preview_id: *const c_char,
    path: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let preview_id = match read_str(preview_id) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let path = match read_str(path) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let handle = match h.as_ref() {
            Some(handle) => handle,
            None => return TERRANE_ERR_NULL_ARG,
        };
        let previews = handle.previews.lock().unwrap_or_else(|e| e.into_inner());
        match previews.read_asset_json(&preview_id, &path) {
            Ok(json) => {
                write_out(out_output, json);
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e);
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Read verified blob bytes for a macOS/iOS custom-scheme response. On success
/// writes JSON with base64 data, contentType, and hash.
///
/// # Safety
/// `app` and `name` must be valid C strings. `out_output`/`out_error` must be
/// valid pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_blob_read(
    h: *mut TerraneHandle,
    app: *const c_char,
    name: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let app = match read_str(app) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let name = match read_str(name) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let handle = match h.as_ref() {
            Some(handle) => handle,
            None => return TERRANE_ERR_NULL_ARG,
        };
        let core = handle.inner.lock().unwrap_or_else(|e| e.into_inner());
        let granted = terrane_cap_auth::namespace_granted(
            core.state(),
            &local_owner_principal(core.state()),
            &app,
            "blob",
        );
        match granted {
            Ok(true) => {}
            Ok(false) => {
                write_out(out_error, "permission required for blob".to_string());
                return TERRANE_ERR_DISPATCH;
            }
            Err(e) => {
                write_out(out_error, e.to_string());
                return TERRANE_ERR_DISPATCH;
            }
        }
        let meta = match core
            .state()
            .blob
            .blobs
            .get(&app)
            .and_then(|names| names.get(&name))
        {
            Some(meta) => meta.clone(),
            None => {
                write_out(out_error, format!("key not found: {app}/{name}"));
                return TERRANE_ERR_DISPATCH;
            }
        };
        drop(core);
        match crate::blob_store::read_verified(&handle.home, &meta.hash) {
            Ok(bytes) => {
                let json = format!(
                    "{{\"content\":\"{}\",\"contentType\":\"{}\",\"hash\":\"{}\"}}",
                    B64.encode(bytes),
                    json_string_content(&meta.mime),
                    meta.hash
                );
                write_out(out_output, json);
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e.to_string());
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Read an in-memory preview asset by preview id and frame-relative path.
/// Alias kept short for native hosts and docs.
///
/// # Safety
/// Same as [`terrane_preview_read_asset`].
#[no_mangle]
pub unsafe extern "C" fn terrane_preview_asset(
    h: *mut TerraneHandle,
    preview_id: *const c_char,
    path: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    terrane_preview_read_asset(h, preview_id, path, out_output, out_error)
}

/// Invoke an in-memory preview backend. On success writes the backend's returned
/// output string. Preview writes fold into preview State only; no event log is
/// appended.
///
/// # Safety
/// `preview_id`, `verb`, and each `argv[i]` must be valid C strings; `argv`
/// must point to `argc` elements (or be null when `argc == 0`).
/// `out_output`/`out_error` must be valid pointers to write a `char*` into (or
/// null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_preview_invoke(
    h: *mut TerraneHandle,
    preview_id: *const c_char,
    verb: *const c_char,
    argc: usize,
    argv: *const *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let preview_id = match read_str(preview_id) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let verb = match read_str(verb) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let args = match read_argv(argc, argv) {
            Ok(a) => a,
            Err(code) => return code,
        };
        let handle = match h.as_ref() {
            Some(handle) => handle,
            None => return TERRANE_ERR_NULL_ARG,
        };
        let mut previews = handle.previews.lock().unwrap_or_else(|e| e.into_inner());
        match previews.invoke_backend(&preview_id, &verb, &args) {
            Ok(output) => {
                write_out(out_output, output);
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e);
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Generate a draft app through the core builder capability. On success writes
/// JSON with `{ id, appId, name, prompt, harness, status, error, files }`.
///
/// # Safety
/// `app_id`, `name`, `prompt`, and `harness` must be valid C strings. `harness`
/// may be an empty string to use the default app-generation harness.
#[no_mangle]
pub unsafe extern "C" fn terrane_builder_generate(
    h: *mut TerraneHandle,
    app_id: *const c_char,
    name: *const c_char,
    prompt: *const c_char,
    harness: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let app_id = match read_str(app_id) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let name = match read_str(name) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let prompt = match read_str(prompt) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let harness = match read_str(harness) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let handle = match h.as_ref() {
            Some(handle) => handle,
            None => return TERRANE_ERR_NULL_ARG,
        };
        let mut core = handle.inner.lock().unwrap_or_else(|e| e.into_inner());
        match crate::generate_app_json(&mut core, &app_id, &name, &prompt, Some(&harness)) {
            Ok(json) => {
                write_out(out_output, json);
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e);
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Build an app frontend using terrane-app-build. On success writes JSON:
/// `{"dist":"<path>","files":<count>}`.
///
/// # Safety
/// `app_dir` must be a valid C string. `out_output`/`out_error` must be valid
/// pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_build_app(
    app_dir: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let app_dir = match read_str(app_dir) {
            Ok(s) => s,
            Err(code) => return code,
        };
        match terrane_app_build::build_app(terrane_app_build::BuildOptions {
            app_dir: PathBuf::from(app_dir),
            check_only: false,
        }) {
            Ok(result) => {
                write_out(
                    out_output,
                    format!(
                        "{{\"dist\":\"{}\",\"files\":{}}}",
                        json_string_content(&result.dist.to_string_lossy()),
                        result.files.len()
                    ),
                );
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e);
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Render the shared landing-page HTML for a host-supplied app catalog.
/// `catalog_json` is the host's catalog as `{"apps":[{"id","name","icon","has_ui"}]}`
/// (treated as opaque text — the page's script parses it); `app_href_template`
/// is the per-app link with an `{id}` placeholder, e.g.
/// `terrane-app://{id}/frame/`. Writes the HTML document to `out_output`.
///
/// # Safety
/// `catalog_json` and `app_href_template` must be valid C strings.
/// `out_output`/`out_error` must be valid pointers to write a `char*` into (or
/// null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_home_page(
    catalog_json: *const c_char,
    app_href_template: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let catalog_json = match read_str(catalog_json) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let app_href_template = match read_str(app_href_template) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let html = crate::home_page(&crate::HomePageOptions {
            app_href_template: &app_href_template,
            catalog_json: Some(&catalog_json),
            ..Default::default()
        });
        write_out(out_output, html);
        TERRANE_OK
    }));
    finish(code, out_error)
}

/// Free a string returned by the Terrane host C ABI. Null-safe; non-null pointers are
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

/// Provision the MLX local-model runtime for the workspace at `home`
/// (null/empty = default home, matching [`terrane_open`]). Blocking: the first
/// run may download the runtime (~hundreds of MB). Writes a human summary to
/// `out_output`. No handle needed — runtime provisioning is edge plumbing and
/// records nothing in the event log.
///
/// # Safety
/// `home` must be null or a valid C string; `out_output`/`out_error` must be
/// valid pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_local_model_setup_mlx(
    home: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let home = match read_home(home) {
            Ok(home) => home,
            Err(code) => return code,
        };
        match crate::local_llm::setup_mlx(&home) {
            Ok(summary) => {
                write_out(out_output, summary);
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e.to_string());
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Resident mlx server status for the workspace at `home` as a JSON object:
/// `{"running", "pid", "port", "idleSecs", "models"}`.
///
/// # Safety
/// Same as [`terrane_local_model_setup_mlx`].
#[no_mangle]
pub unsafe extern "C" fn terrane_local_model_server_status(
    home: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let home = match read_home(home) {
            Ok(home) => home,
            Err(code) => return code,
        };
        write_out(out_output, crate::local_llm::mlx_server_status_json(&home));
        TERRANE_OK
    }));
    finish(code, out_error)
}

/// Begin a native STT capture session: records `stt.session.open` and registers
/// a runner that drains PCM from [`terrane_stt_push_pcm`].
///
/// # Safety
/// `app` and `session_id` must be valid NUL-terminated UTF-8 C strings.
#[no_mangle]
pub unsafe extern "C" fn terrane_stt_session_begin(
    h: *mut TerraneHandle,
    app: *const c_char,
    session_id: *const c_char,
    sample_rate_hz: u32,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| -> c_int {
        if h.is_null() {
            return TERRANE_ERR_NULL_ARG;
        }
        let app = match read_str(app) {
            Ok(value) => value,
            Err(code) => return code,
        };
        let session_id = match read_str(session_id) {
            Ok(value) => value,
            Err(code) => return code,
        };
        match crate::stt_edge::session_begin(h as usize, &app, &session_id, sample_rate_hz) {
            Ok(()) => TERRANE_OK,
            Err(message) => {
                eprintln!("terrane-host: stt session begin failed: {message}");
                TERRANE_ERR_DISPATCH
            }
        }
    })) {
        Ok(code) => code,
        Err(_) => TERRANE_ERR_PANIC,
    }
}

/// Enqueue mono Int16 PCM for a session opened with [`terrane_stt_session_begin`].
/// Real-time safe: only pushes into a bounded ring.
///
/// # Safety
/// `session_id` must be a valid C string. `pcm` must point to `len` samples when
/// `len > 0`.
#[no_mangle]
pub unsafe extern "C" fn terrane_stt_push_pcm(
    session_id: *const c_char,
    pcm: *const i16,
    len: usize,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| -> c_int {
        let session_id = match read_str(session_id) {
            Ok(value) => value,
            Err(code) => return code,
        };
        let samples = if len == 0 {
            &[][..]
        } else {
            if pcm.is_null() {
                return TERRANE_ERR_NULL_ARG;
            }
            std::slice::from_raw_parts(pcm, len)
        };
        match crate::stt_edge::push_pcm(&session_id, samples) {
            Ok(()) => TERRANE_OK,
            Err(message) => {
                eprintln!("terrane-host: stt push pcm failed: {message}");
                TERRANE_ERR_DISPATCH
            }
        }
    })) {
        Ok(code) => code,
        Err(_) => TERRANE_ERR_PANIC,
    }
}

/// End a native STT session: drops the runner and records `stt.session.close-host`.
///
/// # Safety
/// `app`, `session_id`, and `reason` must be valid NUL-terminated UTF-8 C strings.
#[no_mangle]
pub unsafe extern "C" fn terrane_stt_session_end(
    h: *mut TerraneHandle,
    app: *const c_char,
    session_id: *const c_char,
    reason: *const c_char,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| -> c_int {
        if h.is_null() {
            return TERRANE_ERR_NULL_ARG;
        }
        let app = match read_str(app) {
            Ok(value) => value,
            Err(code) => return code,
        };
        let session_id = match read_str(session_id) {
            Ok(value) => value,
            Err(code) => return code,
        };
        let reason = match read_str(reason) {
            Ok(value) => value,
            Err(code) => return code,
        };
        match crate::stt_edge::session_end(h as usize, &app, &session_id, &reason) {
            Ok(()) => TERRANE_OK,
            Err(message) => {
                eprintln!("terrane-host: stt session end failed: {message}");
                TERRANE_ERR_DISPATCH
            }
        }
    })) {
        Ok(code) => code,
        Err(_) => TERRANE_ERR_PANIC,
    }
}

/// Stop the native STT worker and drop all capture sessions. Safe at process exit.
#[no_mangle]
pub extern "C" fn terrane_stt_shutdown() {
    let _ = catch_unwind(crate::stt_edge::shutdown);
}

/// Release in-process local-model inference engines. Call once before a
/// normal process exit (e.g. from `applicationWillTerminate`): a cached
/// llama.cpp model still holding Metal buffers when ggml's static destructors
/// run aborts the process. Safe to call at any time, including when nothing
/// is cached.
#[no_mangle]
pub extern "C" fn terrane_local_model_shutdown() {
    let _ = catch_unwind(crate::local_llm_shutdown);
}

/// Stop the resident mlx server for the workspace at `home`, if one is
/// running. Writes a short human summary to `out_output`.
///
/// # Safety
/// Same as [`terrane_local_model_setup_mlx`].
#[no_mangle]
pub unsafe extern "C" fn terrane_local_model_server_stop(
    home: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let home = match read_home(home) {
            Ok(home) => home,
            Err(code) => return code,
        };
        match crate::local_llm::mlx_server_stop(&home) {
            Ok(message) => {
                write_out(out_output, message);
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e.to_string());
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

// ---- internals ----

/// Resolve an RFC 7231 `Accept-Language` header to the best supported Terrane
/// language code (e.g. `"fr-CH, en;q=0.8"` -> `"fr"`). Pure: no handle or home
/// required. Writes the canonical code to `out_output` (e.g. `"zh-Hans"`).
///
/// # Safety
/// `header` must be a valid C string. `out_output`/`out_error` must be valid
/// pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_i18n_negotiate(
    header: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let header = match read_str(header) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let code = terrane_i18n::from_accept_language(&header);
        write_out(out_output, code.to_string());
        TERRANE_OK
    }));
    finish(code, out_error)
}

/// The canonical supported language codes as a JSON array, e.g.
/// `["en","es","zh-Hans",...]`. Lets native UIs populate language pickers from
/// one source of truth. Pure: no handle required.
///
/// # Safety
/// `out_output`/`out_error` must be valid pointers to write a `char*` into (or
/// null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_i18n_supported(
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let json =
            serde_json::to_string(terrane_i18n::SUPPORTED).unwrap_or_else(|_| "[]".to_string());
        write_out(out_output, json);
        TERRANE_OK
    }));
    finish(code, out_error)
}

/// Import checked-in i18n catalogs (`i18n/system/*.json` and
/// `apps/*/i18n/*.json`) at `path` into the workspace's public KV bucket via
/// one trusted-host `kv.public.import`. Idempotent and replay-safe. Writes a
/// human summary to `out_output`.
///
/// # Safety
/// `path` must be a valid C string; `out_output`/`out_error` must be valid
/// pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_i18n_import(
    h: *mut TerraneHandle,
    path: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let code = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let path = match read_str(path) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let handle = match h.as_ref() {
            Some(handle) => handle,
            None => return TERRANE_ERR_NULL_ARG,
        };
        let mut core = handle.inner.lock().unwrap_or_else(|e| e.into_inner());
        match crate::import_i18n_dir(&mut core, std::path::Path::new(&path)) {
            Ok(outcome) => {
                write_out(out_output, outcome.message());
                TERRANE_OK
            }
            Err(e) => {
                write_out(out_error, e);
                TERRANE_ERR_DISPATCH
            }
        }
    }));
    finish(code, out_error)
}

/// Read the localized message bundle for `code` as a JSON object, for a native
/// host to push to a UI. `app_id` empty = the shell-chrome (`system`) bundle;
/// otherwise the app frame bundle (`system` + that app's domain). English is the
/// fallback layer; keys are `<domain>.<key>` (e.g. `"todo.add"`). Unsupported or
/// empty `code` falls back to the default language.
///
/// # Safety
/// `code`/`app_id` must be valid C strings; `out_output`/`out_error` must be
/// valid pointers to write a `char*` into (or null to ignore).
#[no_mangle]
pub unsafe extern "C" fn terrane_i18n_bundle(
    h: *mut TerraneHandle,
    code: *const c_char,
    app_id: *const c_char,
    out_output: *mut *mut c_char,
    out_error: *mut *mut c_char,
) -> c_int {
    null_out(out_output);
    null_out(out_error);
    let rc = catch_unwind(AssertUnwindSafe(|| -> c_int {
        let code = match read_str(code) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let app_id = match read_str(app_id) {
            Ok(s) => s,
            Err(code) => return code,
        };
        let handle = match h.as_ref() {
            Some(handle) => handle,
            None => return TERRANE_ERR_NULL_ARG,
        };
        let core = handle.inner.lock().unwrap_or_else(|e| e.into_inner());
        let canonical = terrane_i18n::canonical(&code).unwrap_or(terrane_i18n::DEFAULT);
        let map = if app_id.trim().is_empty() {
            crate::i18n::system_bundle(&core, canonical)
        } else {
            crate::i18n::app_bundle(&core, canonical, app_id.trim())
        };
        let json = serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string());
        write_out(out_output, json);
        TERRANE_OK
    }));
    finish(rc, out_error)
}

/// Lock the core, dispatch, and write the output (backend string for runtime commands,
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
    match crate::dispatch_on_core(&mut core, &name, &args) {
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
            write_out(out_error, "panic in terrane-host ffi".to_string());
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

/// A `home` argument: null/empty selects the default home (like `terrane_open`).
unsafe fn read_home(p: *const c_char) -> Result<PathBuf, c_int> {
    if p.is_null() {
        return Ok(crate::home_dir());
    }
    let raw = read_str(p)?;
    if raw.trim().is_empty() {
        Ok(crate::home_dir())
    } else {
        Ok(PathBuf::from(raw))
    }
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

fn json_string_content(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
