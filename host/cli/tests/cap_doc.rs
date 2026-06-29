//! E2E for capability documentation through the `terrane-host` CLI front door.
//! This proves the binary returns the shared CapabilityDoc render, not only the
//! lower-level Rust helper.

use std::path::Path;
use std::process::Command;

use tempfile::tempdir;

fn host(home: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_terrane-host"))
        .args(args)
        .env("TERRANE_HOME", home)
        .output()
        .expect("spawn terrane-host");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn cap_info_returns_relational_db_document() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = host(home, &["cap", "info", "relational_db", "--format", "json"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains(r#""namespace":"relational_db""#), "out: {out}");
    assert!(out.contains(r#""status":"stable""#), "out: {out}");
    assert!(
        out.contains("terrane.relational_db.tableSpec.v1"),
        "out: {out}"
    );
    assert!(out.contains("terrane.relational_db.query.v1"), "out: {out}");
    assert!(!out.contains("Reserved kv layout"), "out: {out}");

    let (ok, skill, err) = host(home, &["cap", "info", "relational_db", "--format", "skill"]);
    assert!(ok, "stderr: {err}");
    assert!(skill.contains("# relational_db"), "skill: {skill}");
    assert!(
        skill.contains("ctx.resource.relational_db"),
        "skill: {skill}"
    );
    assert!(
        skill.contains("schemas/terrane.relational_db.tableSpec.v1"),
        "skill: {skill}"
    );

    let (ok, internal, err) = host(
        home,
        &[
            "cap",
            "info",
            "relational_db",
            "--format",
            "json",
            "--include-internal",
        ],
    );
    assert!(ok, "stderr: {err}");
    assert!(
        internal.contains("Reserved kv layout"),
        "internal: {internal}"
    );
}

#[test]
fn cap_info_returns_document_capability_completion_doc() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = host(home, &["cap", "info", "document", "--format", "json"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains(r#""namespace":"document""#), "out: {out}");
    assert!(out.contains(r#""status":"planned""#), "out: {out}");
    assert!(out.contains("document.schema.json"), "out: {out}");

    let (ok, skill, err) = host(home, &["cap", "info", "document", "--format", "skill"]);
    assert!(ok, "stderr: {err}");
    assert!(skill.contains("# document"), "skill: {skill}");
    assert!(skill.contains("ctx.resource.document"), "skill: {skill}");
}

#[test]
fn cap_list_exposes_capability_summaries() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = host(home, &["cap", "list", "--format", "json"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains(r#""namespace":"kv""#), "out: {out}");
    assert!(out.contains(r#""namespace":"document""#), "out: {out}");
    assert!(out.contains(r#""namespace":"relational_db""#), "out: {out}");
}
