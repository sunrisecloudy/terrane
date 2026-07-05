use std::collections::BTreeMap;

use terrane_cap_interface::AppId;

pub const MAX_TEXT_BYTES: usize = 32 * 1024;
pub const MAX_RENDERS_PER_APP: usize = 100;
pub const MIN_RATE_MILLI: u32 = 500;
pub const MAX_RATE_MILLI: u32 = 2000;
pub const DEFAULT_RATE_MILLI: u32 = 1000;
pub const TTS_MIME_WAV: &str = "audio/wav";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderRecord {
    pub app: String,
    pub text_hash: String,
    pub voice: Option<String>,
    pub rate_milli: u32,
    pub blob_hash: String,
    pub size: u64,
    pub mime: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TtsState {
    pub renders: BTreeMap<AppId, BTreeMap<String, RenderRecord>>,
    pub order: BTreeMap<AppId, Vec<String>>,
}
