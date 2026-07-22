//! Stable wire and package contracts for native Terrane capability workers.
//!
//! Workers are ordinary native Rust executables. They communicate with the
//! host over bounded, length-prefixed JSON frames so no Rust trait object or
//! compiler-specific ABI crosses the process boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use terrane_cap_interface::{
    Decision, Effect, EventRecord, ExecutionPrincipal, QueryValue, ReadValue, Request,
    RuntimeOutput,
};

pub const PROTOCOL_VERSION: u32 = 1;
pub const BUNDLE_FORMAT_VERSION: u32 = 1;
pub const LOCK_FORMAT_VERSION: u32 = 1;
pub const INDEX_FORMAT_VERSION: u32 = 1;
pub const DEFAULT_MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const FUNDAMENTAL_CAPABILITIES: [&str; 8] = [
    "app",
    "auth",
    "kv",
    "connection",
    "blob",
    "person",
    "replica",
    "telemetry",
];

pub fn is_fundamental(namespace: &str) -> bool {
    FUNDAMENTAL_CAPABILITIES.contains(&namespace)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedResourceMethod {
    pub name: String,
    pub kind: String,
    pub params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedGrantResourceSpec {
    pub namespace: String,
    pub selector_schema_id: String,
    pub selector_schema_json: String,
    pub verbs: Vec<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDeclaration {
    pub commands: Vec<String>,
    pub events: Vec<String>,
    pub queries: Vec<String>,
    pub resources: Vec<OwnedResourceMethod>,
    pub grant_resources: Vec<OwnedGrantResourceSpec>,
    pub subscriptions: Vec<String>,
    #[serde(default)]
    pub delegated_events: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationMode {
    #[default]
    Demand,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub format_version: u32,
    pub namespace: String,
    pub version: String,
    pub protocol_version: u32,
    pub state_schema_version: u32,
    pub platform: String,
    pub architecture: String,
    pub executable: String,
    pub executable_sha256: String,
    pub signature: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub activation: ActivationMode,
    pub declaration: CapabilityDeclaration,
}

impl BundleManifest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.format_version != BUNDLE_FORMAT_VERSION {
            return Err(ProtocolError::Invalid(format!(
                "unsupported bundle format version {}",
                self.format_version
            )));
        }
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolError::Invalid(format!(
                "unsupported worker protocol version {}",
                self.protocol_version
            )));
        }
        validate_token("namespace", &self.namespace)?;
        validate_token("version", &self.version)?;
        validate_token("platform", &self.platform)?;
        validate_token("architecture", &self.architecture)?;
        validate_executable_name(&self.executable)?;
        validate_sha256(&self.executable_sha256)?;
        if self.state_schema_version == 0 {
            return Err(ProtocolError::Invalid(
                "state schema version must be positive".into(),
            ));
        }
        let mut dependencies = BTreeSet::new();
        for dependency in &self.dependencies {
            validate_token("dependency namespace", dependency)?;
            if dependency == &self.namespace {
                return Err(ProtocolError::Invalid(format!(
                    "capability {} depends on itself",
                    self.namespace
                )));
            }
            if !dependencies.insert(dependency) {
                return Err(ProtocolError::Invalid(format!(
                    "duplicate dependency {dependency} for {}",
                    self.namespace
                )));
            }
        }
        validate_declaration(&self.namespace, &self.declaration)
    }

    pub fn signing_payload(&self) -> Result<Vec<u8>, ProtocolError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Unsigned<'a> {
            format_version: u32,
            namespace: &'a str,
            version: &'a str,
            protocol_version: u32,
            state_schema_version: u32,
            platform: &'a str,
            architecture: &'a str,
            executable: &'a str,
            executable_sha256: &'a str,
            dependencies: &'a [String],
            activation: ActivationMode,
            declaration: &'a CapabilityDeclaration,
        }
        serde_json::to_vec(&Unsigned {
            format_version: self.format_version,
            namespace: &self.namespace,
            version: &self.version,
            protocol_version: self.protocol_version,
            state_schema_version: self.state_schema_version,
            platform: &self.platform,
            architecture: &self.architecture,
            executable: &self.executable,
            executable_sha256: &self.executable_sha256,
            dependencies: &self.dependencies,
            activation: self.activation,
            declaration: &self.declaration,
        })
        .map_err(ProtocolError::Json)
    }

    pub fn verify_bundle(
        &self,
        bundle_dir: &Path,
        verifying_key: &VerifyingKey,
    ) -> Result<PathBuf, ProtocolError> {
        self.validate()?;
        let executable = bundle_dir.join(&self.executable);
        let actual = sha256_file(&executable)?;
        if actual != self.executable_sha256 {
            return Err(ProtocolError::Invalid(format!(
                "capability {} executable hash mismatch: expected {}, got {actual}",
                self.namespace, self.executable_sha256
            )));
        }
        let signature_bytes = decode_hex(&self.signature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|e| ProtocolError::Invalid(format!("invalid bundle signature: {e}")))?;
        verifying_key
            .verify(&self.signing_payload()?, &signature)
            .map_err(|e| {
                ProtocolError::Invalid(format!("bundle signature verification failed: {e}"))
            })?;
        Ok(executable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityIndexArtifact {
    pub archive: String,
    pub archive_sha256: String,
    pub manifest: BundleManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityIndex {
    pub format_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_base_url: Option<String>,
    pub artifacts: BTreeMap<String, CapabilityIndexArtifact>,
    pub signature: String,
}

impl CapabilityIndex {
    pub fn signing_payload(&self) -> Result<Vec<u8>, ProtocolError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Unsigned<'a> {
            format_version: u32,
            download_base_url: &'a Option<String>,
            artifacts: &'a BTreeMap<String, CapabilityIndexArtifact>,
        }
        serde_json::to_vec(&Unsigned {
            format_version: self.format_version,
            download_base_url: &self.download_base_url,
            artifacts: &self.artifacts,
        })
        .map_err(ProtocolError::Json)
    }

    pub fn validate(&self, verifying_key: &VerifyingKey) -> Result<(), ProtocolError> {
        if self.format_version != INDEX_FORMAT_VERSION {
            return Err(ProtocolError::Invalid(format!(
                "unsupported capability index version {}",
                self.format_version
            )));
        }
        if self.artifacts.is_empty() {
            return Err(ProtocolError::Invalid("capability index is empty".into()));
        }
        for (namespace, artifact) in &self.artifacts {
            if namespace != &artifact.manifest.namespace {
                return Err(ProtocolError::Invalid(format!(
                    "capability index key {namespace} does not match manifest namespace {}",
                    artifact.manifest.namespace
                )));
            }
            artifact.manifest.validate()?;
            validate_archive_name(&artifact.archive)?;
            validate_sha256(&artifact.archive_sha256)?;
            let signature = Signature::from_slice(&decode_hex(&artifact.manifest.signature)?)
                .map_err(|error| {
                    ProtocolError::Invalid(format!("invalid bundle signature: {error}"))
                })?;
            verifying_key
                .verify(&artifact.manifest.signing_payload()?, &signature)
                .map_err(|error| {
                    ProtocolError::Invalid(format!("bundle signature verification failed: {error}"))
                })?;
        }
        let signature = Signature::from_slice(&decode_hex(&self.signature)?)
            .map_err(|error| ProtocolError::Invalid(format!("invalid index signature: {error}")))?;
        verifying_key
            .verify(&self.signing_payload()?, &signature)
            .map_err(|error| {
                ProtocolError::Invalid(format!("index signature verification failed: {error}"))
            })?;
        Ok(())
    }
}

fn validate_archive_name(value: &str) -> Result<(), ProtocolError> {
    if !value.ends_with(".tcap")
        || value.contains('/')
        || value.contains('\\')
        || value.contains("..")
    {
        return Err(ProtocolError::Invalid(format!(
            "invalid capability archive name: {value}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedCapability {
    pub namespace: String,
    pub version: String,
    pub bundle_sha256: String,
    pub executable_sha256: String,
    pub platform: String,
    pub architecture: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityLockfile {
    pub format_version: u32,
    pub capabilities: BTreeMap<String, LockedCapability>,
}

impl Default for CapabilityLockfile {
    fn default() -> Self {
        Self {
            format_version: LOCK_FORMAT_VERSION,
            capabilities: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkerRequest {
    Hello {
        protocol_version: u32,
        namespace: String,
        session_nonce: String,
    },
    Restore {
        snapshot: Vec<u8>,
        last_applied_seq: u64,
        dependencies: BTreeMap<String, Vec<u8>>,
    },
    Replay {
        records: Vec<EventRecord>,
    },
    Fold {
        seq: u64,
        record: EventRecord,
        dependencies: BTreeMap<String, Vec<u8>>,
    },
    Decide {
        request: Request,
        dependencies: BTreeMap<String, Vec<u8>>,
    },
    Query {
        name: String,
        args: Vec<String>,
        dependencies: BTreeMap<String, Vec<u8>>,
    },
    ReadResource {
        app: String,
        name: String,
        args: Vec<String>,
        dependencies: BTreeMap<String, Vec<u8>>,
    },
    ResourceCallOutput {
        app: String,
        method: String,
        records: Vec<EventRecord>,
    },
    RunRuntime {
        app: String,
        source: String,
        source_files: Option<BTreeMap<String, String>>,
        app_name: String,
        input: Vec<String>,
        principal: ExecutionPrincipal,
    },
    ExecuteEffect {
        effect: Effect,
    },
    Snapshot,
    Health,
    BackgroundStatus,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkerResponse {
    Hello {
        protocol_version: u32,
        namespace: String,
        version: String,
        session_nonce: String,
        manifest_sha256: String,
    },
    Ack {
        last_applied_seq: u64,
    },
    Decision {
        decision: Decision,
    },
    QueryValue {
        value: QueryValue,
    },
    ReadValue {
        value: ReadValue,
    },
    RuntimeOutput {
        output: RuntimeOutput,
        records: Vec<EventRecord>,
    },
    EffectRecords {
        records: Vec<EventRecord>,
    },
    Snapshot {
        payload: Vec<u8>,
        last_applied_seq: u64,
        state_sha256: String,
    },
    Health {
        ready: bool,
        detail: String,
    },
    BackgroundStatus {
        keep_alive: bool,
        reason: String,
    },
    Error {
        code: String,
        message: String,
        retryable: bool,
    },
}

#[derive(Debug)]
pub enum ProtocolError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(String),
    FrameTooLarge { size: usize, max: usize },
    Eof,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Json(error) => write!(f, "JSON error: {error}"),
            Self::Invalid(message) => write!(f, "invalid capability protocol data: {message}"),
            Self::FrameTooLarge { size, max } => {
                write!(
                    f,
                    "capability protocol frame is {size} bytes; maximum is {max}"
                )
            }
            Self::Eof => f.write_str("capability worker closed the protocol stream"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<std::io::Error> for ProtocolError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), ProtocolError> {
    write_frame_with_limit(writer, value, DEFAULT_MAX_FRAME_BYTES)
}

pub fn write_frame_with_limit<T: Serialize>(
    writer: &mut impl Write,
    value: &T,
    max: usize,
) -> Result<(), ProtocolError> {
    let payload = serde_json::to_vec(value).map_err(ProtocolError::Json)?;
    if payload.len() > max || payload.len() > u32::MAX as usize {
        return Err(ProtocolError::FrameTooLarge {
            size: payload.len(),
            max,
        });
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> Result<T, ProtocolError> {
    read_frame_with_limit(reader, DEFAULT_MAX_FRAME_BYTES)
}

pub fn read_frame_with_limit<T: DeserializeOwned>(
    reader: &mut impl Read,
    max: usize,
) -> Result<T, ProtocolError> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(ProtocolError::Eof)
        }
        Err(error) => return Err(ProtocolError::Io(error)),
    }
    let size = u32::from_le_bytes(length) as usize;
    if size > max {
        return Err(ProtocolError::FrameTooLarge { size, max });
    }
    let mut payload = vec![0u8; size];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(ProtocolError::Json)
}

pub fn sha256_file(path: &Path) -> Result<String, ProtocolError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

pub fn validate_dependency_graph(
    manifests: &BTreeMap<String, BundleManifest>,
) -> Result<(), ProtocolError> {
    for (namespace, manifest) in manifests {
        manifest.validate()?;
        if namespace != &manifest.namespace {
            return Err(ProtocolError::Invalid(format!(
                "bundle index key {namespace} does not match manifest namespace {}",
                manifest.namespace
            )));
        }
        for dependency in &manifest.dependencies {
            if !manifests.contains_key(dependency) {
                return Err(ProtocolError::Invalid(format!(
                    "capability {namespace} depends on missing capability {dependency}"
                )));
            }
        }
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for namespace in manifests.keys() {
        visit(namespace, manifests, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn visit(
    namespace: &str,
    manifests: &BTreeMap<String, BundleManifest>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) -> Result<(), ProtocolError> {
    if visited.contains(namespace) {
        return Ok(());
    }
    if !visiting.insert(namespace.to_string()) {
        return Err(ProtocolError::Invalid(format!(
            "capability dependency cycle contains {namespace}"
        )));
    }
    for dependency in &manifests[namespace].dependencies {
        visit(dependency, manifests, visiting, visited)?;
    }
    visiting.remove(namespace);
    visited.insert(namespace.to_string());
    Ok(())
}

fn validate_declaration(
    namespace: &str,
    declaration: &CapabilityDeclaration,
) -> Result<(), ProtocolError> {
    let mut owned = BTreeSet::new();
    for (label, names) in [
        ("command", &declaration.commands),
        ("event", &declaration.events),
        ("query", &declaration.queries),
    ] {
        for name in names {
            let Some((owner, _)) = name.split_once('.') else {
                return Err(ProtocolError::Invalid(format!(
                    "{label} declaration must be namespaced: {name}"
                )));
            };
            if owner != namespace {
                return Err(ProtocolError::Invalid(format!(
                    "{label} {name} does not belong to {namespace}"
                )));
            }
            if !owned.insert((label, name)) {
                return Err(ProtocolError::Invalid(format!(
                    "duplicate {label} declaration: {name}"
                )));
            }
        }
    }
    for method in &declaration.resources {
        validate_token("resource method", &method.name)?;
        if !matches!(method.kind.as_str(), "read" | "write" | "call") {
            return Err(ProtocolError::Invalid(format!(
                "resource method {} has invalid kind {}",
                method.name, method.kind
            )));
        }
    }
    for grant in &declaration.grant_resources {
        if grant.namespace != namespace {
            return Err(ProtocolError::Invalid(format!(
                "grant resource {} is owned by {}, expected {namespace}",
                grant.selector_schema_id, grant.namespace
            )));
        }
        validate_token("selector schema id", &grant.selector_schema_id)?;
        if grant.verbs.is_empty() {
            return Err(ProtocolError::Invalid(format!(
                "grant resource {} has no verbs",
                grant.selector_schema_id
            )));
        }
    }
    for event in &declaration.delegated_events {
        let Some((owner, _)) = event.split_once('.') else {
            return Err(ProtocolError::Invalid(format!(
                "delegated event must be namespaced: {event}"
            )));
        };
        if owner == namespace {
            return Err(ProtocolError::Invalid(format!(
                "delegated event {event} is already owned by {namespace}"
            )));
        }
    }
    Ok(())
}

fn validate_token(label: &str, value: &str) -> Result<(), ProtocolError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(ProtocolError::Invalid(format!(
            "{label} is unsafe: {value:?}"
        )));
    }
    Ok(())
}

fn validate_executable_name(value: &str) -> Result<(), ProtocolError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || matches!(value, "." | "..")
    {
        return Err(ProtocolError::Invalid(format!(
            "unsafe capability executable name: {value:?}"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), ProtocolError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProtocolError::Invalid(format!(
            "invalid SHA-256 digest: {value:?}"
        )));
    }
    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ProtocolError> {
    if !value.len().is_multiple_of(2) {
        return Err(ProtocolError::Invalid("hex value has odd length".into()));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> Result<u8, ProtocolError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(ProtocolError::Invalid("invalid hex digit".into())),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::Cursor;

    fn manifest(namespace: &str) -> BundleManifest {
        BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            namespace: namespace.into(),
            version: "0.1.0".into(),
            protocol_version: PROTOCOL_VERSION,
            state_schema_version: 1,
            platform: "macos".into(),
            architecture: "aarch64".into(),
            executable: "worker".into(),
            executable_sha256: "0".repeat(64),
            signature: String::new(),
            dependencies: Vec::new(),
            activation: ActivationMode::Demand,
            declaration: CapabilityDeclaration {
                commands: vec![format!("{namespace}.run")],
                events: vec![format!("{namespace}.ran")],
                ..CapabilityDeclaration::default()
            },
        }
    }

    #[test]
    fn frames_round_trip_and_enforce_limit() {
        let request = WorkerRequest::Health;
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).unwrap();
        let decoded: WorkerRequest = read_frame(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(decoded, request);

        let error = write_frame_with_limit(&mut Vec::new(), &request, 1).unwrap_err();
        assert!(matches!(error, ProtocolError::FrameTooLarge { .. }));
    }

    #[test]
    fn declaration_must_match_namespace() {
        let mut value = manifest("time");
        value.declaration.commands[0] = "net.run".into();
        assert!(value
            .validate()
            .unwrap_err()
            .to_string()
            .contains("does not belong"));
    }

    #[test]
    fn dependency_cycles_are_rejected() {
        let mut left = manifest("left");
        left.dependencies.push("right".into());
        let mut right = manifest("right");
        right.dependencies.push("left".into());
        let manifests = BTreeMap::from([("left".into(), left), ("right".into(), right)]);
        assert!(validate_dependency_graph(&manifests)
            .unwrap_err()
            .to_string()
            .contains("cycle"));
    }

    #[test]
    fn executable_and_manifest_signature_are_verified() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("worker");
        std::fs::write(&executable, b"worker bytes").unwrap();
        let mut value = manifest("time");
        value.executable_sha256 = sha256_file(&executable).unwrap();
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        value.signature = hex(&signing.sign(&value.signing_payload().unwrap()).to_bytes());
        assert_eq!(
            value
                .verify_bundle(dir.path(), &signing.verifying_key())
                .unwrap(),
            executable
        );
    }
}
