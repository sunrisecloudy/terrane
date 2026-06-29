//! The `harness` capability — requests generated Terrane artifacts from
//! swappable external code-generation harnesses.

use terrane_cap_interface::{
    ensure_app_exists, non_empty, CapManifest, Capability, CommandCtx, CommandSpec, Decision,
    Effect, Error, EventRecord, EventSpec, Result, StateStore,
};

mod args;
mod events;
mod prompts;
mod state;

use args::parse_harness_args;
use terrane_cap_builder as builder;

pub use args::DEFAULT_HARNESS;
pub use events::{js_completed_event, js_failed_event, js_generated_event, js_requested_event};
pub use prompts::{
    app_bundle_prompt, parse_run_js_output, run_js_prompt, APP_BUNDLE_OUTPUT_SCHEMA,
    RUN_JS_OUTPUT_SCHEMA,
};
pub use state::{HarnessJsRun, HarnessState};

pub struct HarnessCapability;

impl Capability for HarnessCapability {
    fn namespace(&self) -> &'static str {
        "harness"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "harness.generate-app",
                },
                CommandSpec {
                    name: "harness.run-js",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "harness.js.requested",
                },
                EventSpec {
                    kind: "harness.js.generated",
                },
                EventSpec {
                    kind: "harness.js.completed",
                },
                EventSpec {
                    kind: "harness.js.failed",
                },
            ],
            queries: Vec::new(),
            resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "harness.generate-app" => {
                let parsed = parse_harness_args(args, 4)?;
                let draft_id = builder::validate_id(&parsed.required[0], "draft id")?;
                let app_id = builder::validate_id(&parsed.required[1], "app id")?;
                let name = non_empty(parsed.required[2].clone(), "app name")?;
                let prompt = non_empty(parsed.tail, "prompt")?;
                Ok(Decision::Effect(Effect::GenerateAppWithHarness {
                    draft_id,
                    app_id,
                    name,
                    harness: parsed.harness,
                    prompt,
                }))
            }
            "harness.run-js" => {
                let parsed = parse_harness_args(args, 3)?;
                let run_id = builder::validate_id(&parsed.required[0], "run id")?;
                let app_id = builder::validate_id(&parsed.required[1], "app id")?;
                ensure_app_exists(ctx.bus, &app_id)?;
                let prompt = non_empty(parsed.tail, "prompt")?;
                Ok(Decision::Effect(Effect::RunHarnessJs {
                    run_id,
                    app_id,
                    harness: parsed.harness,
                    prompt,
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        events::fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        events::describe(record)
    }
}

#[cfg(test)]
mod tests;
