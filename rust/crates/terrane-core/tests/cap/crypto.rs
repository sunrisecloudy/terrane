//! Engine tests for the `crypto` capability driven through the real QuickJS
//! runtime. The load-bearing property: a vault app can store real secrets while
//! the plaintext and the master password never enter the (plaintext) event log —
//! only ciphertext does — and replay rebuilds the vault from that ciphertext
//! without any key material.

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_core::{Core, LOCAL_OWNER_SUBJECT};

use crate::helpers::req;

/// A minimal vault backend: create a vault + seal an item, then unlock + open it.
const BACKEND: &str = r#"
var crypto = ctx.resource.crypto;
var kv = ctx.resource.kv;
function handle(input) {
    var verb = input[0];
    if (verb === "create") {
        var v = JSON.parse(crypto.newVault(input[1]));
        if (!v.ok) return "ERR:newvault";
        kv.set("meta", v.meta);
        var item = JSON.stringify({ name: "github", secret: input[2] });
        var sealed = JSON.parse(crypto.seal(v.session, item));
        if (!sealed.ok) return "ERR:seal:" + sealed.reason;
        kv.set("item:github", sealed.blob);
        return "created";
    }
    if (verb === "read") {
        var u = JSON.parse(crypto.unlock(input[1], kv.get("meta")));
        if (!u.ok) return "LOCKED:" + u.reason;
        var opened = JSON.parse(crypto.open(u.session, kv.get("item:github")));
        if (!opened.ok) return "ERR:" + opened.reason;
        return opened.plaintext;
    }
    if (verb === "stored") {
        return kv.get("item:github");
    }
    return "?";
}
"#;

fn write_bundle(dir: &Path, name: &str, manifest: &str, backend: &str) -> String {
    let bundle = dir.join(name);
    fs::create_dir(&bundle).unwrap();
    fs::write(bundle.join("manifest.json"), manifest).unwrap();
    fs::write(bundle.join("main.js"), backend).unwrap();
    bundle.to_str().unwrap().to_string()
}

fn grant(core: &mut Core, app: &str, namespace: &str) {
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, app, namespace]))
        .unwrap();
}

fn install_vault(dir: &Path) -> Core {
    let src = write_bundle(
        dir,
        "vault",
        r#"{ "id": "vault", "name":"Vault","runtime":"js","backend":"main.js", "resources": ["kv","crypto"] }"#,
        BACKEND,
    );
    let mut core = Core::open(dir.join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["vault", "Vault", "--source", &src]))
        .unwrap();
    grant(&mut core, "vault", "kv");
    grant(&mut core, "vault", "crypto");
    core
}

const MASTER: &str = "master-sentinel-pw-987";
const SECRET: &str = "PLAINTEXT-SENTINEL-never-log-42";

#[test]
fn secrets_seal_to_ciphertext_and_never_enter_the_log() {
    let dir = tempdir().unwrap();
    let log_path = dir.path().join("log.bin");
    let mut core = install_vault(dir.path());

    let records = core
        .dispatch(req("js-runtime.run", &["vault", "create", MASTER, SECRET]))
        .unwrap();

    assert_eq!(core.take_last_output().as_deref(), Some("created"));

    // Option A: the run records only kv.* events (meta + item), nothing crypto.
    assert!(
        records.iter().all(|r| r.kind.starts_with("kv.")),
        "vault create must record only kv events, got: {:?}",
        records.iter().map(|r| &r.kind).collect::<Vec<_>>()
    );

    // The stored item is a base64 ciphertext blob, not the plaintext.
    core.dispatch(req("js-runtime.run", &["vault", "stored"])).unwrap();
    let stored = core.take_last_output().unwrap_or_default();
    assert!(!stored.contains(SECRET), "stored value leaked plaintext");
    assert!(!stored.is_empty());

    // The strongest proof: the raw on-disk log contains neither the plaintext
    // secret nor the master password anywhere.
    let raw = fs::read(&log_path).unwrap();
    assert!(
        !contains(&raw, SECRET.as_bytes()),
        "plaintext secret found in the event log"
    );
    assert!(
        !contains(&raw, MASTER.as_bytes()),
        "master password found in the event log"
    );

    // Replay rebuilds the vault from the ciphertext kv events alone.
    assert!(core.replay_matches().unwrap());
}

#[test]
fn unlock_gate_opens_only_with_the_right_master_password() {
    let dir = tempdir().unwrap();
    let mut core = install_vault(dir.path());
    core.dispatch(req("js-runtime.run", &["vault", "create", MASTER, SECRET]))
        .unwrap();

    // Right password decrypts back to the exact plaintext item.
    core.dispatch(req("js-runtime.run", &["vault", "read", MASTER]))
        .unwrap();
    let opened = core.take_last_output().unwrap_or_default();
    assert!(opened.contains(SECRET), "correct master should reveal the secret");
    assert!(opened.contains("github"));

    // Wrong password is refused without revealing anything.
    core.dispatch(req("js-runtime.run", &["vault", "read", "wrong-password"]))
        .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("LOCKED:bad_password")
    );
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
