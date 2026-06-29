//! The `kv` capability — a per-app key/value store. Reacts to `app.removed` by
//! dropping that app's data.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord,
    EventSpec, ReadValue, ResourceReadCtx, Result, StateStore,
};

mod commands;
mod doc;
mod events;
mod resources;
mod storage;
mod types;

pub const RESERVED_PREFIX: &str = "__terrane/";
pub const APP_BUNDLE_KEY_PREFIX: &str = "__terrane/app-bundle/";
pub const APP_BUNDLE_SOURCE_PREFIX: &str = "kv://app-bundle/";
pub const DEFAULT_KV_STORAGE_PATH: &str = "terrane.db";
pub const LOG_VALUE_PREVIEW_CHARS: usize = 50;
pub(crate) const DEFAULT_SCAN_LIMIT: usize = 100;
pub(crate) const MAX_SCAN_LIMIT: usize = 500;

pub use events::{delete_event, set_event, storage_configured_event};
pub use storage::{sync_full_storage, sync_storage_after_commit};
pub(crate) use types::bounded_limit;
pub use types::{
    app_bundle_app_id, app_bundle_files, app_bundle_key, app_bundle_source, delete_prefix_events,
    get_value, is_reserved_key, scan_prefix, scan_range, storage_binding, storage_plan, KvState,
    KvStorageBackend, KvStorageBinding, KvStoragePlan, KvStorageState,
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

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::kv_doc(include_internal)
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
