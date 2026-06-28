use std::any::Any;

use borsh::BorshSerialize;
use terrane_cap_crdt::{crdt_export_hex, crdt_list_strings, CrdtCapability, CrdtState};
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Error, QueryValue, ReadValue,
    ResourceReadCtx, StateStore,
};

#[derive(Default)]
struct Store {
    crdt: CrdtState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "crdt" => Some(&self.crdt),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "crdt" => Some(&mut self.crdt),
            _ => None,
        }
    }
}

struct Bus {
    app_exists: bool,
    peer: Option<u64>,
}

impl CapBus for Bus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(self.app_exists)),
            ("replica", "peer") => Ok(QueryValue::U64(self.peer)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[derive(BorshSerialize)]
struct Removed {
    id: String,
}

fn commit_one(decision: Decision) -> terrane_cap_interface::EventRecord {
    let Decision::Commit(events) = decision else {
        panic!("expected committed CRDT update");
    };
    assert_eq!(events.len(), 1);
    events.into_iter().next().unwrap()
}

#[test]
fn crdt_capability_records_map_list_and_text_updates() {
    let cap = CrdtCapability;
    let bus = Bus {
        app_exists: true,
        peer: Some(5),
    };
    let mut store = Store::default();

    for (name, args) in [
        (
            "crdt.mapSet",
            vec!["demo", "profile", "name", "Ada Lovelace"],
        ),
        ("crdt.listPush", vec!["demo", "todos", "ship tests"]),
        ("crdt.textInsert", vec!["demo", "notes", "0", "hello"]),
    ] {
        let event = commit_one(
            cap.decide(
                CommandCtx {
                    state: &store,
                    bus: &bus,
                },
                name,
                &args.into_iter().map(String::from).collect::<Vec<_>>(),
            )
            .unwrap(),
        );
        cap.fold(&mut store, &event).unwrap();
    }

    let ctx = ResourceReadCtx {
        state: &store,
        bus: &bus,
        app: "demo",
    };
    assert_eq!(
        cap.read_resource(ctx, "mapGet", &["profile".into(), "name".into()])
            .unwrap(),
        ReadValue::OptString(Some("Ada Lovelace".into()))
    );
    assert_eq!(
        cap.read_resource(ctx, "listAll", &["todos".into()])
            .unwrap(),
        ReadValue::StringList(vec!["ship tests".into()])
    );
    assert_eq!(
        cap.read_resource(ctx, "textGet", &["notes".into()])
            .unwrap(),
        ReadValue::OptString(Some("hello".into()))
    );
}

#[test]
fn crdt_capability_exports_merges_and_cleans_removed_apps() {
    let cap = CrdtCapability;
    let bus = Bus {
        app_exists: true,
        peer: Some(9),
    };
    let mut source = Store::default();
    let mut target = Store::default();

    let event = commit_one(
        cap.decide(
            CommandCtx {
                state: &source,
                bus: &bus,
            },
            "crdt.listPush",
            &["demo".into(), "items".into(), "one".into()],
        )
        .unwrap(),
    );
    cap.fold(&mut source, &event).unwrap();

    let update_hex = crdt_export_hex(&source, "demo", &target).unwrap().unwrap();
    let merge = commit_one(
        cap.decide(
            CommandCtx {
                state: &target,
                bus: &bus,
            },
            "crdt.merge",
            &["demo".into(), update_hex],
        )
        .unwrap(),
    );
    cap.fold(&mut target, &merge).unwrap();
    assert_eq!(
        crdt_list_strings(&target, "demo", "items"),
        vec!["one".to_string()]
    );

    cap.fold(
        &mut target,
        &encode_event("app.removed", &Removed { id: "demo".into() }).unwrap(),
    )
    .unwrap();
    assert!(crdt_list_strings(&target, "demo", "items").is_empty());
}
