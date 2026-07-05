use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

fn write_receiver_bundle(dir: &std::path::Path) {
    fs::create_dir(dir).unwrap();
    fs::write(
        dir.join("manifest.json"),
        r#"{
          "id": "receiver",
          "name": "Receiver",
          "runtime": "js",
          "backend": "main.js",
          "resources": ["kv"],
          "interfaces": ["inbox", "items"],
          "fileTypes": [{ "ext": "txt", "mime": "text/plain" }]
        }"#,
    )
    .unwrap();
    fs::write(
        dir.join("main.js"),
        r#"
          var actions = {
            "common.receive": { run: function (args) {
              ctx.resource.kv.set("received/" + (args[0] || ""), args[1] || "");
              return JSON.stringify({ ok: true });
            }},
            "common.list": { run: function () { return JSON.stringify([]); } },
            "common.get": { run: function (args) {
              return JSON.stringify({ ok: false, error: { code: "NotFound", id: args[0] || "" } });
            }}
          };
        "#,
    )
    .unwrap();
}

#[test]
fn terrane_open_send_url_delivers_link_via_common_receive() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("receiver-bundle");
    write_receiver_bundle(&bundle);

    let (ok, out, err) = terrane(home, &["app", "install", bundle.to_str().unwrap()]);
    assert!(ok, "install failed: {out} {err}");

    let payload = "%7B%22text%22%3A%22hello%22%7D";
    let url = format!("terrane://send/receiver?kind=link&payload={payload}");
    let (ok, out, err) = terrane(home, &["open", &url]);
    assert!(ok, "open send failed: {out} {err}");
    assert!(out.contains("delivered link payload to receiver"), "out: {out}");

    let (ok, out, err) = terrane(home, &["state"]);
    assert!(ok, "state failed: {err}");
    assert!(out.contains(r#""text":"hello""#), "out: {out}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(
        log.contains("app.link.registered receiver scheme-route terrane://send/receiver"),
        "log: {log}"
    );
    assert!(
        log.contains("interop.called terrane-host -> receiver common.receive"),
        "log: {log}"
    );
}

#[test]
fn terrane_open_item_uri_delivers_item_focus_payload() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("receiver-bundle");
    write_receiver_bundle(&bundle);

    let (ok, out, err) = terrane(home, &["app", "install", bundle.to_str().unwrap()]);
    assert!(ok, "install failed: {out} {err}");

    let (ok, out, err) = terrane(
        home,
        &["open", "terrane://app/receiver/item/folder%2Falpha"],
    );
    assert!(ok, "open item failed: {out} {err}");

    let (ok, out, err) = terrane(home, &["state"]);
    assert!(ok, "state failed: {err}");
    assert!(out.contains(r#""item":"folder/alpha""#), "out: {out}");
}

#[test]
fn terrane_open_registered_file_imports_blob_and_delivers_reference() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("receiver-bundle");
    write_receiver_bundle(&bundle);
    let file = home.join("note.txt");
    fs::write(&file, "hello from a file").unwrap();

    let (ok, out, err) = terrane(home, &["app", "install", bundle.to_str().unwrap()]);
    assert!(ok, "install failed: {out} {err}");

    let (ok, out, err) = terrane(home, &["open", file.to_str().unwrap()]);
    assert!(ok, "open file failed: {out} {err}");
    assert!(out.contains("imported file note.txt to receiver"), "out: {out}");

    let (ok, out, err) = terrane(home, &["state"]);
    assert!(ok, "state failed: {err}");
    assert!(out.contains(r#""name":"note.txt""#), "out: {out}");
    assert!(out.contains(r#""mime":"text/plain""#), "out: {out}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("blob.stored receiver/note.txt"), "log: {log}");
    assert!(
        log.contains("interop.called terrane-host -> receiver common.receive"),
        "log: {log}"
    );
}

#[test]
fn terrane_open_rejects_unknown_filetype() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let file = home.join("note.unknown");
    fs::write(&file, "hello").unwrap();

    let (ok, _out, err) = terrane(home, &["open", file.to_str().unwrap()]);
    assert!(!ok, "open should fail for an unclaimed extension");
    assert!(err.contains("no app registered"), "err: {err}");
}
