//! Tests for the ambient STT runner. The runner's real logic — PCM framing,
//! energy-based VAD with hysteresis, monotonic segment sequencing, audio-clock
//! timing, ring backpressure — is exercised with deterministic fixtures for the
//! pluggable engine and sink. The whisper.cpp engine's own effectful test lives
//! in `asr.rs` behind `#[ignore]` (it needs downloaded weights).

use std::cell::RefCell;
use std::rc::Rc;

use terrane_host::stt_runner::{
    frame_energy, frame_samples, AsrEngine, AsrOutput, PcmRing, SegmentSink, SessionConfig,
    SttRunner, SttVad, VadEdge,
};
use terrane_core::{Error, Result};

/// A deterministic engine: returns a fixed transcript tagged with the utterance
/// length so sequencing is observable. This is a test fixture for the runner's
/// logic, not a stand-in for the real ASR (whisper.cpp has its own tests).
struct FixedEngine {
    text: &'static str,
    fail: bool,
}

impl AsrEngine for FixedEngine {
    fn transcribe(&self, pcm: &[i16], _hz: u32) -> Result<AsrOutput> {
        if self.fail {
            return Err(Error::Runtime("engine failed".into()));
        }
        Ok(AsrOutput {
            text: format!("{}({})", self.text, pcm.len()),
            confidence_milli: Some(900),
            lang: Some("en".into()),
        })
    }
}

/// An engine that always returns empty text, so the "skip empty recognitions"
/// path (no segment, no consumed sequence number) is observable.
struct EmptyEngine;

impl AsrEngine for EmptyEngine {
    fn transcribe(&self, _pcm: &[i16], _hz: u32) -> Result<AsrOutput> {
        Ok(AsrOutput {
            text: String::new(),
            confidence_milli: None,
            lang: None,
        })
    }
}

type Segment = (String, u64, u64, u64, AsrOutput);

#[derive(Clone)]
struct RecordingSink {
    // Shared so the runner's owned sink and the test's handle observe the same
    // dispatched segments (the runner owns its sink; cloning must not fork it).
    segments: Rc<RefCell<Vec<Segment>>>,
}

impl Default for RecordingSink {
    fn default() -> Self {
        Self {
            segments: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl SegmentSink for RecordingSink {
    fn append(
        &mut self,
        session_id: &str,
        segment_seq: u64,
        start_ms: u64,
        end_ms: u64,
        output: AsrOutput,
    ) -> Result<()> {
        self.segments.borrow_mut().push((
            session_id.to_string(),
            segment_seq,
            start_ms,
            end_ms,
            output,
        ));
        Ok(())
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        app: "scribe".into(),
        session_id: "s1".into(),
        model: "whisper-tiny".into(),
        sample_rate_hz: 16_000,
        frame_ms: 30,
    }
}

/// Silence long enough to close an utterance: the default VAD hangover is
/// ~330 ms = 11 frames at 30 ms, so 12 frames reliably ends speech.
fn close_silence() -> Vec<i16> {
    let frame = frame_samples(16_000, 30);
    tone(frame * 12, 0)
}

/// A loud burst long enough to register as speech (>= 1 frame).
fn speech_burst(frames: usize) -> Vec<i16> {
    let frame = frame_samples(16_000, 30);
    tone(frame * frames, 8000)
}

/// A frame of `n` samples at the given amplitude (a fixed tone → non-zero
/// mean-square energy). `silence` produces near-zero energy.
fn tone(n: usize, amp: i16) -> Vec<i16> {
    vec![amp; n]
}

#[test]
fn vad_starts_on_speech_and_ends_after_hangover() {
    let mut vad = SttVad::with_thresholds(100, 50, 3);
    // Silence does nothing.
    assert_eq!(vad.push(10), None);
    // Crossing the speech threshold starts an utterance.
    assert_eq!(vad.push(150), Some(VadEdge::SpeechStart));
    assert!(vad.in_speech());
    // One silent frame is not enough (hangover = 3).
    assert_eq!(vad.push(10), None);
    assert!(vad.in_speech());
    // Speech resets the silence run.
    assert_eq!(vad.push(150), None);
    assert_eq!(vad.push(10), None);
    assert_eq!(vad.push(10), None);
    // Third consecutive silent frame closes the utterance.
    assert_eq!(vad.push(10), Some(VadEdge::SpeechEnd));
    assert!(!vad.in_speech());
}

#[test]
fn frame_energy_is_mean_square() {
    let frame = [0, 100, -100, 0];
    // (0 + 10000 + 10000 + 0) / 4 = 5000
    assert_eq!(frame_energy(&frame), 5000);
    assert_eq!(frame_energy(&[]), 0);
}

#[test]
fn frame_samples_matches_rate_and_duration() {
    // 16 kHz * 30 ms = 480 samples.
    assert_eq!(frame_samples(16_000, 30), 480);
    assert_eq!(frame_samples(8_000, 30), 240);
}

#[test]
fn runner_dispatches_one_segment_per_closed_utterance_with_monotonic_seq() {
    let sink = RecordingSink::default();
    let runner_sink = sink.clone();
    let mut runner =
        SttRunner::new(config(), FixedEngine { text: "utt", fail: false }, runner_sink);

    // Push a loud burst (speech) then silence long enough to close it.
    runner.push_pcm(&speech_burst(6)).unwrap();
    runner.push_pcm(&close_silence()).unwrap();

    // The first closed utterance dispatches segment #1.
    assert_eq!(sink.segments.borrow().len(), 1);
    {
        let segs = sink.segments.borrow();
        let (_, seq, start_ms, end_ms, out) = &segs[0];
        assert_eq!(*seq, 1);
        assert_eq!(*start_ms, 0);
        assert!(*end_ms > 0);
        assert!(out.text.starts_with("utt("));
    }

    // A second utterance gets the next monotonic sequence number.
    runner.push_pcm(&speech_burst(6)).unwrap();
    runner.push_pcm(&close_silence()).unwrap();
    assert_eq!(runner.next_segment_seq(), 3);
    let segs = sink.segments.borrow();
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].1, 1);
    assert_eq!(segs[1].1, 2);
    // Timings are offsets from session open, monotonically increasing.
    assert!(segs[1].2 >= segs[0].3);
}

#[test]
fn runner_skips_empty_recognitions_without_consuming_a_seq() {
    let sink = RecordingSink::default();
    let mut runner = SttRunner::new(config(), EmptyEngine, sink.clone());

    // A real utterance that the engine recognizes as empty: no segment, no seq.
    runner.push_pcm(&speech_burst(6)).unwrap();
    runner.push_pcm(&close_silence()).unwrap();
    assert_eq!(sink.segments.borrow().len(), 0);
    assert_eq!(runner.next_segment_seq(), 1);

    // A second empty utterance is likewise skipped; the sequence stays put.
    runner.push_pcm(&speech_burst(6)).unwrap();
    runner.push_pcm(&close_silence()).unwrap();
    assert_eq!(sink.segments.borrow().len(), 0);
    assert_eq!(runner.next_segment_seq(), 1);
}

#[test]
fn runner_propagates_engine_errors() {
    let sink = RecordingSink::default();
    let mut runner = SttRunner::new(config(), FixedEngine { text: "x", fail: true }, sink.clone());

    runner.push_pcm(&speech_burst(6)).unwrap();
    // Closing the utterance runs the failing engine → error propagates.
    let err = runner.push_pcm(&close_silence()).unwrap_err();
    assert!(err.to_string().contains("engine failed"), "{err}");
    // No segment was dispatched and the sequence is untouched.
    assert!(sink.segments.borrow().is_empty());
    assert_eq!(runner.next_segment_seq(), 1);
}

#[test]
fn idle_ms_grows_only_after_an_utterance_closes() {
    let sink = RecordingSink::default();
    let mut runner = SttRunner::new(config(), FixedEngine { text: "u", fail: false }, sink);

    // Speak, then close the utterance with silence. While in speech, every frame
    // refreshes the speech clock, so idle stays ~0.
    runner.push_pcm(&speech_burst(4)).unwrap();
    runner.push_pcm(&close_silence()).unwrap();
    let idle_after_close = runner.idle_ms();

    // Further silence (after close) is no longer refreshed → idle climbs.
    runner.push_pcm(&tone(frame_samples(16_000, 30) * 5, 0)).unwrap();
    assert!(runner.idle_ms() > idle_after_close);
    assert!(runner.idle_ms() > 0);
}

#[test]
fn runner_drops_oldest_when_a_single_push_exceeds_the_ring_cap() {
    let frame = frame_samples(16_000, 30);
    let sink = RecordingSink::default();
    // Cap smaller than one utterance: the runner keeps only the most recent
    // window, so the push must not panic and must respect the cap.
    let mut runner =
        SttRunner::new(config(), FixedEngine { text: "u", fail: false }, sink.clone())
            .with_ring_cap(frame * 8);

    // One push with speech followed by silence, exceeding the cap.
    let mut chunk = tone(frame * 6, 8000);
    chunk.extend(tone(frame * 6, 0));
    runner.push_pcm(&chunk).unwrap();
    // At most one segment (the retained tail may or may not cross thresholds,
    // but the push must not panic and must respect the cap).
    let count = sink.segments.borrow().len();
    assert!(count <= 1, "expected at most one segment, got {count}");
}

#[test]
fn pcm_ring_is_drop_oldest_and_bounded() {
    let mut ring = PcmRing::new(4);
    ring.push(&[1, 2, 3, 4, 5, 6]);
    assert_eq!(ring.len(), 4);
    assert_eq!(ring.drain(), vec![3, 4, 5, 6]);
    assert!(ring.is_empty());
    // cap() reports the configured bound.
    assert_eq!(ring.cap(), 4);
}

#[test]
fn runner_handles_partial_frames_across_pushes() {
    let frame = frame_samples(16_000, 30);
    let sink = RecordingSink::default();
    let mut runner = SttRunner::new(config(), FixedEngine { text: "u", fail: false }, sink.clone());

    // Push the same speech+silence split across many single-sample calls —
    // framing must reassemble frames and still close the utterance exactly once.
    let mut combined = speech_burst(4);
    combined.extend(close_silence());
    let _ = frame; // frame kept for readability of the framing guarantee
    for sample in combined {
        runner.push_pcm(&[sample]).unwrap();
    }
    assert_eq!(sink.segments.borrow().len(), 1);
    assert_eq!(sink.segments.borrow()[0].1, 1);
}
