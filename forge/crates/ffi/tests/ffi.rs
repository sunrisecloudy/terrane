use forge_domain::CoreResponse;
use std::ffi::{CStr, CString};
use std::ptr;

fn c(value: &str) -> CString {
    CString::new(value).unwrap()
}

unsafe fn take_string(ptr: *mut std::ffi::c_char) -> String {
    assert!(!ptr.is_null(), "FFI returned a null string pointer");
    let value = CStr::from_ptr(ptr).to_str().unwrap().to_string();
    forge_ffi::forge_string_free(ptr);
    value
}

fn command(name: &str, payload: serde_json::Value) -> String {
    serde_json::json!({
        "request_id": "r1",
        "actor": { "actor": "dev", "role": "owner" },
        "workspace_id": "ws1",
        "name": name,
        "payload": payload
    })
    .to_string()
}

#[test]
fn workspace_open_round_trips_over_c_abi() {
    let workspace = c("ws-ffi");
    let handle = unsafe { forge_ffi::forge_core_open_in_memory(workspace.as_ptr()) };
    assert!(!handle.is_null());

    let cmd = c(&command("workspace.open", serde_json::json!({})));
    let json = unsafe { take_string(forge_ffi::forge_core_handle_command(handle, cmd.as_ptr())) };
    let response: CoreResponse = serde_json::from_str(&json).unwrap();
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(
        response.payload["workspace_id"],
        serde_json::json!("ws-ffi")
    );

    unsafe { forge_ffi::forge_core_close(handle) };
}

#[test]
fn malformed_command_returns_structured_response() {
    let workspace = c("ws-ffi");
    let handle = unsafe { forge_ffi::forge_core_open_in_memory(workspace.as_ptr()) };
    assert!(!handle.is_null());

    let bad = c("{");
    let json = unsafe { take_string(forge_ffi::forge_core_handle_command(handle, bad.as_ptr())) };
    let response: CoreResponse = serde_json::from_str(&json).unwrap();
    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code(), "ValidationError");

    unsafe { forge_ffi::forge_core_close(handle) };
}

#[test]
fn null_handle_returns_structured_response() {
    let cmd = c(&command("workspace.open", serde_json::json!({})));
    let json = unsafe {
        take_string(forge_ffi::forge_core_handle_command(
            ptr::null_mut(),
            cmd.as_ptr(),
        ))
    };
    let response: CoreResponse = serde_json::from_str(&json).unwrap();
    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code(), "ValidationError");
}

#[test]
fn drain_events_returns_json_envelope() {
    let workspace = c("ws-ffi");
    let handle = unsafe { forge_ffi::forge_core_open_in_memory(workspace.as_ptr()) };
    assert!(!handle.is_null());

    let json = unsafe { take_string(forge_ffi::forge_core_drain_events(handle)) };
    let envelope: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(envelope["ok"], serde_json::json!(true));
    assert_eq!(envelope["events"], serde_json::json!([]));

    unsafe { forge_ffi::forge_core_close(handle) };
}

#[test]
fn failed_open_sets_structured_last_error() {
    let handle = unsafe { forge_ffi::forge_core_open_in_memory(ptr::null()) };
    assert!(handle.is_null());

    let json = unsafe { take_string(forge_ffi::forge_core_last_error()) };
    let envelope: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(envelope["ok"], serde_json::json!(false));
    assert_eq!(
        envelope["error"]["kind"],
        serde_json::json!("ValidationError")
    );
}
