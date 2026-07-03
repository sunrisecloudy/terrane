//! A process-global, in-RAM session keyring — the vault equivalent of an
//! ssh-agent. Derived keys live here and *only* here: never in State, never in
//! an event, never on disk. A fresh [`RuntimeResourceHost`] (hence a fresh
//! capability instance) is built per backend run, so the keyring cannot hang off
//! the capability struct; it is a `static` that outlives individual runs and is
//! shared across every app-backend invocation in the host process.
//!
//! Consequences by host:
//! - Long-lived hosts (MCP server, web server): a session unlocked on one invoke
//!   is usable on the next, until it is locked or idles out.
//! - The CLI (one process per command): the keyring starts empty each command,
//!   so a CLI flow unlocks and uses the vault within a single backend run.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::primitives::{random_bytes, VaultKey, KEY_LEN};

/// How long an idle session stays unlocked before it auto-locks.
pub const DEFAULT_TTL: Duration = Duration::from_secs(15 * 60);

struct Session {
    /// The app id that unlocked this session. A session is only usable by the
    /// same app, so one app cannot open another app's vault by guessing an id.
    app: String,
    key: VaultKey,
    last_used: Instant,
    ttl: Duration,
}

impl Session {
    fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.last_used) > self.ttl
    }
}

static KEYRING: LazyLock<Mutex<HashMap<String, Session>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn store() -> std::sync::MutexGuard<'static, HashMap<String, Session>> {
    // Poisoning only means a prior holder panicked; the map itself is still
    // consistent, so recover the guard rather than propagate the panic.
    KEYRING.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn new_session_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    // The session id is a 128-bit bearer token for the unlocked vault key, so it
    // MUST be unpredictable. If the CSPRNG is unavailable we fail closed (return
    // None) rather than mint a guessable id — a time-seeded fallback would hand
    // out an ~all-zero, trivially guessable token.
    random_bytes(&mut bytes).ok()?;
    let mut out = String::with_capacity(32);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    Some(out)
}

/// Store a freshly derived key and return its opaque session id, bound to `app`.
/// Returns `None` if secure randomness is unavailable (the caller must surface
/// an error and NOT treat the vault as unlocked).
pub fn unlock(app: &str, key: VaultKey) -> Option<String> {
    let id = new_session_id()?;
    let mut guard = store();
    guard.insert(
        id.clone(),
        Session {
            app: app.to_string(),
            key,
            last_used: Instant::now(),
            ttl: DEFAULT_TTL,
        },
    );
    Some(id)
}

/// Run `f` with the unlocked key for `(app, session)`, refreshing its idle timer.
/// Returns `None` if the session is unknown, expired, or belongs to another app —
/// all indistinguishable to the caller, so a wrong id leaks nothing.
pub fn with_key<T>(app: &str, session: &str, f: impl FnOnce(&[u8; KEY_LEN]) -> T) -> Option<T> {
    let mut guard = store();
    let now = Instant::now();
    let ok = match guard.get(session) {
        Some(s) if s.app == app && !s.expired(now) => true,
        Some(s) if s.expired(now) => {
            guard.remove(session);
            false
        }
        _ => false,
    };
    if !ok {
        return None;
    }
    let s = guard.get_mut(session)?;
    s.last_used = now;
    Some(f(&s.key))
}

/// True if `(app, session)` is currently unlocked (and refreshes it if so).
pub fn is_unlocked(app: &str, session: &str) -> bool {
    with_key(app, session, |_| ()).is_some()
}

/// Forget a session, wiping its key. Idempotent.
pub fn lock(session: &str) {
    store().remove(session);
}
