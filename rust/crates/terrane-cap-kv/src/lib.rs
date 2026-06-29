//! The `kv` capability — a per-app key/value store. Reacts to `app.removed` by
//! dropping that app's data.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord,
    EventSpec, ReadValue, ResourceReadCtx, Result, StateStore,
};

mod commands;
mod events;
mod resources;
mod storage;
mod types;

pub use storage::{sync_full_storage, sync_storage_after_commit};
pub use types::{
    storage_binding, storage_plan, KvState, KvStorageBackend, KvStorageBinding, KvStoragePlan,
    KvStorageState,
};

pub struct KvCapability;

impl Capability for KvCapability {
    fn namespace(&self) -> &'static str {
        "kv"
    }

    /// The app-scoped key/value surface backends get on `ctx.resource.kv`.
    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "kv.set" },
                CommandSpec { name: "kv.rm" },
                CommandSpec { name: "kv.delete" },
                CommandSpec {
                    name: "kv.storage.set",
                },
                CommandSpec {
                    name: "kv.storage.clear",
                },
            ],
            events: vec![
                EventSpec { kind: "kv.set" },
                EventSpec { kind: "kv.deleted" },
                EventSpec {
                    kind: "kv.storage.configured",
                },
                EventSpec {
                    kind: "kv.storage.cleared",
                },
            ],
            queries: Vec::new(),
            resources: resources::resource_methods(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "kv.set" => commands::decide_set(ctx, args),
            "kv.rm" | "kv.delete" => commands::decide_delete(ctx, args),
            "kv.storage.set" => commands::decide_storage_set(ctx, args),
            "kv.storage.clear" => commands::decide_storage_clear(ctx, args),
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
        resources::read(ctx, name, args)
    }
}

#[cfg(test)]
mod tests;
