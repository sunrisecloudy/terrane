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
