use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    decode_app_removed, decode_event, state_mut, EventRecord, Result, StateStore,
};

use crate::{KvState, KvStorageBackend, KvStorageBinding};

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Set {
    pub app: String,
    pub key: String,
    pub value: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Deleted {
    pub app: String,
    pub key: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct StorageConfigured {
    pub app: Option<String>,
    pub backend: KvStorageBackend,
    pub path: Option<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct StorageCleared {
    pub app: Option<String>,
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "kv.set" => {
            let e: Set = decode_event(record)?;
            state_mut::<KvState>(state, "kv")?
                .data
                .entry(e.app)
                .or_default()
                .insert(e.key, e.value);
        }
        "kv.deleted" => {
            let e: Deleted = decode_event(record)?;
            let state = state_mut::<KvState>(state, "kv")?;
            if let Some(kv) = state.data.get_mut(&e.app) {
                kv.remove(&e.key);
                if kv.is_empty() {
                    state.data.remove(&e.app);
                }
            }
        }
        "app.removed" => {
            let e = decode_app_removed(record)?;
            let state = state_mut::<KvState>(state, "kv")?;
            state.data.remove(&e.id);
            state.storage.apps.remove(&e.id);
        }
        "kv.storage.configured" => {
            let e: StorageConfigured = decode_event(record)?;
            let binding = KvStorageBinding::new(e.backend, e.path)?;
            let state = state_mut::<KvState>(state, "kv")?;
            match e.app {
                Some(app) => {
                    state.storage.apps.insert(app, binding);
                }
                None => {
                    state.storage.default = binding;
                }
            }
        }
        "kv.storage.cleared" => {
            let e: StorageCleared = decode_event(record)?;
            let state = state_mut::<KvState>(state, "kv")?;
            match e.app {
                Some(app) => {
                    state.storage.apps.remove(&app);
                }
                None => {
                    state.storage.default = KvStorageBinding::default();
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "kv.set" => {
            let e: Set = decode_event(record).ok()?;
            Some(format!("kv.set {}/{} = {}", e.app, e.key, e.value))
        }
        "kv.deleted" => {
            let e: Deleted = decode_event(record).ok()?;
            Some(format!("kv.deleted {}/{}", e.app, e.key))
        }
        "kv.storage.configured" => {
            let e: StorageConfigured = decode_event(record).ok()?;
            let binding = KvStorageBinding::new(e.backend, e.path).ok()?;
            Some(match e.app {
                Some(app) => format!("kv.storage {app} -> {}", binding.describe()),
                None => format!("kv.storage default -> {}", binding.describe()),
            })
        }
        "kv.storage.cleared" => {
            let e: StorageCleared = decode_event(record).ok()?;
            Some(match e.app {
                Some(app) => format!("kv.storage {app} cleared"),
                None => "kv.storage default cleared".to_string(),
            })
        }
        _ => None,
    }
}
