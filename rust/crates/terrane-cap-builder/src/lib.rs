//! The `builder` capability — replayable app-generation drafts.
//!
//! Builder owns draft state and validates generated bundle files. Agent-specific
//! prompting/execution lives in sibling capabilities such as `harness`.

mod events;
mod json;
mod types;
mod validation;

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, Decision, EventRecord, EventSpec, Result, StateStore,
};

pub use events::{failed_event, generated_event, requested_event};
pub use json::draft_json;
pub use types::{BuilderDraft, BuilderFile, BuilderState};
pub use validation::{parse_generated_files, validate_files, validate_id};

pub struct BuilderCapability;

impl Capability for BuilderCapability {
    fn namespace(&self) -> &'static str {
        "builder"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: Vec::new(),
            events: vec![
                EventSpec {
                    kind: "builder.requested",
                },
                EventSpec {
                    kind: "builder.generated",
                },
                EventSpec {
                    kind: "builder.failed",
                },
            ],
            queries: Vec::new(),
            resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn decide(&self, _ctx: CommandCtx<'_>, name: &str, _args: &[String]) -> Result<Decision> {
        Err(terrane_cap_interface::Error::InvalidInput(format!(
            "unknown command: {name}"
        )))
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
