use terrane_cap_interface::{
    state_ref, Error, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

use crate::types::TtsState;

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Call {
            name: "speak",
            params: &["text", "options"],
        },
        ResourceMethod::Call {
            name: "render",
            params: &["text", "options"],
        },
        ResourceMethod::Read {
            name: "voices",
            params: &[],
        },
        ResourceMethod::Read {
            name: "renders",
            params: &[],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "voices" => read_voices(ctx),
        "renders" => read_renders(ctx.state, ctx.app),
        other => {
            let _ = args;
            Err(Error::InvalidInput(format!(
                "unknown resource read: tts.{other}"
            )))
        }
    }
}

fn read_voices(ctx: ResourceReadCtx<'_>) -> Result<ReadValue> {
    let Some(host) = ctx.host else {
        return Err(Error::Runtime(
            "tts.voices needs a host edge synthesizer".into(),
        ));
    };
    Ok(ReadValue::OptString(Some(host.sample("tts.voices", &[])?)))
}

fn read_renders(state: &dyn StateStore, app: &str) -> Result<ReadValue> {
    let tts = state_ref::<TtsState>(state, "tts")?;
    let encoded = serde_json::to_string(
        &tts.renders
            .get(app)
            .map(|m| m.values().collect::<Vec<_>>())
            .unwrap_or_default()
            .iter()
            .map(|render| {
                serde_json::json!({
                    "textHash": render.text_hash,
                    "voice": render.voice,
                    "rateMilli": render.rate_milli,
                    "blobHash": render.blob_hash,
                    "size": render.size,
                    "mime": render.mime,
                    "durationMs": render.duration_ms,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|e| Error::InvalidInput(format!("tts renders encode failed: {e}")))?;
    Ok(ReadValue::OptString(Some(encoded)))
}
