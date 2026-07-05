use std::any::Any;

use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryValue, StateStore,
};
use terrane_cap_publish::{
    identity_created_event, installed_event, trusted_event, PublishCapability, PublishState,
};

const PUBKEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const SIG: &str = "BAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBA==";
const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn publish_install_decides_signed_bundle_effect() {
    let cap = PublishCapability;
    let mut state = Store::default();
    let Decision::Effect(Effect::InstallSignedBundle { source }) = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &NoBus,
            },
            "publish.install",
            &["demo.terrane".to_string()],
        )
        .unwrap()
    else {
        panic!("publish.install should request InstallSignedBundle");
    };
    assert_eq!(source, "demo.terrane");

    let err = cap
        .decide(
            CommandCtx {
                state: &state,
                bus: &NoBus,
            },
            "publish.install",
            &[],
        )
        .unwrap_err();
    assert!(err.to_string().contains("archive path"));
    assert!(state.get_mut("publish").is_some());
}

#[test]
fn publish_events_fold_trust_provenance_and_removal() {
    let cap = PublishCapability;
    let mut state = Store::default();
    cap.fold(&mut state, &identity_created_event(PUBKEY, "0x2a").unwrap())
        .unwrap();
    cap.fold(&mut state, &trusted_event(PUBKEY, "veha").unwrap())
        .unwrap();
    cap.fold(
        &mut state,
        &installed_event("demo", "1.2.3", HASH, PUBKEY, "veha").unwrap(),
    )
    .unwrap();

    let publish = state.get("publish").unwrap().downcast_ref::<PublishState>().unwrap();
    assert_eq!(publish.identity.as_deref(), Some(PUBKEY));
    assert_eq!(publish.trusted.get(PUBKEY).map(String::as_str), Some("veha"));
    assert_eq!(publish.provenance["demo"].version, "1.2.3");

    let removed = terrane_cap_interface::encode_event("app.removed", &Removed {
        id: "demo".to_string(),
    })
    .unwrap();
    cap.fold(&mut state, &removed).unwrap();
    let publish = state.get("publish").unwrap().downcast_ref::<PublishState>().unwrap();
    assert!(!publish.provenance.contains_key("demo"));
    assert!(publish.trusted.contains_key(PUBKEY));
}

#[test]
fn publish_validation_rejects_bad_public_material() {
    assert!(trusted_event("short", "veha").is_err());
    assert!(trusted_event(PUBKEY, "").is_err());
    assert!(installed_event("bad/app", "1.0.0", HASH, PUBKEY, "veha").is_err());
    assert!(terrane_cap_publish::validate_signature(SIG).is_ok());
}

#[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
struct Removed {
    id: String,
}

#[derive(Default)]
struct Store {
    publish: PublishState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "publish" => Some(&self.publish),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "publish" => Some(&mut self.publish),
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
