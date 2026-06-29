//! The `crdt` capability — per-app conflict-free replicated documents backed by
//! [Loro](https://loro.dev). Where `kv` is a last-writer-wins string store, a
//! `crdt` document *merges*: two replicas that edited concurrently converge to
//! the same value with no lost writes. One Loro document per app holds named
//! Map, List, and Text containers.
//!
//! Loro is non-deterministic at the edge (a fresh op carries an author PeerID
//! and the export embeds it), so writes run once in `decide` on a fork of the
//! current doc and fold only imports the recorded bytes. Replay re-imports the
//! same bytes in the same order, so replay identity holds.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, EventPattern, EventRecord,
    EventSpec, ReadValue, ResourceReadCtx, Result, StateStore,
};

#[cfg(test)]
use terrane_cap_interface::Error;

mod commands;
mod doc;
mod events;
mod resources;
mod state;
mod sync;

pub use resources::crdt_list_strings;
pub use state::CrdtState;
pub use sync::{crdt_export_from_vv, crdt_export_hex, crdt_vv, to_hex};

pub struct CrdtCapability;

impl Capability for CrdtCapability {
    fn namespace(&self) -> &'static str {
        "crdt"
    }

    /// The app-scoped CRDT surface backends get on `ctx.resource.crdt`. Every
    /// method's first arg is a container name.
    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "crdt.mapSet",
                },
                CommandSpec {
                    name: "crdt.mapDel",
                },
                CommandSpec {
                    name: "crdt.listPush",
                },
                CommandSpec {
                    name: "crdt.listInsert",
                },
                CommandSpec {
                    name: "crdt.listDel",
                },
                CommandSpec {
                    name: "crdt.textInsert",
                },
                CommandSpec {
                    name: "crdt.textDel",
                },
                CommandSpec { name: "crdt.merge" },
            ],
            events: vec![EventSpec {
                kind: "crdt.update",
            }],
            queries: Vec::new(),
            resources: resources::resource_methods(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::crdt_doc(include_internal)
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
}

#[cfg(test)]
mod tests;
