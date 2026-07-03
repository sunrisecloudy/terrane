//! The `sysinfo` capability — live host system metrics (CPU, memory, disk,
//! network, battery, top processes) for monitoring apps.
//!
//! Metrics are **live reads**, not recorded effects. Each read samples the host
//! through the edge [`LiveHost`](terrane_cap_interface::LiveHost) and records
//! nothing, so replay-identity is preserved: a monitor polling every second
//! never grows the event log, and the app only ever replays what it explicitly
//! writes elsewhere. A pure core (no edge to sample from) returns an error, so
//! apps must feature-detect `ctx.resource.sysinfo` and degrade.
//!
//! Stateless by construction: no commands, no events, no `fold`, no `State`
//! slice. The whole capability is a thin, validated forwarder to the edge.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, Decision, Error, EventRecord, GrantResourceSpec, ReadValue,
    ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod doc;

/// The `ctx.resource.sysinfo.*` read methods, each forwarded to the edge under
/// the same name. `snapshot` returns every section at once; the rest are the
/// individual sections. Kept in lockstep with [`SysinfoCapability::manifest`].
pub const DOMAINS: &[&str] = &[
    "snapshot",
    "cpu",
    "memory",
    "disk",
    "network",
    "battery",
    "system",
    "processes",
];

pub struct SysinfoCapability;

impl Capability for SysinfoCapability {
    fn namespace(&self) -> &'static str {
        "sysinfo"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: Vec::new(),
            events: Vec::new(),
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Read {
                    name: "snapshot",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "cpu",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "memory",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "disk",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "network",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "battery",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "system",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "processes",
                    params: &["sortBy", "limit"],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "sysinfo",
                &["read"],
                "Live host system metrics: CPU, memory, disk, network, battery, and top \
                 processes. Sampled at the edge on each read and never recorded, so replay is \
                 unaffected.",
            )],
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::sysinfo_doc(include_internal)
    }

    fn decide(&self, _ctx: CommandCtx<'_>, name: &str, _args: &[String]) -> Result<Decision> {
        Err(Error::InvalidInput(format!("unknown command: {name}")))
    }

    fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
        // Stateless: sysinfo records no events, so replay has nothing to fold.
        Ok(())
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        if !DOMAINS.contains(&name) {
            return Err(Error::InvalidInput(format!(
                "unknown resource read: sysinfo.{name}"
            )));
        }
        let Some(host) = ctx.host else {
            return Err(Error::Runtime(
                "sysinfo reads need a live host to sample; this core has no edge".into(),
            ));
        };
        Ok(ReadValue::OptString(Some(host.sample(name, args)?)))
    }
}
