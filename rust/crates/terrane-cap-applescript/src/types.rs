use std::collections::BTreeMap;

use terrane_cap_interface::AppId;

/// One recorded AppleScript execution for an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    pub script: String,
    pub ok: bool,
    pub output: String,
    pub error: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

/// Per-app run history, newest last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppleScriptState {
    pub runs: BTreeMap<AppId, Vec<RunRecord>>,
}

pub const MAX_RUNS_PER_APP: usize = 100;
pub const MAX_SCRIPT_BYTES: usize = 64 * 1024;