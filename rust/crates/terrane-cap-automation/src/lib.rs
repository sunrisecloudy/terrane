//! The `automation` capability — app-owned event-triggered rules and host-owned
//! firing/suppression facts for replay-stable automations.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, EventPattern, EventRecord,
    EventSpec, GrantResourceSpec, QueryCtx, QuerySpec, QueryValue, ReadValue, ResourceMethod,
    ResourceReadCtx, Result, StateStore,
};

mod commands;
mod doc;
mod events;
pub mod matcher;
mod resources;
mod types;

pub use events::decode_fired;
pub use matcher::{event_json, event_ref, matching_rules, render_args, PER_COMMIT_FIRE_BUDGET};
pub use types::{AutomationState, FireStats, MatchEvent, MatchingRule, RuleEntry, RuleSpec};

pub struct AutomationCapability;

impl Capability for AutomationCapability {
    fn namespace(&self) -> &'static str {
        "automation"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "automation.set",
                },
                CommandSpec {
                    name: "automation.rm",
                },
                CommandSpec {
                    name: "automation.fire",
                },
                CommandSpec {
                    name: "automation.suppress",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "automation.set",
                },
                EventSpec {
                    kind: "automation.removed",
                },
                EventSpec {
                    kind: "automation.fired",
                },
                EventSpec {
                    kind: "automation.suppressed",
                },
            ],
            queries: vec![
                QuerySpec {
                    name: "automation.list",
                },
                QuerySpec {
                    name: "automation.stat",
                },
            ],
            resources: resources::resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "automation",
                &["read", "write"],
                "App-scoped event automation rules.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::automation_doc(include_internal)
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
            "list" => {
                let app = terrane_cap_interface::arg(args, 0, "app")?;
                terrane_cap_interface::ensure_app_exists(ctx.bus, &app)?;
                match resources::read_list(ctx.state, &app)? {
                    ReadValue::StringMap(map) => Ok(QueryValue::Json(
                        serde_json::Value::Object(
                            map.into_iter()
                                .map(|(key, value)| {
                                    let parsed = serde_json::from_str(&value)
                                        .unwrap_or(serde_json::Value::Null);
                                    (key, parsed)
                                })
                                .collect(),
                        )
                        .to_string(),
                    )),
                    other => Err(terrane_cap_interface::Error::Runtime(format!(
                        "automation.list returned unexpected value: {other:?}"
                    ))),
                }
            }
            "stat" => {
                let app = terrane_cap_interface::arg(args, 0, "app")?;
                terrane_cap_interface::ensure_app_exists(ctx.bus, &app)?;
                let name = terrane_cap_interface::arg(args, 1, "name")?;
                match resources::read_stat(ctx.state, &app, &name)? {
                    ReadValue::OptString(Some(value)) => Ok(QueryValue::Json(value)),
                    ReadValue::OptString(None) => Ok(QueryValue::Json("null".to_string())),
                    other => Err(terrane_cap_interface::Error::Runtime(format!(
                        "automation.stat returned unexpected value: {other:?}"
                    ))),
                }
            }
            other => Err(terrane_cap_interface::Error::InvalidInput(format!(
                "unknown query: automation.{other}"
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
