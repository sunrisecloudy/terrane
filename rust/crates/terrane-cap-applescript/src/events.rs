use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, truncate, EventRecord, Result,
    StateStore,
};

use crate::types::{AppleScriptState, MAX_RUNS_PER_APP, RunRecord};

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Ran {
    pub app: String,
    pub script: String,
    pub ok: bool,
    pub output: String,
    pub error: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Checked {
    pub app: String,
    pub script: String,
    pub ok: bool,
    pub error: String,
}

pub fn ran_event(
    app: &str,
    script: &str,
    ok: bool,
    output: &str,
    error: &str,
    exit_code: i32,
    duration_ms: u64,
) -> Result<EventRecord> {
    encode_event(
        "applescript.ran",
        &Ran {
            app: app.to_string(),
            script: script.to_string(),
            ok,
            output: output.to_string(),
            error: error.to_string(),
            exit_code,
            duration_ms,
        },
    )
}

pub fn checked_event(app: &str, script: &str, ok: bool, error: &str) -> Result<EventRecord> {
    encode_event(
        "applescript.checked",
        &Checked {
            app: app.to_string(),
            script: script.to_string(),
            ok,
            error: error.to_string(),
        },
    )
}

pub fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "applescript.ran" => {
            let e: Ran = decode_event(record)?;
            let runs = state_mut::<AppleScriptState>(state, "applescript")?
                .runs
                .entry(e.app)
                .or_default();
            runs.push(RunRecord {
                script: e.script,
                ok: e.ok,
                output: e.output,
                error: e.error,
                exit_code: e.exit_code,
                duration_ms: e.duration_ms,
            });
            while runs.len() > MAX_RUNS_PER_APP {
                runs.remove(0);
            }
        }
        "app.removed" => {
            let e = decode_app_removed(record)?;
            state_mut::<AppleScriptState>(state, "applescript")?
                .runs
                .remove(&e.id);
        }
        _ => {}
    }
    Ok(())
}

pub fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "applescript.ran" => {
            let e: Ran = decode_event(record).ok()?;
            Some(format!(
                "applescript.ran {} {} ok={} exit={} ({} ms)",
                e.app,
                truncate(&e.script, 50),
                e.ok,
                e.exit_code,
                e.duration_ms
            ))
        }
        "applescript.checked" => {
            let e: Checked = decode_event(record).ok()?;
            Some(format!(
                "applescript.checked {} {} ok={}",
                e.app,
                truncate(&e.script, 50),
                e.ok
            ))
        }
        _ => None,
    }
}

pub fn run_json_from_records(records: &[EventRecord]) -> Option<String> {
    for record in records {
        if record.kind != "applescript.ran" {
            continue;
        }
        let e: Ran = decode_event(record).ok()?;
        return Some(json_run_result(
            e.ok,
            &e.output,
            &e.error,
            e.exit_code,
            e.duration_ms,
        ));
    }
    None
}

pub fn check_json_from_records(records: &[EventRecord]) -> Option<String> {
    for record in records {
        if record.kind != "applescript.checked" {
            continue;
        }
        let e: Checked = decode_event(record).ok()?;
        return Some(json_check_result(e.ok, &e.error));
    }
    None
}

pub fn runs_json_for_app(state: &dyn StateStore, app: &str) -> Result<String> {
    let slice = terrane_cap_interface::state_ref::<AppleScriptState>(state, "applescript")?;
    let runs = slice.runs.get(app).cloned().unwrap_or_default();
    Ok(json_run_records(&runs))
}

fn json_run_result(ok: bool, output: &str, error: &str, exit_code: i32, duration_ms: u64) -> String {
    format!(
        "{{\"ok\":{},\"output\":{},\"error\":{},\"exitCode\":{},\"durationMs\":{}}}",
        json_bool(ok),
        json_string(output),
        json_string(error),
        exit_code,
        duration_ms
    )
}

fn json_check_result(ok: bool, error: &str) -> String {
    format!(
        "{{\"ok\":{},\"error\":{}}}",
        json_bool(ok),
        json_string(error)
    )
}

fn json_run_records(runs: &[RunRecord]) -> String {
    let items: Vec<String> = runs
        .iter()
        .map(|r| {
            format!(
                "{{\"script\":{},\"ok\":{},\"output\":{},\"error\":{},\"exitCode\":{},\"durationMs\":{}}}",
                json_string(&r.script),
                json_bool(r.ok),
                json_string(&r.output),
                json_string(&r.error),
                r.exit_code,
                r.duration_ms
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn json_bool(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}