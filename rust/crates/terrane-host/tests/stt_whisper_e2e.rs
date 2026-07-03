//! Effectful STT pipeline e2e through the native edge worker + whisper.cpp.
//! Default `cargo test` skips these (`#[ignore]`); run deliberately:
//! `cargo test -p terrane-host --features asr-engine --test stt_whisper_e2e -- --ignored`

#![cfg(feature = "asr-engine")]

use std::ffi::CString;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use terrane_host::ffi::{
    terrane_close, terrane_dispatch, terrane_open, terrane_stt_push_pcm,
    terrane_stt_session_begin, terrane_stt_session_end, terrane_stt_shutdown, TERRANE_OK,
};

fn catalog_scribe(handle: *mut terrane_host::ffi::TerraneHandle) {
    let command = CString::new("app.add").unwrap();
    let id = CString::new("scribe").unwrap();
    let name = CString::new("Scribe").unwrap();
    let argv = [id.as_ptr(), name.as_ptr()];
    let mut out = std::ptr::null_mut();
    let mut err = std::ptr::null_mut();
    let code = unsafe {
        terrane_dispatch(
            handle,
            command.as_ptr(),
            argv.len(),
            argv.as_ptr(),
            &mut out,
            &mut err,
        )
    };
    assert_eq!(code, TERRANE_OK, "catalog scribe failed");
    unsafe {
        if !out.is_null() {
            terrane_host::ffi::terrane_string_free(out);
        }
        if !err.is_null() {
            terrane_host::ffi::terrane_string_free(err);
        }
    }
}

fn model_from_env() -> Option<PathBuf> {
    match std::env::var("TERRANE_STT_MODEL") {
        Ok(path) if !path.trim().is_empty() => Some(PathBuf::from(path)),
        _ => {
            eprintln!("skipping: set TERRANE_STT_MODEL to a local whisper model file");
            None
        }
    }
}

fn wav_from_env() -> Option<PathBuf> {
    match std::env::var("TERRANE_STT_WAV") {
        Ok(path) if !path.trim().is_empty() => Some(PathBuf::from(path)),
        _ => {
            eprintln!("skipping: set TERRANE_STT_WAV to a 16 kHz mono WAV fixture");
            None
        }
    }
}

#[test]
#[ignore = "real whisper STT edge e2e; needs TERRANE_STT_MODEL and TERRANE_STT_WAV; run with `cargo test -p terrane-host --features asr-engine --test stt_whisper_e2e -- --ignored`"]
fn native_stt_edge_whisper_transcribes_wav_fixture() {
    let Some(_model) = model_from_env() else {
        return;
    };
    let Some(wav_path) = wav_from_env() else {
        return;
    };

    let reader = hound::WavReader::open(&wav_path).expect("open wav");
    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "fixture must be mono");
    assert_eq!(spec.sample_rate, 16_000, "fixture must be 16 kHz");
    let pcm: Vec<i16> = reader
        .into_samples::<i16>()
        .map(|sample| sample.expect("valid sample"))
        .collect();
    assert!(!pcm.is_empty(), "fixture must contain samples");

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let home_c = CString::new(home.to_str().unwrap()).unwrap();
    let handle = unsafe { terrane_open(home_c.as_ptr()) };
    assert!(!handle.is_null());
    catalog_scribe(handle);

    let app = CString::new("scribe").unwrap();
    let session = CString::new("s-whisper-1").unwrap();
    let code = unsafe {
        terrane_stt_session_begin(handle, app.as_ptr(), session.as_ptr(), spec.sample_rate)
    };
    assert_eq!(code, TERRANE_OK, "session begin failed");

    let session_id = CString::new("s-whisper-1").unwrap();
    let code = unsafe {
        terrane_stt_push_pcm(session_id.as_ptr(), pcm.as_ptr(), pcm.len())
    };
    assert_eq!(code, TERRANE_OK, "push pcm failed");

    // Whisper + VAD need time to drain the full utterance on the worker thread.
    thread::sleep(Duration::from_secs(30));

    let reason = CString::new("stopped").unwrap();
    let code = unsafe {
        terrane_stt_session_end(handle, app.as_ptr(), session.as_ptr(), reason.as_ptr())
    };
    assert_eq!(code, TERRANE_OK, "session end failed");

    unsafe {
        terrane_stt_shutdown();
        terrane_close(handle);
    }

    let core = terrane_host::open_at_home(home).expect("reopen home");
    let session = core
        .state()
        .stt
        .sessions
        .get("scribe")
        .and_then(|sessions| sessions.get("s-whisper-1"));
    let session = session.expect("session missing after whisper capture");
    assert!(!session.segments.is_empty(), "expected whisper segment(s)");
    let text = session
        .segments
        .values()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        !text.trim().is_empty() && !text.contains("stub("),
        "expected real whisper transcript, got {text:?}"
    );
}