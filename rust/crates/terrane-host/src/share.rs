use sha2::{Digest as _, Sha256};
use terrane_core::{ExecutionPrincipal, QueryValue, Request};

use crate::HostCore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteToken {
    pub token: String,
    pub token_hash: String,
}

pub fn invite(core: &mut HostCore, app: &str, rights: &str, note: &str) -> Result<InviteToken, String> {
    let token = crate::edge::mint_invite_token().map_err(|e| e.to_string())?;
    let token_hash = token_hash(&token);
    core.dispatch(Request::trusted_host(
        "share.invite",
        vec![
            app.to_string(),
            rights.to_string(),
            note.to_string(),
            token_hash.clone(),
        ],
    ))
    .map_err(|e| e.to_string())?;
    Ok(InviteToken { token, token_hash })
}

pub fn redeem(
    core: &mut HostCore,
    app: &str,
    token_or_hash: &str,
    grantee: &str,
) -> Result<(), String> {
    let token_hash = normalize_token(token_or_hash)?;
    let records = core
        .dispatch(Request::trusted_host(
            "share.redeem",
            vec![app.to_string(), token_hash, grantee.to_string()],
        ))
        .map_err(|e| e.to_string())?;
    let rights = records
        .iter()
        .find_map(share_redeemed_rights)
        .ok_or_else(|| "share redeem did not record a grant".to_string())?;
    core.dispatch(Request::trusted_host(
        "auth.grant",
        vec![
            grantee.to_string(),
            app.to_string(),
            "kv".to_string(),
            rights,
        ],
    ))
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn revoke(core: &mut HostCore, app: &str, grantee: &str) -> Result<(), String> {
    core.dispatch(Request::trusted_host(
        "share.revoke",
        vec![app.to_string(), grantee.to_string()],
    ))
    .map_err(|e| e.to_string())?;
    core.dispatch(Request::trusted_host(
        "auth.revoke",
        vec![grantee.to_string(), app.to_string(), "kv".to_string()],
    ))
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list(core: &HostCore, app: &str) -> Result<String, String> {
    match core
        .query("share", "list", &[app.to_string()])
        .map_err(|e| e.to_string())?
    {
        QueryValue::Json(json) => Ok(json),
        other => Err(format!("share.list returned unexpected value: {other:?}")),
    }
}

pub fn invites(core: &HostCore, app: &str) -> Result<String, String> {
    match core
        .query("share", "invites", &[app.to_string()])
        .map_err(|e| e.to_string())?
    {
        QueryValue::Json(json) => Ok(json),
        other => Err(format!("share.invites returned unexpected value: {other:?}")),
    }
}

pub fn ensure_read(core: &HostCore, app: &str, grantee: &str) -> Result<(), String> {
    let grantee = terrane_cap_share::validate_grantee(grantee).map_err(|e| e.to_string())?;
    if terrane_cap_share::has_read(&core.state().share, app, &grantee) {
        Ok(())
    } else {
        Err(format!(
            "share permission required: app {app} grantee {grantee} missing read"
        ))
    }
}

pub fn ensure_write(core: &HostCore, app: &str, grantee: &str) -> Result<(), String> {
    let grantee = terrane_cap_share::validate_grantee(grantee).map_err(|e| e.to_string())?;
    if terrane_cap_share::has_write(&core.state().share, app, &grantee) {
        Ok(())
    } else {
        Err(format!(
            "share permission required: app {app} grantee {grantee} missing write"
        ))
    }
}

pub fn grantee_for_peer(peer_hex: &str) -> Result<String, String> {
    let grantee = format!("replica:{peer_hex}");
    terrane_cap_share::validate_grantee(&grantee)
        .map_err(|e| e.to_string())
        .map(|_| grantee)
}

pub fn token_hash(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

pub fn principal_for_grantee(grantee: &str) -> Result<ExecutionPrincipal, String> {
    let grantee = terrane_cap_share::validate_grantee(grantee).map_err(|e| e.to_string())?;
    Ok(ExecutionPrincipal {
        org: terrane_core::LOCAL_ORG.to_string(),
        subject: grantee,
        source: "share".to_string(),
    })
}

fn normalize_token(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Err("share token must not be empty".to_string())
    } else {
        Ok(token_hash(value))
    }
}

fn share_redeemed_rights(record: &terrane_core::EventRecord) -> Option<String> {
    if record.kind != "share.redeemed" {
        return None;
    }
    borsh::from_slice::<terrane_cap_share::Redeemed>(&record.payload)
        .ok()
        .map(|event| event.rights)
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
