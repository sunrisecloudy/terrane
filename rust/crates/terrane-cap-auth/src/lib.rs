//! The `auth` capability owns durable authorization facts and folded AuthState.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord,
    EventSpec,
    ExecutionPrincipal, ReadValue, ResourceReadCtx, Result, StateStore, LOCAL_OWNER_SUBJECT,
    LOCAL_SOURCE, NAMESPACE_SELECTOR_SCHEMA_ID,
};

mod doc;
#[cfg(test)]
mod tests;

const DEFAULT_VERBS: &[&str] = &["read", "write"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthState {
    pub grants: BTreeMap<String, AuthGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthGrant {
    pub org: String,
    pub subject: String,
    pub app: String,
    pub namespace: String,
    pub selector_schema_id: String,
    pub selector_id: String,
    pub selector_json: String,
    pub resource_id: String,
    pub verbs: Vec<String>,
    pub granted_by: String,
    pub source: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Granted {
    org: String,
    subject: String,
    app: String,
    namespace: String,
    selector_schema_id: String,
    selector_id: String,
    selector_json: String,
    resource_id: String,
    verbs: Vec<String>,
    granted_by: String,
    source: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Revoked {
    org: String,
    subject: String,
    app: String,
    resource_id: String,
    revoked_by: String,
    source: String,
}

pub struct AuthCapability;

impl Capability for AuthCapability {
    fn namespace(&self) -> &'static str {
        "auth"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "auth.grant" },
                CommandSpec {
                    name: "auth.revoke",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "auth.granted",
                },
                EventSpec {
                    kind: "auth.revoked",
                },
            ],
            queries: Vec::new(),
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::auth_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "auth.grant" => decide_grant(ctx, args),
            "auth.revoke" => decide_revoke(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "auth.granted" => decode_event::<Granted>(record).ok().map(|e| {
                format!(
                    "granted {} access to {} for app {}",
                    e.subject, e.resource_id, e.app
                )
            }),
            "auth.revoked" => decode_event::<Revoked>(record).ok().map(|e| {
                format!(
                    "revoked {} access to {} for app {}",
                    e.subject, e.resource_id, e.app
                )
            }),
            _ => None,
        }
    }

    fn read_resource(
        &self,
        _ctx: ResourceReadCtx<'_>,
        name: &str,
        _args: &[String],
    ) -> Result<ReadValue> {
        Err(Error::InvalidInput(format!(
            "auth has no public resource API: {name}"
        )))
    }
}

fn decide_grant(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let subject = non_empty(arg(args, 0, "subject")?, "subject")?;
    let app = non_empty(arg(args, 1, "app")?, "app")?;
    let namespace = non_empty(arg(args, 2, "namespace")?, "namespace")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_segment_input("subject", &subject)?;
    validate_segment_input("app", &app)?;
    validate_segment_input("namespace", &namespace)?;

    let verbs = match args.get(3) {
        Some(raw) => parse_verbs(raw)?,
        None => DEFAULT_VERBS
            .iter()
            .map(|verb| (*verb).to_string())
            .collect(),
    };
    let resource_id = namespace_resource_id(&namespace);
    let key = grant_key(
        terrane_cap_interface::LOCAL_ORG,
        &subject,
        &app,
        &resource_id,
    );
    if state_ref::<AuthState>(ctx.state, "auth")?
        .grants
        .contains_key(&key)
    {
        return Ok(Decision::Commit(Vec::new()));
    }

    Ok(Decision::Commit(vec![granted_event(Granted {
        org: terrane_cap_interface::LOCAL_ORG.to_string(),
        subject,
        app,
        namespace: namespace.clone(),
        selector_schema_id: NAMESPACE_SELECTOR_SCHEMA_ID.to_string(),
        selector_id: String::new(),
        selector_json: format!(r#"{{"namespace":"{}"}}"#, json_string(&namespace)),
        resource_id,
        verbs,
        granted_by: LOCAL_OWNER_SUBJECT.to_string(),
        source: LOCAL_SOURCE.to_string(),
    })?]))
}

fn decide_revoke(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let subject = non_empty(arg(args, 0, "subject")?, "subject")?;
    let app = non_empty(arg(args, 1, "app")?, "app")?;
    let namespace = non_empty(arg(args, 2, "namespace")?, "namespace")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_segment_input("subject", &subject)?;
    validate_segment_input("app", &app)?;
    validate_segment_input("namespace", &namespace)?;

    let resource_id = namespace_resource_id(&namespace);
    let key = grant_key(
        terrane_cap_interface::LOCAL_ORG,
        &subject,
        &app,
        &resource_id,
    );
    if !state_ref::<AuthState>(ctx.state, "auth")?
        .grants
        .contains_key(&key)
    {
        return Ok(Decision::Commit(Vec::new()));
    }

    Ok(Decision::Commit(vec![revoked_event(Revoked {
        org: terrane_cap_interface::LOCAL_ORG.to_string(),
        subject,
        app,
        resource_id,
        revoked_by: LOCAL_OWNER_SUBJECT.to_string(),
        source: LOCAL_SOURCE.to_string(),
    })?]))
}

fn granted_event(event: Granted) -> Result<EventRecord> {
    encode_event("auth.granted", &event)
}

fn revoked_event(event: Revoked) -> Result<EventRecord> {
    encode_event("auth.revoked", &event)
}

fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "auth.granted" => {
            let event: Granted = decode_event(record)?;
            let key = grant_key(&event.org, &event.subject, &event.app, &event.resource_id);
            state_mut::<AuthState>(state, "auth")?.grants.insert(
                key,
                AuthGrant {
                    org: event.org,
                    subject: event.subject,
                    app: event.app,
                    namespace: event.namespace,
                    selector_schema_id: event.selector_schema_id,
                    selector_id: event.selector_id,
                    selector_json: event.selector_json,
                    resource_id: event.resource_id,
                    verbs: event.verbs,
                    granted_by: event.granted_by,
                    source: event.source,
                },
            );
        }
        "auth.revoked" => {
            let event: Revoked = decode_event(record)?;
            let key = grant_key(&event.org, &event.subject, &event.app, &event.resource_id);
            state_mut::<AuthState>(state, "auth")?.grants.remove(&key);
        }
        "app.removed" => {
            let event = decode_app_removed(record)?;
            state_mut::<AuthState>(state, "auth")?
                .grants
                .retain(|_, grant| grant.app != event.id);
        }
        _ => {}
    }
    Ok(())
}

pub fn namespace_granted(
    state: &dyn StateStore,
    principal: &ExecutionPrincipal,
    app: &str,
    namespace: &str,
) -> Result<bool> {
    let resource_id = namespace_resource_id(namespace);
    let key = grant_key(&principal.org, &principal.subject, app, &resource_id);
    Ok(state_ref::<AuthState>(state, "auth")?
        .grants
        .contains_key(&key))
}

pub fn namespace_resource_id(namespace: &str) -> String {
    namespace.to_string()
}

pub fn grant_key(org: &str, subject: &str, app: &str, resource_id: &str) -> String {
    format!(
        "orgs/{}/subjects/{}/apps/{}/resources/{}",
        encode_segment(org),
        encode_segment(subject),
        encode_segment(app),
        encode_segment(resource_id)
    )
}

pub fn encode_segment(raw: &str) -> String {
    let mut out = String::new();
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(hex(byte >> 4));
            out.push(hex(byte & 0x0f));
        }
    }
    out
}

pub fn decode_segment(encoded: &str) -> Result<String> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 2 >= bytes.len() {
            return Err(Error::InvalidInput(format!(
                "bad percent escape in key segment: {encoded:?}"
            )));
        }
        let high = unhex(bytes[i + 1])?;
        let low = unhex(bytes[i + 2])?;
        out.push((high << 4) | low);
        i += 3;
    }
    String::from_utf8(out)
        .map_err(|e| Error::InvalidInput(format!("bad UTF-8 in key segment: {e}")))
}

fn non_empty(value: String, label: &str) -> Result<String> {
    if value.trim().is_empty() {
        return Err(Error::InvalidInput(format!("{label} must not be empty")));
    }
    Ok(value)
}

fn validate_segment_input(label: &str, value: &str) -> Result<()> {
    if value.contains('%') {
        return Err(Error::InvalidInput(format!(
            "{label} must not contain raw percent escapes"
        )));
    }
    Ok(())
}

fn parse_verbs(raw: &str) -> Result<Vec<String>> {
    let verbs: Vec<_> = raw
        .split(',')
        .map(str::trim)
        .filter(|verb| !verb.is_empty())
        .map(ToString::to_string)
        .collect();
    if verbs.is_empty() {
        return Err(Error::InvalidInput("verbs must not be empty".into()));
    }
    for verb in &verbs {
        if !verb
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(Error::InvalidInput(format!("unsafe grant verb: {verb:?}")));
        }
    }
    Ok(verbs)
}

fn hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => unreachable!("nibble"),
    }
}

fn unhex(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::InvalidInput(format!(
            "bad hex digit in key segment: {:?}",
            byte as char
        ))),
    }
}

fn json_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
