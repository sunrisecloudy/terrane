//! The `publish` capability — replayable trust and provenance for signed app bundles.
//!
//! Archive parsing, ed25519 verification, TOFU prompting, and private key access
//! are host-edge effects. This capability folds only public facts: publisher
//! identity, trusted publisher keys, and installed app provenance.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_event, encode_event, state_mut, AppId, CapManifest, Capability, CommandCtx,
    CommandSpec, Decision, Effect, Error, EventPattern, EventRecord, EventSpec, Result, StateStore,
};

mod doc;

pub const MAX_ARCHIVE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ARCHIVE_FILES: usize = 512;
pub const FORMAT_VERSION: u32 = 1;
pub const PUBKEY_B64_LEN: usize = 44;
pub const SIG_B64_LEN: usize = 88;
pub const MAX_LABEL_LEN: usize = 128;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishState {
    pub identity: Option<String>,
    pub trusted: BTreeMap<String, String>,
    pub provenance: BTreeMap<AppId, Provenance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub version: String,
    pub bundle_hash: String,
    pub publisher_pubkey: String,
    pub publisher_label: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Installed {
    app: String,
    version: String,
    bundle_hash: String,
    publisher_pubkey: String,
    publisher_label: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Trusted {
    pubkey: String,
    label: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct IdentityCreated {
    pubkey: String,
    replica_peer: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct AppRemoved {
    id: String,
}

pub struct PublishCapability;

impl Capability for PublishCapability {
    fn namespace(&self) -> &'static str {
        "publish"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec {
                name: "publish.install",
            }],
            events: vec![
                EventSpec {
                    kind: "publish.identity-created",
                },
                EventSpec {
                    kind: "publish.trusted",
                },
                EventSpec {
                    kind: "publish.installed",
                },
            ],
            queries: Vec::new(),
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: vec![EventPattern { kind: "app.removed" }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::publish_doc(include_internal)
    }

    fn decide(&self, _ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "publish.install" => {
                let source = arg(args, 0, "archive path")?;
                if args.len() != 1 {
                    return Err(Error::InvalidInput(format!(
                        "publish.install takes exactly one archive path, got {}",
                        args.len()
                    )));
                }
                validate_archive_source(&source)?;
                Ok(Decision::Effect(Effect::InstallSignedBundle { source }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "publish.identity-created" => {
                let e: IdentityCreated = decode_event(record)?;
                validate_pubkey(&e.pubkey)?;
                validate_replica_peer(&e.replica_peer)?;
                let state = state_mut::<PublishState>(state, "publish")?;
                if state.identity.is_none() {
                    state.identity = Some(e.pubkey);
                }
            }
            "publish.trusted" => {
                let e: Trusted = decode_event(record)?;
                validate_pubkey(&e.pubkey)?;
                validate_label(&e.label)?;
                state_mut::<PublishState>(state, "publish")?
                    .trusted
                    .entry(e.pubkey)
                    .or_insert(e.label);
            }
            "publish.installed" => {
                let e: Installed = decode_event(record)?;
                validate_app_id(&e.app)?;
                terrane_cap_app::validate_version(&e.version)?;
                validate_hash(&e.bundle_hash)?;
                validate_pubkey(&e.publisher_pubkey)?;
                validate_label(&e.publisher_label)?;
                state_mut::<PublishState>(state, "publish")?
                    .provenance
                    .insert(
                        e.app,
                        Provenance {
                            version: e.version,
                            bundle_hash: e.bundle_hash,
                            publisher_pubkey: e.publisher_pubkey,
                            publisher_label: e.publisher_label,
                        },
                    );
            }
            "app.removed" => {
                let e: AppRemoved = decode_event(record)?;
                state_mut::<PublishState>(state, "publish")?
                    .provenance
                    .remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "publish.identity-created" => decode_event::<IdentityCreated>(record)
                .ok()
                .map(|e| format!("publish.identity-created {}", short_key(&e.pubkey))),
            "publish.trusted" => decode_event::<Trusted>(record)
                .ok()
                .map(|e| format!("publish.trusted {} \"{}\"", short_key(&e.pubkey), e.label)),
            "publish.installed" => decode_event::<Installed>(record)
                .ok()
                .map(|e| {
                    format!(
                        "publish.installed {} {} by {} ({})",
                        e.app,
                        e.version,
                        e.publisher_label,
                        short_key(&e.publisher_pubkey)
                    )
                }),
            _ => None,
        }
    }
}

pub fn installed_event(
    app: impl Into<String>,
    version: impl Into<String>,
    bundle_hash: impl Into<String>,
    publisher_pubkey: impl Into<String>,
    publisher_label: impl Into<String>,
) -> Result<EventRecord> {
    let event = Installed {
        app: app.into(),
        version: version.into(),
        bundle_hash: bundle_hash.into(),
        publisher_pubkey: publisher_pubkey.into(),
        publisher_label: publisher_label.into(),
    };
    validate_app_id(&event.app)?;
    terrane_cap_app::validate_version(&event.version)?;
    validate_hash(&event.bundle_hash)?;
    validate_pubkey(&event.publisher_pubkey)?;
    validate_label(&event.publisher_label)?;
    encode_event("publish.installed", &event)
}

pub fn trusted_event(pubkey: impl Into<String>, label: impl Into<String>) -> Result<EventRecord> {
    let event = Trusted {
        pubkey: pubkey.into(),
        label: label.into(),
    };
    validate_pubkey(&event.pubkey)?;
    validate_label(&event.label)?;
    encode_event("publish.trusted", &event)
}

pub fn identity_created_event(
    pubkey: impl Into<String>,
    replica_peer: impl Into<String>,
) -> Result<EventRecord> {
    let event = IdentityCreated {
        pubkey: pubkey.into(),
        replica_peer: replica_peer.into(),
    };
    validate_pubkey(&event.pubkey)?;
    validate_replica_peer(&event.replica_peer)?;
    encode_event("publish.identity-created", &event)
}

pub fn validate_archive_source(source: &str) -> Result<()> {
    if source.trim().is_empty() {
        return Err(Error::InvalidInput("publish archive path must not be empty".into()));
    }
    Ok(())
}

pub fn validate_format_version(format_version: u32) -> Result<()> {
    if format_version != FORMAT_VERSION {
        return Err(Error::InvalidInput(format!(
            "unsupported publish formatVersion {format_version}; newest supported is {FORMAT_VERSION}"
        )));
    }
    Ok(())
}

pub fn validate_archive_limits(byte_len: usize, file_count: usize) -> Result<()> {
    if byte_len > MAX_ARCHIVE_BYTES {
        return Err(Error::InvalidInput(format!(
            "publish archive exceeds {MAX_ARCHIVE_BYTES} bytes"
        )));
    }
    if file_count > MAX_ARCHIVE_FILES {
        return Err(Error::InvalidInput(format!(
            "publish archive has too many files: max {MAX_ARCHIVE_FILES}"
        )));
    }
    Ok(())
}

pub fn validate_pubkey(value: &str) -> Result<()> {
    validate_b64_len(value, PUBKEY_B64_LEN, "publisher public key")
}

pub fn validate_signature(value: &str) -> Result<()> {
    validate_b64_len(value, SIG_B64_LEN, "publisher signature")
}

pub fn validate_label(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > MAX_LABEL_LEN {
        return Err(Error::InvalidInput(format!(
            "publisher label must be 1..={MAX_LABEL_LEN} bytes"
        )));
    }
    Ok(())
}

pub fn validate_hash(value: &str) -> Result<()> {
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(Error::InvalidInput(format!(
        "bundle hash must be lowercase sha256 hex: {value:?}"
    )))
}

fn validate_b64_len(value: &str, len: usize, label: &str) -> Result<()> {
    if value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
    {
        return Ok(());
    }
    Err(Error::InvalidInput(format!("{label} must be base64 length {len}")))
}

fn validate_replica_peer(value: &str) -> Result<()> {
    if value.starts_with("0x") && value.len() > 2 {
        return Ok(());
    }
    Err(Error::InvalidInput(format!(
        "replica_peer must be hex display like 0x..., got {value:?}"
    )))
}

fn validate_app_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "app id is unsafe: {id:?}; use ASCII letters, digits, '-' or '_'"
        )));
    }
    Ok(())
}

fn short_key(pubkey: &str) -> String {
    pubkey.chars().take(10).collect()
}
