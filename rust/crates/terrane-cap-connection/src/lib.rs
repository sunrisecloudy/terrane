//! The `connection` capability — replayable metadata for host-edge secrets.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_event, encode_event, state_mut, state_ref, CapManifest, Capability, CommandCtx,
    CommandSpec, Decision, Error, EventRecord, EventSpec, GrantResourceSpec, ReadValue,
    ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod doc;

pub const MAX_CONNECTIONS: usize = 64;
pub const MAX_NAME_LEN: usize = 64;
pub const MAX_SECRET_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionState {
    pub connections: BTreeMap<String, ConnMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnMeta {
    pub kind: String,
    pub config_public_json: String,
    pub authorized: bool,
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionStatus {
    pub name: String,
    pub kind: String,
    pub authorized: bool,
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Defined {
    name: String,
    kind: String,
    config_public_json: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Authorized {
    name: String,
    scopes: Vec<String>,
    expires_at: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Refreshed {
    name: String,
    expires_at: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Removed {
    name: String,
}

pub struct ConnectionCapability;

impl Capability for ConnectionCapability {
    fn namespace(&self) -> &'static str {
        "connection"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "connection.define",
                },
                CommandSpec {
                    name: "connection.remove",
                },
                CommandSpec {
                    name: "connection.mark_authorized",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "connection.defined",
                },
                EventSpec {
                    kind: "connection.authorized",
                },
                EventSpec {
                    kind: "connection.refreshed",
                },
                EventSpec {
                    kind: "connection.removed",
                },
            ],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Read {
                    name: "list",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "stat",
                    params: &["name"],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "connection",
                &["read", "call"],
                "Per-name host credentials consumed only through $secret substitution.",
            )],
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::connection_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "connection.define" => {
                let conn_name = validate_name(&arg(args, 0, "name")?)?;
                let kind = validate_kind(&arg(args, 1, "kind")?)?;
                let config_public_json = validate_public_config(&kind, &arg(args, 2, "config_public_json")?)?;
                let state = state_ref::<ConnectionState>(ctx.state, "connection")?;
                if !state.connections.contains_key(&conn_name) && state.connections.len() >= MAX_CONNECTIONS {
                    return Err(Error::InvalidInput(format!(
                        "connection limit exceeded: max {MAX_CONNECTIONS}"
                    )));
                }
                Ok(Decision::Commit(vec![defined_event(
                    &conn_name,
                    &kind,
                    &config_public_json,
                )?]))
            }
            "connection.remove" => {
                let conn_name = validate_name(&arg(args, 0, "name")?)?;
                Ok(Decision::Commit(vec![removed_event(&conn_name)?]))
            }
            "connection.mark_authorized" => {
                let conn_name = validate_name(&arg(args, 0, "name")?)?;
                let scopes = parse_scopes(args.get(1).map(String::as_str).unwrap_or(""))?;
                let expires_at = args.get(2).cloned().unwrap_or_default();
                if expires_at.trim().is_empty() {
                    return Err(Error::InvalidInput("expires_at must not be empty".into()));
                }
                let state = state_ref::<ConnectionState>(ctx.state, "connection")?;
                if !state.connections.contains_key(&conn_name) {
                    return Err(Error::InvalidInput(format!(
                        "unknown connection: {conn_name}"
                    )));
                }
                let event = if state
                    .connections
                    .get(&conn_name)
                    .is_some_and(|meta| meta.authorized)
                {
                    refreshed_event(&conn_name, &expires_at)?
                } else {
                    authorized_event(&conn_name, scopes, &expires_at)?
                };
                Ok(Decision::Commit(vec![event]))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "connection.defined" => {
                let e: Defined = decode_event(record)?;
                state_mut::<ConnectionState>(state, "connection")?
                    .connections
                    .insert(
                        e.name,
                        ConnMeta {
                            kind: e.kind,
                            config_public_json: e.config_public_json,
                            authorized: false,
                            scopes: Vec::new(),
                            expires_at: None,
                        },
                    );
            }
            "connection.authorized" => {
                let e: Authorized = decode_event(record)?;
                if let Some(meta) = state_mut::<ConnectionState>(state, "connection")?
                    .connections
                    .get_mut(&e.name)
                {
                    meta.authorized = true;
                    meta.scopes = e.scopes;
                    meta.expires_at = Some(e.expires_at);
                }
            }
            "connection.refreshed" => {
                let e: Refreshed = decode_event(record)?;
                if let Some(meta) = state_mut::<ConnectionState>(state, "connection")?
                    .connections
                    .get_mut(&e.name)
                {
                    meta.authorized = true;
                    meta.expires_at = Some(e.expires_at);
                }
            }
            "connection.removed" => {
                let e: Removed = decode_event(record)?;
                state_mut::<ConnectionState>(state, "connection")?
                    .connections
                    .remove(&e.name);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "connection.defined" => decode_event::<Defined>(record)
                .ok()
                .map(|e| format!("connection.defined {} ({})", e.name, e.kind)),
            "connection.authorized" => decode_event::<Authorized>(record)
                .ok()
                .map(|e| format!("connection.authorized {} expires {}", e.name, e.expires_at)),
            "connection.refreshed" => decode_event::<Refreshed>(record)
                .ok()
                .map(|e| format!("connection.refreshed {} expires {}", e.name, e.expires_at)),
            "connection.removed" => decode_event::<Removed>(record)
                .ok()
                .map(|e| format!("connection.removed {}", e.name)),
            _ => None,
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "list" => {
                let mut map = BTreeMap::new();
                for (name, meta) in &state_ref::<ConnectionState>(ctx.state, "connection")?.connections {
                    map.insert(name.clone(), status_json(name, meta));
                }
                Ok(ReadValue::StringMap(map))
            }
            "stat" => {
                let conn_name = validate_name(&arg(args, 0, "name")?)?;
                let state = state_ref::<ConnectionState>(ctx.state, "connection")?;
                let meta = state.connections.get(&conn_name).ok_or_else(|| {
                    Error::InvalidInput(format!("unknown connection: {conn_name}"))
                })?;
                Ok(ReadValue::OptString(Some(status_json(&conn_name, meta))))
            }
            other => Err(Error::InvalidInput(format!(
                "connection.{other} is not a readable resource"
            ))),
        }
    }
}

pub fn defined_event(name: &str, kind: &str, config_public_json: &str) -> Result<EventRecord> {
    encode_event(
        "connection.defined",
        &Defined {
            name: name.to_string(),
            kind: kind.to_string(),
            config_public_json: config_public_json.to_string(),
        },
    )
}

pub fn authorized_event(name: &str, scopes: Vec<String>, expires_at: &str) -> Result<EventRecord> {
    encode_event(
        "connection.authorized",
        &Authorized {
            name: name.to_string(),
            scopes,
            expires_at: expires_at.to_string(),
        },
    )
}

pub fn refreshed_event(name: &str, expires_at: &str) -> Result<EventRecord> {
    encode_event(
        "connection.refreshed",
        &Refreshed {
            name: name.to_string(),
            expires_at: expires_at.to_string(),
        },
    )
}

pub fn removed_event(name: &str) -> Result<EventRecord> {
    encode_event(
        "connection.removed",
        &Removed {
            name: name.to_string(),
        },
    )
}

pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::InvalidInput("connection name must not be empty".into()));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(Error::InvalidInput(format!(
            "connection name exceeds {MAX_NAME_LEN} chars"
        )));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(
            "connection name must match [a-z0-9-_]".into(),
        ));
    }
    Ok(name.to_string())
}

pub fn validate_field(field: &str) -> Result<String> {
    let field = field.trim();
    if field.is_empty() {
        return Err(Error::InvalidInput("connection field must not be empty".into()));
    }
    if !field
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(
            "connection field must match [a-z0-9-_]".into(),
        ));
    }
    Ok(field.to_string())
}

pub fn split_secret_ref(reference: &str) -> Result<(String, String)> {
    let (name, field) = match reference.split_once('.') {
        Some((name, field)) => (name, field),
        None => (reference, "key"),
    };
    Ok((validate_name(name)?, validate_field(field)?))
}

pub fn connection_resource_id(name: &str) -> Result<String> {
    Ok(format!("connection:{}", validate_name(name)?))
}

pub fn validate_secret_len(secret: &str) -> Result<()> {
    if secret.len() > MAX_SECRET_BYTES {
        return Err(Error::InvalidInput(format!(
            "connection secret field exceeds {MAX_SECRET_BYTES} bytes"
        )));
    }
    Ok(())
}

pub fn status(state: &dyn StateStore, name: &str) -> Result<Option<ConnectionStatus>> {
    let name = validate_name(name)?;
    Ok(state_ref::<ConnectionState>(state, "connection")?
        .connections
        .get(&name)
        .map(|meta| ConnectionStatus {
            name,
            kind: meta.kind.clone(),
            authorized: meta.authorized,
            scopes: meta.scopes.clone(),
            expires_at: meta.expires_at.clone(),
        }))
}

pub fn all_statuses(state: &dyn StateStore) -> Result<Vec<ConnectionStatus>> {
    Ok(state_ref::<ConnectionState>(state, "connection")?
        .connections
        .iter()
        .map(|(name, meta)| ConnectionStatus {
            name: name.clone(),
            kind: meta.kind.clone(),
            authorized: meta.authorized,
            scopes: meta.scopes.clone(),
            expires_at: meta.expires_at.clone(),
        })
        .collect())
}

fn validate_kind(kind: &str) -> Result<String> {
    match kind {
        "apiKey" | "oauth2" | "smtp" => Ok(kind.to_string()),
        other => Err(Error::InvalidInput(format!(
            "connection kind must be apiKey, oauth2, or smtp: {other}"
        ))),
    }
}

fn validate_public_config(kind: &str, raw: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("config_public_json must be JSON: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("config_public_json must be a JSON object".into()))?;
    for forbidden in ["key", "password", "client_secret", "access_token", "refresh_token"] {
        if obj.contains_key(forbidden) {
            return Err(Error::InvalidInput(format!(
                "config_public_json must not contain secret field {forbidden}"
            )));
        }
    }
    match kind {
        "apiKey" => {}
        "oauth2" => {
            for required in ["auth_url", "token_url", "client_id"] {
                if obj.get(required).and_then(|v| v.as_str()).is_none() {
                    return Err(Error::InvalidInput(format!(
                        "oauth2 config_public_json missing string {required}"
                    )));
                }
            }
        }
        "smtp" => {
            for required in ["host", "username"] {
                if obj.get(required).and_then(|v| v.as_str()).is_none() {
                    return Err(Error::InvalidInput(format!(
                        "smtp config_public_json missing string {required}"
                    )));
                }
            }
        }
        _ => {}
    }
    serde_json::to_string(&value)
        .map_err(|e| Error::InvalidInput(format!("canonicalize public config: {e}")))
}

fn parse_scopes(raw: &str) -> Result<Vec<String>> {
    let mut scopes = raw
        .split(',')
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    for scope in &scopes {
        if scope.contains('%') {
            return Err(Error::InvalidInput("scope must not contain percent escapes".into()));
        }
    }
    Ok(scopes)
}

fn status_json(name: &str, meta: &ConnMeta) -> String {
    serde_json::json!({
        "name": name,
        "kind": meta.kind,
        "authorized": meta.authorized,
        "scopes": meta.scopes,
        "expires_at": meta.expires_at,
    })
    .to_string()
}
