//! End-to-end tests for the i18n edge importer: it walks real catalog files on
//! disk, merges them deterministically, and seeds the public KV bucket via one
//! trusted-host `kv.public.import`. No mocks — these open a real core in a
//! throwaway `$TERRANE_HOME` and assert against folded state + the sqlite
//! projection.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use rusqlite::OptionalExtension;
use tempfile::tempdir;
use terrane_cap_kv::PUBLIC_BUCKET_APP_ID;
use terrane_host::{import_i18n_dir, open_at_home};

/// Write a flat `{key: value}` JSON object to `path`.
fn write_catalog(path: &Path, pairs: &[(&str, &str)]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let map: BTreeMap<&str, &str> = pairs.iter().copied().collect();
    let json = serde_json::to_string(&map).unwrap();
    fs::write(path, json).unwrap();
}

fn public_value(home: &Path, key: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(home.join("terrane.db")).unwrap();
    conn.query_row(
        "SELECT value FROM kv_entries WHERE app = ?1 AND key = ?2",
        [PUBLIC_BUCKET_APP_ID, key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .unwrap()
}

#[test]
fn import_seeds_public_bucket_and_projects_to_sqlite() {
    let home = tempdir().unwrap();
    let root = tempdir().unwrap();

    write_catalog(
        &root.path().join("i18n/system/en.json"),
        &[("menu.file", "File"), ("menu.edit", "Edit")],
    );
    write_catalog(
        &root.path().join("i18n/system/es.json"),
        &[("menu.file", "Archivo"), ("menu.edit", "Editar")],
    );
    write_catalog(
        &root.path().join("apps/todo/i18n/en.json"),
        &[("added", "added #{id} {text}")],
    );
    write_catalog(
        &root.path().join("apps/todo/i18n/es.json"),
        &[("added", "añadido #{id} {text}")],
    );

    let mut core = open_at_home(home.path()).unwrap();
    let outcome = import_i18n_dir(&mut core, root.path()).unwrap();

    // 2 system keys * 2 languages + 1 todo key * 2 languages = 6 entries.
    assert_eq!(outcome.entries, 6);
    assert_eq!(outcome.languages, 2);
    assert_eq!(outcome.domains, 2);

    // Folded state carries the public keys with the i18n/<code>/<domain>.<key> shape.
    let bucket = &core.state().kv.data[PUBLIC_BUCKET_APP_ID];
    assert_eq!(bucket["i18n/en/system.menu.file"], "File");
    assert_eq!(bucket["i18n/es/system.menu.file"], "Archivo");
    assert_eq!(bucket["i18n/en/todo.added"], "added #{id} {text}");
    assert_eq!(bucket["i18n/es/todo.added"], "añadido #{id} {text}");

    // The default sqlite projection reflects the same rows.
    assert_eq!(
        public_value(home.path(), "i18n/es/todo.added"),
        Some("añadido #{id} {text}".into())
    );

    assert!(core.replay_matches().unwrap());
}

#[test]
fn import_is_idempotent_and_deterministic() {
    let root = tempdir().unwrap();
    write_catalog(&root.path().join("i18n/system/en.json"), &[("a", "1")]);
    write_catalog(&root.path().join("i18n/system/de.json"), &[("a", "eins")]);

    // Two separate homes, identical import → identical public state.
    let home_a = tempdir().unwrap();
    let mut core_a = open_at_home(home_a.path()).unwrap();
    let outcome_a = import_i18n_dir(&mut core_a, root.path()).unwrap();

    let home_b = tempdir().unwrap();
    let mut core_b = open_at_home(home_b.path()).unwrap();
    let outcome_b = import_i18n_dir(&mut core_b, root.path()).unwrap();

    assert_eq!(outcome_a, outcome_b);
    assert_eq!(core_a.state().kv.data[PUBLIC_BUCKET_APP_ID], core_b.state().kv.data[PUBLIC_BUCKET_APP_ID]);

    // Re-importing the same root into the same core keeps state identical.
    let before = core_a.state().kv.data[PUBLIC_BUCKET_APP_ID].clone();
    import_i18n_dir(&mut core_a, root.path()).unwrap();
    assert_eq!(core_a.state().kv.data[PUBLIC_BUCKET_APP_ID], before);
}

#[test]
fn import_keys_are_sorted_within_the_committed_event_batch() {
    let home = tempdir().unwrap();
    let root = tempdir().unwrap();
    // Deliberately unsorted keys across files; the merged payload must be sorted.
    write_catalog(&root.path().join("i18n/system/en.json"), &[("zebra", "z"), ("alpha", "a")]);
    write_catalog(&root.path().join("apps/beta/i18n/en.json"), &[("mango", "m")]);

    let mut core = open_at_home(home.path()).unwrap();
    import_i18n_dir(&mut core, root.path()).unwrap();

    // BTreeMap-backed state iterates sorted; verify the keys come out ordered.
    let keys: Vec<&String> = core
        .state()
        .kv
        .data[PUBLIC_BUCKET_APP_ID]
        .keys()
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "public bucket keys must be stored in sorted order");
}

#[test]
fn import_rejects_unsupported_language_code() {
    let home = tempdir().unwrap();
    let root = tempdir().unwrap();
    write_catalog(&root.path().join("i18n/system/klingon.json"), &[("a", "1")]);

    let mut core = open_at_home(home.path()).unwrap();
    let err = import_i18n_dir(&mut core, root.path()).unwrap_err();
    assert!(
        err.contains("unsupported language code"),
        "expected unsupported-code error, got: {err}"
    );
    // Nothing was committed.
    assert!(core
        .state()
        .kv
        .data
        .get(PUBLIC_BUCKET_APP_ID)
        .is_none_or(|m| m.is_empty()));
}

#[test]
fn import_rejects_non_string_values() {
    let home = tempdir().unwrap();
    let root = tempdir().unwrap();
    fs::create_dir_all(root.path().join("i18n/system")).unwrap();
    // A number value is not a flat string map.
    fs::write(
        root.path().join("i18n/system/en.json"),
        r#"{"count":1}"#,
    )
    .unwrap();

    let mut core = open_at_home(home.path()).unwrap();
    let err = import_i18n_dir(&mut core, root.path()).unwrap_err();
    assert!(
        err.contains("must be a string"),
        "expected non-string-value error, got: {err}"
    );
}

#[test]
fn import_errors_when_no_catalogs_found() {
    let home = tempdir().unwrap();
    let root = tempdir().unwrap();

    let mut core = open_at_home(home.path()).unwrap();
    let err = import_i18n_dir(&mut core, root.path()).unwrap_err();
    assert!(
        err.contains("no i18n catalogs found"),
        "expected empty-catalog error, got: {err}"
    );
}

#[test]
fn import_canonicalizes_language_code_casing() {
    let home = tempdir().unwrap();
    let root = tempdir().unwrap();
    // Lowercase filename; the public key must use the canonical spelling.
    write_catalog(&root.path().join("i18n/system/zh-hans.json"), &[("hi", "你好")]);

    let mut core = open_at_home(home.path()).unwrap();
    import_i18n_dir(&mut core, root.path()).unwrap();

    let bucket = &core.state().kv.data[PUBLIC_BUCKET_APP_ID];
    assert!(
        bucket.contains_key("i18n/zh-Hans/system.hi"),
        "public key must use canonical code spelling; got keys: {:?}",
        bucket.keys().collect::<Vec<_>>()
    );
    assert_eq!(bucket["i18n/zh-Hans/system.hi"], "你好");
}

#[test]
fn imported_strings_readable_through_public_resource_read() {
    use terrane_core::{ExecutionPrincipal, ReadValue, RuntimeHost};
    use terrane_core::RuntimeResourceHost;

    let home = tempdir().unwrap();
    let root = tempdir().unwrap();
    write_catalog(
        &root.path().join("i18n/system/en.json"),
        &[("menu.file", "File")],
    );
    write_catalog(
        &root.path().join("apps/todo/i18n/en.json"),
        &[("added", "added")],
    );

    let mut core = open_at_home(home.path()).unwrap();
    import_i18n_dir(&mut core, root.path()).unwrap();

    // An app backend with a kv grant reads the seeded strings back.
    let mut host = RuntimeResourceHost::new_with_temporary_resource_grants(
        "todo",
        core.state().clone(),
        ExecutionPrincipal::local_owner(),
        vec!["kv".to_string()],
    );
    assert_eq!(
        host.read_resource("kv", "public", &["i18n/en/system.menu.file".to_string()])
            .unwrap(),
        ReadValue::OptString(Some("File".into()))
    );
    assert_eq!(
        host.read_resource("kv", "public", &["i18n/en/todo.added".to_string()])
            .unwrap(),
        ReadValue::OptString(Some("added".into()))
    );
    // publicAll returns the whole seeded bucket.
    let all = host.read_resource("kv", "publicAll", &[]).unwrap();
    let ReadValue::StringMap(map) = all else {
        panic!("expected StringMap, got {all:?}");
    };
    assert_eq!(map.len(), 2);
}

#[test]
fn i18n_negotiate_cli_resolves_accept_language() {
    use crate::helpers::terrane;
    let home = tempdir().unwrap();
    let (ok, out, err) = terrane(home.path(), &["i18n", "negotiate", "fr-CH, en;q=0.8"]);
    assert!(ok, "stderr: {err}");
    assert_eq!(out.trim(), "fr");
}

#[test]
fn i18n_import_cli_seeds_public_bucket() {
    use crate::helpers::terrane;
    let home = tempdir().unwrap();
    let root = tempdir().unwrap();
    write_catalog(
        &root.path().join("i18n/system/en.json"),
        &[("menu.file", "File")],
    );

    let (ok, out, err) = terrane(
        home.path(),
        &["i18n", "import", root.path().to_str().unwrap()],
    );
    assert!(ok, "i18n import failed: {err}");
    assert!(out.contains("imported"), "out: {out}");

    // The seeded string lands in the default sqlite projection.
    assert_eq!(
        public_value(home.path(), "i18n/en/system.menu.file"),
        Some("File".into())
    );
}

// ---- C ABI (FFI) ------------------------------------------------------------

use std::ffi::{CStr, CString};
use std::ptr;
use terrane_host::ffi;

/// Take a `char*` produced by the FFI back to a Rust `String`, then free it.
unsafe fn take_string(p: *mut std::os::raw::c_char) -> String {
    let s = CStr::from_ptr(p).to_string_lossy().into_owned();
    ffi::terrane_string_free(p);
    s
}

#[test]
fn ffi_i18n_negotiate_resolves_header() {
    let header = CString::new("fr-CH, en;q=0.8").unwrap();
    let mut out = ptr::null_mut();
    let mut err = ptr::null_mut();
    let code = unsafe { ffi::terrane_i18n_negotiate(header.as_ptr(), &mut out, &mut err) };
    assert_eq!(code, ffi::TERRANE_OK, "stderr: {}", unsafe {
        take_string(err)
    });
    assert_eq!(unsafe { take_string(out) }, "fr");
}

#[test]
fn ffi_i18n_negotiate_canonicalizes_casing() {
    let header = CString::new("zh-cn").unwrap();
    let mut out = ptr::null_mut();
    let mut err = ptr::null_mut();
    let code = unsafe { ffi::terrane_i18n_negotiate(header.as_ptr(), &mut out, &mut err) };
    assert_eq!(code, ffi::TERRANE_OK);
    assert_eq!(unsafe { take_string(out) }, "zh-Hans");
}

#[test]
fn ffi_i18n_supported_returns_json_array() {
    let mut out = ptr::null_mut();
    let mut err = ptr::null_mut();
    let code = unsafe { ffi::terrane_i18n_supported(&mut out, &mut err) };
    assert_eq!(code, ffi::TERRANE_OK, "stderr: {}", unsafe {
        take_string(err)
    });
    let json = unsafe { take_string(out) };
    let arr: Vec<String> = serde_json::from_str(&json).unwrap();
    assert!(arr.contains(&"en".to_string()));
    assert!(arr.contains(&"zh-Hans".to_string()));
    assert!(arr.contains(&"pt-BR".to_string()));
}

#[test]
fn ffi_i18n_import_seeds_public_bucket_via_handle() {
    let home = tempdir().unwrap();
    let root = tempdir().unwrap();
    write_catalog(
        &root.path().join("i18n/system/en.json"),
        &[("menu.file", "File")],
    );
    write_catalog(
        &root.path().join("apps/todo/i18n/en.json"),
        &[("added", "added")],
    );

    let home_c = CString::new(home.path().to_str().unwrap()).unwrap();
    let handle = unsafe { ffi::terrane_open(home_c.as_ptr()) };
    assert!(!handle.is_null(), "terrane_open should succeed");

    let path_c = CString::new(root.path().to_str().unwrap()).unwrap();
    let mut out = ptr::null_mut();
    let mut err = ptr::null_mut();
    let code =
        unsafe { ffi::terrane_i18n_import(handle, path_c.as_ptr(), &mut out, &mut err) };
    assert_eq!(code, ffi::TERRANE_OK, "stderr: {}", unsafe {
        take_string(err)
    });
    let message = unsafe { take_string(out) };
    assert!(message.contains("imported"), "out: {message}");

    // The seeded strings are readable through the workspace, proving the import
    // went through the real core commit path.
    assert_eq!(
        public_value(home.path(), "i18n/en/system.menu.file"),
        Some("File".into())
    );
    assert_eq!(
        public_value(home.path(), "i18n/en/todo.added"),
        Some("added".into())
    );

    unsafe { ffi::terrane_close(handle) };
}
