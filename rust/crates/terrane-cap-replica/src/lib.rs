//! The `replica` capability — this home's stable identity.
//!
//! A multi-user system needs each replica (each `TERRANE_HOME`) to author its
//! CRDT edits under a *distinct, stable* Loro PeerID — distinct so two replicas
//! never collide on `(peer, counter)` and lose a write on merge, stable so all of
//! one home's edits are attributable to one peer and the oplog stays compact.
//!
//! The id is non-deterministic (it must be globally unique), so it's minted the
//! same way every other edge effect is: `replica.init` returns an
//! [`Effect::NewReplicaId`](crate::Effect) that the edge fills with OS entropy,
//! and the result is recorded once as a `replica.initialized` event. Replay reads
//! the id back from the log — never re-mints it — so identity is replay-stable.
//! The `crdt` capability reads [`ReplicaState::peer`] and authors under it.

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::Capability;
use terrane_cap_interface::{
    decode_event, encode_event, restore_state, snapshot_state, state_mut, state_ref, CapManifest,
    CommandCtx, CommandSpec, Decision, Effect, Error, EventRecord, EventSpec, QueryCtx, QuerySpec,
    QueryValue, Result, StateStore,
};

mod doc;

/// This capability's slice of State: the home's PeerID, once minted.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ReplicaState {
    pub peer: Option<u64>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Initialized {
    peer: u64,
}

pub struct ReplicaCapability;

impl Capability for ReplicaCapability {
    fn namespace(&self) -> &'static str {
        "replica"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec {
                name: "replica.init",
            }],
            events: vec![EventSpec {
                kind: "replica.initialized",
            }],
            queries: vec![QuerySpec {
                name: "replica.peer",
            }],
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::replica_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, _args: &[String]) -> Result<Decision> {
        match name {
            // Idempotent: a home mints its identity exactly once. Re-running is a
            // no-op so callers can "ensure identity" cheaply before any write.
            "replica.init" => {
                if state_ref::<ReplicaState>(ctx.state, "replica")?
                    .peer
                    .is_some()
                {
                    Ok(Decision::Commit(vec![]))
                } else {
                    Ok(Decision::Effect(Effect::NewReplicaId))
                }
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, _args: &[String]) -> Result<QueryValue> {
        match name {
            "peer" => Ok(QueryValue::U64(
                state_ref::<ReplicaState>(ctx.state, "replica")?.peer,
            )),
            other => Err(Error::InvalidInput(format!(
                "unknown query: replica.{other}"
            ))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        if record.kind == "replica.initialized" {
            let e: Initialized = decode_event(record)?;
            // First identity wins — guard against a duplicated init event ever
            // changing a home's peer on replay.
            let state = state_mut::<ReplicaState>(state, "replica")?;
            if state.peer.is_none() {
                state.peer = Some(e.peer);
            }
        }
        Ok(())
    }

    fn snapshot(&self, state: &dyn StateStore) -> Result<Option<Vec<u8>>> {
        snapshot_state::<ReplicaState>(state, self.namespace())
    }

    fn restore(&self, state: &mut dyn StateStore, payload: &[u8]) -> Result<()> {
        restore_state::<ReplicaState>(state, self.namespace(), payload)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind == "replica.initialized" {
            let e: Initialized = decode_event(record).ok()?;
            return Some(format!("replica.initialized peer={:#x}", e.peer));
        }
        None
    }
}

/// Build the `replica.initialized` event from an edge-minted PeerID. Called by an
/// [`EffectRunner`](crate::EffectRunner) handling [`Effect::NewReplicaId`].
pub fn initialized_event(peer: u64) -> Result<EventRecord> {
    encode_event("replica.initialized", &Initialized { peer })
}

#[cfg(test)]
mod tests;
