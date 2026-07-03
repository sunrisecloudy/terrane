//! Tests for the on-device ASR engine. The pure surface runs by default;
//! anything that loads real weights needs a model at `TERRANE_STT_MODEL` and
//! is `#[ignore]`d.

use terrane_asr::{pcm_i16_to_f32_mono_16k, AsrError, WHISPER_SAMPLE_RATE_HZ};

#[cfg(feature = "whisper")]
use std::path::PathBuf;

#[cfg(feature = "whisper")]
use terrane_asr::{cached_whisper, ModelFile, WhisperEngine};

#[cfg(feature = "whisper")]
fn model_from_env() -> Option<PathBuf> {
    match std::env::var("TERRANE_STT_MODEL") {
        Ok(path) if !path.trim().is_empty() => Some(PathBuf::from(path)),
        _ => {
            eprintln!("skipping: set TERRANE_STT_MODEL to a local whisper model file");
            None
        }
    }
}

#[cfg(feature = "whisper")]
fn wav_fixture() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("TERRANE_STT_WAV") {
        let path = path.trim();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    let default = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../terrane-host/tests/fixtures/hello-16k-mono.wav");
    if default.is_file() {
        return Some(default);
    }
    eprintln!("skipping: set TERRANE_STT_WAV or add terrane-host/tests/fixtures/hello-16k-mono.wav");
    None
}

#[test]
fn resampling_rejects_zero_sample_rate() {
    let err = pcm_i16_to_f32_mono_16k(&[1, 2, 3], 0).unwrap_err();
    assert!(matches!(err, AsrError::Transcribe(_)));
}

#[test]
fn resampling_at_whisper_rate_matches_manual_conversion() {
    let pcm = [0i16, 16_384, -16_384];
    let out = pcm_i16_to_f32_mono_16k(&pcm, WHISPER_SAMPLE_RATE_HZ).unwrap();
    assert_eq!(out.len(), 3);
    assert!((out[1] - 0.5).abs() < 1e-4);
    assert!((out[2] + 0.5).abs() < 1e-4);
}

#[cfg(feature = "whisper")]
#[test]
fn missing_model_file_fails_fast_with_a_typed_error() {
    let err = match WhisperEngine::new(&ModelFile {
        path: PathBuf::from("/nonexistent/whisper.bin"),
    }) {
        Err(err) => err,
        Ok(_) => panic!("loading a missing file should fail"),
    };
    assert!(matches!(err, AsrError::Load(_)));
    assert!(err.to_string().contains("/nonexistent/whisper.bin"), "{err}");
}

#[cfg(feature = "whisper")]
#[test]
fn cached_whisper_never_caches_load_failures() {
    let missing = ModelFile {
        path: PathBuf::from("/nonexistent/cached-whisper.bin"),
    };
    assert!(matches!(cached_whisper(&missing), Err(AsrError::Load(_))));
    assert!(matches!(cached_whisper(&missing), Err(AsrError::Load(_))));
}

#[cfg(feature = "whisper")]
#[test]
#[ignore = "real whisper inference; needs TERRANE_STT_MODEL and TERRANE_STT_WAV; run with `cargo test -p terrane-asr --features whisper -- --ignored`"]
fn whisper_transcribes_short_wav() {
    let Some(model_path) = model_from_env() else {
        return;
    };
    let Some(wav_path) = wav_fixture() else {
        return;
    };

    let reader = hound::WavReader::open(&wav_path).expect("open wav");
    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "fixture must be mono");
    assert_eq!(spec.sample_rate, 16_000, "fixture must be 16 kHz");
    assert_eq!(spec.bits_per_sample, 16, "fixture must be 16-bit PCM");
    let pcm: Vec<i16> = reader
        .into_samples::<i16>()
        .map(|sample| sample.expect("valid sample"))
        .collect();

    let engine = WhisperEngine::new(&ModelFile { path: model_path }).expect("load model");
    let out = engine
        .transcribe(&pcm, spec.sample_rate)
        .expect("transcribe wav");
    assert!(
        !out.text.trim().is_empty(),
        "expected non-empty transcript, got {:?}",
        out
    );
    terrane_asr::clear_whisper_cache();
}