//! e2e for the `apps/password-manager` bundle driven through the real `terrane`
//! binary. Each `terrane` call is its own process, so the crypto session keyring
//! starts empty every command — exactly the CLI story. The vault therefore
//! unlocks inline by passing the master password as the `auth` argument.
//!
//! The vault's synced data lives in a CRDT doc (ciphertext only); the audit trail
//! is local kv. Deterministic + local (Argon2id + AEAD), so it runs by DEFAULT.
//! The load-bearing assertion mirrors the core test: neither the plaintext secret
//! nor the item name ever appears in the on-disk log.

use std::path::{Path, PathBuf};

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

/// Install the app into `home` and grant the resources it needs.
fn install(home: &Path, src: &str, namespaces: &[&str]) {
    let (ok, out, err) = terrane(
        home,
        &["app", "add", "password-manager", "Password Manager", "--source", src],
    );
    assert!(ok && out.contains("app.added"), "app add: {out} {err}");
    for ns in namespaces {
        let (ok, _, err) = terrane(
            home,
            &["auth", "grant", "user:local-owner", "password-manager", ns],
        );
        assert!(ok, "grant {ns}: {err}");
    }
}

#[test]
fn password_manager_vault_lifecycle_and_no_plaintext_in_log() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source();
    install(home, &src, &["kv", "crypto", "crdt"]);

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

    // Add a login. Ids are random now, so refer to items by name afterwards.
    let out = run(&["add-login", MASTER, "GitHub", "octocat", SECRET, "https://github.com"]);
    assert!(out.contains("\"ok\":true") && out.contains("GitHub"), "add-login: {out}");

    // List returns metadata but never the secret.
    let out = run(&["list", MASTER]);
    assert!(out.contains("GitHub"), "list: {out}");
    assert!(!out.contains(SECRET), "list leaked the password: {out}");

    // Reveal the full item — this is where the secret legitimately appears.
    let out = run(&["get", MASTER, "GitHub"]);
    assert!(out.contains(SECRET), "get should reveal the secret: {out}");
    assert!(out.contains("octocat"), "get: {out}");

    // Reveal just the password.
    let out = run(&["password", MASTER, "GitHub"]);
    assert!(out.contains(SECRET), "password: {out}");

    // Wrong master password is refused.
    let out = run(&["get", "wrong-password", "GitHub"]);
    assert!(out.contains("bad_password"), "wrong password: {out}");

    // Generators need no unlock.
    let out = run(&["generate", "{\"length\":24,\"symbols\":false}"]);
    assert!(out.contains("\"ok\":true") && out.contains("password"), "generate: {out}");

    // The audit trail is readable without a password and logged the reveal.
    let out = run(&["audit"]);
    assert!(out.contains("item.reveal"), "audit: {out}");

    // Change the master password: every blob is re-sealed under the new key.
    let new_master = "a different battery staple";
    let out = run(&["change-master", MASTER, new_master]);
    assert!(
        out.contains("\"ok\":true") && out.contains("\"reencrypted\":1"),
        "change-master: {out}"
    );
    let out = run(&["get", MASTER, "GitHub"]);
    assert!(out.contains("bad_password"), "old master should fail: {out}");
    let out = run(&["get", new_master, "GitHub"]);
    assert!(out.contains(SECRET), "new master should reveal secret: {out}");
    assert!(out.contains("octocat"), "new master get: {out}");

    // The vault is stored in a CRDT doc (so it can sync); audit stays in kv.
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(
        log.contains("crdt.update password-manager"),
        "vault should be stored in crdt: {log}"
    );
    assert!(!log.contains(SECRET), "plaintext secret in the log: {log}");

    // Stronger than `terrane log` (which truncates values): scan the RAW on-disk
    // log. Neither the password, the username, NOR the item NAME may appear.
    let raw = std::fs::read(home.join("log.bin")).unwrap();
    let leaks = |needle: &str| raw.windows(needle.len()).any(|w| w == needle.as_bytes());
    assert!(!leaks(SECRET), "raw log leaked the password");
    assert!(!leaks("octocat"), "raw log leaked the username");
    assert!(!leaks("GitHub"), "raw log leaked the item name");

    // Replay rebuilds the vault (crdt + kv events) with no re-run of the JS.
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay: {out}");
}

/// Multi-device: create the vault on one home, `terrane sync` its CRDT vault into
/// a second home, and confirm the second home unlocks with the same master
/// password and decrypts the synced item — while its log holds only ciphertext.
#[test]
fn password_manager_syncs_encrypted_vault_across_homes() {
    let src = app_source();
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    let a = dir_a.path();
    let b = dir_b.path();

    install(a, &src, &["kv", "crypto", "crdt"]);
    install(b, &src, &["kv", "crypto", "crdt"]);

    let run = |home: &Path, args: &[&str]| -> String {
        let mut full = vec!["js-runtime", "run", "password-manager"];
        full.extend_from_slice(args);
        let (ok, out, err) = terrane(home, &full);
        assert!(ok, "run {args:?} at {home:?} failed: {err}");
        out.trim().to_string()
    };

    // Device A creates the vault and adds an item.
    assert!(run(a, &["init", MASTER]).contains("\"ok\":true"));
    assert!(run(a, &["add-login", MASTER, "GitHub", "octocat", SECRET, "https://github.com"])
        .contains("GitHub"));

    // Device B has no vault yet.
    assert!(run(b, &["status"]).contains("\"exists\":false"), "B should start empty");

    // B pulls A's encrypted vault (meta + item) over the CRDT sync.
    let (ok, out, err) = terrane(b, &["sync", "password-manager", "--from", a.to_str().unwrap()]);
    assert!(ok, "sync failed: {err}");
    assert!(out.contains("synced"), "sync out: {out}");

    // Now B sees the vault and, with the SAME master password, decrypts the item.
    assert!(run(b, &["status"]).contains("\"exists\":true"), "B should see the synced vault");
    let out = run(b, &["get", MASTER, "GitHub"]);
    assert!(out.contains(SECRET), "B should decrypt the synced item: {out}");
    assert!(out.contains("octocat"), "B get: {out}");

    // The synced data on B is ciphertext only — the secret/name never hit B's log.
    let raw = std::fs::read(b.join("log.bin")).unwrap();
    let leaks = |needle: &str| raw.windows(needle.len()).any(|w| w == needle.as_bytes());
    assert!(!leaks(SECRET), "synced secret leaked into B's log");
    assert!(!leaks("GitHub"), "synced item name leaked into B's log");

    assert!(terrane(b, &["replay"]).0, "B replays cleanly");
}

/// Real HIBP breach check over the network (k-anonymity). `#[ignore]`d because
/// it hits api.pwnedpasswords.com; run with `cargo test -p terrane-host -- --ignored`.
#[test]
#[ignore = "hits the real api.pwnedpasswords.com"]
fn password_manager_breach_check_flags_a_known_pwned_password() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source();
    install(home, &src, &["kv", "crypto", "crdt", "net"]);

    terrane(home, &["js-runtime", "run", "password-manager", "init", MASTER]);
    // "password" is famously in every breach corpus.
    terrane(
        home,
        &["js-runtime", "run", "password-manager", "add-login", MASTER, "Test", "user", "password", "https://x.test"],
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "password-manager", "breach", MASTER, "Test"]);
    assert!(ok, "breach failed: {err}");
    assert!(out.contains("\"breached\":true"), "breach should flag it: {out}");

    // And the breach check must NOT have recorded the response or the prefix.
    let raw = std::fs::read(home.join("log.bin")).unwrap();
    assert!(
        !raw.windows(9).any(|w| w == b"pwnedpass"),
        "a breach URL/response leaked into the log"
    );
}

/// `terrane run <app> <verb> --ask` reads the master password from stdin and
/// splices it in as the vault app's `auth` argument, so it never appears on
/// argv. Piping stdin here stands in for the interactive hidden prompt.
#[test]
fn password_manager_ask_reads_master_from_stdin_not_argv() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source();
    install(home, &src, &["kv", "crypto", "crdt"]);

    // Setup: create the vault + an item (master on argv is fine for setup).
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "password-manager", "init", MASTER]);
    assert!(ok && out.contains("\"ok\":true"), "init: {out} {err}");
    let (ok, out, _) = terrane(
        home,
        &["js-runtime", "run", "password-manager", "add-login", MASTER, "GitHub", "octocat", SECRET, "https://github.com"],
    );
    assert!(ok && out.contains("GitHub"), "add-login: {out}");

    // Interactive unlock: the master arrives on stdin, NOT argv. Note the argv
    // below contains only the verb + item name — no password.
    let (ok, out, err) = terrane_stdin(
        home,
        &["run", "password-manager", "get", "--ask", "GitHub"],
        &format!("{MASTER}\n"),
    );
    assert!(ok, "run --ask failed: {err}");
    assert!(out.contains(SECRET), "ask-unlock should reveal the secret: {out}");
    assert!(out.contains("octocat"), "ask-unlock get: {out}");

    // A wrong password piped in is refused, just like the argv path.
    let (ok, out, err) = terrane_stdin(
        home,
        &["run", "password-manager", "get", "--ask", "GitHub"],
        "not-the-master\n",
    );
    assert!(ok, "run --ask (wrong) should still exit ok: {err}");
    assert!(out.contains("bad_password"), "wrong piped master: {out}");
}
