use forge_domain::CoreResponse;
use std::ffi::{CStr, CString};
use std::ptr;
use std::time::{SystemTime, UNIX_EPOCH};

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
    command_with_applet(name, payload, None)
}

fn command_with_applet(name: &str, payload: serde_json::Value, applet_id: Option<&str>) -> String {
    serde_json::json!({
        "request_id": "r1",
        "actor": { "actor": "dev", "role": "owner" },
        "workspace_id": "ws1",
        "applet_id": applet_id,
        "name": name,
        "payload": payload
    })
    .to_string()
}

fn demo_manifest() -> serde_json::Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": {
            "storage": { "read": ["app/*"], "write": ["app/*"] },
            "db": { "read": ["tasks"], "write": ["tasks"] },
            "ui": true
        },
        "limits": {
            "wall_ms": 3000,
            "fuel": 10000000,
            "memory_bytes": 67108864,
            "max_host_calls": 10000,
            "storage_bytes": 10485760,
            "log_bytes": 262144
        }
    })
}

fn demo_ts() -> &'static str {
    r#"
        export async function main(ctx: any, input: any): Promise<any> {
            const title: string = input && input.title ? input.title : "Ship W0";
            const id = await ctx.db.insert("tasks", { title: title, done: false });
            await ctx.storage.set("app/last", { id: id });
            ctx.log("rendered task " + id);
            await ctx.ui.render({
                type: "Stack",
                direction: "v",
                children: [
                    { type: "Text", text: "Tasks" },
                    { type: "List", items: [ { type: "Text", text: title } ] }
                ]
            });
            return { ok: true, value: { id: id } };
        }
    "#
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

#[test]
fn successful_open_clears_last_error() {
    let failed = unsafe { forge_ffi::forge_core_open_in_memory(ptr::null()) };
    assert!(failed.is_null());
    let json = unsafe { take_string(forge_ffi::forge_core_last_error()) };
    let envelope: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        envelope["error"]["kind"],
        serde_json::json!("ValidationError")
    );

    let workspace = c("ws-ffi");
    let handle = unsafe { forge_ffi::forge_core_open_in_memory(workspace.as_ptr()) };
    assert!(!handle.is_null());
    assert!(forge_ffi::forge_core_last_error().is_null());

    unsafe { forge_ffi::forge_core_close(handle) };
}

#[test]
fn null_command_json_returns_structured_response() {
    let workspace = c("ws-ffi");
    let handle = unsafe { forge_ffi::forge_core_open_in_memory(workspace.as_ptr()) };
    assert!(!handle.is_null());

    let json = unsafe { take_string(forge_ffi::forge_core_handle_command(handle, ptr::null())) };
    let response: CoreResponse = serde_json::from_str(&json).unwrap();
    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code(), "ValidationError");

    unsafe { forge_ffi::forge_core_close(handle) };
}

#[test]
fn string_free_accepts_null() {
    unsafe { forge_ffi::forge_string_free(ptr::null_mut()) };
}

#[test]
fn file_backed_open_round_trips_over_c_abi() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("forge-ffi-{}-{unique}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let path_c = c(path.to_str().unwrap());
    let workspace = c("ws-file-ffi");

    let handle = unsafe { forge_ffi::forge_core_open(path_c.as_ptr(), workspace.as_ptr()) };
    assert!(!handle.is_null());

    let cmd = c(&command("workspace.open", serde_json::json!({})));
    let json = unsafe { take_string(forge_ffi::forge_core_handle_command(handle, cmd.as_ptr())) };
    let response: CoreResponse = serde_json::from_str(&json).unwrap();
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(
        response.payload["workspace_id"],
        serde_json::json!("ws-file-ffi")
    );

    unsafe { forge_ffi::forge_core_close(handle) };
    let _ = std::fs::remove_file(path);
}

#[test]
fn install_run_and_drain_events_cross_the_c_abi() {
    let workspace = c("ws-ffi");
    let handle = unsafe { forge_ffi::forge_core_open_in_memory(workspace.as_ptr()) };
    assert!(!handle.is_null());

    let install = c(&command_with_applet(
        "applet.install",
        serde_json::json!({
            "manifest": demo_manifest(),
            "sources": { "src/main.ts": demo_ts() }
        }),
        Some("app_demo"),
    ));
    let install_json = unsafe {
        take_string(forge_ffi::forge_core_handle_command(
            handle,
            install.as_ptr(),
        ))
    };
    let install_response: CoreResponse = serde_json::from_str(&install_json).unwrap();
    assert!(install_response.ok, "{:?}", install_response.error);

    let run = c(&command_with_applet(
        "runtime.run",
        serde_json::json!({ "input": { "title": "Buy milk" } }),
        Some("app_demo"),
    ));
    let run_json =
        unsafe { take_string(forge_ffi::forge_core_handle_command(handle, run.as_ptr())) };
    let run_response: CoreResponse = serde_json::from_str(&run_json).unwrap();
    assert!(run_response.ok, "{:?}", run_response.error);
    assert_eq!(run_response.payload["ok"], serde_json::json!(true));
    assert_eq!(
        run_response.payload["result"]["value"]["id"],
        serde_json::json!("tasks/1")
    );

    let events_json = unsafe { take_string(forge_ffi::forge_core_drain_events(handle)) };
    let envelope: serde_json::Value = serde_json::from_str(&events_json).unwrap();
    assert_eq!(envelope["ok"], serde_json::json!(true));
    let kinds: Vec<&str> = envelope["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"applet.installed"));
    assert!(kinds.contains(&"run.started"));
    assert!(kinds.contains(&"ui.patch"));
    assert!(kinds.contains(&"run.completed"));

    let ui_patch = envelope["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == serde_json::json!("ui.patch"))
        .expect("ui.patch event should be emitted");
    assert!(ui_patch["payload"]["tree"].to_string().contains("Buy milk"));

    unsafe { forge_ffi::forge_core_close(handle) };
}

#[test]
fn checked_in_c_header_declares_the_exported_abi() {
    let header =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/include/forge_ffi.h"))
            .unwrap();

    for prototype in [
        "typedef struct ForgeCoreHandle ForgeCoreHandle;",
        "ForgeCoreHandle *forge_core_open(const char *path, const char *workspace_id);",
        "ForgeCoreHandle *forge_core_open_in_memory(const char *workspace_id);",
        "char *forge_core_handle_command(ForgeCoreHandle *handle, const char *command_json);",
        "char *forge_core_drain_events(ForgeCoreHandle *handle);",
        "char *forge_core_last_error(void);",
        "void forge_core_close(ForgeCoreHandle *handle);",
        "void forge_string_free(char *value);",
    ] {
        assert!(header.contains(prototype), "header missing {prototype}");
    }
}
