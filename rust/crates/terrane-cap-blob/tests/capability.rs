use terrane_cap_blob::{stored_event, BlobCapability, BlobState};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Effect, QueryValue, StateStore,
};

#[derive(Default)]
struct TestState {
    blob: BlobState,
}

impl StateStore for TestState {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        (namespace == "blob").then_some(&self.blob as &dyn std::any::Any)
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        (namespace == "blob").then_some(&mut self.blob as &mut dyn std::any::Any)
    }
}

struct AppsExist;

impl CapBus for AppsExist {
    fn query(
        &self,
        _cap: &str,
        _name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        Ok(QueryValue::Bool(true))
    }
}

#[test]
fn put_effect_contains_bytes_but_event_does_not() {
    let cap = BlobCapability;
    let state = TestState::default();
    let bus = AppsExist;
    let ctx = CommandCtx {
        state: &state,
        bus: &bus,
    };

    let decision = cap
        .decide(
            ctx,
            "blob.put",
            &[
                "app".into(),
                "avatar.png".into(),
                "image/png".into(),
                "aGVsbG8=".into(),
            ],
        )
        .unwrap();

    let Decision::Effect(Effect::BlobStore {
        app,
        name,
        mime,
        hash,
        bytes,
    }) = decision
    else {
        panic!("blob.put should produce BlobStore effect");
    };
    assert_eq!(app, "app");
    assert_eq!(name, "avatar.png");
    assert_eq!(mime, "image/png");
    assert_eq!(
        hash,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert_eq!(bytes, b"hello");

    let event = stored_event(app, name, hash, 5, mime).unwrap();
    assert!(!String::from_utf8_lossy(&event.payload).contains("hello"));
}
