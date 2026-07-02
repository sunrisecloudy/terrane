//! Exercise the C ABI in-process (the crate's `rlib` output makes the extern
//! fns callable from Rust). No GUI needed — this is the keystone proof.

use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::path::Path;
use std::ptr;

use tempfile::tempdir;
use terrane_core::{read_log, Core};
use terrane_host::ffi::*;

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

/// A backend using the CRDT resource, so FFI runtime invocation exercises stable
/// replica authoring rather than plain kv storage.
const CRDT_BACKEND: &str = r#"
var crdt = ctx.resource.crdt;
function handle(input) {
    var verb = input[0];
    if (verb === "set") { crdt.mapSet("prefs", input[1], input[2]); return "ok"; }
    if (verb === "get") {
        var v = crdt.mapGet("prefs", input[1]);
        return v == null ? "(none)" : v;
    }
    if (verb === "push") { crdt.listPush("todo", input[1]); return "" + crdt.listAll("todo").length; }
    if (verb === "list") { return crdt.listAll("todo").join(","); }
    return "?";
}
"#;

fn write_bundle(dir: &Path) -> String {
    let bundle = dir.join("bundle");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "demo", "name":"Demo","runtime":"js","backend":"main.js", "resources": ["kv"] }"#,
    )
    .unwrap();
    fs::write(bundle.join("main.js"), BACKEND).unwrap();
    bundle.to_str().unwrap().to_string()
}

fn write_crdt_bundle(dir: &Path) -> String {
    let bundle = dir.join("crdt-bundle");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "crdt_demo", "name":"CRDT Demo","runtime":"js","backend":"main.js", "resources": ["crdt"] }"#,
    )
    .unwrap();
    fs::write(bundle.join("main.js"), CRDT_BACKEND).unwrap();
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
    let code = f(
        h,
        head_c.as_ptr(),
        argv.len(),
        argv.as_ptr(),
        &mut out,
        &mut err,
    );
    (code, take_c_string(out), take_c_string(err))
}

unsafe fn take_c_string(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        let s = CStr::from_ptr(p).to_str().unwrap().to_string();
        terrane_string_free(p);
        Some(s)
    }
}

unsafe fn call_preview_create(
    h: *mut TerraneHandle,
    files_json: &str,
) -> (i32, Option<String>, Option<String>) {
    let files = CString::new(files_json).unwrap();
    let mut out: *mut c_char = ptr::null_mut();
    let mut err: *mut c_char = ptr::null_mut();
    let code = terrane_preview_create(h, files.as_ptr(), &mut out, &mut err);
    (code, take_c_string(out), take_c_string(err))
}

unsafe fn call_preview_read_asset(
    h: *mut TerraneHandle,
    id: &str,
    path: &str,
) -> (i32, Option<String>, Option<String>) {
    let id = CString::new(id).unwrap();
    let path = CString::new(path).unwrap();
    let mut out: *mut c_char = ptr::null_mut();
    let mut err: *mut c_char = ptr::null_mut();
    let code = terrane_preview_asset(h, id.as_ptr(), path.as_ptr(), &mut out, &mut err);
    (code, take_c_string(out), take_c_string(err))
}

unsafe fn call_preview_invoke(
    h: *mut TerraneHandle,
    id: &str,
    verb: &str,
    args: &[&str],
) -> (i32, Option<String>, Option<String>) {
    let id = CString::new(id).unwrap();
    let verb = CString::new(verb).unwrap();
    let arg_cs: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();
    let argv: Vec<*const c_char> = arg_cs.iter().map(|c| c.as_ptr()).collect();
    let mut out: *mut c_char = ptr::null_mut();
    let mut err: *mut c_char = ptr::null_mut();
    let code = terrane_preview_invoke(
        h,
        id.as_ptr(),
        verb.as_ptr(),
        argv.len(),
        argv.as_ptr(),
        &mut out,
        &mut err,
    );
    (code, take_c_string(out), take_c_string(err))
}

unsafe fn call_build_app(app_dir: &Path) -> (i32, Option<String>, Option<String>) {
    let app_dir = CString::new(app_dir.to_str().unwrap()).unwrap();
    let mut out: *mut c_char = ptr::null_mut();
    let mut err: *mut c_char = ptr::null_mut();
    let code = terrane_build_app(app_dir.as_ptr(), &mut out, &mut err);
    (code, take_c_string(out), take_c_string(err))
}

unsafe fn call_builder_generate(
    h: *mut TerraneHandle,
    app_id: &str,
    name: &str,
    prompt: &str,
) -> (i32, Option<String>, Option<String>) {
    let app_id = CString::new(app_id).unwrap();
    let name = CString::new(name).unwrap();
    let prompt = CString::new(prompt).unwrap();
    let harness = CString::new("codex").unwrap();
    let mut out: *mut c_char = ptr::null_mut();
    let mut err: *mut c_char = ptr::null_mut();
    let code = terrane_builder_generate(
        h,
        app_id.as_ptr(),
        name.as_ptr(),
        prompt.as_ptr(),
        harness.as_ptr(),
        &mut out,
        &mut err,
    );
    (code, take_c_string(out), take_c_string(err))
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
        let (code, _, err) = call(
            terrane_dispatch,
            h,
            "auth.grant",
            &["user:local-owner", "demo", "kv"],
        );
        assert_eq!(code, TERRANE_OK, "auth.grant err: {err:?}");

        // Write via the backend, then read items back as JSON.
        let (code, out, err) = call(terrane_host_run, h, "demo", &["set", "x", "1"]);
        assert_eq!(code, TERRANE_OK, "host_run set err: {err:?}");
        assert_eq!(out.as_deref(), Some("ok"));

        let (code, out, _) = call(terrane_host_run, h, "demo", &["items"]);
        assert_eq!(code, TERRANE_OK);
        let json = out.unwrap();
        assert!(
            json.contains("\"key\":\"x\"") && json.contains("\"value\":\"1\""),
            "json: {json}"
        );

        terrane_close(h);
    }
}

#[test]
fn open_mints_stable_replica_identity_for_crdt_host_writes() {
    let dir = tempdir().unwrap();
    let src = write_crdt_bundle(dir.path());
    let home = CString::new(dir.path().to_str().unwrap()).unwrap();
    let log = dir.path().join("log.bin");

    unsafe {
        let h = terrane_open(home.as_ptr());
        assert!(!h.is_null(), "open should succeed");

        let reopened = Core::open(&log).unwrap();
        let peer = reopened
            .state()
            .replica
            .peer
            .expect("terrane_open should mint replica identity");
        assert_eq!(
            read_log(&log)
                .unwrap()
                .iter()
                .filter(|record| record.kind == "replica.initialized")
                .count(),
            1,
            "identity should be recorded exactly once"
        );

        let (code, out, err) = call(
            terrane_dispatch,
            h,
            "app.add",
            &["crdt_demo", "CRDT Demo", "--source", &src],
        );
        assert_eq!(code, TERRANE_OK, "app.add err: {err:?}");
        assert_eq!(out.as_deref(), Some("app.added"));
        let (code, _, err) = call(
            terrane_dispatch,
            h,
            "auth.grant",
            &["user:local-owner", "crdt_demo", "crdt"],
        );
        assert_eq!(code, TERRANE_OK, "auth.grant err: {err:?}");

        let (code, out, err) = call(terrane_host_run, h, "crdt_demo", &["set", "theme", "dark"]);
        assert_eq!(code, TERRANE_OK, "host_run set err: {err:?}");
        assert_eq!(out.as_deref(), Some("ok"));

        let (code, out, err) = call(terrane_host_run, h, "crdt_demo", &["push", "a"]);
        assert_eq!(code, TERRANE_OK, "host_run push a err: {err:?}");
        assert_eq!(out.as_deref(), Some("1"));
        let (code, out, err) = call(terrane_host_run, h, "crdt_demo", &["push", "b"]);
        assert_eq!(code, TERRANE_OK, "host_run push b err: {err:?}");
        assert_eq!(out.as_deref(), Some("2"));

        terrane_close(h);

        let reopened = Core::open(&log).unwrap();
        assert_eq!(
            reopened.state().replica.peer,
            Some(peer),
            "peer should persist"
        );
        assert!(reopened.state().crdt.docs.contains_key("crdt_demo"));
        assert!(reopened.replay_matches().unwrap());

        let h = terrane_open(home.as_ptr());
        assert!(!h.is_null(), "reopen should succeed");
        let reopened_again = Core::open(&log).unwrap();
        assert_eq!(
            reopened_again.state().replica.peer,
            Some(peer),
            "reopen must reuse the same home peer"
        );
        assert_eq!(
            read_log(&log)
                .unwrap()
                .iter()
                .filter(|record| record.kind == "replica.initialized")
                .count(),
            1,
            "reopen should not append a second identity event"
        );

        let (code, out, err) = call(terrane_host_run, h, "crdt_demo", &["get", "theme"]);
        assert_eq!(code, TERRANE_OK, "host_run get err: {err:?}");
        assert_eq!(out.as_deref(), Some("dark"));
        let (code, out, err) = call(terrane_host_run, h, "crdt_demo", &["list"]);
        assert_eq!(code, TERRANE_OK, "host_run list err: {err:?}");
        assert_eq!(out.as_deref(), Some("a,b"));

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
            err.as_deref().unwrap_or_default().contains("no such app"),
            "err: {err:?}"
        );
        terrane_close(h);
    }
}

#[test]
fn app_builder_preview_abi_stays_in_memory() {
    let dir = tempdir().unwrap();
    let home = CString::new(dir.path().to_str().unwrap()).unwrap();
    let log = dir.path().join("log.bin");
    let files_json = r#"{"files":[{"path":"manifest.json","content":"{\"id\":\"demo\",\"name\":\"Demo\",\"runtime\":\"js\",\"ui\":\"ui/index.html\",\"backend\":\"main.js\",\"resources\":[\"kv\"]}"},{"path":"ui/index.html","content":"<script src=\"client.js\"></script>"},{"path":"ui/client.js","content":"console.log(\"hi\")"},{"path":"main.js","content":"var kv=ctx.resource.kv;function handle(input){if(input[0]===\"set\"){kv.set(input[1],input[2]);return \"ok\";}if(input[0]===\"get\"){var v=kv.get(input[1]);return v==null?\"(none)\":v;}return \"?\";}"}]}"#;

    unsafe {
        let h = terrane_open(home.as_ptr());
        assert!(!h.is_null(), "open should succeed");
        let before = read_log(&log).unwrap();

        let (code, out, err) = call_preview_create(h, files_json);
        assert_eq!(code, TERRANE_OK, "preview create err: {err:?}");
        let json = out.unwrap();
        assert!(json.contains(r#""id":"preview-demo-1""#), "json: {json}");
        assert!(
            json.contains(r#""frameUrl":"terrane-preview://preview-demo-1/frame/""#),
            "json: {json}"
        );

        let (code, out, err) = call_preview_read_asset(h, "preview-demo-1", "");
        assert_eq!(code, TERRANE_OK, "preview asset err: {err:?}");
        let asset = out.unwrap();
        assert!(
            asset.contains(r#""contentType":"text/html; charset=utf-8""#),
            "asset: {asset}"
        );
        assert!(asset.contains("client.js"), "asset: {asset}");

        let (code, out, err) = call_preview_invoke(h, "preview-demo-1", "set", &["answer", "42"]);
        assert_ne!(
            code, TERRANE_OK,
            "resource-using previews should be default-deny through raw ABI"
        );
        assert!(out.is_none(), "no output on denied preview invoke");
        assert!(
            err.as_deref()
                .unwrap_or_default()
                .contains("cannot read property"),
            "preview set should fail because ctx.resource.kv is unavailable: {err:?}"
        );

        let after = read_log(&log).unwrap();
        assert_eq!(after, before, "preview must not append to the real log");
        assert!(
            after
                .iter()
                .all(|r| r.kind != "app.added" && r.kind != "kv.set"),
            "preview must not install or persist writes: {after:?}"
        );

        terrane_close(h);
    }
}

#[test]
fn builder_generate_rejects_invalid_request_before_harness() {
    let dir = tempdir().unwrap();
    let home = CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        let h = terrane_open(home.as_ptr());
        assert!(!h.is_null(), "open should succeed");

        let (code, out, err) = call_builder_generate(h, "bad/path", "Demo", "make a greeting app");
        assert_eq!(code, TERRANE_ERR_DISPATCH);
        assert!(out.is_none());
        assert!(
            err.as_deref().unwrap_or_default().contains("unsafe"),
            "builder_generate err: {err:?}"
        );

        terrane_close(h);
    }

    let reopened = Core::open(dir.path().join("log.bin")).unwrap();
    assert!(
        reopened.state().builder.drafts.is_empty(),
        "invalid builder request must fail before recording a draft"
    );
    assert!(reopened.replay_matches().unwrap());
}

#[test]
fn app_build_abi_builds_react_frontend_to_dist() {
    let dir = tempdir().unwrap();
    let app = dir.path().join("bmi-lite");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("manifest.json"),
        r#"{
  "id": "bmi-lite",
  "name": "BMI Lite",
  "ui": "dist/index.html",
  "frontend": {
    "tool": "terrane-app-build",
    "entry": "src/main.tsx",
    "styles": ["src/app.css"]
  }
}
"#,
    )
    .unwrap();
    fs::write(app.join("src/app.css"), ".app { color: #123456; }\n").unwrap();
    fs::write(
        app.join("src/main.tsx"),
        r#"import { createRoot } from "react-dom/client";

type Props = { label: string };

function App(props: Props) {
  return <main className="app">{props.label}</main>;
}

createRoot(document.getElementById("root")!).render(<App label="BMI Lite" />);
"#,
    )
    .unwrap();

    unsafe {
        let (code, out, err) = call_build_app(&app);
        assert_eq!(code, TERRANE_OK, "build err: {err:?}");
        let out = out.unwrap();
        assert!(out.contains(r#""dist":"#), "build output: {out}");
        assert!(out.contains(r#""files":"#), "build output: {out}");
        assert!(app.join("dist/index.html").is_file());
        assert!(app.join("dist/assets/app.css").is_file());
        assert!(app.join("dist/assets/modules/src/main.js").is_file());
        assert!(app
            .join("dist/assets/terrane-react-jsx-runtime.js")
            .is_file());

        let module = fs::read_to_string(app.join("dist/assets/modules/src/main.js")).unwrap();
        assert!(
            module.contains("terrane-react-jsx-runtime")
                && module.contains("createRoot")
                && module.contains("BMI Lite"),
            "module: {module}"
        );
    }
}

#[test]
fn home_page_abi_renders_catalog_with_script_safe_escaping() {
    let catalog = r#"{"apps":[{"id":"todo","name":"Todo </script><i>","has_ui":true}]}"#;
    let catalog_c = CString::new(catalog).unwrap();
    let template_c = CString::new("terrane-app://{id}/frame/").unwrap();

    unsafe {
        let mut out: *mut c_char = ptr::null_mut();
        let mut err: *mut c_char = ptr::null_mut();
        let code = terrane_home_page(catalog_c.as_ptr(), template_c.as_ptr(), &mut out, &mut err);
        assert_eq!(code, TERRANE_OK, "err: {:?}", take_c_string(err));
        let html = take_c_string(out).unwrap();
        assert!(html.contains("<h1>Terrane</h1>"), "brand missing: {html}");
        assert!(
            html.contains(r#""appHref":"terrane-app://{id}/frame/""#),
            "app href template missing: {html}"
        );
        assert!(
            html.contains(r"Todo \u003c/script>") && !html.contains("Todo </script>"),
            "catalog must not close the config script element: {html}"
        );

        // Null args → typed error, no crash.
        let mut out: *mut c_char = ptr::null_mut();
        let mut err: *mut c_char = ptr::null_mut();
        let code = terrane_home_page(ptr::null(), template_c.as_ptr(), &mut out, &mut err);
        assert_eq!(code, TERRANE_ERR_NULL_ARG);
        assert!(out.is_null());
    }
}

#[test]
fn null_safety_and_single_use_contracts() {
    unsafe {
        // Null handle → typed error, no crash.
        let mut out: *mut c_char = ptr::null_mut();
        let mut err: *mut c_char = ptr::null_mut();
        let app = CString::new("demo").unwrap();
        let code = terrane_host_run(
            ptr::null_mut(),
            app.as_ptr(),
            0,
            ptr::null(),
            &mut out,
            &mut err,
        );
        assert_eq!(code, TERRANE_ERR_NULL_ARG);

        // Free/close are null-safe no-ops. Non-null pointers are single-use.
        terrane_string_free(ptr::null_mut());
        terrane_close(ptr::null_mut());

        // Null app arg → typed error.
        let dir = tempdir().unwrap();
        let home = CString::new(dir.path().to_str().unwrap()).unwrap();
        let h = terrane_open(home.as_ptr());
        assert!(!h.is_null());
        let code = terrane_host_run(h, ptr::null(), 0, ptr::null(), &mut out, &mut err);
        assert_eq!(code, TERRANE_ERR_NULL_ARG);
        terrane_close(h);
    }
}

#[test]
fn local_model_server_exports_report_and_stop_without_a_runtime() {
    unsafe {
        let dir = tempdir().unwrap();
        let home = CString::new(dir.path().to_str().unwrap()).unwrap();

        let mut out: *mut c_char = ptr::null_mut();
        let mut err: *mut c_char = ptr::null_mut();
        let code = terrane_local_model_server_status(home.as_ptr(), &mut out, &mut err);
        assert_eq!(code, TERRANE_OK);
        let json = CStr::from_ptr(out).to_str().unwrap();
        assert!(json.contains("\"running\":false"), "{json}");
        terrane_string_free(out);

        let mut out: *mut c_char = ptr::null_mut();
        let mut err: *mut c_char = ptr::null_mut();
        let code = terrane_local_model_server_stop(home.as_ptr(), &mut out, &mut err);
        assert_eq!(code, TERRANE_OK);
        let message = CStr::from_ptr(out).to_str().unwrap();
        assert!(message.contains("no resident"), "{message}");
        terrane_string_free(out);
    }
}

#[test]
fn checked_in_c_header_declares_the_exported_abi() {
    let header =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("include/terrane_host.h"))
            .expect("header exists");
    for needle in [
        "TerraneHandle *terrane_open(",
        "int terrane_host_run(",
        "int terrane_dispatch(",
        "int terrane_preview_create(",
        "int terrane_preview_read_asset(",
        "int terrane_preview_asset(",
        "int terrane_preview_invoke(",
        "int terrane_builder_generate(",
        "int terrane_build_app(",
        "int terrane_home_page(",
        "int terrane_local_model_setup_mlx(",
        "int terrane_local_model_server_status(",
        "int terrane_local_model_server_stop(",
        "void terrane_local_model_shutdown(",
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
