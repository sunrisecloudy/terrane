use terrane_cap_interface::{
    decode_app_removed, decode_event, state_mut, EventRecord, Result, StateStore,
};

use crate::state::{CrdtState, Update};

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "crdt.update" => {
            let e: Update = decode_event(record)?;
            let doc = state_mut::<CrdtState>(state, "crdt")?
                .docs
                .entry(e.app.clone())
                .or_default();
            doc.import(&e.bytes).map_err(|err| {
                terrane_cap_interface::Error::Storage(format!("crdt import: {err}"))
            })?;
        }
        "app.removed" => {
            let e = decode_app_removed(record)?;
            state_mut::<CrdtState>(state, "crdt")?.docs.remove(&e.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "crdt.update" => {
            let e: Update = decode_event(record).ok()?;
            Some(format!("crdt.update {} ({} bytes)", e.app, e.bytes.len()))
        }
        _ => None,
    }
}
