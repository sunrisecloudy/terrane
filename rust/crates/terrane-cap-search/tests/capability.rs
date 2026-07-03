use terrane_cap_interface::{
    Capability, CommandCtx, Decision, Error, QueryValue, ReadValue, ResourceReadCtx, StateStore,
};
use terrane_core::Core;

use terrane_cap_search::SearchCapability;

struct TestBus;
impl terrane_cap_interface::CapBus for TestBus {
    fn query(
        &self,
        _cap: &str,
        _name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<terrane_cap_interface::QueryValue> {
        Ok(QueryValue::Bool(true))
    }
}

fn ctx<'a>(state: &'a dyn StateStore) -> CommandCtx<'a> {
    CommandCtx {
        state,
        bus: &TestBus,
    }
}

#[test]
fn decide_upsert_commits_only_kv_events() {
    let mut core = Core::open(tempfile::NamedTempFile::new().unwrap().path()).unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "app.add",
        vec!["notes".into(), "Notes".into()],
    ))
    .unwrap();

    let decision = SearchCapability
        .decide(
            ctx(core.state()),
            "search.upsert",
            &[
                "notes".into(),
                "doc-1".into(),
                "hello world".into(),
            ],
        )
        .unwrap();
    let Decision::Commit(records) = decision else {
        panic!("expected commit");
    };
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "kv.set");
}

#[test]
fn read_resource_status_reports_document_count() {
    let mut core = Core::open(tempfile::NamedTempFile::new().unwrap().path()).unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "app.add",
        vec!["notes".into(), "Notes".into()],
    ))
    .unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "search.upsert",
        vec!["notes".into(), "doc-1".into(), "hello".into()],
    ))
    .unwrap();

    let bus = TestBus;
    let ctx = ResourceReadCtx {
        state: core.state(),
        bus: &bus,
        app: "notes",
        host: None,
    };
    let ReadValue::OptString(Some(raw)) = SearchCapability
        .read_resource(ctx, "status", &[])
        .unwrap()
    else {
        panic!("expected status");
    };
    let status: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(status["documentCount"], 1);
}

#[test]
fn manifest_has_grant_spec_and_empty_fold() {
    let manifest = SearchCapability.manifest();
    assert_eq!(manifest.events.len(), 0);
    assert_eq!(manifest.grant_resources.len(), 1);
    assert_eq!(manifest.grant_resources[0].namespace, "search");
}

#[test]
fn remove_deletes_doc_and_embedding_keys() {
    let mut core = Core::open(tempfile::NamedTempFile::new().unwrap().path()).unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "app.add",
        vec!["notes".into(), "Notes".into()],
    ))
    .unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "search.upsert",
        vec!["notes".into(), "doc-1".into(), "hello".into()],
    ))
    .unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "search.setEmbedding",
        vec!["notes".into(), "doc-1".into(), "[0.5,0.5]".into()],
    ))
    .unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "search.remove",
        vec!["notes".into(), "doc-1".into()],
    ))
    .unwrap();

    let remaining = core.state().kv.data.get("notes");
    assert!(
        remaining.is_none_or(|app_kv| app_kv.keys().all(|k| !k.contains("doc-1")))
    );
}

#[test]
fn unknown_command_is_invalid_input() {
    let core = Core::open(tempfile::NamedTempFile::new().unwrap().path()).unwrap();
    let err = SearchCapability
        .decide(ctx(core.state()), "search.nope", &[])
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[test]
fn app_removed_clears_search_projection_via_kv() {
    let mut core = Core::open(tempfile::NamedTempFile::new().unwrap().path()).unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "app.add",
        vec!["notes".into(), "Notes".into()],
    ))
    .unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "search.upsert",
        vec!["notes".into(), "doc-1".into(), "hello".into()],
    ))
    .unwrap();
    core.dispatch(terrane_core::Request::trusted_host(
        "app.remove",
        vec!["notes".into()],
    ))
    .unwrap();
    assert!(!core.state().kv.data.contains_key("notes"));
}