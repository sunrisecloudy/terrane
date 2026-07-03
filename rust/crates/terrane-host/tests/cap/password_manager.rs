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

use crate::helpers::{terrane, terrane_stdin};

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

    // Change the master password: every blob is re-sealed under the new key.
    // The old password must stop working and the item must stay readable (and
    // still decrypted-only) under the new one.
    let new_master = "a different battery staple";
    let out = run(&["change-master", MASTER, new_master]);
    assert!(
        out.contains("\"ok\":true") && out.contains("\"reencrypted\":1"),
        "change-master: {out}"
    );

    let out = run(&["get", MASTER, "1"]);
    assert!(out.contains("bad_password"), "old master should fail: {out}");

    let out = run(&["get", new_master, "1"]);
    assert!(out.contains(SECRET), "new master should reveal secret: {out}");
    assert!(out.contains("octocat"), "new master get: {out}");

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

    // Stronger than `terrane log` (which truncates values for display): scan the
    // RAW on-disk event log. Neither the password, the username, NOR the item
    // NAME may appear — names are sensitive metadata and must be sealed, not
    // leaked through the plaintext audit trail.
    let raw = std::fs::read(home.join("log.bin")).unwrap();
    let leaks = |needle: &str| raw.windows(needle.len()).any(|w| w == needle.as_bytes());
    assert!(!leaks(SECRET), "raw log leaked the password");
    assert!(!leaks("octocat"), "raw log leaked the username");
    assert!(!leaks("GitHub"), "raw log leaked the item name (audit detail?)");

    // Replay rebuilds the vault from the kv ciphertext events alone.
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay: {out}");
}

/// `terrane run <app> <verb> --ask` reads the master password from stdin and
/// splices it in as the vault app's `auth` argument, so it never appears on
/// argv. Piping stdin here stands in for the interactive hidden prompt.
#[test]
fn password_manager_ask_reads_master_from_stdin_not_argv() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source();

    let (ok, _, err) = terrane(
        home,
        &["app", "add", "password-manager", "PM", "--source", &src],
    );
    assert!(ok, "app add: {err}");
    for ns in ["kv", "crypto"] {
        let (ok, _, err) = terrane(
            home,
            &["auth", "grant", "user:local-owner", "password-manager", ns],
        );
        assert!(ok, "grant {ns}: {err}");
    }

    // Setup: create the vault + an item (master on argv is fine for setup).
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "password-manager", "init", MASTER]);
    assert!(ok && out.contains("\"ok\":true"), "init: {out} {err}");
    let (ok, out, _) = terrane(
        home,
        &["js-runtime", "run", "password-manager", "add-login", MASTER, "GitHub", "octocat", SECRET, "https://github.com"],
    );
    assert!(ok && out.contains("\"id\":1"), "add-login: {out}");

    // Interactive unlock: the master arrives on stdin, NOT argv. Note the argv
    // below contains only the verb + item id — no password.
    let (ok, out, err) = terrane_stdin(
        home,
        &["run", "password-manager", "get", "--ask", "1"],
        &format!("{MASTER}\n"),
    );
    assert!(ok, "run --ask failed: {err}");
    assert!(out.contains(SECRET), "ask-unlock should reveal the secret: {out}");
    assert!(out.contains("octocat"), "ask-unlock get: {out}");

    // A wrong password piped in is refused, just like the argv path.
    let (ok, out, err) = terrane_stdin(
        home,
        &["run", "password-manager", "get", "--ask", "1"],
        "not-the-master\n",
    );
    assert!(ok, "run --ask (wrong) should still exit ok: {err}");
    assert!(out.contains("bad_password"), "wrong piped master: {out}");
}
