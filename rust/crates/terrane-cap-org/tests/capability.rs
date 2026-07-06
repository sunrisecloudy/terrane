//! Public-surface integration tests for the `org` capability.

use ed25519_dalek::{Signer, SigningKey};

use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, Effect, Error, ExecutionPrincipal, QueryCtx,
    QueryValue, Request, StateStore,
};
use terrane_cap_org::{
    created_event, invited_event, invite_redeemed_event, member_granted_event, member_left_event,
    OrgCapability, OrgRecord, OrgState,
};
use terrane_cap_person::{PersonRecord, PersonState};

fn cap() -> OrgCapability {
    OrgCapability
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

#[test]
fn manifest_lists_org_commands_events_queries() {
    let manifest = cap().manifest();
    assert_eq!(manifest.commands.len(), 5);
    assert_eq!(manifest.events.len(), 5);
    assert_eq!(manifest.queries.len(), 2);
}

#[test]
fn validation_helpers_reject_invalid_inputs() {
    assert!(matches!(terrane_cap_org::validate_org_id("short"), Err(Error::InvalidInput(_))));
    assert!(terrane_cap_org::validate_role("owner").is_ok());
    assert!(matches!(terrane_cap_org::validate_role("guest"), Err(Error::InvalidInput(_))));
    assert!(matches!(
        terrane_cap_org::validate_token_hash("nothex"),
        Err(Error::InvalidInput(_))
    ));
    assert!(terrane_cap_org::org_id_for_pubkey(&"0".repeat(64)).is_ok());
    assert!(matches!(
        terrane_cap_org::validate_note(&"x".repeat(terrane_cap_org::MAX_INVITE_NOTE_BYTES + 1)),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn role_grant_message_is_stable() {
    let message =
        terrane_cap_org::role_grant_message("0123456789abcdef", "aabbccddeeff0011", "owner").unwrap();
    assert_eq!(
        message,
        b"terrane.org.role.v1\norg_id=0123456789abcdef\nmember=aabbccddeeff0011\nrole=owner"
    );
}

#[test]
fn signed_role_grant_round_trips_and_rejects_tampered_role_or_member() {
    let signing = SigningKey::from_bytes(&[7; 32]);
    let pubkey = terrane_cap_org::hex(signing.verifying_key().as_bytes());
    let org_id = terrane_cap_org::org_id_for_pubkey(&pubkey).unwrap();
    let member = "aabbccddeeff0011".to_string();
    let sig = terrane_cap_org::hex(
        &signing
            .sign(&terrane_cap_org::role_grant_message(&org_id, &member, "admin").unwrap())
            .to_bytes(),
    );
    assert!(terrane_cap_org::verify_role_sig(&pubkey, &org_id, &member, "admin", &sig).is_ok());
    // Tampering with the role breaks verification.
    assert!(terrane_cap_org::verify_role_sig(&pubkey, &org_id, &member, "owner", &sig).is_err());
    // Tampering with the member breaks verification.
    assert!(terrane_cap_org::verify_role_sig(&pubkey, &org_id, "aabbccddeeff0012", "admin", &sig).is_err());
}

#[test]
fn event_constructors_validate_payloads() {
    assert!(terrane_cap_org::created_event("bad", &"0".repeat(64), "aabbccddeeff0011").is_err());
    assert!(
        terrane_cap_org::created_event("0123456789abcdef", &"0".repeat(64), "bad").is_err()
    );
    assert!(
        terrane_cap_org::invited_event("0123456789abcdef", "guest", &"0".repeat(64), "").is_err()
    );
    assert!(
        terrane_cap_org::invited_event(
            "0123456789abcdef",
            "owner",
            &"0".repeat(64),
            &"x".repeat(terrane_cap_org::MAX_INVITE_NOTE_BYTES + 1)
        )
        .is_err()
    );
}

#[test]
fn doc_has_commands_events_and_internal_notes() {
    let doc = cap().doc(true);
    assert_eq!(doc.namespace, "org");
    assert!(!doc.commands.is_empty());
    assert!(!doc.events.is_empty());
    assert!(doc.internal.iter().any(|note| note.title == "Secret storage"));
    let public = cap().doc(false);
    assert!(public.internal.is_empty());
}

#[test]
fn request_carries_org_principal_field() {
    let principal = ExecutionPrincipal {
        org: "0123456789abcdef".to_string(),
        subject: "user:aabbccddeeff0011".to_string(),
        source: "sync".to_string(),
    };
    let request = Request::trusted_host("org.info", vec![]).with_principal(principal.clone());
    assert_eq!(request.principal, principal);
}

fn seed_person(person: &mut PersonState, person_id: &str, pubkey: &str) {
    person.persons.insert(
        person_id.to_string(),
        PersonRecord {
            person_id: person_id.to_string(),
            pubkey: pubkey.to_string(),
            attestations: Default::default(),
            rotated_to: None,
        },
    );
}

#[test]
fn fold_member_granted_requires_known_signer_person() {
    let signing = SigningKey::from_bytes(&[9; 32]);
    let pubkey = terrane_cap_org::hex(signing.verifying_key().as_bytes());
    let org_id = terrane_cap_org::org_id_for_pubkey(&pubkey).unwrap();
    let founder = "aabbccddeeff0011".to_string();
    let created = created_event(&org_id, &pubkey, &founder).unwrap();
    let sig = terrane_cap_org::hex(
        &signing
            .sign(&terrane_cap_org::role_grant_message(&org_id, &founder, "owner").unwrap())
            .to_bytes(),
    );
    let granted = member_granted_event(&org_id, &founder, "owner", &sig, &founder).unwrap();

    let mut state = OrgState::default();
    let mut person = PersonState::default();

    // Fold the grant before the signer exists in person state -> signer lookup fails.
    {
        let mut store = TwoSlice { org: &mut state, person: &mut person };
        let err = cap().fold(&mut store as &mut dyn StateStore, &granted).unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    seed_person(&mut person, &founder, &pubkey);
    {
        let mut store = TwoSlice { org: &mut state, person: &mut person };
        cap().fold(&mut store as &mut dyn StateStore, &created).unwrap();
        cap().fold(&mut store as &mut dyn StateStore, &granted).unwrap();
    }

    assert!(state.orgs.contains_key(&org_id));
    assert!(state.members.contains_key(&(org_id.clone(), founder.clone())));
    assert_eq!(state.members[&(org_id.clone(), founder.clone())].role, "owner");

    // A leave event marks the membership inactive.
    let left = member_left_event(&org_id, &founder).unwrap();
    {
        let mut store = TwoSlice { org: &mut state, person: &mut person };
        cap().fold(&mut store as &mut dyn StateStore, &left).unwrap();
    }
    assert!(!state.members[&(org_id, founder)].active);
}

#[test]
fn fold_rejects_tampered_signer_in_member_granted() {
    let signing = SigningKey::from_bytes(&[7; 32]);
    let pubkey = terrane_cap_org::hex(signing.verifying_key().as_bytes());
    let org_id = terrane_cap_org::org_id_for_pubkey(&pubkey).unwrap();
    let founder = "aabbccddeeff0011".to_string();
    let created = created_event(&org_id, &pubkey, &founder).unwrap();
    let wrong = SigningKey::from_bytes(&[123; 32]);
    let bad_sig = terrane_cap_org::hex(
        &wrong
            .sign(&terrane_cap_org::role_grant_message(&org_id, &founder, "owner").unwrap())
            .to_bytes(),
    );
    let granted = member_granted_event(&org_id, &founder, "owner", &bad_sig, &founder).unwrap();

    let mut state = OrgState::default();
    let mut person = PersonState::default();
    seed_person(&mut person, &founder, &pubkey);
    {
        let mut store = TwoSlice { org: &mut state, person: &mut person };
        cap().fold(&mut store as &mut dyn StateStore, &created).unwrap();
        let err = cap().fold(&mut store as &mut dyn StateStore, &granted).unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)));
    }
    assert!(state.orgs.contains_key(&org_id));
    assert!(!state.members.contains_key(&(org_id, founder)));
}

#[test]
fn fold_invited_redeemed_marks_invite_closed() {
    let org_id = "0123456789abcdef".to_string();
    let token_hash = "ab".repeat(32);
    let member = "aabbccddeeff0011".to_string();
    let invited = invited_event(&org_id, "member", &token_hash, "join me").unwrap();
    let redeemed = invite_redeemed_event(&org_id, &token_hash, &member).unwrap();

    let mut state = OrgState::default();
    state.orgs.insert(
        org_id.clone(),
        OrgRecord {
            org_id: org_id.clone(),
            pubkey: "ab".repeat(32),
            founder: member.clone(),
        },
    );
    let mut person = PersonState::default();
    {
        let mut store = TwoSlice { org: &mut state, person: &mut person };
        cap().fold(&mut store as &mut dyn StateStore, &invited).unwrap();
    }
    assert!(state.invites[&(org_id.clone(), token_hash.clone())].open);
    {
        let mut store = TwoSlice { org: &mut state, person: &mut person };
        cap().fold(&mut store as &mut dyn StateStore, &redeemed).unwrap();
    }
    assert!(!state.invites[&(org_id, token_hash)].open);
}

#[test]
fn decide_create_returns_org_keygen_effect_when_founder_known() {
    let mut state = OrgState::default();
    let mut person = PersonState::default();
    let mut store = TwoSlice { org: &mut state, person: &mut person };
    let ctx = CommandCtx {
        state: &mut store as &mut dyn StateStore,
        bus: &FakeBus,
    };
    let founder = "aabbccddeeff0011".to_string();
    let decision = cap().decide(ctx, "org.create", &args(&[&founder])).unwrap();
    assert!(matches!(decision, Decision::Effect(Effect::OrgKeygen { founder: f }) if f == founder));
}

#[test]
fn decide_role_set_requires_existing_org() {
    let mut state = OrgState::default();
    let mut person = PersonState::default();
    let mut store = TwoSlice { org: &mut state, person: &mut person };
    let ctx = CommandCtx {
        state: &mut store as &mut dyn StateStore,
        bus: &FakeBus,
    };
    let err = cap()
        .decide(
            ctx,
            "org.role.set",
            &args(&["0123456789abcdef", "aabbccddeeff0011", "owner", "aabbccddeeff0011"]),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[test]
fn decide_join_requires_an_open_invite() {
    let mut state = OrgState::default();
    state.orgs.insert(
        "0123456789abcdef".to_string(),
        OrgRecord {
            org_id: "0123456789abcdef".to_string(),
            pubkey: "ab".repeat(32),
            founder: "aabbccddeeff0011".to_string(),
        },
    );
    let mut person = PersonState::default();
    let mut store = TwoSlice { org: &mut state, person: &mut person };
    let ctx = CommandCtx {
        state: &mut store as &mut dyn StateStore,
        bus: &FakeBus,
    };
    let err = cap()
        .decide(
            ctx,
            "org.join",
            &args(&["0123456789abcdef", &"cd".repeat(32), "aabbccddeeff0011"]),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(_)));
}

#[test]
fn query_info_and_members_return_json_or_empty() {
    let org_id = "0123456789abcdef".to_string();
    let founder = "aabbccddeeff0011".to_string();
    let mut state = OrgState::default();
    state.orgs.insert(
        org_id.clone(),
        OrgRecord {
            org_id: org_id.clone(),
            pubkey: "ab".repeat(32),
            founder: founder.clone(),
        },
    );
    state.members.insert(
        (org_id.clone(), founder.clone()),
        terrane_cap_org::OrgMember {
            org_id: org_id.clone(),
            member: founder.clone(),
            role: "owner".to_string(),
            sig: "ef".repeat(32),
            signer: founder.clone(),
            active: true,
        },
    );
    let person = PersonState::default();
    let store = TwoSliceRead { org: &state, person: &person };
    let ctx = QueryCtx {
        state: &store as &dyn StateStore,
        bus: &FakeBus,
    };
    let info = cap().query(ctx, "info", &[]).unwrap();
    assert!(matches!(info, QueryValue::Json(ref json) if json.contains(&org_id)));
    let members = cap().query(ctx, "members", &args(&[&org_id])).unwrap();
    assert!(matches!(members, QueryValue::Json(ref json) if json.contains(&founder)));
}

struct TwoSlice<'a> {
    org: &'a mut OrgState,
    person: &'a mut PersonState,
}

impl<'a> StateStore for TwoSlice<'a> {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        match namespace {
            "org" => Some(self.org),
            "person" => Some(self.person),
            _ => None,
        }
    }
    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        match namespace {
            "org" => Some(self.org),
            "person" => Some(self.person),
            _ => None,
        }
    }
}

struct TwoSliceRead<'a> {
    org: &'a OrgState,
    person: &'a PersonState,
}

impl<'a> StateStore for TwoSliceRead<'a> {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        match namespace {
            "org" => Some(self.org),
            "person" => Some(self.person),
            _ => None,
        }
    }
    fn get_mut(&mut self, _namespace: &str) -> Option<&mut dyn std::any::Any> {
        None
    }
}

struct FakeBus;

impl CapBus for FakeBus {
    fn query(&self, _cap: &str, _name: &str, _args: &[String]) -> terrane_cap_interface::Result<QueryValue> {
        Err(Error::Runtime("fake bus".into()))
    }
}