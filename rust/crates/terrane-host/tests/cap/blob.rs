use std::fs;
use std::path::Path;

use rusqlite::{params, Connection};
use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn blob_cli_round_trip_uses_verified_sqlite_cas() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let input = home.join("in.txt");
    let output = home.join("out.txt");
    fs::write(&input, b"hello blob").unwrap();

    let (ok, _, err) = terrane(home, &["app", "add", "gallery", "Gallery"]);
    assert!(ok, "app add failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &[
            "blob",
            "put",
            "gallery",
            "images/hello.txt",
            "text/plain",
            input.to_str().unwrap(),
        ],
    );
    assert!(ok, "blob put failed: {err}");
    assert!(out.contains("blob.stored"), "put out: {out}");
    assert!(home.join("blobs.sqlite3").is_file());

    let (ok, out, err) = terrane(home, &["blob", "stat", "gallery", "images/hello.txt"]);
    assert!(ok, "blob stat failed: {err}");
    assert!(out.contains("\"mime\":\"text/plain\""), "stat out: {out}");

    let (ok, out, err) = terrane(home, &["blob", "ls", "gallery", "images/"]);
    assert!(ok, "blob ls failed: {err}");
    assert!(out.contains("images/hello.txt"), "ls out: {out}");

    let (ok, out, err) = terrane(home, &["blob", "verify", "gallery", "images/hello.txt"]);
    assert!(ok, "blob verify failed: {err}");
    assert!(out.contains("ok "), "verify out: {out}");

    let (ok, _, err) = terrane(
        home,
        &[
            "blob",
            "get",
            "gallery",
            "images/hello.txt",
            output.to_str().unwrap(),
        ],
    );
    assert!(ok, "blob get failed: {err}");
    assert_eq!(fs::read(output).unwrap(), b"hello blob");
}

#[test]
fn blob_verify_reports_corrupt_bytes_without_panicking() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let input = home.join("in.txt");
    fs::write(&input, b"hello blob").unwrap();
    terrane(home, &["app", "add", "gallery", "Gallery"]);
    let (ok, _, err) = terrane(
        home,
        &[
            "blob",
            "put",
            "gallery",
            "images/hello.txt",
            "text/plain",
            input.to_str().unwrap(),
        ],
    );
    assert!(ok, "blob put failed: {err}");
    let hash = only_blob_hash(home);
    let conn = Connection::open(home.join("blobs.sqlite3")).unwrap();
    conn.execute(
        "UPDATE blobs SET bytes = ?1 WHERE hash = ?2",
        params![b"not the same".as_slice(), hash],
    )
    .unwrap();

    let (ok, out, err) = terrane(home, &["blob", "verify", "gallery"]);
    assert!(!ok, "corrupt verify should fail: {out}");
    assert!(out.contains("corrupt "), "verify out: {out}, err: {err}");
}

#[test]
fn blob_gc_dry_run_reports_unreferenced_rows() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let input = home.join("in.txt");
    fs::write(&input, b"hello blob").unwrap();
    terrane(home, &["app", "add", "gallery", "Gallery"]);
    terrane(
        home,
        &[
            "blob",
            "put",
            "gallery",
            "images/hello.txt",
            "text/plain",
            input.to_str().unwrap(),
        ],
    );

    let (ok, out, err) = terrane(home, &["blob", "gc"]);
    assert!(ok, "gc before rm failed: {err}");
    assert!(out.contains("would delete 0 blob rows"), "gc out: {out}");

    terrane(home, &["blob", "rm", "gallery", "images/hello.txt"]);
    let (ok, out, err) = terrane(home, &["blob", "gc"]);
    assert!(ok, "gc dry-run failed: {err}");
    assert!(out.contains("would delete 1 blob rows"), "gc out: {out}");
}

#[test]
fn sync_from_home_copies_blob_metadata_and_sidecar_bytes() {
    let source_dir = tempdir().unwrap();
    let local_dir = tempdir().unwrap();
    let source = source_dir.path();
    let local = local_dir.path();
    let input = source.join("in.txt");
    let output = local.join("out.txt");
    fs::write(&input, b"synced blob").unwrap();

    terrane(source, &["app", "add", "gallery", "Gallery"]);
    let (ok, _, err) = terrane(
        source,
        &[
            "blob",
            "put",
            "gallery",
            "images/sync.txt",
            "text/plain",
            input.to_str().unwrap(),
        ],
    );
    assert!(ok, "source blob put failed: {err}");
    terrane(local, &["app", "add", "gallery", "Gallery"]);

    let (ok, out, err) = terrane(
        local,
        &["sync", "gallery", "--from", source.to_str().unwrap()],
    );
    assert!(ok, "sync failed: {err}");
    assert!(
        out.contains("synced") || out.contains("already up to date"),
        "sync out: {out}"
    );

    let (ok, _, err) = terrane(
        local,
        &[
            "blob",
            "get",
            "gallery",
            "images/sync.txt",
            output.to_str().unwrap(),
        ],
    );
    assert!(ok, "blob get after sync failed: {err}");
    assert_eq!(fs::read(output).unwrap(), b"synced blob");
}

fn only_blob_hash(home: &Path) -> String {
    Connection::open(home.join("blobs.sqlite3"))
        .unwrap()
        .query_row("SELECT hash FROM blobs", [], |row| row.get(0))
        .unwrap()
}
