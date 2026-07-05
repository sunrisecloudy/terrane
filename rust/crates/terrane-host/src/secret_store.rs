//! Host-edge storage and resolution for connection secret fields.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde_json::Value;
use terrane_cap_connection::{split_secret_ref, validate_secret_len};
use terrane_cap_crypto::{b64, open, seal, unb64, KEY_LEN};
use terrane_core::{Error, ExecutionPrincipal, Result, State, LOCAL_OWNER_SUBJECT};

const SERVICE: &str = "terrane";
const FALLBACK_FILE: &str = "secrets.enc";
const FALLBACK_KEY_FILE: &str = "secrets.key";

pub fn set_secret(home: &Path, name: &str, field: &str, value: &str) -> Result<()> {
    validate_secret_len(value)?;
    if !force_file_store() && try_keyring_set(home, name, field, value).is_ok() {
        return Ok(());
    }
    fallback_set(home, name, field, value)
}

pub fn get_secret(home: &Path, name: &str, field: &str) -> Result<String> {
    if force_file_store() {
        return fallback_get(home, name, field);
    }
    match try_keyring_get(home, name, field) {
        Ok(value) => Ok(value),
        Err(_) => fallback_get(home, name, field),
    }
}

fn force_file_store() -> bool {
    std::env::var("TERRANE_SECRET_STORE")
        .map(|value| value == "file")
        .unwrap_or(false)
}

pub fn remove_connection(home: &Path, name: &str) -> Result<()> {
    let _ = try_keyring_delete(home, name, "key");
    let mut fallback = read_fallback(home)?;
    let prefix = format!("{name}.");
    fallback.retain(|key, _| !key.starts_with(&prefix));
    write_fallback(home, &fallback)
}

pub fn resolve_net_request(home: &Path, state: &State, app: &str, request: &str) -> Result<String> {
    let mut value: Value = serde_json::from_str(request)
        .map_err(|e| Error::InvalidInput(format!("net request must be JSON: {e}")))?;
    resolve_json(home, state, app, &mut value)?;
    serde_json::to_string(&value)
        .map_err(|e| Error::InvalidInput(format!("serialize resolved request: {e}")))
}

fn resolve_json(home: &Path, state: &State, app: &str, value: &mut Value) -> Result<()> {
    match value {
        Value::Array(items) => {
            for item in items {
                resolve_json(home, state, app, item)?;
            }
        }
        Value::Object(obj) => {
            if obj.len() == 1 {
                if let Some(secret) = obj.get("$secret") {
                    let reference = secret.as_str().ok_or_else(|| {
                        Error::InvalidInput("$secret value must be a string".into())
                    })?;
                    let (name, field) = split_secret_ref(reference)?;
                    ensure_connection_grant(state, app, &name)?;
                    *value = Value::String(get_secret(home, &name, &field)?);
                    return Ok(());
                }
            }
            for item in obj.values_mut() {
                resolve_json(home, state, app, item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn ensure_connection_grant(state: &State, app: &str, name: &str) -> Result<()> {
    let resource_id = terrane_cap_connection::connection_resource_id(name)?;
    let principal = ExecutionPrincipal::local_owner();
    if terrane_cap_auth::resource_granted(state, &principal, app, &resource_id)? {
        return Ok(());
    }
    Err(Error::InvalidInput(format!(
        "permission required: grant {resource_id} to {app} for {LOCAL_OWNER_SUBJECT}"
    )))
}

fn home_id(home: &Path) -> String {
    home.canonicalize()
        .unwrap_or_else(|_| home.to_path_buf())
        .to_string_lossy()
        .replace('/', "_")
}

fn account(home: &Path, name: &str, field: &str) -> String {
    format!("{}/{}/{}", home_id(home), name, field)
}

fn try_keyring_set(home: &Path, name: &str, field: &str, value: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, &account(home, name, field))
        .map_err(|e| Error::Storage(format!("open keychain entry: {e}")))?;
    entry
        .set_password(value)
        .map_err(|e| Error::Storage(format!("write keychain entry: {e}")))
}

fn try_keyring_get(home: &Path, name: &str, field: &str) -> Result<String> {
    let entry = keyring::Entry::new(SERVICE, &account(home, name, field))
        .map_err(|e| Error::Storage(format!("open keychain entry: {e}")))?;
    entry
        .get_password()
        .map_err(|e| Error::Storage(format!("read keychain entry: {e}")))
}

fn try_keyring_delete(home: &Path, name: &str, field: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, &account(home, name, field))
        .map_err(|e| Error::Storage(format!("open keychain entry: {e}")))?;
    entry
        .delete_credential()
        .map_err(|e| Error::Storage(format!("delete keychain entry: {e}")))
}

fn fallback_path(home: &Path) -> PathBuf {
    home.join(FALLBACK_FILE)
}

fn fallback_key_path(home: &Path) -> PathBuf {
    home.join(FALLBACK_KEY_FILE)
}

fn fallback_set(home: &Path, name: &str, field: &str, value: &str) -> Result<()> {
    fs::create_dir_all(home).map_err(|e| Error::Storage(format!("create home: {e}")))?;
    let key = fallback_key(home)?;
    let mut map = read_fallback(home)?;
    let blob = seal(&key, value.as_bytes()).map_err(|e| Error::Storage(e.to_string()))?;
    map.insert(format!("{name}.{field}"), b64(&blob));
    write_fallback(home, &map)
}

fn fallback_get(home: &Path, name: &str, field: &str) -> Result<String> {
    let key = fallback_key(home)?;
    let map = read_fallback(home)?;
    let blob = map
        .get(&format!("{name}.{field}"))
        .ok_or_else(|| Error::Storage(format!("secret not found: {name}.{field}")))?;
    let bytes = unb64(blob).map_err(|e| Error::Storage(e.to_string()))?;
    let plain = open(&key, &bytes).map_err(|e| Error::Storage(e.to_string()))?;
    String::from_utf8(plain).map_err(|e| Error::Storage(format!("secret is not UTF-8: {e}")))
}

fn fallback_key(home: &Path) -> Result<[u8; KEY_LEN]> {
    let path = fallback_key_path(home);
    if path.exists() {
        let text = fs::read_to_string(&path)
            .map_err(|e| Error::Storage(format!("read fallback key {}: {e}", path.display())))?;
        let bytes = unb64(&text).map_err(|e| Error::Storage(e.to_string()))?;
        return key_from_bytes(&bytes);
    }
    fs::create_dir_all(home).map_err(|e| Error::Storage(format!("create home: {e}")))?;
    let mut key = [0u8; KEY_LEN];
    getrandom::fill(&mut key)
        .map_err(|e| Error::Storage(format!("fallback key randomness unavailable: {e}")))?;
    write_secret_file(&path, b64(&key).as_bytes())?;
    Ok(key)
}

fn key_from_bytes(bytes: &[u8]) -> Result<[u8; KEY_LEN]> {
    if bytes.len() != KEY_LEN {
        return Err(Error::Storage("fallback key has invalid length".into()));
    }
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(bytes);
    Ok(key)
}

fn read_fallback(home: &Path) -> Result<BTreeMap<String, String>> {
    let path = fallback_path(home);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let mut text = String::new();
    fs::File::open(&path)
        .map_err(|e| Error::Storage(format!("open fallback store {}: {e}", path.display())))?
        .read_to_string(&mut text)
        .map_err(|e| Error::Storage(format!("read fallback store {}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| Error::Storage(format!("parse fallback store {}: {e}", path.display())))
}

fn write_fallback(home: &Path, map: &BTreeMap<String, String>) -> Result<()> {
    fs::create_dir_all(home).map_err(|e| Error::Storage(format!("create home: {e}")))?;
    let text = serde_json::to_vec(map)
        .map_err(|e| Error::Storage(format!("serialize fallback store: {e}")))?;
    write_secret_file(&fallback_path(home), &text)
}

fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut opts = OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .map_err(|e| Error::Storage(format!("write {}: {e}", path.display())))?;
    file.write_all(bytes)
        .map_err(|e| Error::Storage(format!("write {}: {e}", path.display())))
}
