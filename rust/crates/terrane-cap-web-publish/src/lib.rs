//! The `web-publish` capability - replayable public URL intent for Premium relay serving.
//!
//! This capability records only durable facts: which apps should be public,
//! their relay slug/domain, and their publish mode. Relay auth, tunnel dialing,
//! request logs, and live health are host/Premium edge concerns and never enter
//! replay state.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_event, encode_event, ensure_app_exists, state_mut, state_ref, CapManifest,
    Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord, EventSpec,
    QueryCtx, QuerySpec, QueryValue, Result, StateStore,
};

mod doc;

pub const MAX_SLUG_LEN: usize = 63;
pub const MAX_DOMAIN_LEN: usize = 253;
pub const MAX_PUBLIC_VERBS: usize = 16;
pub const MAX_INTERACTIVE_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebPublishState {
    pub apps: BTreeMap<String, PublishedApp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedApp {
    pub mode: PublishMode,
    pub slug: String,
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishMode {
    Static,
    Interactive,
}

impl PublishMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PublishMode::Static => "static",
            PublishMode::Interactive => "interactive",
        }
    }
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Enabled {
    app: String,
    mode: String,
    slug: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Disabled {
    app: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct DomainSet {
    app: String,
    domain: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct AppRemoved {
    id: String,
}

pub struct WebPublishCapability;

impl Capability for WebPublishCapability {
    fn namespace(&self) -> &'static str {
        "web-publish"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "web-publish.enable",
                },
                CommandSpec {
                    name: "web-publish.disable",
                },
                CommandSpec {
                    name: "web-publish.domain.set",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "web-publish.enabled",
                },
                EventSpec {
                    kind: "web-publish.disabled",
                },
                EventSpec {
                    kind: "web-publish.domain.set",
                },
            ],
            queries: vec![QuerySpec {
                name: "web-publish.status",
            }],
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: vec![EventPattern { kind: "app.removed" }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::web_publish_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "web-publish.enable" => {
                let app = validate_app(&arg(args, 0, "app")?)?;
                ensure_app_exists(ctx.bus, &app)?;
                let mode = args
                    .get(1)
                    .map(String::as_str)
                    .unwrap_or("static")
                    .parse::<PublishMode>()?;
                let slug = match args.get(2) {
                    Some(value) if !value.trim().is_empty() => validate_slug(value)?,
                    _ => default_slug(&app),
                };
                Ok(Decision::Commit(vec![enabled_event(&app, mode, &slug)?]))
            }
            "web-publish.disable" => {
                let app = validate_app(&arg(args, 0, "app")?)?;
                Ok(Decision::Commit(vec![disabled_event(&app)?]))
            }
            "web-publish.domain.set" => {
                let app = validate_app(&arg(args, 0, "app")?)?;
                let domain = validate_domain(&arg(args, 1, "domain")?)?;
                let state = state_ref::<WebPublishState>(ctx.state, "web-publish")?;
                if !state.apps.contains_key(&app) {
                    return Err(Error::InvalidInput(format!(
                        "cannot set domain for unpublished app: {app}"
                    )));
                }
                Ok(Decision::Commit(vec![domain_set_event(&app, &domain)?]))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "web-publish.enabled" => {
                let e: Enabled = decode_event(record)?;
                let mode = e.mode.parse::<PublishMode>()?;
                validate_app(&e.app)?;
                validate_slug(&e.slug)?;
                state_mut::<WebPublishState>(state, "web-publish")?
                    .apps
                    .insert(
                        e.app,
                        PublishedApp {
                            mode,
                            slug: e.slug,
                            domain: None,
                        },
                    );
            }
            "web-publish.disabled" => {
                let e: Disabled = decode_event(record)?;
                state_mut::<WebPublishState>(state, "web-publish")?
                    .apps
                    .remove(&e.app);
            }
            "web-publish.domain.set" => {
                let e: DomainSet = decode_event(record)?;
                validate_domain(&e.domain)?;
                if let Some(app) = state_mut::<WebPublishState>(state, "web-publish")?
                    .apps
                    .get_mut(&e.app)
                {
                    app.domain = Some(e.domain);
                }
            }
            "app.removed" => {
                let e: AppRemoved = decode_event(record)?;
                state_mut::<WebPublishState>(state, "web-publish")?
                    .apps
                    .remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "status" => Ok(QueryValue::Json(status_json(ctx.state, args)?)),
            other => Err(Error::InvalidInput(format!("unknown query: {other}"))),
        }
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "web-publish.enabled" => decode_event::<Enabled>(record)
                .ok()
                .map(|e| format!("web-publish.enabled {} {} {}", e.app, e.mode, e.slug)),
            "web-publish.disabled" => decode_event::<Disabled>(record)
                .ok()
                .map(|e| format!("web-publish.disabled {}", e.app)),
            "web-publish.domain.set" => decode_event::<DomainSet>(record)
                .ok()
                .map(|e| format!("web-publish.domain.set {} {}", e.app, e.domain)),
            _ => None,
        }
    }

    fn app_of(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "web-publish.enabled" => decode_event::<Enabled>(record).ok().map(|e| e.app),
            "web-publish.disabled" => decode_event::<Disabled>(record).ok().map(|e| e.app),
            "web-publish.domain.set" => decode_event::<DomainSet>(record).ok().map(|e| e.app),
            _ => None,
        }
    }
}

impl std::str::FromStr for PublishMode {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "static" => Ok(PublishMode::Static),
            "interactive" => Ok(PublishMode::Interactive),
            other => Err(Error::InvalidInput(format!(
                "web-publish mode must be static or interactive: {other}"
            ))),
        }
    }
}

pub fn enabled_event(app: &str, mode: PublishMode, slug: &str) -> Result<EventRecord> {
    encode_event(
        "web-publish.enabled",
        &Enabled {
            app: validate_app(app)?,
            mode: mode.as_str().to_string(),
            slug: validate_slug(slug)?,
        },
    )
}

pub fn disabled_event(app: &str) -> Result<EventRecord> {
    encode_event(
        "web-publish.disabled",
        &Disabled {
            app: validate_app(app)?,
        },
    )
}

pub fn domain_set_event(app: &str, domain: &str) -> Result<EventRecord> {
    encode_event(
        "web-publish.domain.set",
        &DomainSet {
            app: validate_app(app)?,
            domain: validate_domain(domain)?,
        },
    )
}

pub fn status_json(state: &dyn StateStore, args: &[String]) -> Result<String> {
    let state = state_ref::<WebPublishState>(state, "web-publish")?;
    if let Some(app) = args.first().filter(|arg| !arg.trim().is_empty()) {
        let app = validate_app(app)?;
        let value = state.apps.get(&app).map(|published| {
            serde_json::json!({
                "app": app,
                "enabled": true,
                "mode": published.mode.as_str(),
                "slug": published.slug,
                "domain": published.domain,
                "url": public_url(published),
            })
        });
        return Ok(value
            .unwrap_or_else(|| serde_json::json!({ "app": app, "enabled": false }))
            .to_string());
    }
    let apps = state
        .apps
        .iter()
        .map(|(app, published)| {
            serde_json::json!({
                "app": app,
                "enabled": true,
                "mode": published.mode.as_str(),
                "slug": published.slug,
                "domain": published.domain,
                "url": public_url(published),
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({ "apps": apps }).to_string())
}

pub fn validate_public_verbs(public_verbs: &[String]) -> Result<()> {
    if public_verbs.len() > MAX_PUBLIC_VERBS {
        return Err(Error::InvalidInput(format!(
            "manifest publicVerbs exceeds {MAX_PUBLIC_VERBS} entries"
        )));
    }
    for verb in public_verbs {
        validate_public_verb(verb)?;
    }
    Ok(())
}

fn validate_app(app: &str) -> Result<String> {
    if app.is_empty()
        || !app
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "app id is unsafe: {app:?}; use ASCII letters, digits, '-' or '_'"
        )));
    }
    Ok(app.to_string())
}

fn validate_slug(slug: &str) -> Result<String> {
    let slug = slug.trim().to_ascii_lowercase();
    if slug.is_empty() || slug.len() > MAX_SLUG_LEN {
        return Err(Error::InvalidInput(format!(
            "web-publish slug must be 1..={MAX_SLUG_LEN} chars"
        )));
    }
    if slug.starts_with('-')
        || slug.ends_with('-')
        || !slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(Error::InvalidInput(
            "web-publish slug must match DNS label characters [a-z0-9-] and not start/end with '-'"
                .into(),
        ));
    }
    Ok(slug)
}

fn validate_domain(domain: &str) -> Result<String> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() || domain.len() > MAX_DOMAIN_LEN || !domain.contains('.') {
        return Err(Error::InvalidInput(
            "web-publish domain must be a non-empty fully qualified domain name".into(),
        ));
    }
    for label in domain.split('.') {
        validate_slug(label)?;
    }
    Ok(domain)
}

fn validate_public_verb(verb: &str) -> Result<()> {
    if verb.is_empty()
        || !verb
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "manifest publicVerbs entry is unsafe: {verb:?}"
        )));
    }
    Ok(())
}

fn default_slug(app: &str) -> String {
    app.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn public_url(published: &PublishedApp) -> String {
    match &published.domain {
        Some(domain) => format!("https://{domain}"),
        None => format!("https://{}.terrane.app", published.slug),
    }
}
