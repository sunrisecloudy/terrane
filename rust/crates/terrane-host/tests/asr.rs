//! Effectful tests for the whisper.cpp ASR bridge. Default `cargo test` stays
//! green; run with `cargo test -p terrane-host --features asr-engine -- --ignored`.

#![cfg(feature = "asr-engine")]

use std::path::PathBuf;

use terrane_host::asr::HostWhisper;
use terrane_host::stt_runner::AsrEngine;

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
#[ignore = "real whisper inference; needs TERRANE_STT_MODEL and TERRANE_STT_WAV; run with `cargo test -p terrane-host --features asr-engine -- --ignored`"]
fn host_whisper_transcribes_short_wav() {
    let Some(model_path) = model_from_env() else {
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

    let engine = HostWhisper::from_path(model_path).expect("load model");
    let out = engine
        .transcribe(&pcm, spec.sample_rate)
        .expect("transcribe wav");
    assert!(
        !out.text.trim().is_empty(),
        "expected non-empty transcript, got {:?}",
        out
    );
}