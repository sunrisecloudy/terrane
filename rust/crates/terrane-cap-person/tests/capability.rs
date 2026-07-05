use std::any::Any;

use ed25519_dalek::{Signer, SigningKey};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryCtx, QueryValue, StateStore,
};
use terrane_cap_person::{
    attestation_message, attested_event, created_event, hex, person_id_for_pubkey, rotated_event,
    rotation_message, PersonCapability, PersonState,
};

#[derive(Default)]
struct Store {
    person: PersonState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "person" => Some(&self.person),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "person" => Some(&mut self.person),
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

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn ctx<'a>(store: &'a Store, bus: &'a NoBus) -> CommandCtx<'a> {
    CommandCtx { state: store, bus }
}

#[test]
fn create_attest_revoke_and_query_public_identity_only() {
    let cap = PersonCapability;
    let bus = NoBus;
    let mut store = Store::default();
    let signing = key(7);
    let pubkey = hex(signing.verifying_key().as_bytes());
    let person_id = person_id_for_pubkey(&pubkey).unwrap();

    assert_eq!(
        cap.decide(ctx(&store, &bus), "person.create", &[]).unwrap(),
        Decision::Effect(Effect::PersonKeygen)
    );

    let created = created_event(&pubkey).unwrap();
    cap.fold(&mut store, &created).unwrap();
    assert_eq!(store.person.persons[&person_id].pubkey, pubkey);

    assert_eq!(
        cap.decide(
            ctx(&store, &bus),
            "person.attest",
            &[person_id.clone(), "email".into(), "me@example.test".into()],
        )
        .unwrap(),
        Decision::Effect(Effect::PersonSign {
            person_id: person_id.clone(),
            kind: "email".into(),
            claim: "me@example.test".into(),
        })
    );

    let message = attestation_message(&person_id, "email", "me@example.test").unwrap();
    let sig = hex(&signing.sign(&message).to_bytes());
    let attested = attested_event(&person_id, "email", "me@example.test", &sig).unwrap();
    cap.fold(&mut store, &attested).unwrap();

    let QueryValue::Json(json) = cap
        .query(
            QueryCtx {
                state: &store,
                bus: &bus,
            },
            "whoami",
            &[],
        )
        .unwrap()
    else {
        panic!("person.whoami should return JSON");
    };
    assert!(json.contains(&person_id));
    assert!(json.contains("me@example.test"));
    assert!(!json.contains("secret"));
    assert!(!format!("{:?}", store.person).contains(&hex(signing.as_bytes())));

    let revoke = cap
        .decide(
            ctx(&store, &bus),
            "person.revoke-attestation",
            &[person_id.clone(), "email".into(), "me@example.test".into()],
        )
        .unwrap();
    let Decision::Commit(records) = revoke else {
        panic!("revoke should commit");
    };
    cap.fold(&mut store, &records[0]).unwrap();
    let QueryValue::Json(json) = cap
        .query(
            QueryCtx {
                state: &store,
                bus: &bus,
            },
            "get",
            std::slice::from_ref(&person_id),
        )
        .unwrap()
    else {
        panic!("person.get should return JSON");
    };
    assert!(!json.contains("me@example.test"));
}

#[test]
fn rotate_accepts_old_key_signature_and_rejects_bad_signature() {
    let cap = PersonCapability;
    let mut store = Store::default();
    let old = key(8);
    let new = key(9);
    let old_pubkey = hex(old.verifying_key().as_bytes());
    let new_pubkey = hex(new.verifying_key().as_bytes());
    let person_id = person_id_for_pubkey(&old_pubkey).unwrap();

    cap.fold(&mut store, &created_event(&old_pubkey).unwrap())
        .unwrap();

    let bad_sig = hex(&new.sign(&rotation_message(&person_id, &new_pubkey).unwrap()).to_bytes());
    let bad = rotated_event(&person_id, &new_pubkey, &bad_sig).unwrap();
    assert!(cap.fold(&mut store, &bad).unwrap_err().to_string().contains("signature"));

    let sig = hex(&old.sign(&rotation_message(&person_id, &new_pubkey).unwrap()).to_bytes());
    cap.fold(&mut store, &rotated_event(&person_id, &new_pubkey, &sig).unwrap())
        .unwrap();
    assert_eq!(store.person.persons[&person_id].pubkey, new_pubkey);
    assert_eq!(
        store.person.persons[&person_id].rotated_to.as_deref(),
        Some(new_pubkey.as_str())
    );
}

#[test]
fn validation_errors_are_typed() {
    let cap = PersonCapability;
    let store = Store::default();
    let bus = NoBus;

    assert!(matches!(
        cap.decide(
            ctx(&store, &bus),
            "person.attest",
            &["not-hex".into(), "email".into(), "me@example.test".into()],
        )
        .unwrap_err(),
        Error::InvalidInput(_)
    ));

    let signing = key(10);
    let pubkey = hex(signing.verifying_key().as_bytes());
    let mut store = Store::default();
    cap.fold(&mut store, &created_event(&pubkey).unwrap()).unwrap();
    let person_id = person_id_for_pubkey(&pubkey).unwrap();
    assert!(cap
        .decide(
            ctx(&store, &bus),
            "person.attest",
            &[person_id, "twitter".into(), "claim".into()],
        )
        .unwrap_err()
        .to_string()
        .contains("attestation kind"));
}
