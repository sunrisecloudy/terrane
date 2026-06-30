use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_core::Core;

use crate::helpers::{grant_resource, req};

fn pack_wat() -> &'static str {
    r#"
    (func $pack (param $ptr i32) (param $len i32) (result i64)
      local.get $ptr
      i64.extend_i32_u
      i64.const 32
      i64.shl
      local.get $len
      i64.extend_i32_u
      i64.or)
    "#
}

fn alloc_wat() -> &'static str {
    r#"
    (global $heap (mut i32) (i32.const 1024))
    (func (export "alloc") (param $len i32) (result i32)
      (local $ptr i32)
      global.get $heap
      local.set $ptr
      global.get $heap
      local.get $len
      i32.add
      global.set $heap
      local.get $ptr)
    "#
}

fn wasm_write_kv() -> Vec<u8> {
    wat::parse_str(format!(
        r#"
        (module
          (import "terrane" "resource_write" (func $resource_write (param i64 i64 i64) (result i32)))
          (memory (export "memory") 1)
          (data (i32.const 16) "kv")
          (data (i32.const 32) "set")
          (data (i32.const 48) "[\"color\",\"blue\"]")
          (data (i32.const 80) "ok")
          {}
          {}
          (func (export "handle") (param $ptr i32) (param $len i32) (result i64)
            i32.const 16
            i32.const 2
            call $pack
            i32.const 32
            i32.const 3
            call $pack
            i32.const 48
            i32.const 16
            call $pack
            call $resource_write
            drop
            i32.const 80
            i32.const 2
            call $pack))
        "#,
        alloc_wat(),
        pack_wat()
    ))
    .unwrap()
}

fn wasm_read_kv() -> Vec<u8> {
    wat::parse_str(format!(
        r#"
        (module
          (import "terrane" "resource_read" (func $resource_read (param i64 i64 i64 i64) (result i32)))
          (memory (export "memory") 1)
          (data (i32.const 16) "kv")
          (data (i32.const 32) "get")
          (data (i32.const 48) "[\"color\"]")
          {}
          {}
          (func (export "handle") (param $ptr i32) (param $len i32) (result i64)
            (local $out_len i32)
            i32.const 16
            i32.const 2
            call $pack
            i32.const 32
            i32.const 3
            call $pack
            i32.const 48
            i32.const 9
            call $pack
            i32.const 128
            i32.const 64
            call $pack
            call $resource_read
            local.set $out_len
            i32.const 128
            local.get $out_len
            call $pack))
        "#,
        alloc_wat(),
        pack_wat()
    ))
    .unwrap()
}

fn write_wasm_bundle(dir: &Path, id: &str, module: Vec<u8>, resources: &[&str]) -> String {
    let bundle = dir.join(id);
    fs::create_dir(&bundle).unwrap();
    fs::write(bundle.join("main.wasm"), module).unwrap();
    let resources_json = resources
        .iter()
        .map(|resource| format!(r#""{resource}""#))
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        bundle.join("manifest.json"),
        format!(
            r#"{{
              "id": "{id}",
              "name": "WASM Test",
              "runtime": "wasm",
              "module": "main.wasm",
              "entry": "handle",
              "resources": [{resources_json}]
            }}"#
        ),
    )
    .unwrap();
    bundle.to_str().unwrap().to_string()
}

#[test]
fn wasm_runtime_records_kv_writes_and_replays() {
    let dir = tempdir().unwrap();
    let source = write_wasm_bundle(dir.path(), "wasm-write", wasm_write_kv(), &["kv"]);
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req(
        "app.add",
        &[
            "wasm-write",
            "WASM Write",
            "--source",
            &source,
            "--runtime",
            "wasm",
        ],
    ))
    .unwrap();
    grant_resource(&mut core, "wasm-write", "kv");

    let records = core
        .dispatch(req("wasm-runtime.run", &["wasm-write"]))
        .unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "kv.set");
    assert_eq!(core.take_last_output().as_deref(), Some("ok"));
    assert_eq!(core.state().kv.data["wasm-write"]["color"], "blue");
    assert!(core.replay_matches().unwrap());
}

#[test]
fn wasm_runtime_reads_real_kv_state() {
    let dir = tempdir().unwrap();
    let source = write_wasm_bundle(dir.path(), "wasm-read", wasm_read_kv(), &["kv"]);
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req(
        "app.add",
        &[
            "wasm-read",
            "WASM Read",
            "--source",
            &source,
            "--runtime",
            "wasm",
        ],
    ))
    .unwrap();
    grant_resource(&mut core, "wasm-read", "kv");
    core.dispatch(req("kv.set", &["wasm-read", "color", "green"]))
        .unwrap();

    let records = core
        .dispatch(req("wasm-runtime.run", &["wasm-read"]))
        .unwrap();

    assert!(records.is_empty());
    assert_eq!(core.take_last_output().as_deref(), Some("\"green\""));
}

#[test]
fn wasm_runtime_enforces_manifest_resources() {
    let dir = tempdir().unwrap();
    let source = write_wasm_bundle(dir.path(), "wasm-denied", wasm_write_kv(), &[]);
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req(
        "app.add",
        &[
            "wasm-denied",
            "WASM Denied",
            "--source",
            &source,
            "--runtime",
            "wasm",
        ],
    ))
    .unwrap();

    let err = core
        .dispatch(req("wasm-runtime.run", &["wasm-denied"]))
        .unwrap_err();

    assert!(err.to_string().contains("error while executing"), "{err}");
    assert!(!core.state().kv.data.contains_key("wasm-denied"));
}
