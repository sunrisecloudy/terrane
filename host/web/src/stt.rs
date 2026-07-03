//! Ambient STT capture transport for the web host.
#![allow(clippy::question_mark)] // nanoserde `DeJson` derive expands noisy closures
//!
//! Shell-owned mic capture pushes PCM over a loopback WebSocket; a background
//! thread runs [`SttRunner`] with a stub ASR engine (real whisper swaps in
//! when `asr-engine` is enabled on `terrane-host`). Finalized segments cross
//! into the core through the trusted admin HTTP route so the single-threaded
//! `Core` is never touched from the WS thread directly.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use nanoserde::{DeJson, SerJson};
use terrane_host::stt_runner::{
    AsrEngine, AsrOutput, SegmentSink, SessionConfig, SttRunner,
};
use terrane_core::Result as TerraneResult;
use tiny_http::Response;
use tungstenite::handshake::server::{Request, Response as WsResponse};
use tungstenite::{accept_hdr, Message};

use crate::http::{
    admin_authorized, header, json_error, json_ok, ADMIN_HEADER, ADMIN_HEADER_VALUE, Resp,
};

const WORKLET_JS: &str = include_str!("js/stt_capture_worklet.js");
const DEFAULT_MODEL: &str = "whisper-tiny";
const DEFAULT_SAMPLE_RATE_HZ: u32 = 16_000;
const HOST_ID: &str = "web-host";

#[derive(DeJson)]
pub struct SttOpenBody {
    pub app: String,
    #[nserde(rename = "sessionId")]
    pub session_id: String,
    #[nserde(default)]
    pub model: String,
    #[nserde(rename = "sampleRateHz", default)]
    pub sample_rate_hz: u32,
}

#[derive(DeJson, SerJson)]
pub struct SttSegmentBody {
    pub app: String,
    #[nserde(rename = "sessionId")]
    pub session_id: String,
    #[nserde(rename = "segmentSeq")]
    pub segment_seq: u64,
    #[nserde(rename = "startMs")]
    pub start_ms: u64,
    #[nserde(rename = "endMs")]
    pub end_ms: u64,
    #[nserde(default)]
    pub confidence_milli: Option<u32>,
    #[nserde(default)]
    pub lang: Option<String>,
    pub text: String,
}

#[derive(DeJson)]
pub struct SttCloseBody {
    pub app: String,
    #[nserde(rename = "sessionId")]
    pub session_id: String,
    #[nserde(default)]
    pub reason: String,
}

#[derive(SerJson)]
struct OkResponse {
    ok: bool,
}

#[derive(SerJson)]
struct SttOpenResponse {
    #[nserde(rename = "sessionId")]
    session_id: String,
    #[nserde(rename = "wsUrl")]
    ws_url: String,
}

static STT: OnceLock<SttSessions> = OnceLock::new();

/// Install the loopback PCM websocket and session registry for this process.
pub fn init(http_addr: &str) -> Result<(), String> {
    let sessions = SttSessions::start(http_addr)?;
    eprintln!(
        "terrane-web: stt pcm websocket on {}",
        &sessions.ws_url
    );
    STT.set(sessions)
        .map_err(|_| "stt transport already initialized".to_string())
}

fn sessions() -> &'static SttSessions {
    STT.get()
        .expect("stt transport not initialized; call stt::init from main")
}

/// Process-global STT session registry plus the loopback PCM WebSocket server.
struct SttSessions {
    http_base: String,
    ws_url: String,
    runners: Arc<Mutex<HashMap<String, SttRunner<StubEngine, HttpSegmentSink>>>>,
}

struct StubEngine;

impl AsrEngine for StubEngine {
    fn transcribe(&self, pcm: &[i16], _sample_rate_hz: u32) -> TerraneResult<AsrOutput> {
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
}

struct HttpSegmentSink {
    http_base: String,
    app: String,
}

impl SegmentSink for HttpSegmentSink {
    fn append(
        &mut self,
        session_id: &str,
        segment_seq: u64,
        start_ms: u64,
        end_ms: u64,
        output: AsrOutput,
    ) -> TerraneResult<()> {
        let body = SttSegmentBody {
            app: self.app.clone(),
            session_id: session_id.to_string(),
            segment_seq,
            start_ms,
            end_ms,
            confidence_milli: output.confidence_milli,
            lang: output.lang.clone(),
            text: output.text,
        };
        post_admin_segment(&self.http_base, &body).map_err(terrane_core::Error::Runtime)
    }
}

impl SttSessions {
    pub fn start(http_addr: &str) -> Result<Self, String> {
        let http_base = format!("http://{http_addr}");
        let listener = ws_bind_listener(http_addr)?;
        let ws_addr = listener
            .local_addr()
            .map_err(|e| format!("stt pcm websocket addr unavailable: {e}"))?;
        let ws_url = ws_public_url(http_addr, &ws_addr);
        let runners = Arc::new(Mutex::new(HashMap::new()));
        let ws_runners = runners.clone();
        let ws_http_base = http_base.clone();
        // Spawn detached so a bind/accept failure cannot take down the HTTP host.
        thread::spawn(move || {
            if let Err(e) = ws_server_loop(listener, ws_runners, ws_http_base) {
                eprintln!("terrane-web: stt pcm websocket stopped: {e}");
            }
        });
        Ok(Self {
            http_base,
            ws_url,
            runners,
        })
    }

    fn admin_open(
        &self,
        core: &mut terrane_host::HostCore,
        body: &SttOpenBody,
    ) -> Result<SttOpenResponse, String> {
        let app = body.app.trim();
        let session_id = body.session_id.trim();
        if app.is_empty() || session_id.is_empty() {
            return Err("app and sessionId are required".into());
        }
        let model = nonempty_or(body.model.trim(), DEFAULT_MODEL);
        let sample_rate_hz = if body.sample_rate_hz == 0 {
            DEFAULT_SAMPLE_RATE_HZ
        } else {
            body.sample_rate_hz
        };
        let args = vec![
            app.to_string(),
            session_id.to_string(),
            HOST_ID.to_string(),
            HOST_ID.to_string(),
            model.to_string(),
            sample_rate_hz.to_string(),
        ];
        terrane_host::dispatch_on_core(core, "stt.session.open", &args).map_err(|e| {
            format!("stt.session.open dispatch failed: {e}")
        })?;

        let cfg = SessionConfig {
            app: app.to_string(),
            session_id: session_id.to_string(),
            model: model.to_string(),
            sample_rate_hz,
            frame_ms: 30,
        };
        let sink = HttpSegmentSink {
            http_base: self.http_base.clone(),
            app: app.to_string(),
        };
        let runner = SttRunner::new(cfg, StubEngine, sink);
        self.runners
            .lock()
            .map_err(|_| "stt session registry poisoned".to_string())?
            .insert(session_id.to_string(), runner);

        Ok(SttOpenResponse {
            session_id: session_id.to_string(),
            ws_url: self.ws_url.clone(),
        })
    }

    fn admin_close(
        &self,
        core: &mut terrane_host::HostCore,
        body: &SttCloseBody,
    ) -> Result<(), String> {
        let app = body.app.trim();
        let session_id = body.session_id.trim();
        if app.is_empty() || session_id.is_empty() {
            return Err("app and sessionId are required".into());
        }
        let reason = nonempty_or(body.reason.trim(), "stopped");
        let args = vec![
            app.to_string(),
            session_id.to_string(),
            reason.to_string(),
        ];
        terrane_host::dispatch_on_core(core, "stt.session.close-host", &args)?;
        self.runners
            .lock()
            .map_err(|_| "stt session registry poisoned".to_string())?
            .remove(session_id);
        Ok(())
    }
}

pub fn admin_segment(
    core: &mut terrane_host::HostCore,
    body: &SttSegmentBody,
) -> Result<(), String> {
    let args = segment_args(body);
    terrane_host::dispatch_on_core(core, "stt.segment.append", &args).map(|_| ())
}

pub fn worklet_response() -> Resp {
    Response::from_data(WORKLET_JS.as_bytes().to_vec())
        .with_header(header("Content-Type", "text/javascript; charset=utf-8"))
}

#[derive(SerJson)]
struct SttConfigResponse {
    #[nserde(rename = "wsUrl")]
    ws_url: String,
}

pub fn config_response() -> Resp {
    json_ok(&SttConfigResponse {
        ws_url: sessions().ws_url.clone(),
    })
}

pub fn admin_open_route(core: &mut terrane_host::HostCore, request: &mut tiny_http::Request) -> Resp {
    if !admin_authorized(request) {
        return json_error(403, "admin header required");
    }
    match parse_body::<SttOpenBody>(request) {
        Ok(body) => match sessions().admin_open(core, &body) {
            Ok(resp) => json_ok(&resp),
            Err(e) => json_error(400, &e),
        },
        Err(resp) => resp,
    }
}

pub fn admin_segment_route(
    core: &mut terrane_host::HostCore,
    request: &mut tiny_http::Request,
) -> Resp {
    if !admin_authorized(request) {
        return json_error(403, "admin header required");
    }
    match parse_body::<SttSegmentBody>(request) {
        Ok(body) => match admin_segment(core, &body) {
            Ok(()) => json_ok(&OkResponse { ok: true }),
            Err(e) => json_error(400, &e),
        },
        Err(resp) => resp,
    }
}

pub fn admin_close_route(core: &mut terrane_host::HostCore, request: &mut tiny_http::Request) -> Resp {
    if !admin_authorized(request) {
        return json_error(403, "admin header required");
    }
    match parse_body::<SttCloseBody>(request) {
        Ok(body) => match sessions().admin_close(core, &body) {
            Ok(()) => json_ok(&OkResponse { ok: true }),
            Err(e) => json_error(400, &e),
        },
        Err(resp) => resp,
    }
}

fn segment_args(body: &SttSegmentBody) -> Vec<String> {
    let mut args = vec![
        body.app.trim().to_string(),
        body.session_id.trim().to_string(),
        body.segment_seq.to_string(),
        body.start_ms.to_string(),
        body.end_ms.to_string(),
    ];
    if let Some(confidence) = body.confidence_milli {
        args.push("--confidence".into());
        args.push(confidence.to_string());
    }
    if let Some(lang) = body.lang.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        args.push("--lang".into());
        args.push(lang.to_string());
    }
    args.push(body.text.trim().to_string());
    args
}

fn parse_body<T: DeJson>(request: &mut tiny_http::Request) -> Result<T, Resp> {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return Err(json_error(400, "cannot read request body"));
    }
    T::deserialize_json(&body).map_err(|e| json_error(400, &format!("bad stt body: {e}")))
}

fn post_admin_segment(http_base: &str, body: &SttSegmentBody) -> Result<(), String> {
    let payload = body.serialize_json();
    let url = format!("{http_base}/__terrane/admin/stt/segment");
    let agent = ureq::Agent::new();
    let response = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set(ADMIN_HEADER, ADMIN_HEADER_VALUE)
        .send_string(&payload)
        .map_err(|e| format!("segment post failed: {e}"))?;
    let status = response.status();
    let message = response.into_string().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(format!("segment post returned {status}: {message}"));
    }
    Ok(())
}

fn ws_server_loop(
    listener: TcpListener,
    runners: Arc<Mutex<HashMap<String, SttRunner<StubEngine, HttpSegmentSink>>>>,
    http_base: String,
) -> Result<(), String> {
    for stream in listener.incoming().flatten() {
        let runners = runners.clone();
        let http_base = http_base.clone();
        thread::spawn(move || {
            if let Err(e) = serve_pcm_socket(stream, runners, http_base) {
                eprintln!("terrane-web: stt pcm session ended: {e}");
            }
        });
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn serve_pcm_socket(
    stream: TcpStream,
    runners: Arc<Mutex<HashMap<String, SttRunner<StubEngine, HttpSegmentSink>>>>,
    _http_base: String,
) -> Result<(), String> {
    let mut handshake_uri = String::new();
    let callback = |req: &Request, resp: WsResponse| {
        handshake_uri = req.uri().to_string();
        Ok(resp)
    };
    let mut socket = accept_hdr(stream, callback).map_err(|e| e.to_string())?;
    let session_id = parse_session_query(&handshake_uri);
    if session_id.is_empty() {
        let _ = socket.close(None);
        return Err("missing session query parameter".into());
    }

    loop {
        match socket.read() {
            Ok(Message::Binary(data)) => {
                if data.len() % 2 != 0 {
                    continue;
                }
                let pcm = bytes_to_i16(&data);
                let mut guard = runners
                    .lock()
                    .map_err(|_| "stt session registry poisoned".to_string())?;
                let Some(runner) = guard.get_mut(&session_id) else {
                    return Err(format!("unknown stt session {session_id}"));
                };
                if let Err(e) = runner.push_pcm(&pcm) {
                    return Err(e.to_string());
                }
                // Segment dispatch happens via loopback HTTP inside SegmentSink.
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    Ok(())
}

fn bytes_to_i16(data: &[u8]) -> Vec<i16> {
    data.chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn parse_session_query(uri: &str) -> String {
    let Some(query) = uri.split('?').nth(1) else {
        return String::new();
    };
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key == "session" {
            return percent_decode(value);
        }
    }
    String::new()
}

fn percent_decode(value: &str) -> String {
    let mut out = String::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn ws_bind_listener(http_addr: &str) -> Result<TcpListener, String> {
    let (host, _) = split_host_port(http_addr);
    let addr: SocketAddr = format!("{host}:0")
        .parse()
        .map_err(|e| format!("invalid stt websocket bind addr: {e}"))?;
    TcpListener::bind(addr)
        .map_err(|e| format!("stt pcm websocket bind failed on {addr}: {e}"))
}

fn ws_public_url(http_addr: &str, ws_addr: &SocketAddr) -> String {
    let (host, _) = split_host_port(http_addr);
    let host = host
        .trim_matches(|c| c == '[' || c == ']')
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    format!("ws://{host}:{}/__terrane/stt/pcm", ws_addr.port())
}

fn split_host_port(addr: &str) -> (&str, &str) {
    addr.rsplit_once(':').unwrap_or((addr, "8780"))
}

fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() { fallback } else { value }
}