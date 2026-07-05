//! Host-edge signed archive support for the `publish` capability.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use serde_json::{json, Value};
use terrane_core::{Error, EventRecord, Result, State};

use crate::edge::EdgeRunner;

const ARCHIVE_HEADER: &[u8] = b"TRNPUBLISH1\n";
const PUBLISH_JSON: &str = "publish.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOutcome {
    pub app: String,
    pub version: String,
    pub path: PathBuf,
    pub bundle_hash: String,
    pub publisher_pubkey: String,
}

impl ExportOutcome {
    pub fn message(&self) -> String {
        format!(
            "exported {} {} -> {} ({})",
            self.app,
            self.version,
            self.path.display(),
            self.bundle_hash
        )
    }
}

struct VerifiedArchive {
    files: BTreeMap<String, String>,
    app: String,
    version: String,
    bundle_hash: String,
    publisher_pubkey: String,
    publisher_label: String,
}

pub fn export_app_archive(
    core: &crate::HostCore,
    app: &str,
    output: Option<&Path>,
) -> Result<ExportOutcome> {
    let home = crate::home_dir();
    let record = core
        .state()
        .app
        .apps
        .get(app)
        .ok_or_else(|| Error::AppNotFound(app.to_string()))?;
    let files = crate::edge::current_bundle_files(app, record.source.as_deref(), core.state())?;
    let manifest = crate::edge::manifest_from_files(&files)?;
    let version = manifest.version.clone();
    let bundle_hash = crate::edge::bundle_hash(&files)?;
    let (pubkey, signature) = sign_archive_metadata(&home, core.state(), app, &version, &bundle_hash)?;
    let publisher_label = publisher_label(core.state());
    let replica_peer = core
        .state()
        .replica
        .peer
        .map(|peer| format!("{peer:#x}"))
        .unwrap_or_else(|| "0x0".to_string());
    let publish_json = publish_json(
        app,
        &version,
        &bundle_hash,
        &pubkey,
        &replica_peer,
        &publisher_label,
        &signature,
    );
    let archive = encode_publish_archive(&files, &publish_json)?;
    let path = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(format!("{app}-{version}.terrane")));
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| {
            Error::Storage(format!("create export directory {}: {e}", parent.display()))
        })?;
    }
    let mut file = fs::File::create(&path)
        .map_err(|e| Error::Storage(format!("create archive {}: {e}", path.display())))?;
    file.write_all(&archive)
        .map_err(|e| Error::Storage(format!("write archive {}: {e}", path.display())))?;
    Ok(ExportOutcome {
        app: app.to_string(),
        version,
        path,
        bundle_hash,
        publisher_pubkey: pubkey,
    })
}

pub fn install_signed_bundle(
    runner: &EdgeRunner,
    state: &State,
    source: &str,
) -> Result<Vec<EventRecord>> {
    let bytes = read_archive_source(source)?;
    terrane_cap_publish::validate_archive_limits(bytes.len(), 0)?;
    let verified = verify_archive(&bytes)?;
    if let Some(existing) = state.publish.provenance.get(&verified.app) {
        if existing.publisher_pubkey != verified.publisher_pubkey {
            return Err(Error::InvalidInput(format!(
                "publisher key changed for installed app {}; trusted review is required before replacing {} with {}",
                verified.app, existing.publisher_pubkey, verified.publisher_pubkey
            )));
        }
    }

    let mut records = Vec::new();
    if !state.publish.trusted.contains_key(&verified.publisher_pubkey) {
        records.push(terrane_cap_publish::trusted_event(
            verified.publisher_pubkey.clone(),
            verified.publisher_label.clone(),
        )?);
    }

    if state.app.apps.contains_key(&verified.app) {
        records.extend(crate::edge::upgrade_app_bundle_files(
            runner,
            &verified.app,
            source,
            verified.files.clone(),
            state,
        )?);
    } else {
        records.extend(crate::edge::import_app_bundle_files(
            source,
            verified.files.clone(),
            &None,
            &None,
            state,
        )?);
    }
    records.push(terrane_cap_publish::installed_event(
        verified.app,
        verified.version,
        verified.bundle_hash,
        verified.publisher_pubkey,
        verified.publisher_label,
    )?);
    Ok(records)
}

fn sign_archive_metadata(
    home: &Path,
    state: &State,
    app: &str,
    version: &str,
    bundle_hash: &str,
) -> Result<(String, String)> {
    let person_id = state
        .person
        .persons
        .keys()
        .next()
        .ok_or_else(|| Error::Runtime("publish export requires a local person identity".into()))?;
    let signing = crate::edge::load_person_key(home, person_id)?;
    let signature = signing.sign(&signing_message(bundle_hash, app, version));
    Ok((
        B64.encode(signing.verifying_key().to_bytes()),
        B64.encode(signature.to_bytes()),
    ))
}

fn verify_archive(bytes: &[u8]) -> Result<VerifiedArchive> {
    let mut files = decode_publish_archive(bytes)?;
    terrane_cap_publish::validate_archive_limits(bytes.len(), files.len())?;
    let publish_text = files
        .remove(PUBLISH_JSON)
        .ok_or_else(|| Error::InvalidInput("signed archive is missing publish.json".into()))?;
    let meta: Value = serde_json::from_str(&publish_text)
        .map_err(|e| Error::InvalidInput(format!("publish.json must be JSON: {e}")))?;
    let format_version = json_u32(&meta, "formatVersion")?;
    terrane_cap_publish::validate_format_version(format_version)?;
    let app = json_str(&meta, "app")?.to_string();
    let version = json_str(&meta, "version")?.to_string();
    let bundle_hash = json_str(&meta, "bundleHash")?.to_string();
    let publisher = meta
        .get("publisher")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::InvalidInput("publish.json publisher must be an object".into()))?;
    let publisher_pubkey = publisher
        .get("pubkey")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput("publish.json publisher.pubkey is required".into()))?
        .to_string();
    let publisher_label = publisher
        .get("label")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput("publish.json publisher.label is required".into()))?
        .to_string();
    let replica_peer = publisher
        .get("replicaPeer")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput("publish.json publisher.replicaPeer is required".into()))?;
    let signature = json_str(&meta, "signature")?.to_string();

    terrane_cap_publish::validate_pubkey(&publisher_pubkey)?;
    terrane_cap_publish::validate_label(&publisher_label)?;
    terrane_cap_publish::validate_signature(&signature)?;
    terrane_cap_publish::validate_hash(&bundle_hash)?;
    terrane_cap_publish::validate_format_version(format_version)?;
    terrane_cap_publish::identity_created_event(&publisher_pubkey, replica_peer)?;

    let manifest = crate::edge::manifest_from_files(&files)?;
    if manifest.id != app {
        return Err(Error::InvalidInput(format!(
            "publish app {app:?} does not match manifest id {:?}",
            manifest.id
        )));
    }
    if manifest.version != version {
        return Err(Error::InvalidInput(format!(
            "publish version {version:?} does not match manifest version {:?}",
            manifest.version
        )));
    }
    let actual_hash = crate::edge::bundle_hash(&files)?;
    if actual_hash != bundle_hash {
        return Err(Error::InvalidInput(format!(
            "publish bundleHash mismatch: expected {bundle_hash}, got {actual_hash}"
        )));
    }
    verify_signature(&publisher_pubkey, &signature, &signing_message(&bundle_hash, &app, &version))?;
    Ok(VerifiedArchive {
        files,
        app,
        version,
        bundle_hash,
        publisher_pubkey,
        publisher_label,
    })
}

fn encode_publish_archive(files: &BTreeMap<String, String>, publish_json: &str) -> Result<Vec<u8>> {
    let mut all = files.clone();
    if all.contains_key(PUBLISH_JSON) {
        return Err(Error::InvalidInput(
            "bundle file publish.json is reserved for signed archive metadata".into(),
        ));
    }
    all.insert(PUBLISH_JSON.to_string(), publish_json.to_string());
    let body = crate::edge::encode_bundle_archive(&all)?;
    let mut out = Vec::with_capacity(ARCHIVE_HEADER.len() + body.len());
    out.extend_from_slice(ARCHIVE_HEADER);
    out.extend_from_slice(&body);
    Ok(out)
}

fn decode_publish_archive(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    if !bytes.starts_with(ARCHIVE_HEADER) {
        return Err(Error::InvalidInput("publish archive header mismatch".into()));
    }
    crate::edge::decode_bundle_archive(&bytes[ARCHIVE_HEADER.len()..])
}

fn publish_json(
    app: &str,
    version: &str,
    bundle_hash: &str,
    pubkey: &str,
    replica_peer: &str,
    label: &str,
    signature: &str,
) -> String {
    json!({
        "formatVersion": terrane_cap_publish::FORMAT_VERSION,
        "app": app,
        "version": version,
        "bundleHash": bundle_hash,
        "publisher": {
            "pubkey": pubkey,
            "replicaPeer": replica_peer,
            "label": label
        },
        "signature": signature
    })
    .to_string()
}

fn signing_message(bundle_hash: &str, app: &str, version: &str) -> Vec<u8> {
    let mut message = Vec::new();
    message.extend_from_slice(bundle_hash.as_bytes());
    message.push(0);
    message.extend_from_slice(app.as_bytes());
    message.push(0);
    message.extend_from_slice(version.as_bytes());
    message
}

fn verify_signature(pubkey: &str, signature: &str, message: &[u8]) -> Result<()> {
    let pubkey_bytes = B64
        .decode(pubkey)
        .map_err(|e| Error::InvalidInput(format!("decode publisher public key: {e}")))?;
    let pubkey_array: [u8; 32] = pubkey_bytes
        .as_slice()
        .try_into()
        .map_err(|_| Error::InvalidInput("publisher public key has invalid length".into()))?;
    let verifying = VerifyingKey::from_bytes(&pubkey_array)
        .map_err(|e| Error::InvalidInput(format!("publisher public key is invalid: {e}")))?;
    let sig_bytes = B64
        .decode(signature)
        .map_err(|e| Error::InvalidInput(format!("decode publisher signature: {e}")))?;
    let sig_array: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| Error::InvalidInput("publisher signature has invalid length".into()))?;
    let sig = Signature::from_bytes(&sig_array);
    verifying
        .verify(message, &sig)
        .map_err(|e| Error::InvalidInput(format!("publisher signature verification failed: {e}")))
}

fn read_archive_source(source: &str) -> Result<Vec<u8>> {
    if source.starts_with("http://") || source.starts_with("https://") {
        return read_archive_url(source);
    }
    fs::read(source).map_err(|e| Error::Storage(format!("read publish archive {source}: {e}")))
}

fn read_archive_url(url: &str) -> Result<Vec<u8>> {
    let response = ureq::get(url)
        .call()
        .map_err(|e| Error::Runtime(format!("fetch publish archive {url}: {e}")))?;
    let mut reader = response
        .into_reader()
        .take(u64::try_from(terrane_cap_publish::MAX_ARCHIVE_BYTES + 1).unwrap_or(u64::MAX));
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Runtime(format!("read publish archive response: {e}")))?;
    Ok(bytes)
}

fn json_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput(format!("publish.json {key} must be a string")))
}

fn json_u32(value: &Value, key: &str) -> Result<u32> {
    let raw = value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::InvalidInput(format!("publish.json {key} must be an integer")))?;
    u32::try_from(raw)
        .map_err(|_| Error::InvalidInput(format!("publish.json {key} is too large")))
}

fn publisher_label(state: &State) -> String {
    std::env::var("USER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| state.person.persons.keys().next().cloned())
        .unwrap_or_else(|| "local-owner".to_string())
}
