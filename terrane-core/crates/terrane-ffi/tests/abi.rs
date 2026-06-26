//! Exercise the C ABI in-process (the crate's `rlib` output makes the extern
//! fns callable from Rust). No GUI needed — this is the keystone proof.

use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::path::Path;
use std::ptr;

use tempfile::tempdir;
use terrane_ffi::*;

/// A backend exposing `set` and an `items` verb that returns JSON.
const BACKEND: &str = r#"
var kv = ctx.resource.kv;
function handle(input) {
    if (input[0] === "set") { kv.set(input[1], input[2]); return "ok"; }
    if (input[0] === "items") {
        var a = kv.all(); var arr = [];
        for (var k in a) { arr.push({ key: k, value: a[k] }); }
        return JSON.stringify(arr);
    }
    return "?";
}
"#;

fn write_bundle(dir: &Path) -> String {
    let bundle = dir.join("bundle");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "demo", "name": "Demo", "backend": "main.js", "resources": ["kv"] }"#,
    )
    .unwrap();
    fs::write(bundle.join("main.js"), BACKEND).unwrap();
    bundle.to_str().unwrap().to_string()
}

/// Call an extern fn with a (name/app, args) tuple; return (code, output, error).
unsafe fn call(
    f: unsafe extern "C" fn(
        *mut TerraneHandle,
        *const c_char,
        usize,
        *const *const c_char,
        *mut *mut c_char,
        *mut *mut c_char,
    ) -> i32,
    h: *mut TerraneHandle,
    head: &str,
    args: &[&str],
) -> (i32, Option<String>, Option<String>) {
    let head_c = CString::new(head).unwrap();
    let arg_cs: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();
    let argv: Vec<*const c_char> = arg_cs.iter().map(|c| c.as_ptr()).collect();
    let mut out: *mut c_char = ptr::null_mut();
    let mut err: *mut c_char = ptr::null_mut();
    let code = f(h, head_c.as_ptr(), argv.len(), argv.as_ptr(), &mut out, &mut err);
    let take = |p: *mut c_char| -> Option<String> {
        if p.is_null() {
            None
        } else {
            let s = CStr::from_ptr(p).to_str().unwrap().to_string();
            terrane_string_free(p);
            Some(s)
        }
    };
    (code, take(out), take(err))
}

#[test]
fn open_host_run_output_free_round_trip() {
    let dir = tempdir().unwrap();
    let src = write_bundle(dir.path());
    let home = CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        let h = terrane_open(home.as_ptr());
        assert!(!h.is_null(), "open should succeed");

        // Register the app via the generic dispatch.
        let (code, out, err) = call(
            terrane_dispatch,
            h,
            "app.add",
            &["demo", "Demo", "--source", &src],
        );
        assert_eq!(code, TERRANE_OK, "app.add err: {err:?}");
        assert_eq!(out.as_deref(), Some("app.added"));

        // Write via the backend, then read items back as JSON.
        let (code, out, err) = call(terrane_host_run, h, "demo", &["set", "x", "1"]);
        assert_eq!(code, TERRANE_OK, "host_run set err: {err:?}");
        assert_eq!(out.as_deref(), Some("ok"));

        let (code, out, _) = call(terrane_host_run, h, "demo", &["items"]);
        assert_eq!(code, TERRANE_OK);
        let json = out.unwrap();
        assert!(json.contains("\"key\":\"x\"") && json.contains("\"value\":\"1\""), "json: {json}");

        terrane_close(h);
    }
}

#[test]
fn host_run_unknown_app_returns_error_not_panic() {
    let dir = tempdir().unwrap();
    let home = CString::new(dir.path().to_str().unwrap()).unwrap();
    unsafe {
        let h = terrane_open(home.as_ptr());
        let (code, out, err) = call(terrane_host_run, h, "ghost", &["items"]);
        assert_ne!(code, TERRANE_OK);
        assert!(out.is_none(), "no output on error");
        assert!(
            err.as_deref().unwrap_or_default().contains("app not found"),
            "err: {err:?}"
        );
        terrane_close(h);
    }
}

#[test]
fn null_safety_and_idempotent_frees() {
    unsafe {
        // Null handle → typed error, no crash.
        let mut out: *mut c_char = ptr::null_mut();
        let mut err: *mut c_char = ptr::null_mut();
        let app = CString::new("demo").unwrap();
        let code = terrane_host_run(ptr::null_mut(), app.as_ptr(), 0, ptr::null(), &mut out, &mut err);
        assert_eq!(code, TERRANE_ERR_NULL_ARG);

        // Free/close are null-safe no-ops.
        terrane_string_free(ptr::null_mut());
        terrane_close(ptr::null_mut());

        // Null app arg → typed error.
        let h = terrane_open(ptr::null());
        assert!(!h.is_null());
        let code = terrane_host_run(h, ptr::null(), 0, ptr::null(), &mut out, &mut err);
        assert_eq!(code, TERRANE_ERR_NULL_ARG);
        terrane_close(h);
    }
}

#[test]
fn checked_in_c_header_declares_the_exported_abi() {
    let header = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("include/terrane_ffi.h"),
    )
    .expect("header exists");
    for needle in [
        "TerraneHandle *terrane_open(",
        "int terrane_host_run(",
        "int terrane_dispatch(",
        "void terrane_string_free(",
        "void terrane_close(",
        "#define TERRANE_OK 0",
        "#define TERRANE_ERR_NULL_ARG 1",
        "#define TERRANE_ERR_UTF8 2",
        "#define TERRANE_ERR_DISPATCH 3",
        "#define TERRANE_ERR_PANIC 4",
        "#define TERRANE_ERR_INTERNAL 5",
    ] {
        assert!(header.contains(needle), "header missing: {needle}");
    }
}
