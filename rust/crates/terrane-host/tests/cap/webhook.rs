use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn webhook_cli_register_rotate_and_list_routes() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (ok, _, err) = terrane(home, &["app", "add", "receiver", "Receiver"]);
    assert!(ok, "app add failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &["webhook", "register", "receiver", "github", "receive"],
    );
    assert!(ok, "webhook register failed: {err}");
    assert!(out.contains(r#""url_path":"/hook/receiver/github/"#), "out: {out}");
    assert!(
        out.contains("deliveries arrive only while a listening Terrane web/mac host is running"),
        "out: {out}"
    );

    let first = token_from(&out);
    let (ok, out, err) = terrane(home, &["webhook", "ls", "receiver"]);
    assert!(ok, "webhook ls failed: {err}");
    assert!(out.contains(&first), "ls out: {out}");

    let (ok, out, err) = terrane(home, &["webhook", "rotate", "receiver", "github"]);
    assert!(ok, "webhook rotate failed: {err}");
    let second = token_from(&out);
    assert_ne!(first, second, "rotation should mint a new URL token");

    let (ok, _, err) = terrane(home, &["webhook", "unregister", "receiver", "github"]);
    assert!(ok, "webhook unregister failed: {err}");
    let (ok, out, err) = terrane(home, &["webhook", "ls", "receiver"]);
    assert!(ok, "webhook ls after unregister failed: {err}");
    assert_eq!(out.trim(), "[]");
}

fn token_from(out: &str) -> String {
    let marker = "/hook/receiver/github/";
    let start = out.find(marker).expect("url path") + marker.len();
    out[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_hexdigit())
        .collect()
}
