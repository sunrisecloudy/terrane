//! e2e for `geo` on the real host. Non-macOS CLI hosts prove the typed
//! unsupported path; the macOS CoreLocation edge is effectful and ignored by
//! default because it requires GUI location services and TCC consent.

#[cfg(not(target_os = "macos"))]
use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
#[cfg(not(target_os = "macos"))]
fn geo_cli_reports_unsupported_and_records_no_observation() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let bundle = home.join("geo-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "geo-app", "name": "Geo", "runtime": "js", "backend": "main.js", "resources": ["geo"] }"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
            function handle(input) {
                return ctx.resource.geo.current("coarse");
            }
        "#,
    )
    .unwrap();

    let (ok, _, err) = terrane(
        home,
        &[
            "app",
            "add",
            "geo-app",
            "Geo",
            "--source",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "geo-app", "geo"]);
    assert!(ok, "auth grant failed: {err}");

    let (ok, out, err) = terrane(home, &["geo", "locate", "geo-app", "coarse"]);
    assert!(!ok, "geo locate should fail on CLI: {out}");
    assert!(
        err.contains("not supported by the CLI host edge")
            || out.contains("not supported by the CLI host edge"),
        "out: {out}\nerr: {err}"
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "geo-app", "go"]);
    assert!(!ok, "geo run should fail on CLI: {out}");
    assert!(
        err.contains("not supported by the CLI host edge")
            || out.contains("not supported by the CLI host edge"),
        "out: {out}\nerr: {err}"
    );

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(!log.contains("geo.observed"), "log: {log}");
}

#[test]
#[cfg(target_os = "macos")]
fn geo_supports_reports_true_on_macos() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = terrane(home, &["geo", "supports"]);
    assert!(ok, "geo supports failed: {err}");
    assert!(out.contains("true"), "out: {out}");
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires macOS GUI location services and TCC consent"]
fn geo_macos_corelocation_records_observation() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = terrane(home, &["app", "add", "geo-app", "Geo"]);
    assert!(ok, "app add failed: {out}\n{err}");
    let (ok, out, err) = terrane(home, &["geo", "locate", "geo-app", "coarse"]);
    assert!(ok, "geo locate failed: {out}\n{err}");
    assert!(out.contains("geo.observed"), "out: {out}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("geo.observed geo-app"), "log: {log}");
    assert!(!log.contains("lat_e7"), "geo describe must redact coordinates: {log}");
}
