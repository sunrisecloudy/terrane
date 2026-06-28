use std::any::Any;

use terrane_cap_interface::CapBus;

use super::*;

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
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        Err(Error::InvalidInput(format!("unknown query: {cap}.{name}")))
    }
}

#[test]
fn initialized_event_folds_only_once() {
    let mut store = Store::default();
    let cap = ReplicaCapability;
    cap.fold(&mut store, &initialized_event(7).unwrap())
        .unwrap();
    cap.fold(&mut store, &initialized_event(8).unwrap())
        .unwrap();

    assert_eq!(store.replica.peer, Some(7));
}

#[test]
fn init_is_an_effect_until_peer_exists_then_noops() {
    let mut store = Store::default();
    let bus = NoBus;
    assert_eq!(
        ReplicaCapability
            .decide(
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

    store.replica.peer = Some(10);
    assert_eq!(
        ReplicaCapability
            .decide(
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
