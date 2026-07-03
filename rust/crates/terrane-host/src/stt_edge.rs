//! Native STT capture edge: enqueue PCM from a real-time audio thread, drain on
//! a worker, and dispatch finalized segments through an open [`TerraneHandle`].
//!
//! The macOS `AVAudioEngine` tap calls [`crate::ffi::terrane_stt_push_pcm`];
//! it must never lock the core or run whisper. A background worker owns the
//! [`crate::stt_runner::SttRunner`] and calls [`crate::dispatch_on_core`] from
//! its thread via [`CoreSegmentSink`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use terrane_core::Result as TerraneResult;

use crate::stt_runner::{
    AsrEngine, AsrOutput, PcmRing, SegmentSink, SessionConfig, SttRunner,
};

const HOST_ID: &str = "macos-host";
const DEFAULT_MODEL: &str = "whisper-tiny";
const DEFAULT_SAMPLE_RATE_HZ: u32 = 16_000;
const DEFAULT_IDLE_MS: u64 = 120_000;
const WORKER_POLL_MS: u64 = 30;

static EDGE: OnceLock<SttEdgeHub> = OnceLock::new();

struct SttEdgeHub {
    sessions: Mutex<HashMap<String, NativeSession>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    stop: Arc<AtomicBool>,
}

struct NativeSession {
    app: String,
    handle: usize,
    ring: Mutex<PcmRing>,
    runner: Mutex<SttRunner<EdgeAsrEngine, CoreSegmentSink>>,
}

struct CoreSegmentSink {
    handle: usize,
    app: String,
}

enum EdgeAsrEngine {
    Stub,
    #[cfg(feature = "asr-engine")]
    Whisper(Arc<Mutex<crate::asr::HostWhisper>>),
}

impl AsrEngine for EdgeAsrEngine {
    fn transcribe(&self, pcm: &[i16], sample_rate_hz: u32) -> TerraneResult<AsrOutput> {
        match self {
            Self::Stub => stub_transcribe(pcm, sample_rate_hz),
            #[cfg(feature = "asr-engine")]
            Self::Whisper(engine) => engine
                .lock()
                .map_err(|_| terrane_core::Error::Runtime("whisper engine poisoned".into()))?
                .transcribe(pcm, sample_rate_hz),
        }
    }
}

impl SegmentSink for CoreSegmentSink {
    fn append(
        &mut self,
        session_id: &str,
        segment_seq: u64,
        start_ms: u64,
        end_ms: u64,
        output: AsrOutput,
    ) -> TerraneResult<()> {
        let args = segment_args(
            &self.app,
            session_id,
            segment_seq,
            start_ms,
            end_ms,
            &output,
        );
        dispatch_on_handle(self.handle, "stt.segment.append", &args)
            .map_err(terrane_core::Error::Runtime)
    }
}

fn hub() -> &'static SttEdgeHub {
    EDGE.get_or_init(|| SttEdgeHub {
        sessions: Mutex::new(HashMap::new()),
        worker: Mutex::new(None),
        stop: Arc::new(AtomicBool::new(false)),
    })
}

/// Stop the worker and drop all native capture sessions. Safe at process exit.
pub fn shutdown() {
    if let Some(hub) = EDGE.get() {
        hub.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = hub.worker.lock().ok().and_then(|mut g| g.take()) {
            let _ = handle.join();
        }
        if let Ok(mut sessions) = hub.sessions.lock() {
            sessions.clear();
        }
    }
}

pub(crate) fn session_begin(
    handle: usize,
    app: &str,
    session_id: &str,
    sample_rate_hz: u32,
) -> std::result::Result<(), String> {
    let app = app.trim();
    let session_id = session_id.trim();
    if app.is_empty() || session_id.is_empty() {
        return Err("app and session_id are required".into());
    }
    let sample_rate_hz = if sample_rate_hz == 0 {
        DEFAULT_SAMPLE_RATE_HZ
    } else {
        sample_rate_hz
    };
    let open_args = vec![
        app.to_string(),
        session_id.to_string(),
        HOST_ID.to_string(),
        HOST_ID.to_string(),
        DEFAULT_MODEL.to_string(),
        sample_rate_hz.to_string(),
    ];
    dispatch_on_handle(handle, "stt.session.open", &open_args)?;

    let cfg = SessionConfig {
        app: app.to_string(),
        session_id: session_id.to_string(),
        model: DEFAULT_MODEL.to_string(),
        sample_rate_hz,
        frame_ms: 30,
    };
    let sink = CoreSegmentSink {
        handle,
        app: app.to_string(),
    };
    let runner = SttRunner::new(cfg, make_engine(), sink);
    let session = NativeSession {
        app: app.to_string(),
        handle,
        ring: Mutex::new(PcmRing::new(ring_cap_samples(sample_rate_hz))),
        runner: Mutex::new(runner),
    };
    hub()
        .sessions
        .lock()
        .map_err(|_| "native stt session registry poisoned".to_string())?
        .insert(session_id.to_string(), session);
    ensure_worker();
    Ok(())
}

pub(crate) fn push_pcm(session_id: &str, pcm: &[i16]) -> std::result::Result<(), String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("session_id is required".into());
    }
    let sessions = hub()
        .sessions
        .lock()
        .map_err(|_| "native stt session registry poisoned".to_string())?;
    let Some(session) = sessions.get(session_id) else {
        return Err(format!("unknown native stt session {session_id}"));
    };
    session
        .ring
        .lock()
        .map_err(|_| "native stt ring poisoned".to_string())?
        .push(pcm);
    Ok(())
}

pub(crate) fn session_end(
    handle: usize,
    app: &str,
    session_id: &str,
    reason: &str,
) -> std::result::Result<(), String> {
    let app = app.trim();
    let session_id = session_id.trim();
    if app.is_empty() || session_id.is_empty() {
        return Err("app and session_id are required".into());
    }
    let reason = if reason.trim().is_empty() {
        "stopped"
    } else {
        reason.trim()
    };
    hub()
        .sessions
        .lock()
        .map_err(|_| "native stt session registry poisoned".to_string())?
        .remove(session_id);
    let args = vec![
        app.to_string(),
        session_id.to_string(),
        reason.to_string(),
    ];
    dispatch_on_handle(handle, "stt.session.close-host", &args)
}

fn ensure_worker() {
    let hub = hub();
    let mut guard = hub
        .worker
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return;
    }
    let stop = hub.stop.clone();
    let handle = thread::spawn(move || worker_loop(stop));
    *guard = Some(handle);
}

fn worker_loop(stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        if let Ok(mut sessions) = hub().sessions.lock() {
            let idle_ms = idle_threshold_ms();
            let mut idle_closes = Vec::new();
            for (session_id, session) in sessions.iter_mut() {
                if let Ok(mut ring) = session.ring.lock() {
                    let chunk = ring.drain();
                    if !chunk.is_empty() {
                        if let Ok(mut runner) = session.runner.lock() {
                            if let Err(error) = runner.push_pcm(&chunk) {
                                eprintln!("terrane-host: native stt runner error: {error}");
                            }
                            if runner.idle_ms() >= idle_ms {
                                idle_closes.push((
                                    session.app.clone(),
                                    session_id.clone(),
                                    session.handle,
                                ));
                            }
                        }
                    } else if let Ok(runner) = session.runner.lock() {
                        if runner.idle_ms() >= idle_ms {
                            idle_closes.push((
                                session.app.clone(),
                                session_id.clone(),
                                session.handle,
                            ));
                        }
                    }
                }
            }
            for (app, session_id, handle) in idle_closes {
                sessions.remove(&session_id);
                let args = vec![
                    app,
                    session_id,
                    "idle".to_string(),
                ];
                if let Err(error) = dispatch_on_handle(handle, "stt.session.close-host", &args) {
                    eprintln!("terrane-host: native stt idle close failed: {error}");
                }
            }
        }
        thread::sleep(Duration::from_millis(WORKER_POLL_MS));
    }
}

fn dispatch_on_handle(
    handle: usize,
    command: &str,
    args: &[String],
) -> std::result::Result<(), String> {
    if handle == 0 {
        return Err("terrane handle is null".into());
    }
    // SAFETY: callers pass a live TerraneHandle* from terrane_open for the process lifetime.
    unsafe {
        crate::ffi::dispatch_on_terrane_handle(handle as *mut crate::ffi::TerraneHandle, command, args)
    }
}

fn segment_args(
    app: &str,
    session_id: &str,
    segment_seq: u64,
    start_ms: u64,
    end_ms: u64,
    output: &AsrOutput,
) -> Vec<String> {
    let mut args = vec![
        app.to_string(),
        session_id.to_string(),
        segment_seq.to_string(),
        start_ms.to_string(),
        end_ms.to_string(),
    ];
    if let Some(confidence) = output.confidence_milli {
        args.push("--confidence".into());
        args.push(confidence.to_string());
    }
    if let Some(lang) = output.lang.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        args.push("--lang".into());
        args.push(lang.to_string());
    }
    args.push(output.text.trim().to_string());
    args
}

fn stub_transcribe(pcm: &[i16], _sample_rate_hz: u32) -> TerraneResult<AsrOutput> {
    if pcm.is_empty() {
        return Ok(AsrOutput {
            text: String::new(),
            confidence_milli: None,
            lang: None,
        });
    }
    Ok(AsrOutput {
        text: format!("stub({})", pcm.len()),
        confidence_milli: Some(500),
        lang: Some("en".into()),
    })
}

fn make_engine() -> EdgeAsrEngine {
    #[cfg(feature = "asr-engine")]
    if let Some(engine) = crate::asr::shared_whisper() {
        return EdgeAsrEngine::Whisper(engine);
    }
    EdgeAsrEngine::Stub
}

fn ring_cap_samples(sample_rate_hz: u32) -> usize {
    // ~30 seconds of mono PCM at the session rate.
    (sample_rate_hz as usize) * 30
}

fn idle_threshold_ms() -> u64 {
    std::env::var("TERRANE_STT_IDLE_MS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_IDLE_MS)
}