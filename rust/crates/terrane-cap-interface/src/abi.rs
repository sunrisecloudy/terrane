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
    NewReplicaId,
}

/// Encode a capability's typed event into a name-tagged [`EventRecord`].
pub fn encode_event<E: BorshSerialize>(kind: &str, event: &E) -> Result<EventRecord> {
    let payload = borsh::to_vec(event).map_err(|e| Error::Storage(e.to_string()))?;
    Ok(EventRecord {
        kind: kind.to_string(),
        payload,
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
