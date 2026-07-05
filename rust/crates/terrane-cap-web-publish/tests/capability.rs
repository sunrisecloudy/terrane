use std::any::Any;

use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Error, QueryValue, StateStore,
};
use terrane_cap_web_publish::{
    disabled_event, domain_set_event, enabled_event, status_json, validate_public_verbs,
    PublishMode, WebPublishCapability, WebPublishState,
};

#[test]
fn web_publish_records_enable_disable_and_domain_facts() {
    let cap = WebPublishCapability;
    let mut state = Store::default();
    let enabled = enabled_event("demo", PublishMode::Interactive, "demo-public").unwrap();
    cap.fold(&mut state, &enabled).unwrap();
    cap.fold(&mut state, &domain_set_event("demo", "demo.example.com").unwrap())
        .unwrap();

    let status = status_json(&state, &["demo".to_string()]).unwrap();
    assert!(status.contains(r#""enabled":true"#), "{status}");
    assert!(status.contains(r#""mode":"interactive""#), "{status}");
    assert!(status.contains(r#""domain":"demo.example.com""#), "{status}");

    cap.fold(&mut state, &disabled_event("demo").unwrap()).unwrap();
    let status = status_json(&state, &["demo".to_string()]).unwrap();
    assert!(status.contains(r#""enabled":false"#), "{status}");
}

#[test]
fn web_publish_decide_validates_app_mode_slug_and_domain() {
    let cap = WebPublishCapability;
    let mut state = Store::default();
    let decision = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &Bus,
            },
            "web-publish.enable",
            &[
                "demo".to_string(),
                "static".to_string(),
                "demo-live".to_string(),
            ],
        )
        .unwrap();
    let terrane_cap_interface::Decision::Commit(records) = decision else {
        panic!("web-publish.enable should commit facts");
    };
    cap.fold(&mut state, &records[0]).unwrap();

    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &Bus,
            },
            "web-publish.enable",
            &["demo".to_string(), "mutable".to_string()],
        )
        .unwrap_err();
    assert!(err.to_string().contains("static or interactive"));

    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &Bus,
            },
            "web-publish.domain.set",
            &["missing".to_string(), "demo.example.com".to_string()],
        )
        .unwrap_err();
    assert!(err.to_string().contains("unpublished app"));
}

#[test]
fn web_publish_replay_identity_and_app_removal_hold() {
    let cap = WebPublishCapability;
    let records = vec![
        enabled_event("demo", PublishMode::Static, "demo-random").unwrap(),
        domain_set_event("demo", "demo.example.com").unwrap(),
        encode_event(
            "app.removed",
            &Removed {
                id: "other".to_string(),
            },
        )
        .unwrap(),
    ];
    let mut original = Store::default();
    let mut replayed = Store::default();
    for record in &records {
        cap.fold(&mut original, record).unwrap();
    }
    for record in &records {
        cap.fold(&mut replayed, record).unwrap();
    }
    assert_eq!(original.web_publish, replayed.web_publish);

    cap.fold(
        &mut original,
        &encode_event(
            "app.removed",
            &Removed {
                id: "demo".to_string(),
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert!(original.web_publish.apps.is_empty());
}

#[test]
fn public_verbs_limit_and_safety_are_enforced() {
    let verbs = (0..16).map(|i| format!("public.verb{i}")).collect::<Vec<_>>();
    validate_public_verbs(&verbs).unwrap();

    let too_many = (0..17).map(|i| format!("public.verb{i}")).collect::<Vec<_>>();
    assert!(validate_public_verbs(&too_many)
        .unwrap_err()
        .to_string()
        .contains("publicVerbs exceeds 16"));

    assert!(validate_public_verbs(&["bad verb".to_string()]).is_err());
}

#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
struct Removed {
    id: String,
}

#[derive(Default)]
struct Store {
    web_publish: WebPublishState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "web-publish" => Some(&self.web_publish),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "web-publish" => Some(&mut self.web_publish),
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
        args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        if cap == "app" && name == "exists" && args.first().is_some_and(|app| app == "demo") {
            return Ok(QueryValue::Bool(true));
        }
        Err(Error::InvalidInput(format!("unknown query: {cap}.{name}")))
    }
}
