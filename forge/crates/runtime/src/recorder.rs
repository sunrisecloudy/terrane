//! Deterministic record/replay recorder + seeded time/random seams.
//!
//! prd-merged/01 CR-8 (deterministic mode), CR-11 (injected clock/RNG). This
//! module is the heart of replay determinism and is **target-independent** — it
//! contains no QuickJS/FFI code so it compiles on `wasm32-unknown-unknown`. The
//! engine drives it; the recorder owns the seeded seams and the ordered
//! call/response trace.
//!
//! Two modes:
//!   * [`Mode::Record`] — each host interaction appends a [`RecordedCall`] that
//!     captures the response the live bridge returned (for `time`/`random` the
//!     *seeded* value). The result is a replayable [`RunRecord`].
//!   * [`Mode::Replay`] — the same program is re-run, but the recorder *serves*
//!     the previously recorded responses and never touches the live bridge for
//!     reads/seams. If the live call sequence diverges from the recording
//!     (method/args mismatch, or an unexpected extra call) the run fails with
//!     `CoreError::RuntimeError("determinism divergence ...")`.

use forge_domain::{CoreError, RecordedCall, Result};

/// Which direction the recorder is operating in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// First execution: capture every host response into the trace.
    Record,
    /// Re-execution: serve recorded responses; diverging calls are an error.
    Replay,
}

/// A deterministic, seeded pseudo-random source (SplitMix64).
///
/// prd-merged/01 CR-11: `ctx.random.next()` must be reproducible from
/// `random_seed` alone, on any platform. SplitMix64 is tiny, has no platform
/// dependencies, and produces an identical stream from the same seed
/// everywhere — exactly what replay determinism requires.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    /// Next raw 64-bit value in the deterministic stream.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Next `f64` in `[0, 1)`, matching JS `Math.random()` semantics by using
    /// the top 53 bits (the mantissa width of an IEEE-754 double).
    pub fn next_f64(&mut self) -> f64 {
        // 53 high bits → [0, 2^53) → divide by 2^53 → [0, 1).
        let bits = self.next_u64() >> 11;
        (bits as f64) / ((1u64 << 53) as f64)
    }
}

/// The deterministic clock seam (prd-merged/01 CR-11). A *logical* clock that
/// starts at `time_start` and advances by one on every `now()` so the trace is
/// reproducible and strictly monotone. This is intentionally not wall-clock:
/// the spine is deterministic, so time is a counter, not a measurement.
#[derive(Debug, Clone)]
pub struct LogicalClock {
    current: i64,
}

impl LogicalClock {
    pub fn new(time_start: u64) -> Self {
        LogicalClock {
            current: time_start as i64,
        }
    }

    /// Read the clock and advance it by one tick.
    pub fn tick(&mut self) -> i64 {
        let v = self.current;
        self.current = self.current.saturating_add(1);
        v
    }
}

/// Records the ordered host-call trace and the seeded seams for one run.
///
/// In record mode it appends; in replay mode it validates the live call against
/// the recording and serves the recorded response. The engine asks the recorder
/// for each effect's response so seams and reads are never re-derived divergently.
#[derive(Debug)]
pub struct RunRecorder {
    mode: Mode,
    rng: SplitMix64,
    clock: LogicalClock,
    /// In record mode: the trace being built. In replay mode: the trace being
    /// consumed (read-only reference copy).
    recorded: Vec<RecordedCall>,
    /// Calls produced this run. In record mode this *is* the output trace; in
    /// replay mode it accumulates so the engine can build a fresh RunRecord that
    /// must fingerprint-match the original.
    produced: Vec<RecordedCall>,
    /// Replay cursor into `recorded`.
    cursor: usize,
}

impl RunRecorder {
    /// A recorder that captures a fresh trace (record mode).
    pub fn recording(random_seed: u64, time_start: u64) -> Self {
        RunRecorder {
            mode: Mode::Record,
            rng: SplitMix64::new(random_seed),
            clock: LogicalClock::new(time_start),
            recorded: Vec::new(),
            produced: Vec::new(),
            cursor: 0,
        }
    }

    /// A recorder that replays a previously captured trace (replay mode).
    ///
    /// The seeded seams are reconstructed from the *same* seed/start so the
    /// reproduced values match; the recorded trace is authoritative and
    /// divergence is an error.
    pub fn replaying(random_seed: u64, time_start: u64, recorded: Vec<RecordedCall>) -> Self {
        RunRecorder {
            mode: Mode::Replay,
            rng: SplitMix64::new(random_seed),
            clock: LogicalClock::new(time_start),
            recorded,
            produced: Vec::new(),
            cursor: 0,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The trace produced by this run (call order is significant).
    pub fn into_calls(self) -> Vec<RecordedCall> {
        self.produced
    }

    /// Number of calls produced so far (== next seq).
    fn next_seq(&self) -> u64 {
        self.produced.len() as u64
    }

    /// Seeded clock read for `ctx.time.now()`. Recorded as `time.now`.
    pub fn now(&mut self) -> Result<i64> {
        let value = serde_json::json!(self.clock.tick());
        let response = self.seam("time.now", serde_json::Value::Null, value)?;
        Ok(response.as_i64().unwrap_or(0))
    }

    /// Seeded RNG read for `ctx.random.next()`. Recorded as `random.next`.
    pub fn random_next(&mut self) -> Result<f64> {
        let value = serde_json::json!(self.rng.next_f64());
        let response = self.seam("random.next", serde_json::Value::Null, value)?;
        Ok(response.as_f64().unwrap_or(0.0))
    }

    /// Record (or replay) a *seam* call (`time`/`random`). In record mode the
    /// freshly seeded `value` is captured. In replay mode the recorded value is
    /// served (so a tampered recording surfaces through the seam too) after the
    /// method matches.
    fn seam(
        &mut self,
        method: &str,
        args: serde_json::Value,
        value: serde_json::Value,
    ) -> Result<serde_json::Value> {
        match self.mode {
            Mode::Record => {
                let seq = self.next_seq();
                self.produced.push(RecordedCall {
                    seq,
                    method: method.to_string(),
                    args,
                    response: value.clone(),
                });
                Ok(value)
            }
            Mode::Replay => self.consume(method, args),
        }
    }

    /// Record (or replay) an *effect/read* call (`storage.*`, `db.*`,
    /// `ui.render`, `log`). In record mode the `live` response the bridge
    /// returned is captured. In replay mode the live bridge is **not** touched —
    /// the recorded response is served — and a method/args mismatch is a
    /// determinism divergence.
    pub fn host_call(
        &mut self,
        method: &str,
        args: serde_json::Value,
        live: impl FnOnce() -> Result<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        match self.mode {
            Mode::Record => {
                let response = live()?;
                let seq = self.next_seq();
                self.produced.push(RecordedCall {
                    seq,
                    method: method.to_string(),
                    args,
                    response: response.clone(),
                });
                Ok(response)
            }
            Mode::Replay => self.consume(method, args),
        }
    }

    /// Replay-mode core: validate the live call against the recording at the
    /// cursor, serve the recorded response, and advance.
    fn consume(&mut self, method: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        let seq = self.next_seq();
        let expected = self.recorded.get(self.cursor).ok_or_else(|| {
            CoreError::RuntimeError(format!(
                "determinism divergence: extra host call #{seq} {method}({args}) not present in the recording"
            ))
        })?;
        if expected.method != method {
            return Err(CoreError::RuntimeError(format!(
                "determinism divergence at call #{seq}: recorded method {:?} but live call was {:?}",
                expected.method, method
            )));
        }
        if expected.args != args {
            return Err(CoreError::RuntimeError(format!(
                "determinism divergence at call #{seq} ({method}): recorded args {} but live args {}",
                expected.args, args
            )));
        }
        let response = expected.response.clone();
        self.cursor += 1;
        self.produced.push(RecordedCall {
            seq,
            method: method.to_string(),
            args,
            response: response.clone(),
        });
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_is_deterministic_for_a_seed() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        // Different seeds diverge.
        let mut c = SplitMix64::new(43);
        assert_ne!(SplitMix64::new(42).next_u64(), c.next_u64());
    }

    #[test]
    fn splitmix64_f64_is_in_unit_interval() {
        let mut r = SplitMix64::new(7);
        for _ in 0..10_000 {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x), "f64 random out of [0,1): {x}");
        }
    }

    #[test]
    fn logical_clock_starts_at_time_start_and_is_monotone() {
        let mut c = LogicalClock::new(1000);
        assert_eq!(c.tick(), 1000);
        assert_eq!(c.tick(), 1001);
        assert_eq!(c.tick(), 1002);
    }

    #[test]
    fn record_captures_seam_and_effect_in_order() {
        let mut r = RunRecorder::recording(42, 1000);
        let t = r.now().unwrap();
        assert_eq!(t, 1000);
        let resp = r
            .host_call("storage.set", serde_json::json!(["k", "v"]), || {
                Ok(serde_json::Value::Null)
            })
            .unwrap();
        assert_eq!(resp, serde_json::Value::Null);
        let calls = r.into_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].method, "time.now");
        assert_eq!(calls[0].seq, 0);
        assert_eq!(calls[1].method, "storage.set");
        assert_eq!(calls[1].seq, 1);
    }

    #[test]
    fn replay_serves_recorded_response_without_touching_live() {
        let recorded = vec![RecordedCall {
            seq: 0,
            method: "storage.get".into(),
            args: serde_json::json!(["k"]),
            response: serde_json::json!("recorded-value"),
        }];
        let mut r = RunRecorder::replaying(42, 1000, recorded);
        let resp = r
            .host_call("storage.get", serde_json::json!(["k"]), || {
                panic!("live bridge must NOT be called in replay mode")
            })
            .unwrap();
        assert_eq!(resp, serde_json::json!("recorded-value"));
    }

    #[test]
    fn replay_detects_method_divergence() {
        let recorded = vec![RecordedCall {
            seq: 0,
            method: "storage.get".into(),
            args: serde_json::json!(["k"]),
            response: serde_json::Value::Null,
        }];
        let mut r = RunRecorder::replaying(42, 1000, recorded);
        let err = r
            .host_call("storage.set", serde_json::json!(["k"]), || {
                Ok(serde_json::Value::Null)
            })
            .unwrap_err();
        assert_eq!(err.code(), "RuntimeError");
        assert!(err.to_string().contains("divergence"), "{err}");
    }

    #[test]
    fn replay_detects_args_divergence() {
        let recorded = vec![RecordedCall {
            seq: 0,
            method: "storage.get".into(),
            args: serde_json::json!(["k"]),
            response: serde_json::Value::Null,
        }];
        let mut r = RunRecorder::replaying(42, 1000, recorded);
        let err = r
            .host_call("storage.get", serde_json::json!(["other"]), || {
                Ok(serde_json::Value::Null)
            })
            .unwrap_err();
        assert_eq!(err.code(), "RuntimeError");
    }

    #[test]
    fn replay_detects_extra_call_past_end_of_recording() {
        let mut r = RunRecorder::replaying(42, 1000, vec![]);
        let err = r.now().unwrap_err();
        assert_eq!(err.code(), "RuntimeError");
        assert!(err.to_string().contains("extra host call"), "{err}");
    }
}
