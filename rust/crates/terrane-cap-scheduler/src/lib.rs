//! The `scheduler` capability — app-owned schedule definitions and host-owned
//! run facts for replay-stable scheduled actions.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, EventPattern, EventRecord,
    EventSpec, GrantResourceSpec, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod commands;
mod cron;
mod doc;
mod events;
mod resources;
mod types;

pub use cron::next_due_after;
pub use events::{run_completed_event, run_failed_event, run_started_event};
pub use resources::schedules_due_at;
pub use types::{RunRecord, RunStatus, ScheduleRecord, SchedulerState};

pub struct SchedulerCapability;

impl Capability for SchedulerCapability {
    fn namespace(&self) -> &'static str {
        "scheduler"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "scheduler.create",
                },
                CommandSpec {
                    name: "scheduler.pause",
                },
                CommandSpec {
                    name: "scheduler.resume",
                },
                CommandSpec {
                    name: "scheduler.remove",
                },
                CommandSpec {
                    name: "scheduler.run.start",
                },
                CommandSpec {
                    name: "scheduler.run.complete",
                },
                CommandSpec {
                    name: "scheduler.run.fail",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "scheduler.created",
                },
                EventSpec {
                    kind: "scheduler.paused",
                },
                EventSpec {
                    kind: "scheduler.resumed",
                },
                EventSpec {
                    kind: "scheduler.removed",
                },
                EventSpec {
                    kind: "scheduler.run.started",
                },
                EventSpec {
                    kind: "scheduler.run.completed",
                },
                EventSpec {
                    kind: "scheduler.run.failed",
                },
            ],
            queries: Vec::new(),
            resources: resources::resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "scheduler",
                &["read", "write"],
                "App-owned schedule definitions and run history.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::scheduler_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        commands::decide(ctx, name, args)
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
        resources::read(ctx, name, args)
    }

    fn resource_api(&self) -> Vec<ResourceMethod> {
        self.manifest().resources
    }
}

#[cfg(test)]
mod tests;
