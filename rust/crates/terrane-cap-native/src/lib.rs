//! The `native` capability — async, replay-safe native OS request facts.

use terrane_cap_interface::{
    arg, state_ref, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error,
    EventPattern, EventRecord, EventSpec, GrantResourceSpec, QueryCtx, QuerySpec, QueryValue,
    ReadValue, ResourceReadCtx, Result, StateStore,
};

mod commands;
mod doc;
mod events;
mod operations;
mod resources;
mod types;

pub use events::{cancelled_event, completed_event, failed_event, platform_observed_event};
pub use operations::{
    operation_catalog, OperationCatalogEntry, OP_CLIPBOARD_WRITE_TEXT, OP_DIALOG_OPEN_FILE,
    OP_EXTERNAL_OPEN_URL, OP_NOTIFICATION_SHOW,
};
pub use types::{NativePlatformObservation, NativeRequestRecord, NativeRequestStatus, NativeState};

pub struct NativeCapability;

impl Capability for NativeCapability {
    fn namespace(&self) -> &'static str {
        "native"
    }

    fn manifest(&self) -> CapManifest {
        let mut commands = Vec::new();
        for name in operations::trusted_commands() {
            commands.push(CommandSpec { name });
        }
        for name in operations::app_callable_commands() {
            commands.push(CommandSpec { name });
        }
        CapManifest {
            commands,
            events: vec![
                EventSpec {
                    kind: "native.platform.observed",
                },
                EventSpec {
                    kind: "native.requested",
                },
                EventSpec {
                    kind: "native.completed",
                },
                EventSpec {
                    kind: "native.failed",
                },
                EventSpec {
                    kind: "native.cancelled",
                },
            ],
            queries: vec![QuerySpec {
                name: "native.supports",
            }],
            resources: resources::resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "native",
                &["read", "write"],
                "Async native OS request queue.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::native_doc(include_internal)
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
            "supports" => {
                let operation = arg(args, 0, "operation id")?;
                let state = state_ref::<NativeState>(ctx.state, "native")?;
                let supported = state
                    .active_host_id
                    .as_ref()
                    .and_then(|host| state.platforms.get(host))
                    .is_some_and(|platform| platform.supported_operations.contains(&operation));
                Ok(QueryValue::Bool(supported))
            }
            other => Err(Error::InvalidInput(format!(
                "unknown query: native.{other}"
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
}

#[cfg(test)]
mod tests;
