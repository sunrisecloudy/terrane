//! e2e for the `apps/password-manager` bundle driven through the real `terrane`
//! binary. Each `terrane` call is its own process, so the crypto session keyring
//! starts empty every command — exactly the CLI story. The vault therefore
//! unlocks inline by passing the master password as the `auth` argument.
//!
//! Deterministic + local (Argon2id + AEAD, no clock/network dependence for the
//! assertions), so it runs by DEFAULT. The load-bearing assertion mirrors the
//! core test: the on-disk log never contains the plaintext secret.

use std::path::PathBuf;

use tempfile::tempdir;

use crate::helpers::terrane;

fn app_source() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/password-manager")
        .canonicalize()
        .expect("apps/password-manager bundle exists")
        .to_str()
        .unwrap()
        .to_string()
}

const MASTER: &str = "correct horse battery staple";
const SECRET: &str = "SENTINEL-never-logged-42";

#[test]
fn password_manager_vault_lifecycle_and_no_plaintext_in_log() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source();

    let (ok, out, err) = terrane(
        home,
        &["app", "add", "password-manager", "Password Manager", "--source", &src],
    );
    assert!(ok, "app add failed: {err}");
    assert!(out.contains("app.added"), "out: {out}");

    for ns in ["kv", "crypto"] {
        let (ok, _, err) = terrane(
            home,
            &["auth", "grant", "user:local-owner", "password-manager", ns],
        );
        assert!(ok, "grant {ns} failed: {err}");
    }

    let run = |args: &[&str]| -> String {
        let mut full = vec!["js-runtime", "run", "password-manager"];
        full.extend_from_slice(args);
        let (ok, out, err) = terrane(home, &full);
        assert!(ok, "run {args:?} failed: {err}");
        out.trim().to_string()
    };

    // Create the vault.
    let out = run(&["init", MASTER]);
    assert!(out.contains("\"ok\":true"), "init: {out}");

    // A second init must be refused.
    let out = run(&["init", MASTER]);
    assert!(out.contains("vault_exists"), "double init: {out}");

    // Add a login (master password used as inline auth).
    let out = run(&["add-login", MASTER, "GitHub", "octocat", SECRET, "https://github.com"]);
    assert!(out.contains("\"id\":1"), "add-login: {out}");

    // List returns metadata but never the secret.
    let out = run(&["list", MASTER]);
    assert!(out.contains("GitHub"), "list: {out}");
    assert!(!out.contains(SECRET), "list leaked the password: {out}");

    // Reveal the full item — this is where the secret legitimately appears.
    let out = run(&["get", MASTER, "1"]);
    assert!(out.contains(SECRET), "get should reveal the secret: {out}");
    assert!(out.contains("octocat"), "get: {out}");

    // Reveal just the password, by name.
    let out = run(&["password", MASTER, "GitHub"]);
    assert!(out.contains(SECRET), "password: {out}");

    // Wrong master password is refused.
    let out = run(&["get", "wrong-password", "1"]);
    assert!(out.contains("bad_password"), "wrong password: {out}");

    // Generators need no unlock.
    let out = run(&["generate", "{\"length\":24,\"symbols\":false}"]);
    assert!(out.contains("\"ok\":true") && out.contains("password"), "generate: {out}");

    // The audit trail is readable without a password and logged the reveal.
    let out = run(&["audit"]);
    assert!(out.contains("item.reveal"), "audit: {out}");

    // THE contract: the plaintext secret is nowhere in the event log.
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(
        log.contains("kv.set password-manager/meta"),
        "meta not stored: {log}"
    );
    assert!(
        log.contains("kv.set password-manager/item:1"),
        "item not stored: {log}"
    );
    assert!(
        !log.contains(SECRET),
        "plaintext secret leaked into the log: {log}"
    );

    // Replay rebuilds the vault from the kv ciphertext events alone.
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay: {out}");
}
