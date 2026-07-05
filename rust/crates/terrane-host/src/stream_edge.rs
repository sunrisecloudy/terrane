use std::path::Path;

use serde_json::json;
use terrane_cap_stream::{sha256_hex, StreamStatus, INLINE_TEXT_LIMIT, MAX_MESSAGE_SIZE};

use crate::{CommandOutcome, HostCore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveredStreamMessage {
    pub records: Vec<crate::EventRecord>,
    pub backend_output: Option<String>,
}

pub fn deliver_text(
    home: &Path,
    core: &mut HostCore,
    app: &str,
    name: &str,
    text: &str,
    received_at: &str,
) -> Result<DeliveredStreamMessage, String> {
    deliver_bytes(
        home,
        core,
        app,
        name,
        text.as_bytes(),
        false,
        received_at,
    )
}

pub fn deliver_bytes(
    home: &Path,
    core: &mut HostCore,
    app: &str,
    name: &str,
    bytes: &[u8],
    force_blob: bool,
    received_at: &str,
) -> Result<DeliveredStreamMessage, String> {
    let stream = core
        .state()
        .stream
        .streams
        .get(app)
        .and_then(|streams| streams.get(name))
        .cloned()
        .ok_or_else(|| format!("unknown stream: {app}/{name}"))?;
    if stream.status != StreamStatus::Open {
        return Err(format!("stream {app}/{name} is closed; cannot ingest"));
    }
    let size = u64::try_from(bytes.len()).map_err(|_| "stream message too large".to_string())?;
    if size > MAX_MESSAGE_SIZE {
        let args = vec![
            app.to_string(),
            name.to_string(),
            "message-too-large".to_string(),
        ];
        let outcome = crate::dispatch_on_core(core, "stream.close-host", &args)?;
        return Ok(DeliveredStreamMessage {
            records: outcome.records,
            backend_output: None,
        });
    }

    let seq = stream.last_seq.saturating_add(1);
    let hash = sha256_hex(bytes);
    let (data_kind, data, linked) = if force_blob || bytes.len() > INLINE_TEXT_LIMIT {
        let blob_name = format!("__stream__/{app}/{name}/{seq}");
        crate::blob_store::insert_if_absent(home, &hash, bytes).map_err(|e| e.to_string())?;
        let link_args = vec![
            app.to_string(),
            blob_name.clone(),
            hash.clone(),
            size.to_string(),
            "application/octet-stream".to_string(),
        ];
        let link = crate::dispatch_on_core(core, "blob.link", &link_args)?;
        ("blob".to_string(), blob_name, link.records)
    } else {
        let data = std::str::from_utf8(bytes)
            .map_err(|_| "inline stream messages must be valid UTF-8 text".to_string())?
            .to_string();
        ("inline".to_string(), data, Vec::new())
    };

    let message_args = vec![
        app.to_string(),
        name.to_string(),
        seq.to_string(),
        data_kind.clone(),
        data.clone(),
        "false".to_string(),
        hash.clone(),
        size.to_string(),
        received_at.to_string(),
    ];
    let message = crate::dispatch_on_core(core, "stream.message", &message_args)?;
    let envelope = json!({
        "app": app,
        "name": name,
        "seq": seq,
        "dataKind": data_kind,
        "data": data,
        "dataIsBase64": false,
        "dataHash": hash,
        "dataSize": size,
        "receivedAt": received_at,
    })
    .to_string();
    let output = crate::invoke_app(core, app, &stream.verb, &[envelope])?;

    let mut records = linked;
    records.extend(message.records);
    Ok(DeliveredStreamMessage {
        records,
        backend_output: Some(output),
    })
}

pub fn reopen_on_core(
    core: &mut HostCore,
    app: &str,
    name: &str,
    attempt: u64,
) -> Result<CommandOutcome, String> {
    let seq_before = core
        .state()
        .stream
        .streams
        .get(app)
        .and_then(|streams| streams.get(name))
        .map(|stream| stream.last_seq)
        .ok_or_else(|| format!("unknown stream: {app}/{name}"))?;
    let args = vec![
        app.to_string(),
        name.to_string(),
        seq_before.to_string(),
        attempt.to_string(),
    ];
    crate::dispatch_on_core(core, "stream.reopened", &args)
}
