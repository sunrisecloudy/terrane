//! The `scheduler` capability — app-owned schedule definitions and host-owned
//! run facts for replay-stable scheduled actions.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, EventPattern, EventRecord,
    EventSpec, GrantResourceSpec, QueryCtx, QuerySpec, QueryValue, ReadValue, ResourceMethod,
    ResourceReadCtx, Result, StateStore,
};

mod commands;
mod cron;
mod doc;
mod events;
mod resources;
mod types;

pub use cron::next_after;
pub use events::fired_event;
pub use resources::schedules_due_at;
pub use types::{DueSchedule, ScheduleEntry, ScheduleKind, ScheduleSpec, SchedulerState};

pub struct SchedulerCapability;

impl Capability for SchedulerCapability {
    fn namespace(&self) -> &'static str {
        "scheduler"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "scheduler.set",
                },
                CommandSpec {
                    name: "scheduler.clear",
                },
                CommandSpec {
                    name: "scheduler.fire",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "scheduler.set",
                },
                EventSpec {
                    kind: "scheduler.cleared",
                },
                EventSpec {
                    kind: "scheduler.fired",
                },
            ],
            queries: vec![QuerySpec {
                name: "scheduler.due",
            }],
            resources: resources::resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "scheduler",
                &["read", "write"],
                "Schedule backend wake-ups (cron / one-shot).",
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

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "due" => {
                let now_ms = terrane_cap_interface::arg(args, 0, "now_ms")?
                    .parse::<u64>()
                    .map_err(|_| {
                        terrane_cap_interface::Error::InvalidInput(
                            "now_ms must be an unsigned integer".into(),
                        )
                    })?;
                let state = terrane_cap_interface::state_ref::<SchedulerState>(
                    ctx.state,
                    "scheduler",
                )?;
                let due = resources::schedules_due_at(state, now_ms)?;
                let json = serde_json::Value::Array(
                    due.into_iter()
                        .map(|item| {
                            serde_json::json!({
                                "app": item.app,
                                "name": item.name,
                                "scheduled_for": item.scheduled_for,
                                "skipped": item.skipped,
                            })
                        })
                        .collect(),
                )
                .to_string();
                Ok(QueryValue::Json(json))
            }
            other => Err(terrane_cap_interface::Error::InvalidInput(format!(
                "unknown query: scheduler.{other}"
            ))),
        }
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
