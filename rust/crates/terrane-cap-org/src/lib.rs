//! The `org` capability — an organization is a shared Terrane home.
//!
//! An org is a Terrane home of its own: its own event log, apps, and data,
//! synced to members under role grants signed against the members' person
//! keys. This capability records the replayable facts — the org identity, open
//! invites, and person-signed role grants — and folds them into [`OrgState`].
//! Enforcement (which signer may issue which grant, who may redeem an invite)
//! is edge policy over the folded state at the sync routes and host helpers,
//! the same stance as `share`.
//!
//! Replay identity holds: every recorded grant carries an ed25519 signature
//! over `(org_id, member, role)` that the fold verifies against the signer's
//! folded person public key, so a rebuilt state matches byte-for-byte without
//! keychain access.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use terrane_cap_interface::{
    arg, decode_event, encode_event, restore_state, snapshot_state, state_mut, state_ref, CapManifest,
    Capability, CommandCtx, CommandSpec, Decision, Effect, Error, EventRecord, EventSpec, QueryCtx,
    QuerySpec, QueryValue, Result, StateStore,
};
use terrane_cap_person::{validate_person_id, PersonState};

mod doc;

pub const ORG_ID_HEX_LEN: usize = 16;
pub const ORG_PUBKEY_BYTES: usize = 32;
pub const ORG_SIGNATURE_BYTES: usize = 64;
pub const MAX_INVITE_NOTE_BYTES: usize = 512;
pub const MAX_ORGS_PER_HOME: usize = 64;
pub const MAX_MEMBERS_PER_ORG: usize = 1_024;
pub const MAX_OPEN_INVITES_PER_ORG: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct OrgState {
    pub orgs: BTreeMap<String, OrgRecord>,
    pub invites: BTreeMap<(String, String), OrgInvite>,
    pub members: BTreeMap<(String, String), OrgMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct OrgRecord {
    pub org_id: String,
    pub pubkey: String,
    pub founder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct OrgInvite {
    pub org_id: String,
    pub role: String,
    pub token_hash: String,
    pub note: String,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct OrgMember {
    pub org_id: String,
    pub member: String,
    pub role: String,
    pub sig: String,
    pub signer: String,
    pub active: bool,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Created {
    org_id: String,
    pubkey: String,
    founder: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Invited {
    org_id: String,
    role: String,
    token_hash: String,
    note: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct InviteRedeemed {
    org_id: String,
    token_hash: String,
    member: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct MemberGranted {
    org_id: String,
    member: String,
    role: String,
    sig: String,
    signer: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct MemberLeft {
    org_id: String,
    member: String,
}

pub struct OrgCapability;

impl Capability for OrgCapability {
    fn namespace(&self) -> &'static str {
        "org"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "org.create" },
                CommandSpec { name: "org.invite" },
                CommandSpec { name: "org.join" },
                CommandSpec { name: "org.leave" },
                CommandSpec { name: "org.role.set" },
            ],
            events: vec![
                EventSpec { kind: "org.created" },
                EventSpec { kind: "org.invited" },
                EventSpec { kind: "org.invite.redeemed" },
                EventSpec { kind: "org.member.granted" },
                EventSpec { kind: "org.member.left" },
            ],
            queries: vec![
                QuerySpec { name: "org.info" },
                QuerySpec { name: "org.members" },
            ],
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::org_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "org.create" => decide_create(ctx, args),
            "org.invite" => decide_invite(ctx, args),
            "org.join" => decide_join(ctx, args),
            "org.leave" => decide_leave(ctx, args),
            "org.role.set" => decide_role_set(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "info" => query_info(ctx, args),
            "members" => query_members(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown query: org.{other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "org.created" => {
                let e: Created = decode_event(record)?;
                validate_org_id(&e.org_id)?;
                validate_pubkey(&e.pubkey)?;
                let founder = validate_person_id(&e.founder)?;
                let expected = org_id_for_pubkey(&e.pubkey)?;
                if expected != e.org_id {
                    return Err(Error::InvalidInput(format!(
                        "org.created id/pubkey mismatch: expected {expected}"
                    )));
                }
                let orgs = state_mut::<OrgState>(state, "org")?;
                if !orgs.orgs.contains_key(&e.org_id) {
                    if orgs.orgs.len() >= MAX_ORGS_PER_HOME {
                        return Err(Error::InvalidInput(format!(
                            "org limit exceeded: max {MAX_ORGS_PER_HOME}"
                        )));
                    }
                    orgs.orgs.insert(
                        e.org_id.clone(),
                        OrgRecord {
                            org_id: e.org_id,
                            pubkey: e.pubkey,
                            founder,
                        },
                    );
                }
            }
            "org.invited" => {
                let e: Invited = decode_event(record)?;
                validate_org_id(&e.org_id)?;
                validate_role(&e.role)?;
                validate_token_hash(&e.token_hash)?;
                validate_note(&e.note)?;
                if !state_ref::<OrgState>(state, "org")?.orgs.contains_key(&e.org_id) {
                    return Err(Error::InvalidInput(format!(
                        "unknown org: {}",
                        e.org_id
                    )));
                }
                let org = state_mut::<OrgState>(state, "org")?;
                let open_count = org
                    .invites
                    .values()
                    .filter(|invite| invite.org_id == e.org_id && invite.open)
                    .count();
                if !org.invites.contains_key(&(e.org_id.clone(), e.token_hash.clone()))
                    && open_count >= MAX_OPEN_INVITES_PER_ORG
                {
                    return Err(Error::InvalidInput(format!(
                        "open invite limit exceeded for org {}: max {MAX_OPEN_INVITES_PER_ORG}",
                        e.org_id
                    )));
                }
                org.invites.insert(
                    (e.org_id.clone(), e.token_hash.clone()),
                    OrgInvite {
                        org_id: e.org_id,
                        role: e.role,
                        token_hash: e.token_hash,
                        note: e.note,
                        open: true,
                    },
                );
            }
            "org.invite.redeemed" => {
                let e: InviteRedeemed = decode_event(record)?;
                validate_org_id(&e.org_id)?;
                validate_token_hash(&e.token_hash)?;
                validate_person_id(&e.member)?;
                if let Some(invite) = state_mut::<OrgState>(state, "org")?
                    .invites
                    .get_mut(&(e.org_id, e.token_hash))
                {
                    invite.open = false;
                }
            }
            "org.member.granted" => {
                let e: MemberGranted = decode_event(record)?;
                let org_id = validate_org_id(&e.org_id)?;
                let member = validate_person_id(&e.member)?;
                let role = validate_role(&e.role)?;
                let sig = validate_signature(&e.sig)?;
                let signer = validate_person_id(&e.signer)?;
                let org_state = state_ref::<OrgState>(state, "org")?;
                if !org_state.orgs.contains_key(&org_id) {
                    return Err(Error::InvalidInput(format!("unknown org: {org_id}")));
                }
                let signer_pubkey = state_ref::<PersonState>(state, "person")?
                    .persons
                    .get(&signer)
                    .map(|person| person.pubkey.clone())
                    .ok_or_else(|| Error::InvalidInput(format!("unknown signer person: {signer}")))?;
                verify_role_sig(&signer_pubkey, &org_id, &member, &role, &sig)?;
                let org_state = state_mut::<OrgState>(state, "org")?;
                if !org_state.members.contains_key(&(org_id.clone(), member.clone()))
                    && org_state
                        .members
                        .values()
                        .filter(|m| m.org_id == org_id)
                        .count()
                        >= MAX_MEMBERS_PER_ORG
                {
                    return Err(Error::InvalidInput(format!(
                        "member limit exceeded for org {org_id}: max {MAX_MEMBERS_PER_ORG}"
                    )));
                }
                org_state.members.insert(
                    (org_id, member),
                    OrgMember {
                        org_id: e.org_id,
                        member: e.member,
                        role,
                        sig,
                        signer,
                        active: true,
                    },
                );
            }
            "org.member.left" => {
                let e: MemberLeft = decode_event(record)?;
                let org_id = validate_org_id(&e.org_id)?;
                let member = validate_person_id(&e.member)?;
                if let Some(membership) = state_mut::<OrgState>(state, "org")?
                    .members
                    .get_mut(&(org_id, member))
                {
                    membership.active = false;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn snapshot(&self, state: &dyn StateStore) -> Result<Option<Vec<u8>>> {
        snapshot_state::<OrgState>(state, self.namespace())
    }

    fn restore(&self, state: &mut dyn StateStore, payload: &[u8]) -> Result<()> {
        restore_state::<OrgState>(state, self.namespace(), payload)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "org.created" => decode_event::<Created>(record)
                .ok()
                .map(|e| format!("org.created {} founder={}", e.org_id, e.founder)),
            "org.invited" => decode_event::<Invited>(record)
                .ok()
                .map(|e| format!("org.invited {} role={}", e.org_id, e.role)),
            "org.invite.redeemed" => decode_event::<InviteRedeemed>(record)
                .ok()
                .map(|e| format!("org.invite.redeemed {} member={}", e.org_id, e.member)),
            "org.member.granted" => decode_event::<MemberGranted>(record)
                .ok()
                .map(|e| format!("org.member.granted {} member={} role={}", e.org_id, e.member, e.role)),
            "org.member.left" => decode_event::<MemberLeft>(record)
                .ok()
                .map(|e| format!("org.member.left {} member={}", e.org_id, e.member)),
            _ => None,
        }
    }
}

fn decide_create(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let founder = validate_person_id(&arg(args, 0, "founder")?)?;
    let org_state = state_ref::<OrgState>(ctx.state, "org")?;
    if org_state.orgs.iter().any(|(_, record)| record.founder == founder) {
        return Ok(Decision::Commit(Vec::new()));
    }
    if org_state.orgs.len() >= MAX_ORGS_PER_HOME {
        return Err(Error::InvalidInput(format!(
            "org limit exceeded: max {MAX_ORGS_PER_HOME}"
        )));
    }
    Ok(Decision::Effect(Effect::OrgKeygen { founder }))
}

fn decide_invite(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let org_id = validate_org_id(&arg(args, 0, "org_id")?)?;
    let role = validate_role(&arg(args, 1, "role")?)?;
    let token_hash = validate_token_hash(&arg(args, 2, "token_hash")?)?;
    let note = args.get(3).cloned().unwrap_or_default();
    validate_note(&note)?;
    if !state_ref::<OrgState>(ctx.state, "org")?.orgs.contains_key(&org_id) {
        return Err(Error::InvalidInput(format!("unknown org: {org_id}")));
    }
    Ok(Decision::Commit(vec![invited_event(
        &org_id, &role, &token_hash, &note,
    )?]))
}

fn decide_join(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let org_id = validate_org_id(&arg(args, 0, "org_id")?)?;
    let token_hash = validate_token_hash(&arg(args, 1, "token_hash")?)?;
    let member = validate_person_id(&arg(args, 2, "member")?)?;
    if !state_ref::<OrgState>(ctx.state, "org")?.orgs.contains_key(&org_id) {
        return Err(Error::InvalidInput(format!("unknown org: {org_id}")));
    }
    let invite = state_ref::<OrgState>(ctx.state, "org")?
        .invites
        .get(&(org_id.clone(), token_hash.clone()))
        .ok_or_else(|| Error::InvalidInput("org invite is not open".to_string()))?;
    if !invite.open {
        return Err(Error::InvalidInput("org invite is already redeemed".to_string()));
    }
    Ok(Decision::Effect(Effect::OrgRoleSign {
        org_id,
        member: member.clone(),
        role: invite.role.clone(),
        signer: member,
        redeem_token_hash: Some(token_hash),
    }))
}

fn decide_leave(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let org_id = validate_org_id(&arg(args, 0, "org_id")?)?;
    let member = validate_person_id(&arg(args, 1, "member")?)?;
    if !state_ref::<OrgState>(ctx.state, "org")?.orgs.contains_key(&org_id) {
        return Err(Error::InvalidInput(format!("unknown org: {org_id}")));
    }
    let org_state = state_ref::<OrgState>(ctx.state, "org")?;
    if !org_state.members.contains_key(&(org_id.clone(), member.clone())) {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![member_left_event(&org_id, &member)?]))
}

fn decide_role_set(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let org_id = validate_org_id(&arg(args, 0, "org_id")?)?;
    let member = validate_person_id(&arg(args, 1, "member")?)?;
    let role = validate_role(&arg(args, 2, "role")?)?;
    let signer = validate_person_id(&arg(args, 3, "signer")?)?;
    if !state_ref::<OrgState>(ctx.state, "org")?.orgs.contains_key(&org_id) {
        return Err(Error::InvalidInput(format!("unknown org: {org_id}")));
    }
    Ok(Decision::Effect(Effect::OrgRoleSign {
        org_id,
        member,
        role,
        signer,
        redeem_token_hash: None,
    }))
}

fn query_info(ctx: QueryCtx<'_>, args: &[String]) -> Result<QueryValue> {
    let state = state_ref::<OrgState>(ctx.state, "org")?;
    let record = match args.first() {
        Some(arg) if !arg.trim().is_empty() => {
            let org_id = validate_org_id(arg)?;
            state.orgs.get(&org_id)
        }
        _ => state.orgs.values().next(),
    };
    Ok(QueryValue::Json(match record {
        Some(record) => org_json(record)?,
        None => "null".to_string(),
    }))
}

fn query_members(ctx: QueryCtx<'_>, args: &[String]) -> Result<QueryValue> {
    let state = state_ref::<OrgState>(ctx.state, "org")?;
    let org_id = match args.first() {
        Some(arg) if !arg.trim().is_empty() => validate_org_id(arg)?,
        _ => match state.orgs.keys().next() {
            Some(id) => id.clone(),
            None => return Ok(QueryValue::Json("[]".to_string())),
        },
    };
    Ok(QueryValue::Json(members_json(state, &org_id)))
}

pub fn created_event(org_id: &str, pubkey: &str, founder: &str) -> Result<EventRecord> {
    encode_event(
        "org.created",
        &Created {
            org_id: validate_org_id(org_id)?,
            pubkey: validate_pubkey(pubkey)?,
            founder: validate_person_id(founder)?,
        },
    )
}

pub fn invited_event(org_id: &str, role: &str, token_hash: &str, note: &str) -> Result<EventRecord> {
    encode_event(
        "org.invited",
        &Invited {
            org_id: validate_org_id(org_id)?,
            role: validate_role(role)?,
            token_hash: validate_token_hash(token_hash)?,
            note: validate_note(note)?,
        },
    )
}

pub fn invite_redeemed_event(
    org_id: &str,
    token_hash: &str,
    member: &str,
) -> Result<EventRecord> {
    encode_event(
        "org.invite.redeemed",
        &InviteRedeemed {
            org_id: validate_org_id(org_id)?,
            token_hash: validate_token_hash(token_hash)?,
            member: validate_person_id(member)?,
        },
    )
}

pub fn member_granted_event(
    org_id: &str,
    member: &str,
    role: &str,
    sig: &str,
    signer: &str,
) -> Result<EventRecord> {
    encode_event(
        "org.member.granted",
        &MemberGranted {
            org_id: validate_org_id(org_id)?,
            member: validate_person_id(member)?,
            role: validate_role(role)?,
            sig: validate_signature(sig)?,
            signer: validate_person_id(signer)?,
        },
    )
}

pub fn member_left_event(org_id: &str, member: &str) -> Result<EventRecord> {
    encode_event(
        "org.member.left",
        &MemberLeft {
            org_id: validate_org_id(org_id)?,
            member: validate_person_id(member)?,
        },
    )
}

pub fn org_id_for_pubkey(pubkey: &str) -> Result<String> {
    let bytes = decode_hex_exact(pubkey, ORG_PUBKEY_BYTES, "pubkey")?;
    let digest = Sha256::digest(bytes);
    Ok(hex(&digest)[..ORG_ID_HEX_LEN].to_string())
}

pub fn role_grant_message(org_id: &str, member: &str, role: &str) -> Result<Vec<u8>> {
    Ok(format!(
        "terrane.org.role.v1\norg_id={}\nmember={}\nrole={}",
        validate_org_id(org_id)?,
        validate_person_id(member)?,
        validate_role(role)?,
    )
    .into_bytes())
}

pub fn verify_role_sig(pubkey: &str, org_id: &str, member: &str, role: &str, sig: &str) -> Result<()> {
    let pubkey_bytes = decode_hex_exact(pubkey, ORG_PUBKEY_BYTES, "pubkey")?;
    let sig_bytes = decode_hex_exact(sig, ORG_SIGNATURE_BYTES, "signature")?;
    let verifying = VerifyingKey::from_bytes(&array_32(&pubkey_bytes)?)
        .map_err(|e| Error::InvalidInput(format!("invalid ed25519 public key: {e}")))?;
    let signature = Signature::from_bytes(&array_64(&sig_bytes)?);
    let message = role_grant_message(org_id, member, role)?;
    verifying
        .verify(&message, &signature)
        .map_err(|e| Error::InvalidInput(format!("invalid org role signature: {e}")))
}

pub fn validate_org_id(org_id: &str) -> Result<String> {
    if org_id.len() != ORG_ID_HEX_LEN
        || !org_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::InvalidInput(format!(
            "org_id must be {ORG_ID_HEX_LEN} hex chars"
        )));
    }
    Ok(org_id.to_ascii_lowercase())
}

pub fn validate_pubkey(pubkey: &str) -> Result<String> {
    Ok(hex(&decode_hex_exact(pubkey, ORG_PUBKEY_BYTES, "pubkey")?))
}

pub fn validate_signature(sig: &str) -> Result<String> {
    Ok(hex(&decode_hex_exact(sig, ORG_SIGNATURE_BYTES, "signature")?))
}

pub fn validate_role(role: &str) -> Result<String> {
    match role {
        "owner" | "admin" | "member" => Ok(role.to_string()),
        other => Err(Error::InvalidInput(format!(
            "role must be owner, admin, or member: {other}"
        ))),
    }
}

pub fn validate_token_hash(token_hash: &str) -> Result<String> {
    if token_hash.len() != 64 || !token_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidInput(
            "token_hash must be 64 ASCII hex characters".into(),
        ));
    }
    Ok(token_hash.to_ascii_lowercase())
}

pub fn validate_note(note: &str) -> Result<String> {
    if note.len() > MAX_INVITE_NOTE_BYTES {
        return Err(Error::InvalidInput(format!(
            "note exceeds {MAX_INVITE_NOTE_BYTES} bytes"
        )));
    }
    Ok(note.to_string())
}

fn org_json(record: &OrgRecord) -> Result<String> {
    serde_json::to_string(&serde_json::json!({
        "org_id": record.org_id,
        "pubkey": record.pubkey,
        "founder": record.founder,
    }))
    .map_err(|e| Error::InvalidInput(format!("serialize org JSON: {e}")))
}

fn members_json(state: &OrgState, org_id: &str) -> String {
    let mut out = String::from("[");
    let mut first = true;
    for member in state.members.values().filter(|m| m.org_id == org_id) {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            "{{\"member\":\"{}\",\"role\":\"{}\",\"signer\":\"{}\",\"active\":{}}}",
            json_escape(&member.member),
            json_escape(&member.role),
            json_escape(&member.signer),
            member.active
        ));
    }
    out.push(']');
    out
}

fn json_escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
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