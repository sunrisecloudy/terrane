//! The `applescript` capability — recorded AppleScript execution on macOS.
//!
//! Runs and compile-checks happen at the edge via `/usr/bin/osascript` and
//! `osacompile`; results are recorded as events so replay never re-executes.
//! Reacts to `app.removed`.

mod doc;
mod events;
mod types;

pub use events::{checked_event, ran_event};
pub use types::{AppleScriptState, RunRecord, MAX_RUNS_PER_APP, MAX_SCRIPT_BYTES};

use terrane_cap_interface::{
    arg, ensure_app_exists, join_tail, CapManifest, Capability, CommandCtx, CommandSpec, Decision,
    Effect, Error, EventPattern, EventRecord, EventSpec, GrantResourceSpec, ReadValue,
    ResourceMethod, ResourceReadCtx, Result, StateStore,
};

pub struct AppleScriptCapability;

impl Capability for AppleScriptCapability {
    fn namespace(&self) -> &'static str {
        "applescript"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "applescript.run",
                },
                CommandSpec {
                    name: "applescript.check",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "applescript.ran",
                },
                EventSpec {
                    kind: "applescript.checked",
                },
            ],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Call {
                    name: "run",
                    params: &["script"],
                },
                ResourceMethod::Call {
                    name: "check",
                    params: &["script"],
                },
                ResourceMethod::Read {
                    name: "runs",
                    params: &[],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "applescript",
                &["call", "read"],
                "Recorded AppleScript run and compile-check effects — arbitrary macOS machine control.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::applescript_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "applescript.run" => decide_run(ctx, args),
            "applescript.check" => decide_check(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        events::fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        events::describe(record)
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        let _ = args;
        match name {
            "runs" => Ok(ReadValue::OptString(Some(events::runs_json_for_app(
                ctx.state, ctx.app,
            )?))),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: applescript.{other}"
            ))),
        }
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "run" => Ok(ReadValue::OptString(events::run_json_from_records(records))),
            "check" => Ok(ReadValue::OptString(events::check_json_from_records(records))),
            other => Err(Error::InvalidInput(format!(
                "applescript.{other} is not a callable resource"
            ))),
        }
    }
}

fn decide_run(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let script = validated_script(ctx, &app, args)?;
    Ok(Decision::Effect(Effect::AppleScriptRun { app, script }))
}

fn decide_check(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let script = validated_script(ctx, &app, args)?;
    Ok(Decision::Effect(Effect::AppleScriptCheck { app, script }))
}

fn validated_script(ctx: CommandCtx<'_>, app: &str, args: &[String]) -> Result<String> {
    ensure_app_exists(ctx.bus, app)?;
    let script = join_tail(args, 1);
    let trimmed = script.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidInput("script must not be empty".into()));
    }
    if script.len() > MAX_SCRIPT_BYTES {
        return Err(Error::InvalidInput(format!(
            "script exceeds {MAX_SCRIPT_BYTES} bytes"
        )));
    }
    Ok(script)
}