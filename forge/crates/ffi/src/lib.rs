//! C ABI boundary for `forge-core`.
//!
//! This crate is intentionally thin: it owns native handles, converts UTF-8 JSON
//! to/from the shared `forge_domain` command/response/event types, and prevents
//! Rust panics from crossing into hosts. All workspace behavior stays in
//! `forge-core`.

use forge_core::WorkspaceCore;
use forge_domain::{CoreCommand, CoreError, CoreEvent, CoreResponse, RequestId};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Opaque handle owned by the C ABI caller.
#[repr(C)]
pub struct ForgeCoreHandle {
    _private: [u8; 0],
}

struct HandleState {
    core: Mutex<Option<WorkspaceCore>>,
}

// SAFETY: the FFI crate is the only constructor for `HandleState`, and it only
// exposes `WorkspaceCore::open` / `in_memory` with the default host factories.
// Every access to the core goes through this mutex; `forge_core_close` removes
// the registry entry before waiting on the same mutex, so no two FFI calls can
// touch the workspace concurrently and late calls cannot reach reclaimed state.
unsafe impl Send for HandleState {}
unsafe impl Sync for HandleState {}

static NEXT_HANDLE_ID: AtomicUsize = AtomicUsize::new(1);
static HANDLE_REGISTRY: OnceLock<Mutex<HashMap<usize, Arc<HandleState>>>> = OnceLock::new();

fn handle_registry() -> &'static Mutex<HashMap<usize, Arc<HandleState>>> {
    HANDLE_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Serialize)]
struct FfiErrorEnvelope<'a> {
    ok: bool,
    error: &'a CoreError,
}

#[derive(Serialize)]
struct EventDrainEnvelope {
    ok: bool,
    events: Vec<CoreEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<CoreError>,
}

fn ffi_request_id() -> RequestId {
    RequestId::new("ffi")
}

fn panic_error() -> CoreError {
    CoreError::RuntimeError("panic caught at forge-ffi boundary".to_string())
}

fn poisoned_handle_error() -> CoreError {
    CoreError::RuntimeError("core handle lock is poisoned".to_string())
}

fn closed_handle_error() -> CoreError {
    CoreError::ValidationError("core handle is closed".to_string())
}

fn exhausted_handles_error() -> CoreError {
    CoreError::ResourceLimitExceeded("exhausted FFI handle ids".to_string())
}

fn handle_id(handle: *mut ForgeCoreHandle) -> Result<usize, CoreError> {
    if handle.is_null() {
        return Err(CoreError::ValidationError(
            "core handle is null".to_string(),
        ));
    }
    Ok(handle as usize)
}

fn allocate_handle(core: WorkspaceCore) -> Result<*mut ForgeCoreHandle, CoreError> {
    let id = NEXT_HANDLE_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| exhausted_handles_error())?;
    let state = Arc::new(HandleState {
        core: Mutex::new(Some(core)),
    });

    let mut registry = handle_registry()
        .lock()
        .map_err(|_| poisoned_handle_error())?;
    if registry.insert(id, state).is_some() {
        return Err(CoreError::RuntimeError(
            "allocated duplicate FFI handle id".to_string(),
        ));
    }
    Ok(id as *mut ForgeCoreHandle)
}

fn get_handle_state(handle: *mut ForgeCoreHandle) -> Result<Arc<HandleState>, CoreError> {
    let id = handle_id(handle)?;
    let registry = handle_registry()
        .lock()
        .map_err(|_| poisoned_handle_error())?;
    registry.get(&id).cloned().ok_or_else(closed_handle_error)
}

fn remove_handle_state(
    handle: *mut ForgeCoreHandle,
) -> Result<Option<Arc<HandleState>>, CoreError> {
    let id = handle_id(handle)?;
    let mut registry = handle_registry()
        .lock()
        .map_err(|_| poisoned_handle_error())?;
    Ok(registry.remove(&id))
}

fn serialize_json<T: Serialize>(value: &T) -> String {
    match serde_json::to_string(value) {
        Ok(json) => json,
        Err(e) => format!(
            r#"{{"request_id":"ffi","ok":false,"payload":null,"error":{{"kind":"RuntimeError","detail":"failed to serialize FFI response: {e}"}}}}"#
        ),
    }
}

fn error_envelope_json(error: &CoreError) -> String {
    serialize_json(&FfiErrorEnvelope { ok: false, error })
}

fn set_last_error(error: &CoreError) {
    let json = error_envelope_json(error);
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(json);
    });
}

fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

fn response_json(response: CoreResponse) -> String {
    if let Some(error) = &response.error {
        set_last_error(error);
    } else {
        clear_last_error();
    }
    serialize_json(&response)
}

fn response_error_json(error: CoreError) -> String {
    set_last_error(&error);
    serialize_json(&CoreResponse::err(ffi_request_id(), error))
}

fn drain_json(events: Vec<CoreEvent>) -> String {
    clear_last_error();
    serialize_json(&EventDrainEnvelope {
        ok: true,
        events,
        error: None,
    })
}

fn drain_error_json(error: CoreError) -> String {
    set_last_error(&error);
    serialize_json(&EventDrainEnvelope {
        ok: false,
        events: Vec::new(),
        error: Some(error),
    })
}

fn into_c_string(json: String) -> *mut c_char {
    let sanitized = json.replace('\0', "\\u0000");
    match CString::new(sanitized) {
        Ok(s) => s.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

unsafe fn read_c_string(ptr: *const c_char, name: &str) -> Result<String, CoreError> {
    if ptr.is_null() {
        return Err(CoreError::ValidationError(format!(
            "{name} pointer is null"
        )));
    }
    let cstr = unsafe { CStr::from_ptr(ptr) };
    cstr.to_str()
        .map(str::to_owned)
        .map_err(|e| CoreError::ValidationError(format!("{name} must be valid UTF-8: {e}")))
}

fn open_in_memory_inner(workspace_id: *const c_char) -> Result<*mut ForgeCoreHandle, CoreError> {
    let workspace_id = unsafe { read_c_string(workspace_id, "workspace_id") }?;
    let core = WorkspaceCore::in_memory(workspace_id)?;
    allocate_handle(core)
}

fn open_inner(
    path: *const c_char,
    workspace_id: *const c_char,
) -> Result<*mut ForgeCoreHandle, CoreError> {
    let path = unsafe { read_c_string(path, "path") }?;
    if path.trim().is_empty() {
        return Err(CoreError::ValidationError(
            "path must not be empty".to_string(),
        ));
    }
    let workspace_id = unsafe { read_c_string(workspace_id, "workspace_id") }?;
    let core = WorkspaceCore::open(Path::new(&path), workspace_id)?;
    allocate_handle(core)
}

fn handle_command_inner(handle: *mut ForgeCoreHandle, command_json: *const c_char) -> String {
    let command_json = match unsafe { read_c_string(command_json, "command_json") } {
        Ok(json) => json,
        Err(error) => return response_error_json(error),
    };
    let command: CoreCommand = match serde_json::from_str(&command_json) {
        Ok(command) => command,
        Err(e) => {
            return response_error_json(CoreError::ValidationError(format!(
                "command_json is not a valid CoreCommand: {e}"
            )))
        }
    };

    let state = match get_handle_state(handle) {
        Ok(state) => state,
        Err(error) => return response_error_json(error),
    };
    let mut core = match state.core.lock() {
        Ok(core) => core,
        Err(_) => return response_error_json(poisoned_handle_error()),
    };
    let Some(core) = core.as_mut() else {
        return response_error_json(closed_handle_error());
    };

    let response = core.handle(command);
    response_json(response)
}

fn drain_events_inner(handle: *mut ForgeCoreHandle) -> String {
    let state = match get_handle_state(handle) {
        Ok(state) => state,
        Err(error) => return drain_error_json(error),
    };
    let mut core = match state.core.lock() {
        Ok(core) => core,
        Err(_) => return drain_error_json(poisoned_handle_error()),
    };
    let Some(core) = core.as_mut() else {
        return drain_error_json(closed_handle_error());
    };

    let events = core.events_mut().drain();
    drain_json(events)
}

/// Open or create a file-backed workspace and return an owned core handle.
///
/// # Safety
///
/// `path` and `workspace_id` must be valid pointers to NUL-terminated UTF-8
/// strings for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn forge_core_open(
    path: *const c_char,
    workspace_id: *const c_char,
) -> *mut ForgeCoreHandle {
    match catch_unwind(AssertUnwindSafe(|| open_inner(path, workspace_id))) {
        Ok(Ok(handle)) => {
            clear_last_error();
            handle
        }
        Ok(Err(error)) => {
            set_last_error(&error);
            ptr::null_mut()
        }
        Err(_) => {
            let error = panic_error();
            set_last_error(&error);
            ptr::null_mut()
        }
    }
}

/// Open an in-memory workspace and return an owned core handle.
///
/// # Safety
///
/// `workspace_id` must be a valid pointer to a NUL-terminated UTF-8 string for
/// the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn forge_core_open_in_memory(
    workspace_id: *const c_char,
) -> *mut ForgeCoreHandle {
    match catch_unwind(AssertUnwindSafe(|| open_in_memory_inner(workspace_id))) {
        Ok(Ok(handle)) => {
            clear_last_error();
            handle
        }
        Ok(Err(error)) => {
            set_last_error(&error);
            ptr::null_mut()
        }
        Err(_) => {
            let error = panic_error();
            set_last_error(&error);
            ptr::null_mut()
        }
    }
}

/// Handle one serialized `CoreCommand` and return a serialized `CoreResponse`.
///
/// The returned string must be released with `forge_string_free`.
///
/// # Safety
///
/// `handle`, when non-null, must be a pointer returned by this library and not
/// yet closed. `command_json` must be a valid pointer to a NUL-terminated UTF-8
/// string for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn forge_core_handle_command(
    handle: *mut ForgeCoreHandle,
    command_json: *const c_char,
) -> *mut c_char {
    let json = match catch_unwind(AssertUnwindSafe(|| {
        handle_command_inner(handle, command_json)
    })) {
        Ok(json) => json,
        Err(_) => response_error_json(panic_error()),
    };
    into_c_string(json)
}

/// Drain events emitted by the core handle.
///
/// Returns `{ "ok": true, "events": [...] }` or a structured `{ "ok": false,
/// "error": ... }` envelope. The returned string must be released with
/// `forge_string_free`.
///
/// # Safety
///
/// `handle`, when non-null, must be a pointer returned by this library and not
/// yet closed.
#[no_mangle]
pub unsafe extern "C" fn forge_core_drain_events(handle: *mut ForgeCoreHandle) -> *mut c_char {
    let json = match catch_unwind(AssertUnwindSafe(|| drain_events_inner(handle))) {
        Ok(json) => json,
        Err(_) => drain_error_json(panic_error()),
    };
    into_c_string(json)
}

/// Return the last structured FFI error JSON for the current thread, or null.
///
/// The returned string, when non-null, must be released with `forge_string_free`.
#[no_mangle]
pub extern "C" fn forge_core_last_error() -> *mut c_char {
    match catch_unwind(|| LAST_ERROR.with(|slot| slot.borrow().clone())) {
        Ok(Some(json)) => into_c_string(json),
        Ok(None) => ptr::null_mut(),
        Err(_) => into_c_string(error_envelope_json(&panic_error())),
    }
}

/// Close a core handle returned by `forge_core_open*`. Null is a no-op.
///
/// # Safety
///
/// `handle`, when non-null, must be a pointer returned by this library. Closing
/// is idempotent: the workspace core is dropped at most once, and later calls
/// fail closed with structured errors.
#[no_mangle]
pub unsafe extern "C" fn forge_core_close(handle: *mut ForgeCoreHandle) {
    if handle.is_null() {
        return;
    }
    match catch_unwind(AssertUnwindSafe(|| -> Result<(), CoreError> {
        let Some(state) = remove_handle_state(handle)? else {
            return Ok(());
        };
        let core = match state.core.lock() {
            Ok(mut slot) => slot.take(),
            Err(_) => return Err(poisoned_handle_error()),
        };
        drop(core);
        Ok(())
    })) {
        Ok(Ok(())) => clear_last_error(),
        Ok(Err(error)) => set_last_error(&error),
        Err(_) => {
            let error = panic_error();
            set_last_error(&error);
        }
    }
}

/// Free a UTF-8 string returned by this library. Null is a no-op.
///
/// # Safety
///
/// `value`, when non-null, must be a pointer returned by this library's string
/// returning functions and must be freed at most once.
#[no_mangle]
pub unsafe extern "C" fn forge_string_free(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        drop(CString::from_raw(value));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn active_handle_count() -> usize {
        handle_registry().lock().unwrap().len()
    }

    #[test]
    fn close_reclaims_the_registry_handle_shell() {
        let before = active_handle_count();
        let workspace = CString::new("ffi-registry-reclaim").unwrap();
        let handle = unsafe { forge_core_open_in_memory(workspace.as_ptr()) };
        assert!(!handle.is_null());
        assert_eq!(active_handle_count(), before + 1);

        unsafe { forge_core_close(handle) };
        assert_eq!(active_handle_count(), before);

        unsafe { forge_core_close(handle) };
        assert_eq!(active_handle_count(), before);
    }
}
