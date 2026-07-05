//! Engine tests for the `history` capability live in the capability crate.
//! This module keeps the shared per-capability harness aware that history is a
//! registered core capability.

use tempfile::tempdir;
use terrane_core::{Core, QueryValue};

use crate::helpers::req;

#[test]
fn history_is_registered_in_default_core() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    let args = vec![
        "notes".to_string(),
        String::new(),
        String::new(),
        "10".to_string(),
    ];
    assert!(matches!(
        core.query("history", "list", &args).unwrap(),
        QueryValue::Json(_)
    ));
}
