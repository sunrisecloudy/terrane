use std::ffi::{c_char, CStr, CString};
use std::ptr;

use tempfile::tempdir;
use terrane_host::ffi::{
    terrane_close, terrane_mcp_handle_json_rpc, terrane_open, terrane_string_free, TERRANE_OK,
};

unsafe fn take_string(raw: *mut c_char) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    let value = CStr::from_ptr(raw).to_string_lossy().into_owned();
    terrane_string_free(raw);
    Some(value)
}

unsafe fn mcp(handle: *mut terrane_host::ffi::TerraneHandle, raw: &str) -> (i32, String, String) {
    let raw = CString::new(raw).unwrap();
    let admin = CString::new("http://127.0.0.1:4242").unwrap();
    let mut out = ptr::null_mut();
    let mut err = ptr::null_mut();
    let code =
        terrane_mcp_handle_json_rpc(handle, raw.as_ptr(), admin.as_ptr(), &mut out, &mut err);
    (
        code,
        take_string(out).unwrap_or_default(),
        take_string(err).unwrap_or_default(),
    )
}

#[test]
fn native_handle_serves_mcp_from_its_live_core() {
    let home = tempdir().unwrap();
    let home = CString::new(home.path().to_string_lossy().as_bytes()).unwrap();
    unsafe {
        let handle = terrane_open(home.as_ptr());
        assert!(!handle.is_null());

        let (code, initialize, error) = mcp(
            handle,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"native-test","version":"1"}}}"#,
        );
        assert_eq!(code, TERRANE_OK, "{error}");
        assert!(initialize.contains(r#""name":"terrane-mcp""#));

        let (code, notification, error) = mcp(
            handle,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );
        assert_eq!(code, TERRANE_OK, "{error}");
        assert!(notification.is_empty());

        let (code, apps, error) = mcp(
            handle,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}}"#,
        );
        assert_eq!(code, TERRANE_OK, "{error}");
        assert!(apps.contains(r#""apps":[]"#), "{apps}");

        terrane_close(handle);
    }
}
