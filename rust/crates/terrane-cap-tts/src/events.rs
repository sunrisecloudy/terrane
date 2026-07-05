use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, truncate, EventRecord, Result,
    StateStore,
};

use crate::types::{RenderRecord, TtsState, MAX_RENDERS_PER_APP};

#[derive(BorshSerialize, BorshDeserialize)]
struct Rendered {
    app: String,
    text_hash: String,
    voice: Option<String>,
    rate_milli: u32,
    blob_hash: String,
    size: u64,
    mime: String,
    duration_ms: u64,
}

pub fn rendered_event(record: &RenderRecord) -> Result<EventRecord> {
    encode_event(
        "tts.rendered",
        &Rendered {
            app: record.app.clone(),
            text_hash: record.text_hash.clone(),
            voice: record.voice.clone(),
            rate_milli: record.rate_milli,
            blob_hash: record.blob_hash.clone(),
            size: record.size,
            mime: record.mime.clone(),
            duration_ms: record.duration_ms,
        },
    )
}

pub(crate) fn render_from_records(records: &[EventRecord]) -> Result<Option<RenderRecord>> {
    for record in records.iter().rev() {
        if record.kind == "tts.rendered" {
            let rendered: Rendered = decode_event(record)?;
            return Ok(Some(RenderRecord {
                app: rendered.app,
                text_hash: rendered.text_hash,
                voice: rendered.voice,
                rate_milli: rendered.rate_milli,
                blob_hash: rendered.blob_hash,
                size: rendered.size,
                mime: rendered.mime,
                duration_ms: rendered.duration_ms,
            }));
        }
    }
    Ok(None)
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "tts.rendered" => {
            let e: Rendered = decode_event(record)?;
            let app = e.app.clone();
            let state = state_mut::<TtsState>(state, "tts")?;
            let renders = state
                .renders
                .entry(app)
                .or_default();
            let order = state.order.entry(e.app.clone()).or_default();
            order.retain(|hash| hash != &e.text_hash);
            order.push(e.text_hash.clone());
            renders.insert(
                e.text_hash.clone(),
                RenderRecord {
                    app: e.app,
                    text_hash: e.text_hash,
                    voice: e.voice,
                    rate_milli: e.rate_milli,
                    blob_hash: e.blob_hash,
                    size: e.size,
                    mime: e.mime,
                    duration_ms: e.duration_ms,
                },
            );
            while order.len() > MAX_RENDERS_PER_APP {
                if order.is_empty() {
                    break;
                }
                let key = order.remove(0);
                renders.remove(&key);
            }
        }
        "app.removed" => {
            let e = decode_app_removed(record)?;
            let state = state_mut::<TtsState>(state, "tts")?;
            state.renders.remove(&e.id);
            state.order.remove(&e.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    if record.kind != "tts.rendered" {
        return None;
    }
    let e: Rendered = decode_event(record).ok()?;
    let prefix = truncate(&e.text_hash, 12);
    let voice = e.voice.unwrap_or_else(|| "default".to_string());
    Some(format!(
        "tts.rendered {}/{} voice={} rate={} duration_ms={} blob={}",
        e.app, prefix, voice, e.rate_milli, e.duration_ms, e.blob_hash
    ))
}
