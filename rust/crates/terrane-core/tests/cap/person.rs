//! Engine tests for `person`: recorded edge keygen/signing effects and replay
//! identity from public events only.

use std::cell::Cell;

use ed25519_dalek::{Signer, SigningKey};
use tempfile::tempdir;
use terrane_cap_person::{
    attestation_message, attested_event, created_event, hex, person_id_for_pubkey, rotated_event,
    rotation_message,
};
use terrane_core::{Core, Effect, EffectRunner, EventRecord, QueryValue, State};

use crate::helpers::req;

struct PersonEdge {
    create_seed: u8,
    rotate_seed: u8,
    created: Cell<bool>,
}

impl PersonEdge {
    fn new(create_seed: u8, rotate_seed: u8) -> Self {
        Self {
            create_seed,
            rotate_seed,
            created: Cell::new(false),
        }
    }
}

impl EffectRunner for PersonEdge {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::PersonKeygen => {
                self.created.set(true);
                let signing = SigningKey::from_bytes(&[self.create_seed; 32]);
                Ok(vec![created_event(&hex(signing.verifying_key().as_bytes()))?])
            }
            Effect::PersonSign {
                person_id,
                kind,
                claim,
            } => {
                let signing = SigningKey::from_bytes(&[self.create_seed; 32]);
                let sig = signing.sign(&attestation_message(person_id, kind, claim)?);
                Ok(vec![attested_event(
                    person_id,
                    kind,
                    claim,
                    &hex(&sig.to_bytes()),
                )?])
            }
            Effect::PersonRotate { person_id } => {
                let old = SigningKey::from_bytes(&[self.create_seed; 32]);
                let new = SigningKey::from_bytes(&[self.rotate_seed; 32]);
                let new_pubkey = hex(new.verifying_key().as_bytes());
                let sig = old.sign(&rotation_message(person_id, &new_pubkey)?);
                Ok(vec![rotated_event(person_id, &new_pubkey, &hex(&sig.to_bytes()))?])
            }
            other => Err(terrane_core::Error::Runtime(format!(
                "unexpected effect: {other:?}"
            ))),
        }
    }
}

#[test]
fn person_create_attest_rotate_replays_from_public_events() {
    let dir = tempdir().unwrap();
    let mut core =
        Core::open_with(dir.path().join("log.bin"), PersonEdge::new(11, 12)).unwrap();

    let created = core.dispatch(req("person.create", &[])).unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].kind, "person.created");
    let old_pubkey = hex(SigningKey::from_bytes(&[11; 32]).verifying_key().as_bytes());
    let person_id = person_id_for_pubkey(&old_pubkey).unwrap();
    assert!(core.state().person.persons.contains_key(&person_id));

    let attested = core
        .dispatch(req(
            "person.attest",
            &[&person_id, "email", "me@example.test"],
        ))
        .unwrap();
    assert_eq!(attested[0].kind, "person.attested");
    assert_eq!(
        core.state().person.persons[&person_id]
            .attestations
            .values()
            .filter(|att| !att.revoked)
            .count(),
        1
    );

    let rotated = core.dispatch(req("person.rotate", &[&person_id])).unwrap();
    assert_eq!(rotated[0].kind, "person.rotated");
    let new_pubkey = hex(SigningKey::from_bytes(&[12; 32]).verifying_key().as_bytes());
    assert_eq!(core.state().person.persons[&person_id].pubkey, new_pubkey);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn person_queries_return_json_or_null() {
    let dir = tempdir().unwrap();
    let mut core =
        Core::open_with(dir.path().join("log.bin"), PersonEdge::new(13, 14)).unwrap();

    assert_eq!(
        core.query("person", "whoami", &[]).unwrap(),
        QueryValue::Json("null".to_string())
    );
    core.dispatch(req("person.create", &[])).unwrap();
    let QueryValue::Json(json) = core.query("person", "whoami", &[]).unwrap() else {
        panic!("whoami should return JSON");
    };
    assert!(json.contains("person_id"));
    assert!(!json.contains(&hex(SigningKey::from_bytes(&[13; 32]).as_bytes())));
}
