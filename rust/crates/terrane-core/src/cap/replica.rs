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

use crate::{Error, EventRecord, Result};
use borsh::{BorshDeserialize, BorshSerialize};

use super::Capability;
use crate::{decode_event, encode_event, Decision, Effect, State};

/// This capability's slice of State: the home's PeerID, once minted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

    fn decide(&self, state: &State, name: &str, _args: &[String]) -> Result<Decision> {
        match name {
            // Idempotent: a home mints its identity exactly once. Re-running is a
            // no-op so callers can "ensure identity" cheaply before any write.
            "replica.init" => {
                if state.replica.peer.is_some() {
                    Ok(Decision::Commit(vec![]))
                } else {
                    Ok(Decision::Effect(Effect::NewReplicaId))
                }
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut State, record: &EventRecord) -> Result<()> {
        if record.kind == "replica.initialized" {
            let e: Initialized = decode_event(record)?;
            // First identity wins — guard against a duplicated init event ever
            // changing a home's peer on replay.
            if state.replica.peer.is_none() {
                state.replica.peer = Some(e.peer);
            }
        }
        Ok(())
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
