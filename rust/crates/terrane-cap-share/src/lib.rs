//! The `share` capability records app share invites, accepted grants, and
//! revocations. Enforcement lives at sync/web edges over folded ShareState.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, restore_state,
    snapshot_state, state_mut, state_ref, CapManifest, Capability, CommandCtx, CommandSpec,
    Decision, Effect, Error, EventPattern, EventRecord, EventSpec, QueryCtx, QuerySpec,
    QueryValue, Result, StateStore,
};

mod doc;

pub const MAX_INVITE_NOTE_BYTES: usize = 512;
pub const INVITE_TOKEN_BYTES: usize = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ShareState {
    pub invites: BTreeMap<(String, String), InviteRecord>,
    pub shares: BTreeMap<(String, String), ShareRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct InviteRecord {
    pub app: String,
    pub rights: String,
    pub token_hash: String,
    pub note: String,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ShareRecord {
    pub app: String,
    pub grantee: String,
    pub rights: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Invited {
    pub app: String,
    pub rights: String,
    pub token_hash: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Redeemed {
    pub app: String,
    pub token_hash: String,
    pub grantee: String,
    pub rights: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Revoked {
    pub app: String,
    pub grantee: String,
}

pub struct ShareCapability;

impl Capability for ShareCapability {
    fn namespace(&self) -> &'static str {
        "share"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "share.invite" },
                CommandSpec {
                    name: "share.redeem",
                },
                CommandSpec {
                    name: "share.revoke",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "share.invited",
                },
                EventSpec {
                    kind: "share.redeemed",
                },
                EventSpec {
                    kind: "share.revoked",
                },
            ],
            queries: vec![
                QuerySpec { name: "share.list" },
                QuerySpec {
                    name: "share.invites",
                },
            ],
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::share_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "share.invite" => decide_invite(ctx, args),
            "share.redeem" => decide_redeem(ctx, args),
            "share.revoke" => decide_revoke(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        let app = validate_app_arg(&arg(args, 0, "app")?)?;
        match name {
            "list" => Ok(QueryValue::Json(shares_json(
                state_ref::<ShareState>(ctx.state, "share")?,
                &app,
            ))),
            "invites" => Ok(QueryValue::Json(invites_json(
                state_ref::<ShareState>(ctx.state, "share")?,
                &app,
            ))),
            other => Err(Error::InvalidInput(format!("unknown query: share.{other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "share.invited" => {
                let event: Invited = decode_event(record)?;
                validate_app_arg(&event.app)?;
                validate_rights(&event.rights)?;
                validate_token_hash(&event.token_hash)?;
                validate_note(&event.note)?;
                state_mut::<ShareState>(state, "share")?.invites.insert(
                    (event.app.clone(), event.token_hash.clone()),
                    InviteRecord {
                        app: event.app,
                        rights: event.rights,
                        token_hash: event.token_hash,
                        note: event.note,
                        open: true,
                    },
                );
            }
            "share.redeemed" => {
                let event: Redeemed = decode_event(record)?;
                validate_app_arg(&event.app)?;
                validate_rights(&event.rights)?;
                validate_token_hash(&event.token_hash)?;
                validate_grantee(&event.grantee)?;
                let share = state_mut::<ShareState>(state, "share")?;
                if let Some(invite) = share
                    .invites
                    .get_mut(&(event.app.clone(), event.token_hash.clone()))
                {
                    invite.open = false;
                }
                share.shares.insert(
                    (event.app.clone(), event.grantee.clone()),
                    ShareRecord {
                        app: event.app,
                        grantee: event.grantee,
                        rights: event.rights,
                    },
                );
            }
            "share.revoked" => {
                let event: Revoked = decode_event(record)?;
                validate_app_arg(&event.app)?;
                validate_grantee(&event.grantee)?;
                state_mut::<ShareState>(state, "share")?
                    .shares
                    .remove(&(event.app, event.grantee));
            }
            "app.removed" => {
                let event = decode_app_removed(record)?;
                let share = state_mut::<ShareState>(state, "share")?;
                share.invites.retain(|(app, _), _| app != &event.id);
                share.shares.retain(|(app, _), _| app != &event.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn snapshot(&self, state: &dyn StateStore) -> Result<Option<Vec<u8>>> {
        snapshot_state::<ShareState>(state, self.namespace())
    }

    fn restore(&self, state: &mut dyn StateStore, payload: &[u8]) -> Result<()> {
        restore_state::<ShareState>(state, self.namespace(), payload)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "share.invited" => {
                let event: Invited = decode_event(record).ok()?;
                Some(format!("share.invited {} {}", event.app, event.rights))
            }
            "share.redeemed" => {
                let event: Redeemed = decode_event(record).ok()?;
                Some(format!(
                    "share.redeemed {} {} {}",
                    event.app, event.grantee, event.rights
                ))
            }
            "share.revoked" => {
                let event: Revoked = decode_event(record).ok()?;
                Some(format!("share.revoked {} {}", event.app, event.grantee))
            }
            _ => None,
        }
    }

    fn app_of(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "share.invited" => decode_event::<Invited>(record).ok().map(|event| event.app),
            "share.redeemed" => decode_event::<Redeemed>(record).ok().map(|event| event.app),
            "share.revoked" => decode_event::<Revoked>(record).ok().map(|event| event.app),
            _ => None,
        }
    }
}

fn decide_invite(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = validate_app_arg(&arg(args, 0, "app")?)?;
    let rights = validate_rights(&arg(args, 1, "rights")?)?;
    let note = args.get(2).cloned().unwrap_or_default();
    validate_note(&note)?;
    ensure_app_exists(ctx.bus, &app)?;

    if let Some(hash) = args.get(3) {
        let token_hash = validate_token_hash(hash)?;
        return Ok(Decision::Commit(vec![invited_event(
            &app,
            &rights,
            &token_hash,
            &note,
        )?]));
    }

    Ok(Decision::Effect(Effect::NewInviteToken { app, rights, note }))
}

fn decide_redeem(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = validate_app_arg(&arg(args, 0, "app")?)?;
    let token_hash = validate_token_hash(&arg(args, 1, "token_hash")?)?;
    let grantee = validate_grantee(&arg(args, 2, "grantee")?)?;
    ensure_app_exists(ctx.bus, &app)?;

    let share = state_ref::<ShareState>(ctx.state, "share")?;
    let invite = share
        .invites
        .get(&(app.clone(), token_hash.clone()))
        .ok_or_else(|| Error::InvalidInput("share invite is not open".to_string()))?;
    if !invite.open {
        return Err(Error::InvalidInput(
            "share invite is already redeemed".to_string(),
        ));
    }
    Ok(Decision::Commit(vec![redeemed_event(
        &app,
        &token_hash,
        &grantee,
        &invite.rights,
    )?]))
}

fn decide_revoke(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = validate_app_arg(&arg(args, 0, "app")?)?;
    let grantee = validate_grantee(&arg(args, 1, "grantee")?)?;
    ensure_app_exists(ctx.bus, &app)?;
    if !state_ref::<ShareState>(ctx.state, "share")?
        .shares
        .contains_key(&(app.clone(), grantee.clone()))
    {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![revoked_event(&app, &grantee)?]))
}

pub fn invited_event(app: &str, rights: &str, token_hash: &str, note: &str) -> Result<EventRecord> {
    encode_event(
        "share.invited",
        &Invited {
            app: validate_app_arg(app)?,
            rights: validate_rights(rights)?,
            token_hash: validate_token_hash(token_hash)?,
            note: validate_note(note)?,
        },
    )
}

pub fn redeemed_event(
    app: &str,
    token_hash: &str,
    grantee: &str,
    rights: &str,
) -> Result<EventRecord> {
    encode_event(
        "share.redeemed",
        &Redeemed {
            app: validate_app_arg(app)?,
            token_hash: validate_token_hash(token_hash)?,
            grantee: validate_grantee(grantee)?,
            rights: validate_rights(rights)?,
        },
    )
}

pub fn revoked_event(app: &str, grantee: &str) -> Result<EventRecord> {
    encode_event(
        "share.revoked",
        &Revoked {
            app: validate_app_arg(app)?,
            grantee: validate_grantee(grantee)?,
        },
    )
}

pub fn has_read(state: &ShareState, app: &str, grantee: &str) -> bool {
    state
        .shares
        .get(&(app.to_string(), grantee.to_string()))
        .is_some_and(|share| share.rights == "read" || share.rights == "write")
}

pub fn has_write(state: &ShareState, app: &str, grantee: &str) -> bool {
    state
        .shares
        .get(&(app.to_string(), grantee.to_string()))
        .is_some_and(|share| share.rights == "write")
}

pub fn validate_app_arg(app: &str) -> Result<String> {
    let value = app.trim();
    if value.is_empty() {
        return Err(Error::InvalidInput("app must not be empty".into()));
    }
    Ok(value.to_string())
}

pub fn validate_rights(rights: &str) -> Result<String> {
    match rights {
        "read" | "write" => Ok(rights.to_string()),
        _ => Err(Error::InvalidInput(
            "rights must be either read or write".to_string(),
        )),
    }
}

pub fn validate_token_hash(token_hash: &str) -> Result<String> {
    if token_hash.len() != 64 || !token_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidInput(
            "token_hash must be 64 ASCII hex characters".to_string(),
        ));
    }
    Ok(token_hash.to_ascii_lowercase())
}

pub fn validate_grantee(grantee: &str) -> Result<String> {
    let value = grantee.trim();
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_' | b'.'))
    {
        return Err(Error::InvalidInput(
            "grantee must be a safe subject such as replica:<hex> or user:<id>".to_string(),
        ));
    }
    if let Some(peer) = value.strip_prefix("replica:") {
        if peer.is_empty() || peer.len() > 32 || !peer.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::InvalidInput(
                "replica grantee must be replica:<1..32 hex>".to_string(),
            ));
        }
    }
    Ok(value.to_string())
}

fn validate_note(note: &str) -> Result<String> {
    if note.len() > MAX_INVITE_NOTE_BYTES {
        return Err(Error::InvalidInput(format!(
            "note exceeds {MAX_INVITE_NOTE_BYTES} bytes"
        )));
    }
    Ok(note.to_string())
}

fn shares_json(state: &ShareState, app: &str) -> String {
    let mut out = String::from("[");
    let mut first = true;
    for share in state.shares.values().filter(|share| share.app == app) {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            "{{\"app\":\"{}\",\"grantee\":\"{}\",\"rights\":\"{}\"}}",
            json_escape(&share.app),
            json_escape(&share.grantee),
            json_escape(&share.rights)
        ));
    }
    out.push(']');
    out
}

fn invites_json(state: &ShareState, app: &str) -> String {
    let mut out = String::from("[");
    let mut first = true;
    for invite in state
        .invites
        .values()
        .filter(|invite| invite.app == app && invite.open)
    {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            "{{\"app\":\"{}\",\"rights\":\"{}\",\"tokenHash\":\"{}\",\"note\":\"{}\"}}",
            json_escape(&invite.app),
            json_escape(&invite.rights),
            json_escape(&invite.token_hash),
            json_escape(&invite.note)
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
