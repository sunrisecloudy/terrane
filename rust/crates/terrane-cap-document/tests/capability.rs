use std::any::Any;

use terrane_cap_document::{
    Document, DocumentCapability, DocumentState, MAX_DOCUMENTS_PER_APP,
};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Error, QueryValue, ReadValue, ResourceReadCtx,
    StateStore,
};

#[derive(Default)]
struct Store {
    document: DocumentState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "document" => Some(&self.document),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "document" => Some(&mut self.document),
            _ => None,
        }
    }
}

struct AppBus;

impl CapBus for AppBus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(true)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[test]
fn document_capability_decides_folds_and_reads_public_surface() {
    let cap = DocumentCapability;
    let bus = AppBus;
    let mut store = Store::default();
    let Decision::Commit(events) = cap
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "document.create",
            &[
                "notes".into(),
                "daily".into(),
                "Daily".into(),
                "body".into(),
                r#"{"draft":true}"#.into(),
            ],
        )
        .unwrap()
    else {
        panic!("document.create should commit");
    };
    cap.fold(&mut store, &events[0]).unwrap();

    let ctx = ResourceReadCtx {
        state: &store,
        bus: &bus,
        app: "notes",
        host: None,
    };
    let ReadValue::OptString(Some(json)) = cap
        .read_resource(ctx, "get", &["daily".into()])
        .unwrap()
    else {
        panic!("document.get should return JSON");
    };
    assert!(json.contains(r#""body":"body""#), "json: {json}");
}

#[test]
fn document_capability_enforces_per_app_quota_without_global_store() {
    let cap = DocumentCapability;
    let bus = AppBus;
    let mut store = Store::default();
    let docs = store.document.docs.entry("notes".into()).or_default();
    for i in 0..MAX_DOCUMENTS_PER_APP {
        let id = format!("doc-{i}");
        docs.insert(
            id.clone(),
            Document {
                id,
                title: "Title".into(),
                body: String::new(),
                metadata_json: "{}".into(),
                updated_seq: None,
            },
        );
    }

    assert!(matches!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "document.create",
            &[
                "notes".into(),
                "overflow".into(),
                "Title".into(),
                String::new(),
                "{}".into(),
            ],
        ),
        Err(Error::InvalidInput(_))
    ));
}
