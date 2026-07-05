//! The `person` capability — durable local identity as an ed25519 public key.
//!
//! The private key is never part of this capability's state or events. Commands
//! that need signing return edge effects; the host stores/uses the secret key in
//! the connection secret store and records only public keys, claims, and
//! signatures. Replay folds those public events and verifies signatures without
//! touching keychain material.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use terrane_cap_interface::{
    arg, decode_event, encode_event, restore_state, snapshot_state, state_mut, state_ref,
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Effect, Error, EventRecord,
    EventSpec, QueryCtx, QuerySpec, QueryValue, Result, StateStore,
};

mod doc;

pub const PUBKEY_BYTES: usize = 32;
pub const SIGNATURE_BYTES: usize = 64;
pub const PERSON_ID_HEX_LEN: usize = 16;
pub const MAX_ATTESTATIONS_PER_PERSON: usize = 128;
pub const MAX_CLAIM_LEN: usize = 512;

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PersonState {
    pub persons: BTreeMap<String, PersonRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PersonRecord {
    pub person_id: String,
    pub pubkey: String,
    pub attestations: BTreeMap<String, AttestationRecord>,
    pub rotated_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AttestationRecord {
    pub kind: String,
    pub claim: String,
    pub sig: String,
    pub revoked: bool,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Created {
    person_id: String,
    pubkey: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Attested {
    person_id: String,
    kind: String,
    claim: String,
    sig: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Revoked {
    person_id: String,
    kind: String,
    claim: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Rotated {
    old: String,
    new_pubkey: String,
    sig: String,
}

pub struct PersonCapability;

impl Capability for PersonCapability {
    fn namespace(&self) -> &'static str {
        "person"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "person.create",
                },
                CommandSpec {
                    name: "person.attest",
                },
                CommandSpec {
                    name: "person.revoke-attestation",
                },
                CommandSpec {
                    name: "person.rotate",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "person.created",
                },
                EventSpec {
                    kind: "person.attested",
                },
                EventSpec {
                    kind: "person.attestation-revoked",
                },
                EventSpec {
                    kind: "person.rotated",
                },
            ],
            queries: vec![
                QuerySpec {
                    name: "person.whoami",
                },
                QuerySpec { name: "person.get" },
            ],
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::person_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "person.create" => {
                if !args.is_empty() {
                    return Err(Error::InvalidInput(format!(
                        "person.create takes no arguments, got {}",
                        args.len()
                    )));
                }
                if primary_person(state_ref::<PersonState>(ctx.state, "person")?).is_some() {
                    Ok(Decision::Commit(Vec::new()))
                } else {
                    Ok(Decision::Effect(Effect::PersonKeygen))
                }
            }
            "person.attest" => {
                let person_id = validate_person_id(&arg(args, 0, "person_id")?)?;
                let kind = validate_attestation_kind(&arg(args, 1, "kind")?)?;
                let claim = validate_claim(&arg(args, 2, "claim")?)?;
                let state = state_ref::<PersonState>(ctx.state, "person")?;
                let person = state.persons.get(&person_id).ok_or_else(|| {
                    Error::InvalidInput(format!("unknown person: {person_id}"))
                })?;
                if !person.attestations.contains_key(&attestation_key(&kind, &claim))
                    && person.attestations.len() >= MAX_ATTESTATIONS_PER_PERSON
                {
                    return Err(Error::InvalidInput(format!(
                        "person attestation limit exceeded: max {MAX_ATTESTATIONS_PER_PERSON}"
                    )));
                }
                Ok(Decision::Effect(Effect::PersonSign {
                    person_id,
                    kind,
                    claim,
                }))
            }
            "person.revoke-attestation" => {
                let person_id = validate_person_id(&arg(args, 0, "person_id")?)?;
                let kind = validate_attestation_kind(&arg(args, 1, "kind")?)?;
                let claim = validate_claim(&arg(args, 2, "claim")?)?;
                if !state_ref::<PersonState>(ctx.state, "person")?
                    .persons
                    .contains_key(&person_id)
                {
                    return Err(Error::InvalidInput(format!(
                        "unknown person: {person_id}"
                    )));
                }
                Ok(Decision::Commit(vec![attestation_revoked_event(
                    &person_id, &kind, &claim,
                )?]))
            }
            "person.rotate" => {
                let person_id = validate_person_id(&arg(args, 0, "person_id")?)?;
                if !state_ref::<PersonState>(ctx.state, "person")?
                    .persons
                    .contains_key(&person_id)
                {
                    return Err(Error::InvalidInput(format!(
                        "unknown person: {person_id}"
                    )));
                }
                Ok(Decision::Effect(Effect::PersonRotate {
                    person_id,
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        let state = state_ref::<PersonState>(ctx.state, "person")?;
        match name {
            "whoami" => {
                let Some(person) = primary_person(state) else {
                    return Ok(QueryValue::Json("null".to_string()));
                };
                Ok(QueryValue::Json(person_json(person)?))
            }
            "get" => {
                let person_id = validate_person_id(&arg(args, 0, "person_id")?)?;
                match state.persons.get(&person_id) {
                    Some(person) => Ok(QueryValue::Json(person_json(person)?)),
                    None => Ok(QueryValue::Json("null".to_string())),
                }
            }
            other => Err(Error::InvalidInput(format!(
                "unknown query: person.{other}"
            ))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "person.created" => {
                let e: Created = decode_event(record)?;
                validate_person_id(&e.person_id)?;
                validate_pubkey(&e.pubkey)?;
                let expected = person_id_for_pubkey(&e.pubkey)?;
                if expected != e.person_id {
                    return Err(Error::InvalidInput(format!(
                        "person.created id/pubkey mismatch: expected {expected}"
                    )));
                }
                state_mut::<PersonState>(state, "person")?
                    .persons
                    .entry(e.person_id.clone())
                    .or_insert_with(|| PersonRecord {
                        person_id: e.person_id,
                        pubkey: e.pubkey,
                        attestations: BTreeMap::new(),
                        rotated_to: None,
                    });
            }
            "person.attested" => {
                let e: Attested = decode_event(record)?;
                validate_attestation_kind(&e.kind)?;
                validate_claim(&e.claim)?;
                validate_signature(&e.sig)?;
                let person = state_mut::<PersonState>(state, "person")?
                    .persons
                    .get_mut(&e.person_id)
                    .ok_or_else(|| Error::InvalidInput(format!("unknown person: {}", e.person_id)))?;
                verify_attestation_sig(&person.pubkey, &e.person_id, &e.kind, &e.claim, &e.sig)?;
                person.attestations.insert(
                    attestation_key(&e.kind, &e.claim),
                    AttestationRecord {
                        kind: e.kind,
                        claim: e.claim,
                        sig: e.sig,
                        revoked: false,
                    },
                );
            }
            "person.attestation-revoked" => {
                let e: Revoked = decode_event(record)?;
                let person = state_mut::<PersonState>(state, "person")?
                    .persons
                    .get_mut(&e.person_id)
                    .ok_or_else(|| Error::InvalidInput(format!("unknown person: {}", e.person_id)))?;
                if let Some(attestation) = person.attestations.get_mut(&attestation_key(&e.kind, &e.claim)) {
                    attestation.revoked = true;
                }
            }
            "person.rotated" => {
                let e: Rotated = decode_event(record)?;
                validate_person_id(&e.old)?;
                validate_pubkey(&e.new_pubkey)?;
                validate_signature(&e.sig)?;
                let person = state_mut::<PersonState>(state, "person")?
                    .persons
                    .get_mut(&e.old)
                    .ok_or_else(|| Error::InvalidInput(format!("unknown person: {}", e.old)))?;
                verify_rotation_sig(person, &e.new_pubkey, &e.sig)?;
                person.rotated_to = Some(e.new_pubkey.clone());
                person.pubkey = e.new_pubkey;
            }
            _ => {}
        }
        Ok(())
    }

    fn snapshot(&self, state: &dyn StateStore) -> Result<Option<Vec<u8>>> {
        snapshot_state::<PersonState>(state, self.namespace())
    }

    fn restore(&self, state: &mut dyn StateStore, payload: &[u8]) -> Result<()> {
        restore_state::<PersonState>(state, self.namespace(), payload)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "person.created" => decode_event::<Created>(record)
                .ok()
                .map(|e| format!("person.created {}", e.person_id)),
            "person.attested" => decode_event::<Attested>(record)
                .ok()
                .map(|e| format!("person.attested {} {}={}", e.person_id, e.kind, e.claim)),
            "person.attestation-revoked" => decode_event::<Revoked>(record)
                .ok()
                .map(|e| format!("person.attestation-revoked {} {}={}", e.person_id, e.kind, e.claim)),
            "person.rotated" => decode_event::<Rotated>(record)
                .ok()
                .map(|e| format!("person.rotated {}", e.old)),
            _ => None,
        }
    }
}

pub fn created_event(pubkey: &str) -> Result<EventRecord> {
    let pubkey = validate_pubkey(pubkey)?;
    let person_id = person_id_for_pubkey(&pubkey)?;
    encode_event("person.created", &Created { person_id, pubkey })
}

pub fn attested_event(person_id: &str, kind: &str, claim: &str, sig: &str) -> Result<EventRecord> {
    encode_event(
        "person.attested",
        &Attested {
            person_id: validate_person_id(person_id)?,
            kind: validate_attestation_kind(kind)?,
            claim: validate_claim(claim)?,
            sig: validate_signature(sig)?,
        },
    )
}

pub fn attestation_revoked_event(person_id: &str, kind: &str, claim: &str) -> Result<EventRecord> {
    encode_event(
        "person.attestation-revoked",
        &Revoked {
            person_id: validate_person_id(person_id)?,
            kind: validate_attestation_kind(kind)?,
            claim: validate_claim(claim)?,
        },
    )
}

pub fn rotated_event(old: &str, new_pubkey: &str, sig: &str) -> Result<EventRecord> {
    encode_event(
        "person.rotated",
        &Rotated {
            old: validate_person_id(old)?,
            new_pubkey: validate_pubkey(new_pubkey)?,
            sig: validate_signature(sig)?,
        },
    )
}

pub fn person_id_for_pubkey(pubkey: &str) -> Result<String> {
    let bytes = decode_hex_exact(pubkey, PUBKEY_BYTES, "pubkey")?;
    let digest = Sha256::digest(bytes);
    Ok(hex(&digest)[..PERSON_ID_HEX_LEN].to_string())
}

pub fn attestation_message(person_id: &str, kind: &str, claim: &str) -> Result<Vec<u8>> {
    Ok(format!(
        "terrane.person.attest.v1\nperson_id={}\nkind={}\nclaim={}",
        validate_person_id(person_id)?,
        validate_attestation_kind(kind)?,
        validate_claim(claim)?
    )
    .into_bytes())
}

pub fn rotation_message(person_id: &str, new_pubkey: &str) -> Result<Vec<u8>> {
    Ok(format!(
        "terrane.person.rotate.v1\nperson_id={}\nnew_pubkey={}",
        validate_person_id(person_id)?,
        validate_pubkey(new_pubkey)?
    )
    .into_bytes())
}

pub fn verify_attestation_sig(
    pubkey: &str,
    person_id: &str,
    kind: &str,
    claim: &str,
    sig: &str,
) -> Result<()> {
    verify_sig(pubkey, &attestation_message(person_id, kind, claim)?, sig)
}

pub fn verify_rotation_sig(person: &PersonRecord, new_pubkey: &str, sig: &str) -> Result<()> {
    let message = rotation_message(&person.person_id, new_pubkey)?;
    if verify_sig(&person.pubkey, &message, sig).is_ok() {
        return Ok(());
    }
    for attestation in person.attestations.values() {
        if attestation.kind == "device-key"
            && !attestation.revoked
            && validate_pubkey(&attestation.claim).is_ok()
            && verify_sig(&attestation.claim, &message, sig).is_ok()
        {
            return Ok(());
        }
    }
    Err(Error::InvalidInput(
        "person.rotated signature is not valid for current or attested device key".into(),
    ))
}

pub fn validate_person_id(person_id: &str) -> Result<String> {
    if person_id.len() != PERSON_ID_HEX_LEN
        || !person_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::InvalidInput(format!(
            "person_id must be {PERSON_ID_HEX_LEN} hex chars"
        )));
    }
    Ok(person_id.to_ascii_lowercase())
}

pub fn validate_pubkey(pubkey: &str) -> Result<String> {
    Ok(hex(&decode_hex_exact(pubkey, PUBKEY_BYTES, "pubkey")?))
}

pub fn validate_signature(sig: &str) -> Result<String> {
    Ok(hex(&decode_hex_exact(sig, SIGNATURE_BYTES, "signature")?))
}

pub fn validate_attestation_kind(kind: &str) -> Result<String> {
    match kind {
        "replica" | "premium-account" | "email" | "device-key" => Ok(kind.to_string()),
        other => Err(Error::InvalidInput(format!(
            "attestation kind must be replica, premium-account, email, or device-key: {other}"
        ))),
    }
}

pub fn validate_claim(claim: &str) -> Result<String> {
    let claim = claim.trim();
    if claim.is_empty() {
        return Err(Error::InvalidInput("attestation claim must not be empty".into()));
    }
    if claim.len() > MAX_CLAIM_LEN {
        return Err(Error::InvalidInput(format!(
            "attestation claim exceeds {MAX_CLAIM_LEN} chars"
        )));
    }
    Ok(claim.to_string())
}

fn verify_sig(pubkey: &str, message: &[u8], sig: &str) -> Result<()> {
    let pubkey = decode_hex_exact(pubkey, PUBKEY_BYTES, "pubkey")?;
    let sig = decode_hex_exact(sig, SIGNATURE_BYTES, "signature")?;
    let verifying = VerifyingKey::from_bytes(&array_32(&pubkey)?)
        .map_err(|e| Error::InvalidInput(format!("invalid ed25519 public key: {e}")))?;
    let signature = Signature::from_bytes(&array_64(&sig)?);
    verifying
        .verify(message, &signature)
        .map_err(|e| Error::InvalidInput(format!("invalid ed25519 signature: {e}")))
}

fn primary_person(state: &PersonState) -> Option<&PersonRecord> {
    state.persons.values().next()
}

fn person_json(person: &PersonRecord) -> Result<String> {
    let attestations: Vec<_> = person
        .attestations
        .values()
        .filter(|attestation| !attestation.revoked)
        .map(|attestation| {
            serde_json::json!({
                "kind": attestation.kind,
                "claim": attestation.claim,
                "sig": attestation.sig,
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "person_id": person.person_id,
        "pubkey": person.pubkey,
        "rotated_to": person.rotated_to,
        "attestations": attestations,
    }))
    .map_err(|e| Error::InvalidInput(format!("serialize person JSON: {e}")))
}

fn attestation_key(kind: &str, claim: &str) -> String {
    format!("{kind}\0{claim}")
}

fn decode_hex_exact(value: &str, expected: usize, label: &str) -> Result<Vec<u8>> {
    let value = value.trim();
    if value.len() != expected * 2 {
        return Err(Error::InvalidInput(format!(
            "{label} must be {} hex chars",
            expected * 2
        )));
    }
    let mut out = Vec::with_capacity(expected);
    let bytes = value.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = hex_val(bytes[i], label)?;
        let lo = hex_val(bytes[i + 1], label)?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(byte: u8, label: &str) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::InvalidInput(format!("{label} must be hex"))),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(TABLE[(byte >> 4) as usize] as char);
        out.push(TABLE[(byte & 0x0f) as usize] as char);
    }
    out
}

fn array_32(bytes: &[u8]) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| Error::InvalidInput("expected 32 bytes".into()))
}

fn array_64(bytes: &[u8]) -> Result<[u8; 64]> {
    bytes
        .try_into()
        .map_err(|_| Error::InvalidInput("expected 64 bytes".into()))
}
