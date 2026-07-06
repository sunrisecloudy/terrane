use tempfile::tempdir;
use terrane_core::{Core, Error};

use crate::helpers::{grant_resource, req};

#[test]
fn interop_call_requires_grant() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["caller", "Caller"]))
        .unwrap();
    core.dispatch(req("app.add", &["target", "Target"]))
        .unwrap();

    let err = core
        .dispatch(req(
            "interop.call",
            &["caller", "target", "common.list", "caller"],
        ))
        .unwrap_err();

    assert!(err.to_string().contains("permission required"));
}

#[test]
fn interop_rejects_cycles_and_depth_before_effect() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["a", "A"])).unwrap();
    core.dispatch(req("app.add", &["b", "B"])).unwrap();
    grant_resource(&mut core, "a", "interop");

    let cycle = core
        .dispatch(req("interop.call", &["a", "b", "common.list", "a>b"]))
        .unwrap_err();
    assert!(cycle.to_string().contains("InteropCycle"));

    let depth = core
        .dispatch(req(
            "interop.call",
            &["a", "b", "common.list", "a>x>y>z>w"],
        ))
        .unwrap_err();
    assert!(matches!(depth, Error::InvalidInput(_)));
    assert!(depth.to_string().contains("InteropDepthExceeded"));
}

#[test]
fn interop_send_without_default_target_raises_picker() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["caller", "Caller"]))
        .unwrap();

    // No picked default target for the interface yet: send must raise the
    // powerbox picker signal rather than delivering.
    let err = core
        .dispatch(req(
            "interop.send",
            &["caller", "inbox", "text", "hello"],
        ))
        .unwrap_err();
    assert!(
        err.to_string().contains("interop_pick_required:"),
        "err: {err}"
    );
}

#[test]
fn interop_pick_without_target_raises_picker() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["caller", "Caller"]))
        .unwrap();

    // A two-arg pick (an app asking to choose) raises the picker; it never
    // records a grant on the app's behalf.
    let err = core
        .dispatch(req("interop.pick", &["caller", "inbox"]))
        .unwrap_err();
    assert!(
        err.to_string().contains("interop_pick_required:"),
        "err: {err}"
    );
}
