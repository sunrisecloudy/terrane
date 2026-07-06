//! Engine tests for `org`: recorded edge keygen/signing effects and replay
//! identity from public events only.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use ed25519_dalek::{Signer, SigningKey};
use tempfile::tempdir;
use terrane_cap_org::{
    invite_redeemed_event, member_granted_event, org_id_for_pubkey, role_grant_message,
};
use terrane_cap_person::{created_event, hex};
use terrane_core::{Core, Effect, EffectRunner, EventRecord, QueryValue, Result, State};

use crate::helpers::req;

struct OrgEdge {
    keys: RefCell<HashMap<String, SigningKey>>,
    roles: RefCell<HashMap<String, SigningKey>>,
    next_person_seed: Cell<u8>,
    next_org_seed: Cell<u8>,
}

impl OrgEdge {
    fn new() -> Self {
        Self {
            keys: RefCell::new(HashMap::new()),
            roles: RefCell::new(HashMap::new()),
            next_person_seed: Cell::new(1),
            next_org_seed: Cell::new(200),
        }
    }

    fn mint_signing(&self, next_seed: &Cell<u8>) -> SigningKey {
        let seed = next_seed.get();
        next_seed.set(seed.wrapping_add(1));
        SigningKey::from_bytes(&[seed; 32])
    }
}

impl EffectRunner for OrgEdge {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::PersonKeygen => {
                let signing = self.mint_signing(&self.next_person_seed);
                let pubkey = hex(signing.verifying_key().as_bytes());
                let person_id = terrane_cap_person::person_id_for_pubkey(&pubkey)?;
                self.keys.borrow_mut().insert(person_id, signing);
                Ok(vec![created_event(&pubkey)?])
            }
            Effect::PersonSign {
                person_id,
                kind,
                claim,
            } => {
                let signing = self
                    .keys
                    .borrow()
                    .get(person_id)
                    .ok_or_else(|| terrane_core::Error::Runtime(format!("unknown person: {person_id}")))?
                    .clone();
                let message = terrane_cap_person::attestation_message(person_id, kind, claim)?;
                let sig = signing.sign(&message);
                Ok(vec![terrane_cap_person::attested_event(
                    person_id,
                    kind,
                    claim,
                    &hex(&sig.to_bytes()),
                )?])
            }
            Effect::OrgKeygen { founder } => {
                let signing = self.mint_signing(&self.next_org_seed);
                let pubkey = hex(signing.verifying_key().as_bytes());
                let org_id = org_id_for_pubkey(&pubkey)?;
                self.roles.borrow_mut().insert(org_id.clone(), signing);
                let mut records = vec![terrane_cap_org::created_event(&org_id, &pubkey, founder)?];
                if let Some(founder_key) = self.keys.borrow().get(founder).cloned() {
                    let message = role_grant_message(&org_id, founder, "owner")?;
                    records.push(member_granted_event(
                        &org_id,
                        founder,
                        "owner",
                        &hex(&founder_key.sign(&message).to_bytes()),
                        founder,
                    )?);
                }
                Ok(records)
            }
            Effect::OrgRoleSign {
                org_id,
                member,
                role,
                signer,
                redeem_token_hash,
            } => {
                let signing = self
                    .keys
                    .borrow()
                    .get(signer)
                    .ok_or_else(|| terrane_core::Error::Runtime(format!("unknown signer person: {signer}")))?
                    .clone();
                let message = role_grant_message(org_id, member, role)?;
                let sig = signing.sign(&message);
                let mut records = vec![member_granted_event(
                    org_id,
                    member,
                    role,
                    &hex(&sig.to_bytes()),
                    signer,
                )?];
                if let Some(token_hash) = redeem_token_hash {
                    records.push(invite_redeemed_event(org_id, token_hash, member)?);
                }
                Ok(records)
            }
            other => Err(terrane_core::Error::Runtime(format!(
                "unexpected effect: {other:?}"
            ))),
        }
    }
}

#[test]
fn org_create_invite_join_role_set_leave_replays_from_public_events() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), OrgEdge::new()).unwrap();

    // Founding person.
    core.dispatch(req("person.create", &[])).unwrap();
    let founder_id = core
        .state()
        .person
        .persons
        .keys()
        .next()
        .cloned()
        .unwrap();

    // Org.create emits org.created + org.member.granted (owner).
    let records = core.dispatch(req("org.create", &[&founder_id])).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, "org.created");
    assert_eq!(records[1].kind, "org.member.granted");
    // Read org_id via the folded state instead of decoding borsh manually.
    let org_id = core.state().org.orgs.keys().next().cloned().unwrap();
    assert!(core.state().org.members[&(org_id.clone(), founder_id.clone())].active);
    assert_eq!(
        core.state().org.members[&(org_id.clone(), founder_id.clone())].role,
        "owner"
    );

    // Query surface.
    let QueryValue::Json(info) = core.query("org", "info", &[]).unwrap() else {
        panic!("org.info should return JSON");
    };
    assert!(info.contains(&org_id));
    let QueryValue::Json(members) = core
        .query("org", "members", [org_id.clone()].as_slice())
        .unwrap()
    else {
        panic!("org.members should return JSON");
    };
    assert!(members.contains(&founder_id));

    // Open an invite for an admin role and redeem it as the founder. The edge
    // runner mints no second person (person.create is idempotent on a home),
    // so we exercise the join path by having the founder redeem their own
    // invite: the join self-signs with the founder key and records
    // member.granted + invite.redeemed.
    let token_hash = "ab".repeat(32);
    core.dispatch(req(
        "org.invite",
        &[&org_id, "admin", &token_hash, "promote me"],
    ))
    .unwrap();
    assert!(core.state().org.invites[&(org_id.clone(), token_hash.clone())].open);

    let join_records = core
        .dispatch(req("org.join", &[&org_id, &token_hash, &founder_id]))
        .unwrap();
    assert_eq!(join_records.len(), 2);
    assert_eq!(join_records[0].kind, "org.member.granted");
    assert_eq!(join_records[1].kind, "org.invite.redeemed");
    assert!(!core.state().org.invites[&(org_id.clone(), token_hash)].open);
    assert_eq!(
        core.state().org.members[&(org_id.clone(), founder_id.clone())].role,
        "admin"
    );
    assert_eq!(
        core.state().org.members[&(org_id.clone(), founder_id.clone())].signer,
        founder_id
    );

    // The admin restores the owner role via role.set, self-signing.
    core.dispatch(req(
        "org.role.set",
        &[&org_id, &founder_id, "owner", &founder_id],
    ))
    .unwrap();
    assert_eq!(
        core.state().org.members[&(org_id.clone(), founder_id.clone())].role,
        "owner"
    );

    // Member leaves.
    core.dispatch(req("org.leave", &[&org_id, &founder_id]))
        .unwrap();
    assert!(!core.state().org.members[&(org_id, founder_id)].active);

    // Replay identity: re-reading the log must rebuild byte-for-byte equal state.
    assert!(core.replay_matches().unwrap());
}

#[test]
fn org_member_granted_with_tampered_signature_is_rejected_by_fold() {
    // Use OrgEdge to mint a person + org, then inject a forged grant event and
    // confirm fold rejects it (signature verifies against the signer's pubkey).
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), OrgEdge::new()).unwrap();

    core.dispatch(req("person.create", &[])).unwrap();
    let founder_id = core
        .state()
        .person
        .persons
        .keys()
        .next()
        .cloned()
        .unwrap();
    core.dispatch(req("org.create", &[&founder_id])).unwrap();
    let org_id = core.state().org.orgs.keys().next().cloned().unwrap();

    // A forged signature by a different key over the same message.
    let wrong = SigningKey::from_bytes(&[222; 32]);
    let forged = hex(
        &wrong
            .sign(&role_grant_message(&org_id, &founder_id, "owner").unwrap())
            .to_bytes(),
    );
    let forged_event = member_granted_event(&org_id, &founder_id, "owner", &forged, &founder_id)
        .unwrap();

    // Simulate a host-applied forged record: dispatch returns the records, the
    // engine folds them; here we test the fold directly through a dispatch-less
    // path by re-applying via the trait method on fresh state.
    let mut state = State::default();
    // Re-fold everything the engine has folded so far to seed person pubkey.
    for record in core.log_records().unwrap() {
        terrane_core::fold_records_in_memory(&mut state, &[record]).unwrap();
    }
    let err = terrane_core::fold_records_in_memory(&mut state, &[forged_event]).unwrap_err();
    assert!(matches!(err, terrane_core::Error::InvalidInput(_)));
}