use std::any::Any;

use terrane_cap_connection::{all_statuses, split_secret_ref, ConnectionCapability, ConnectionState};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Error, QueryValue, StateStore,
};

#[derive(Default)]
struct Store {
    connection: ConnectionState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "connection" => Some(&self.connection),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "connection" => Some(&mut self.connection),
            _ => None,
        }
    }
}

struct Bus;

impl CapBus for Bus {
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
fn define_authorize_refresh_remove_replays_metadata_only() {
    let cap = ConnectionCapability;
    let bus = Bus;
    let mut store = Store::default();

    let Decision::Commit(define) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "connection.define",
            &["github".into(), "apiKey".into(), "{}".into()],
        )
        .unwrap()
    else {
        panic!("connection.define should commit");
    };
    cap.fold(&mut store, &define[0]).unwrap();

    let Decision::Commit(auth) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "connection.mark_authorized",
            &["github".into(), "repo,user".into(), "2030-01-01T00:00:00Z".into()],
        )
        .unwrap()
    else {
        panic!("connection.mark_authorized should commit");
    };
    assert_eq!(auth[0].kind, "connection.authorized");
    cap.fold(&mut store, &auth[0]).unwrap();

    let Decision::Commit(refresh) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "connection.mark_authorized",
            &["github".into(), "ignored".into(), "2030-01-02T00:00:00Z".into()],
        )
        .unwrap()
    else {
        panic!("connection.mark_authorized refresh should commit");
    };
    assert_eq!(refresh[0].kind, "connection.refreshed");
    cap.fold(&mut store, &refresh[0]).unwrap();

    let statuses = all_statuses(&store).unwrap();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].name, "github");
    assert_eq!(statuses[0].kind, "apiKey");
    assert!(statuses[0].authorized);
    assert_eq!(statuses[0].scopes, vec!["repo".to_string(), "user".to_string()]);
    assert_eq!(
        statuses[0].expires_at.as_deref(),
        Some("2030-01-02T00:00:00Z")
    );

    let serialized = format!("{:?}{:?}", define, auth);
    assert!(!serialized.contains("raw-secret"));

    let Decision::Commit(remove) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "connection.remove",
            &["github".into()],
        )
        .unwrap()
    else {
        panic!("connection.remove should commit");
    };
    cap.fold(&mut store, &remove[0]).unwrap();
    assert!(all_statuses(&store).unwrap().is_empty());
}

#[test]
fn validates_names_public_config_and_secret_refs() {
    let cap = ConnectionCapability;
    let store = Store::default();
    let bus = Bus;

    assert!(cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "connection.define",
            &["GitHub".into(), "apiKey".into(), "{}".into()],
        )
        .unwrap_err()
        .to_string()
        .contains("[a-z0-9-_]"));

    assert!(cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "connection.define",
            &["github".into(), "apiKey".into(), r#"{"key":"secret"}"#.into()],
        )
        .unwrap_err()
        .to_string()
        .contains("must not contain secret field"));

    assert_eq!(
        split_secret_ref("smtp-default.password").unwrap(),
        ("smtp-default".to_string(), "password".to_string())
    );
    assert_eq!(
        split_secret_ref("openai").unwrap(),
        ("openai".to_string(), "key".to_string())
    );
}
