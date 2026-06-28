//! Event-log integrity tests.

use std::fs;

use tempfile::tempdir;
use terrane_core::read_log;
use terrane_core::Error;

#[test]
fn partial_length_prefix_is_corruption_not_clean_eof() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    fs::write(&log, [3u8, 0, 0]).unwrap();

    let err = read_log(&log).expect_err("partial length prefix must fail");
    match err {
        Error::Storage(msg) => assert!(msg.contains("truncated log record length"), "{msg}"),
        other => panic!("expected storage error, got {other:?}"),
    }
}

#[test]
fn empty_log_still_reads_as_empty_history() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    fs::write(&log, []).unwrap();

    assert!(read_log(&log).unwrap().is_empty());
}
