use std::any::Any;

use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryCtx, QueryValue, StateStore,
};
use terrane_cap_replica::{initialized_event, ReplicaCapability, ReplicaState};

#[derive(Default)]
struct Store {
    replica: ReplicaState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "replica" => Some(&self.replica),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "replica" => Some(&mut self.replica),
            _ => None,
        }
    }
}

struct NoBus;

impl CapBus for NoBus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        Err(Error::InvalidInput(format!("unknown query: {cap}.{name}")))
    }
}

#[test]
fn replica_capability_initializes_and_queries_peer() {
    let cap = ReplicaCapability;
    let bus = NoBus;
    let mut store = Store::default();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "replica.init",
            &[],
        )
        .unwrap(),
        Decision::Effect(Effect::NewReplicaId)
    );

    let record = initialized_event(0xabc).unwrap();
    assert_eq!(record.kind, "replica.initialized");
    cap.fold(&mut store, &record).unwrap();

    assert_eq!(
        cap.query(
            QueryCtx {
                state: &store,
                bus: &bus,
            },
            "peer",
            &[],
        )
        .unwrap(),
        QueryValue::U64(Some(0xabc))
    );
    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "replica.init",
            &[],
        )
        .unwrap(),
        Decision::Commit(Vec::new())
    );
}
