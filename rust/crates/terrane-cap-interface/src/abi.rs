use borsh::{BorshDeserialize, BorshSerialize};

/// Identifier for a saved app. Caller-supplied and stable.
pub type AppId = String;

pub const LOCAL_ORG: &str = "local";
pub const LOCAL_OWNER_SUBJECT: &str = "user:local-owner";
pub const LOCAL_SOURCE: &str = "local";

/// The authority under which a live runtime request executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPrincipal {
    pub org: String,
    pub subject: String,
    pub source: String,
}

impl ExecutionPrincipal {
    pub fn local_owner() -> Self {
        Self {
            org: LOCAL_ORG.to_string(),
            subject: LOCAL_OWNER_SUBJECT.to_string(),
            source: LOCAL_SOURCE.to_string(),
        }
    }

    pub fn app_caller(app: impl Into<String>) -> Self {
        Self {
            org: LOCAL_ORG.to_string(),
            subject: format!("app:{}", app.into()),
            source: "interop".to_string(),
        }
    }

    pub fn actor(&self) -> String {
        self.subject.clone()
    }
}

impl Default for ExecutionPrincipal {
    fn default() -> Self {
        Self::local_owner()
    }
}

/// Whether a host/control-plane surface admitted a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandAuthority {
    #[default]
    Public,
    TrustedHost,
}

impl CommandAuthority {
    pub fn is_trusted_host(self) -> bool {
        matches!(self, Self::TrustedHost)
    }
}

/// A command as it arrives at the core: a namespaced name like `"app.add"` plus
/// the caller's argument tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub name: String,
    pub args: Vec<String>,
    pub principal: ExecutionPrincipal,
    pub authority: CommandAuthority,
}

impl Request {
    pub fn new(name: impl Into<String>, args: Vec<String>) -> Self {
        Request {
            name: name.into(),
            args,
            principal: ExecutionPrincipal::local_owner(),
            authority: CommandAuthority::Public,
        }
    }

    pub fn trusted_host(name: impl Into<String>, args: Vec<String>) -> Self {
        Request::new(name, args).with_trusted_host()
    }

    pub fn with_principal(mut self, principal: ExecutionPrincipal) -> Self {
        self.principal = principal;
        self
    }

    pub fn with_trusted_host(mut self) -> Self {
        self.authority = CommandAuthority::TrustedHost;
        self
    }
}

/// A recorded event on the wire.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EventRecord {
    pub kind: String,
    pub payload: Vec<u8>,
    pub actor: String,
}

/// Typed errors. No panics on real paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    AppExists(AppId),
    AppNotFound(AppId),
    KeyNotFound(AppId, String),
    InvalidInput(String),
    Storage(String),
    Runtime(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AppExists(id) => write!(f, "app already exists: {id}"),
            Error::AppNotFound(id) => write!(f, "app not found: {id}"),
            Error::KeyNotFound(app, key) => write!(f, "key not found: {app}/{key}"),
            Error::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            Error::Storage(msg) => write!(f, "storage error: {msg}"),
            Error::Runtime(msg) => write!(f, "runtime error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Convenience alias used throughout the engine and capability crates.
pub type Result<T> = std::result::Result<T, Error>;

/// What a command resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Commit(Vec<EventRecord>),
    Effect(Effect),
    /// Like [`Decision::Effect`], but the runner's result is returned to the
    /// caller and NEVER recorded — a live, unrecorded query whose response must
    /// not enter the replayed event log (e.g. a privacy-preserving breach check
    /// whose HIBP bucket, and the SHA-1 prefix that fetched it, must never be
    /// persisted). Only valid from a `ResourceMethod::Call`; not replay-stable.
    TransientEffect(Effect),
    Runtime(RuntimeRequest),
}

/// A request for a runtime capability to execute an app backend once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequest {
    pub app: String,
    pub input: Vec<String>,
}

/// The non-record output of one runtime execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOutput {
    pub output: String,
}

/// A side effect the engine must perform in the outside world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    HttpGet {
        app: String,
        url: String,
    },
    HttpRequest {
        app: String,
        request: String,
    },
    BrowserRender {
        app: String,
        request: String,
    },
    ModelCall {
        app: String,
        agent: String,
        prompt: String,
    },
    GenerateAppWithHarness {
        draft_id: String,
        app_id: String,
        name: String,
        harness: String,
        prompt: String,
    },
    RunHarnessJs {
        run_id: String,
        app_id: String,
        harness: String,
        prompt: String,
    },
    ImportAppBundle {
        source: String,
        storage_backend: Option<String>,
        storage_path: Option<String>,
    },
    BlobStore {
        app: String,
        name: String,
        mime: String,
        hash: String,
        bytes: Vec<u8>,
    },
    MediaTransform {
        app: String,
        source_hash: String,
        source_mime: String,
        ops_json: String,
        dest_name: String,
    },
    NewReplicaId,
    LocalModelCall {
        app: String,
        model: String,
        prompt: String,
        system: Option<String>,
        /// Prior (user, assistant) exchanges to continue from, oldest first.
        history: Vec<(String, String)>,
        schema: Option<String>,
        grammar: Option<String>,
    },
    LocalModelPull {
        id: String,
        repo: String,
        backend: String,
        /// The file inside the repo for gguf pulls; mlx pulls snapshot the repo.
        file: Option<String>,
        context_length: Option<u32>,
        chat_template: Option<String>,
        max_tokens: Option<u32>,
        temperature_milli: Option<u32>,
        draft_model: Option<String>,
        /// A recognized embedding-preset name (e.g. `"nomic"`) when pulling an
        /// embedding model; the runner resolves it into the recorded config.
        embed_preset: Option<String>,
    },
    /// Encode one or more texts into dense vectors with a registered embedding
    /// model. The pooled, (optionally) L2-normalized vectors are recorded as the
    /// effect result; replay never re-runs inference.
    LocalModelEmbed {
        app: String,
        model: String,
        texts: Vec<String>,
        /// Apply the model's query prefix (search side) rather than the document
        /// prefix (index side) — asymmetric encoders need the distinction.
        query: bool,
    },
    /// Read the host wall clock once and record the observation as a
    /// `time.observed` event. The edge performs the read; replay folds the
    /// recorded fact and never consults a clock.
    ObserveTime {
        app: String,
    },
    /// Append one structured log line to the per-app ring buffer
    /// (`$TERRANE_HOME/logs/<app>/current.jsonl`). Transient when emitted as a
    /// `debug`/`info`/`warn` write — recorded NOTHING — and recorded (an error
    /// line) when emitted as an `error` call, where the edge also returns the
    /// `telemetry.error` event so crash facts fold into State on replay. The
    /// core never opens the buffer; only the host edge does, like `blobs.sqlite3`.
    AppLog {
        app: String,
        level: String,
        msg: String,
        data: String,
    },
    AppCall {
        chain: Vec<String>,
        target: String,
        verb: String,
        args: Vec<String>,
    },
    McpCall {
        app: String,
        connection: String,
        tool: String,
        args: String,
        args_redacted: String,
        timeout_ms: u64,
    },
    McpTools {
        app: String,
        connection: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemUri {
    pub app: String,
    pub item: String,
}

pub fn format_item_uri(app: &str, item: &str) -> String {
    format!("terrane://app/{}/item/{}", app, percent_encode(item))
}

pub fn parse_item_uri(uri: &str) -> Result<ItemUri> {
    let rest = uri
        .strip_prefix("terrane://app/")
        .ok_or_else(|| Error::InvalidInput(format!("not a Terrane app item URI: {uri}")))?;
    let Some((app, encoded_item)) = rest.split_once("/item/") else {
        return Err(Error::InvalidInput(format!(
            "item URI must be terrane://app/<appId>/item/<itemId>: {uri}"
        )));
    };
    if app.is_empty() {
        return Err(Error::InvalidInput("item URI app id must not be empty".into()));
    }
    Ok(ItemUri {
        app: app.to_string(),
        item: percent_decode(encoded_item)?,
    })
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(hex((b >> 4) & 0x0f));
            out.push(hex(b & 0x0f));
        }
    }
    out
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hi = *bytes.get(i + 1).ok_or_else(|| {
                    Error::InvalidInput(format!("bad percent escape in item URI: {value}"))
                })?;
                let lo = *bytes.get(i + 2).ok_or_else(|| {
                    Error::InvalidInput(format!("bad percent escape in item URI: {value}"))
                })?;
                out.push((from_hex(hi)? << 4) | from_hex(lo)?);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out)
        .map_err(|e| Error::InvalidInput(format!("item URI is not valid UTF-8: {e}")))
}

fn hex(n: u8) -> char {
    match n {
        0..=9 => char::from(b'0' + n),
        _ => char::from(b'A' + (n - 10)),
    }
}

fn from_hex(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(10 + b - b'a'),
        b'A'..=b'F' => Ok(10 + b - b'A'),
        _ => Err(Error::InvalidInput(format!(
            "bad hex digit in item URI percent escape: {}",
            char::from(b)
        ))),
    }
}

/// Encode a capability's typed event into a name-tagged [`EventRecord`].
pub fn encode_event<E: BorshSerialize>(kind: &str, event: &E) -> Result<EventRecord> {
    let payload = borsh::to_vec(event).map_err(|e| Error::Storage(e.to_string()))?;
    Ok(EventRecord {
        kind: kind.to_string(),
        payload,
        actor: String::new(),
    })
}

/// Decode an [`EventRecord`]'s payload back into a capability's typed event.
pub fn decode_event<E: BorshDeserialize>(record: &EventRecord) -> Result<E> {
    borsh::from_slice::<E>(&record.payload)
        .map_err(|e| Error::Storage(format!("corrupt {} payload: {e}", record.kind)))
}

/// The namespace of a dotted name (`"app.add"` -> `"app"`).
pub fn namespace_of(name: &str) -> Result<&str> {
    name.split_once('.').map(|(ns, _)| ns).ok_or_else(|| {
        Error::InvalidInput(format!("command name must be 'namespace.verb': {name}"))
    })
}
