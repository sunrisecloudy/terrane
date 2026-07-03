//! The edge system-metrics sampler (`EdgeRunner as LiveHost`) returns well-formed
//! JSON for the always-present domains. It reads the real host but is fast and
//! non-destructive, so it runs by default — rate fields simply read zero on the
//! first sample, exactly as a freshly started monitor would.

use terrane_core::LiveHost;
use terrane_host::EdgeRunner;

#[test]
fn system_domain_reports_host_identity_and_uptime() {
    let json = EdgeRunner::default().sample("system", &[]).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert!(
        value.get("uptimeSeconds").and_then(|v| v.as_u64()).is_some(),
        "system should report uptime: {json}"
    );
    assert!(value.get("arch").is_some(), "system should report arch");
}

#[test]
fn snapshot_carries_every_section_and_real_memory() {
    let json = EdgeRunner::default().sample("snapshot", &[]).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    for section in [
        "cpu",
        "memory",
        "disk",
        "network",
        "battery",
        "system",
        "processes",
    ] {
        assert!(value.get(section).is_some(), "snapshot missing {section}");
    }
    assert!(
        value["memory"]["total"].as_u64().unwrap_or(0) > 0,
        "a real host always has memory"
    );
}

#[test]
fn processes_respects_the_limit_argument() {
    let json = EdgeRunner::default()
        .sample("processes", &["cpu".into(), "3".into()])
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let rows = value["processes"].as_array().expect("processes array");
    assert!(rows.len() <= 3, "limit not honored: {}", rows.len());
}

#[test]
fn unknown_domain_is_an_error() {
    assert!(EdgeRunner::default().sample("gpu", &[]).is_err());
}
