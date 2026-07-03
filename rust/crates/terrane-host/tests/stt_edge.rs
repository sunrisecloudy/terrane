//! Native STT edge e2e: PCM enqueue → worker → stub ASR → `stt.segment.append`.
//! Default `cargo test` uses the deterministic stub engine (no weights).

use std::ffi::CString;
use std::thread;
use std::time::Duration;

use terrane_host::ffi::{
    terrane_close, terrane_dispatch, terrane_open, terrane_stt_push_pcm,
    terrane_stt_session_begin, terrane_stt_session_end, terrane_stt_shutdown, TERRANE_OK,
};
use terrane_host::stt_runner::frame_samples;

fn speech_burst(frames: usize) -> Vec<i16> {
    let frame = frame_samples(16_000, 30);
    vec![8000_i16; frame * frames]
}

fn close_silence() -> Vec<i16> {
    let frame = frame_samples(16_000, 30);
    vec![0_i16; frame * 12]
}

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

#[test]
fn native_stt_edge_stub_records_segment_from_pcm_queue() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let home_c = CString::new(home.to_str().unwrap()).unwrap();
    let handle = unsafe { terrane_open(home_c.as_ptr()) };
    assert!(!handle.is_null());
    catalog_scribe(handle);

    let app = CString::new("scribe").unwrap();
    let session = CString::new("s-edge-1").unwrap();
    let code = unsafe {
        terrane_stt_session_begin(handle, app.as_ptr(), session.as_ptr(), 16_000)
    };
    assert_eq!(code, TERRANE_OK, "session begin failed");

    let mut pcm = speech_burst(6);
    pcm.extend(close_silence());
    let session_id = CString::new("s-edge-1").unwrap();
    let code = unsafe {
        terrane_stt_push_pcm(session_id.as_ptr(), pcm.as_ptr(), pcm.len())
    };
    assert_eq!(code, TERRANE_OK, "push pcm failed");

    thread::sleep(Duration::from_millis(500));

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
        .and_then(|sessions| sessions.get("s-edge-1"));
    let session = session.expect("session missing after stub capture");
    assert!(!session.segments.is_empty(), "expected at least one segment");
    let text = session.segments.values().next().expect("segment text").text.clone();
    assert!(
        text.starts_with("stub("),
        "default test must use stub ASR, got {text}"
    );
}