//! The `builder` capability — replayable app-generation drafts.
//!
//! Builder owns draft state and validates generated bundle files. Agent-specific
//! prompting/execution lives in sibling capabilities such as `codex`.

mod events;
mod json;
mod types;
mod validation;

use terrane_domain::{EventRecord, Result};

use super::Capability;
use crate::{Decision, State};

pub use events::{failed_event, generated_event, requested_event};
pub use json::draft_json;
pub use types::{BuilderDraft, BuilderFile, BuilderState};
pub use validation::{parse_generated_files, validate_files, validate_id};

pub struct BuilderCapability;

impl Capability for BuilderCapability {
    fn namespace(&self) -> &'static str {
        "builder"
    }

    fn decide(&self, _state: &State, name: &str, _args: &[String]) -> Result<Decision> {
        Err(terrane_domain::Error::InvalidInput(format!(
            "unknown command: {name}"
        )))
    }

    fn fold(&self, state: &mut State, record: &EventRecord) -> Result<()> {
        events::fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        events::describe(record)
    }
}
