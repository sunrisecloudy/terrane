//! The ambient speech-to-text session runner — the host-edge producer that owns
//! the continuous capture lifecycle the `stt` capability only records.
//!
//! The core never sees audio. This runner frames incoming PCM, runs a simple
//! energy-based VAD with hysteresis to close utterances, hands each closed
//! utterance to a pluggable [`AsrEngine`] (whisper.cpp behind the `asr-engine`
//! feature in `asr.rs`; a test fixture in tests), and dispatches one finalized
//! `stt.segment.append` per utterance through a [`SegmentSink`] the host wires
//! to `dispatch_on_core`. Segment sequence numbers are minted monotonically and
//! start at 1, matching the capability's fold contract; `start_ms`/`end_ms` are
//! offsets from session open derived purely from the sample clock (no wall clock
//! in the recorded facts), so replay identity holds.
//!
//! Mic capture (web `getUserMedia`/AudioWorklet, macOS `AVAudioEngine`) only
//! pushes PCM here via [`SttRunner::push_pcm`]; it never touches the core. The
//! real-time audio thread enqueues, VAD + ASR run on the runner's thread.

use std::collections::VecDeque;

use terrane_core::Result;

/// A finalized ASR result for one closed utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrOutput {
    pub text: String,
    /// Confidence in thousandths (0-1000), if the engine reports one.
    pub confidence_milli: Option<u32>,
    /// BCP-47 language code, if known.
    pub lang: Option<String>,
}

/// Pluggable on-device ASR. The whisper.cpp backend lives in `asr.rs` behind the
/// `asr-engine` feature; tests use a deterministic fixture. Called on the
/// runner's thread, never on a real-time audio thread.
pub trait AsrEngine {
    fn transcribe(&self, pcm: &[i16], sample_rate_hz: u32) -> Result<AsrOutput>;
}

/// Where the runner delivers a finalized segment. The host implements this to
/// dispatch `stt.segment.append` (trusted) against the open core.
pub trait SegmentSink {
    fn append(
        &mut self,
        session_id: &str,
        segment_seq: u64,
        start_ms: u64,
        end_ms: u64,
        output: AsrOutput,
    ) -> Result<()>;
}

/// A simple energy-based voice activity detector with hysteresis: speech starts
/// when a frame's energy crosses `speech_energy`, and ends only after
/// `hangover_frames` consecutive sub-`silence_energy` frames. Pure and
/// deterministic — fully driven by the per-frame energy the runner computes.
#[derive(Debug, Clone)]
pub struct SttVad {
    speech_energy: u64,
    silence_energy: u64,
    hangover_frames: u32,
    in_speech: bool,
    silence_run: u32,
}

/// The frame-level events the VAD emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadEdge {
    /// Energy crossed the speech threshold; an utterance starts here.
    SpeechStart,
    /// Enough silent frames accrued after speech; the utterance ends here.
    SpeechEnd,
}

impl SttVad {
    /// A conservative default: speech above ~4.7% of full-scale mean-square
    /// energy (`50M` of the ~1.07e9 i16 max), silence below ~0.05% (`500K`),
    /// and a ~330ms hangover at the configured frame duration to avoid clipping
    /// short pauses.
    pub fn new(frame_ms: u32) -> Self {
        Self::with_thresholds(50_000_000, 500_000, frames_for_duration(frame_ms, 330))
    }

    /// Build with explicit thresholds (mean-square energy units) and hangover.
    pub fn with_thresholds(
        speech_energy: u64,
        silence_energy: u64,
        hangover_frames: u32,
    ) -> Self {
        Self {
            speech_energy,
            silence_energy: silence_energy.min(speech_energy),
            hangover_frames: hangover_frames.max(1),
            in_speech: false,
            silence_run: 0,
        }
    }

    pub fn in_speech(&self) -> bool {
        self.in_speech
    }

    /// Feed one frame's mean-square energy. Returns the edge it crossed, if any.
    pub fn push(&mut self, energy: u64) -> Option<VadEdge> {
        if !self.in_speech {
            if energy >= self.speech_energy {
                self.in_speech = true;
                self.silence_run = 0;
                return Some(VadEdge::SpeechStart);
            }
            None
        } else if energy < self.silence_energy {
            self.silence_run += 1;
            if self.silence_run >= self.hangover_frames {
                self.in_speech = false;
                self.silence_run = 0;
                return Some(VadEdge::SpeechEnd);
            }
            None
        } else {
            self.silence_run = 0;
            None
        }
    }
}

fn frames_for_duration(frame_ms: u32, duration_ms: u32) -> u32 {
    (duration_ms.div_ceil(frame_ms)).max(1)
}

/// The number of samples in one analysis frame at `sample_rate_hz`.
pub fn frame_samples(sample_rate_hz: u32, frame_ms: u32) -> usize {
    ((sample_rate_hz as u64 * frame_ms as u64) / 1000) as usize
}

/// Mean-square energy of a frame: `sum(sample^2) / n`. A fixed-point amplitude
/// proxy that avoids the `sqrt` of true RMS while preserving ordering.
pub fn frame_energy(samples: &[i16]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let sum: u64 = samples
        .iter()
        .map(|s| (*s as i64 * *s as i64) as u64)
        .sum();
    sum / samples.len() as u64
}

/// Configuration for one capture session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub app: String,
    pub session_id: String,
    pub model: String,
    pub sample_rate_hz: u32,
    /// Target analysis frame duration in milliseconds (default 30).
    pub frame_ms: u32,
}

/// The ambient runner. Generic over the ASR engine and the segment sink so the
/// deterministic parts (framing, VAD, sequencing, timing) are testable without
/// any model or core.
pub struct SttRunner<E: AsrEngine, S: SegmentSink> {
    cfg: SessionConfig,
    engine: E,
    sink: S,
    vad: SttVad,
    frame_size: usize,
    /// Drop-oldest bounded buffer so a slow consumer cannot let live audio grow
    /// unbounded; the plan calls this out as load-bearing backpressure.
    ring_cap_samples: usize,
    frame_buf: Vec<i16>,
    /// Accumulated PCM for the utterance currently in speech.
    utterance: Vec<i16>,
    utterance_start_ms: u64,
    /// Monotonic high-water mark; the next finalized segment is this + 1.
    next_segment_seq: u64,
    /// Total samples pushed since session open — the audio clock.
    samples_seen: u64,
    /// Offset (in samples) of the most recent speech frame, for idle tracking.
    last_speech_sample: u64,
}

impl<E: AsrEngine, S: SegmentSink> SttRunner<E, S> {
    pub fn new(cfg: SessionConfig, engine: E, sink: S) -> Self {
        let frame_size = frame_samples(cfg.sample_rate_hz, cfg.frame_ms.max(1)).max(1);
        let vad = SttVad::new(cfg.frame_ms.max(1));
        Self {
            cfg,
            engine,
            sink,
            vad,
            frame_size,
            ring_cap_samples: 0,
            frame_buf: Vec::new(),
            utterance: Vec::new(),
            utterance_start_ms: 0,
            next_segment_seq: 1,
            samples_seen: 0,
            last_speech_sample: 0,
        }
    }

    /// Cap the retained PCM (drop-oldest when full). 0 = unbounded. Backpressure
    /// for a slow consumer; the dropped audio never reaches the core.
    pub fn with_ring_cap(mut self, samples: usize) -> Self {
        self.ring_cap_samples = samples;
        self
    }

    pub fn session_id(&self) -> &str {
        &self.cfg.session_id
    }

    pub fn app_id(&self) -> &str {
        &self.cfg.app
    }

    pub fn next_segment_seq(&self) -> u64 {
        self.next_segment_seq
    }

    /// Audio-clock idle: milliseconds of pushed silence since the last speech
    /// frame. Deterministic and replay-relevant (the host closes an idle
    /// session by dispatching `stt.session.close-host reason="idle"` once this
    /// crosses its threshold).
    pub fn idle_ms(&self) -> u64 {
        ms(self.samples_seen - self.last_speech_sample, self.cfg.sample_rate_hz)
    }

    /// Push a chunk of mono PCM. Frames the audio, runs the VAD, and on each
    /// closed utterance transcribes it and dispatches a finalized segment.
    pub fn push_pcm(&mut self, samples: &[i16]) -> Result<()> {
        if self.ring_cap_samples > 0 && samples.len() > self.ring_cap_samples {
            // A single oversized push: keep the most recent window only.
            let start = samples.len() - self.ring_cap_samples;
            return self.push_pcm(&samples[start..]);
        }
        for sample in samples {
            self.frame_buf.push(*sample);
            self.samples_seen += 1;
            if self.frame_buf.len() >= self.frame_size {
                let frame = std::mem::take(&mut self.frame_buf);
                self.process_frame(&frame)?;
            }
        }
        Ok(())
    }

    fn process_frame(&mut self, frame: &[i16]) -> Result<()> {
        let frame_start_sample = self.samples_seen.saturating_sub(frame.len() as u64);
        let energy = frame_energy(frame);
        match self.vad.push(energy) {
            Some(VadEdge::SpeechStart) => {
                self.utterance.clear();
                self.utterance_start_ms = ms(frame_start_sample, self.cfg.sample_rate_hz);
                self.utterance.extend_from_slice(frame);
                self.last_speech_sample = self.samples_seen;
            }
            Some(VadEdge::SpeechEnd) => {
                self.utterance.extend_from_slice(frame);
                self.last_speech_sample = self.samples_seen;
                self.finalize_utterance()?;
            }
            None => {
                if self.vad.in_speech() {
                    self.utterance.extend_from_slice(frame);
                    self.last_speech_sample = self.samples_seen;
                }
            }
        }
        Ok(())
    }

    fn finalize_utterance(&mut self) -> Result<()> {
        if self.utterance.is_empty() {
            return Ok(());
        }
        let pcm = std::mem::take(&mut self.utterance);
        let output = self.engine.transcribe(&pcm, self.cfg.sample_rate_hz)?;
        // Skip empty recognitions (engine heard no speech) without consuming a
        // sequence number, so finalized segments always carry real text.
        if !output.text.trim().is_empty() {
            let end_ms = ms(self.samples_seen, self.cfg.sample_rate_hz);
            let seq = self.next_segment_seq;
            self.sink.append(
                &self.cfg.session_id,
                seq,
                self.utterance_start_ms,
                end_ms,
                output,
            )?;
            self.next_segment_seq += 1;
        }
        Ok(())
    }
}

/// Convert a sample offset to a millisecond offset from session open.
fn ms(samples: u64, sample_rate_hz: u32) -> u64 {
    if sample_rate_hz == 0 {
        return 0;
    }
    (samples * 1000) / sample_rate_hz as u64
}

/// A trivially small bounded ring used by the host when it needs to enqueue
/// live PCM between the audio thread and the runner thread. Drop-oldest.
#[derive(Debug, Clone)]
pub struct PcmRing {
    buf: VecDeque<i16>,
    cap: usize,
}

impl PcmRing {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap.min(1 << 20)),
            cap,
        }
    }

    pub fn push(&mut self, samples: &[i16]) {
        for s in samples {
            if self.cap > 0 && self.buf.len() >= self.cap {
                self.buf.pop_front();
            }
            self.buf.push_back(*s);
        }
    }

    pub fn drain(&mut self) -> Vec<i16> {
        self.buf.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn cap(&self) -> usize {
        self.cap
    }
}
