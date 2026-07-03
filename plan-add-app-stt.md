# Ambient Speech-to-Text for Terrane — Implementation Plan & Log

> **Scope note:** the request said "text to speech," but every referenced app
> (MacWhisper, Handy, OpenWhispr, Whispering, VoiceInk, WhisperDesk,
> whisper.cpp) is **speech-to-text (STT)**. This plan is for STT: always-listening
> capture → rolling on-device transcript → user highlights a slice afterward and
> picks where it goes. If TTS (type text → speak audio) is actually wanted, this
> needs a redesign.
>
> This is a **living document**. Sections 1–4 + 6 are the verified design;
> section 0 is the implementation status (what shipped, with commits); section 5
> is the phase plan updated to reality; section 7 records every decision that
> diverged from the original design; section 8 is the precise handoff for the
> remaining phases.

This plan was produced from a research pass over the codebase and verified
against real code — `Effect` is a closed enum (`terrane-host` `abi.rs:142`), and
runtime resource writes refuse `Decision::Effect | Decision::Runtime`
(`terrane-core/src/lib.rs:731`). That constraint forces the architecture below.

---

## 0. Implementation status (living)

| Phase | Status | Commit | What landed |
|---|---|---|---|
| **1** — capability + scribe app | ✅ DONE | `a3abd38f` | `terrane-cap-stt` crate (5 events, fold, decide, describe, `Eq` state), wired into `terrane-core`, `apps/scribe`, 13 engine tests, `docs/APP_API.md` |
| **2a** — CLI host-edge verbs | ✅ DONE | `081315e4` | `terrane stt open/append/close/trim` + real `apps/scribe` binary e2e (Option-A replay) |
| **2b** — session runner | ✅ DONE | `0c67d2c6` | `SttSessionRunner` (`stt_runner.rs`): energy VAD w/ hysteresis, monotonic seq, audio-clock timing, drop-oldest `PcmRing`, pluggable `AsrEngine`/`SegmentSink`; 10 tests |
| — public-authz inventory | ✅ DONE | `a470e978` | `stt.*` commands classified (select/stop grant-gated; 4 trusted verbs refused) |
| **2c** — web capture transport | ⏳ PENDING | — | shell `getUserMedia`/AudioWorklet + WebSocket ingest + admin append route |
| **3** — real ASR (whisper.cpp) | ⏳ PENDING | — | `terrane-asr` w/ `whisper` feature (whisper-rs), `AsrEngine` impl, `#[ignore]` weights test |
| **4** — macOS native mic | ⏳ PENDING | — | `AVAudioEngine` + `terrane_stt_push_pcm` C ABI + bridging header |
| **5** — consent + always-on UX | ⏳ PENDING | — | host consent dialog, LISTENING indicator, idle auto-close, sinks |
| **6** — deferred backends | ⏳ DEFERRED | — | Parakeet/MLX, `stt.pull`, log compaction/TTL |

**Quality bar held:** every shipped phase is green on
`cargo test --workspace --locked` and `cargo clippy --workspace --all-targets --locked -- -D warnings`.
No mocks in shipped tests — the runner's deterministic tests use a clearly-labeled
`FixedEngine` test fixture (the runner's framing/VAD/sequencing logic is the system
under test); the real whisper engine gets the `#[ignore]` effectful test in Phase 3.

**Workflow gotcha (read before building):** the repo's shared cargo cache
(`~/Library/Caches/terrane/cargo-target/all`) gets **corrupted by concurrent
main-repo builds** when this worktree builds into it (symptom: phantom fields like
`embed_preset`/`embedding` from the main repo's divergent source leak into this
worktree's compile). This worktree builds isolated under
`CARGO_TARGET_DIR=~/Library/Caches/terrane/cargo-target/eager-grothendieck-5eba20`.
**Use that isolated dir for all cargo commands in the remaining phases.**

---

## The one architectural decision everything hangs on

`terrane-cap-local-model` runs a model via `Decision::Effect` — a **one-shot**
call inside the synchronous `decide→commit→fold` loop. Ambient STT cannot work
that way: it is a **continuous producer**, and `terrane-core/src/lib.rs:731`
forbids effect-driven resource writes anyway. So:

```
                          ┌───────────── HOST EDGE (effects, never replayed) ──────────────┐
  mic ──▶ getUserMedia /  │  bounded PCM ring ──▶ VAD ──▶ whisper.cpp ──▶ finalized segment │
          AVAudioEngine   │  (raw audio is EPHEMERAL — never logged, never an event)         │
                          └───────────────────────────────┬────────────────────────────────┘
                                                           │ trusted dispatch (host-only)
                                                           ▼
   CORE (deterministic, replayable):  stt.segment.appended ──▶ fold ──▶ SttState ──▶ (persisted log)
                                                           ▲                    │
   app JS ── ctx.resource.stt.select() ── stt.selection.made ┘        ctx.resource.stt.segments() (free read)
                                                           │
                                                    sink: clipboard / field / app / note
```

**Mic capture, VAD, and ASR inference live entirely at the edge. Only finalized
transcript _text_ (plus integer timings) crosses into the core, as ordinary
events.** Replay never re-runs the mic or whisper — it only folds the recorded
segments (Option A, exactly how `local-model.responded` caches its result). Raw
audio is never an event, which sidesteps the blob-ref blocker `review-025`
flagged for mic/camera.

> ✅ **Implemented as designed in Phase 1/2b.** No `Decision::Effect` variant was
> added for STT (correct — the edge dispatches `stt.segment.append` directly).

---

## 1. The capability — `terrane-cap-stt` (namespace `stt`) ✅

Owns the *transcript*, not the audio. All state is `BTreeMap` + integer fields so
`SttState` derives `Eq` and `replay_matches()` compares exactly (the
`temperature_milli` integer-scaling trick from `terrane-cap-local-model`).

### Events (`{kind, payload}`) — as shipped

| Event | Payload | Who emits |
|---|---|---|
| `stt.session.opened` | `app, session_id, host_id, executor_host_id, origin_replica?, model, sample_rate_hz` | host (after consent) |
| `stt.segment.appended` | `app, session_id, segment_seq, start_ms, end_ms, text, confidence_milli?, lang?` | host (per VAD-final utterance) |
| `stt.session.closed` | `app, session_id, reason` (`stopped`/`idle`/`revoked`/`host-exit`/`error:…`) | host or app |
| `stt.selection.made` | `app, session_id, selection_id, from_segment_seq, to_segment_seq, text, sink` | app (`stt.select`) |
| `stt.retention.trimmed` | `app, session_id, dropped_before_seq` | host (bounds live state) |

> 🔧 **Deviation:** `opened_seq` / `next_open_seq` from the original draft were
> **dropped** as unused — `segment_seq` is the only monotonic id that matters and
> it is edge-minted. See §7.

Replay-safety details (all shipped + tested):

- `session_id` / `segment_seq` / timings all arrive **inside** events, never
  minted in `decide`.
- `start_ms` / `end_ms` are offsets from session-open (no wall clock in core).
- `segment_seq` is monotonic with **first-wins idempotence** so retries / future
  LAN sync converge; duplicate/out-of-order ⇒ no-op fold.
- `executor_host_id` + `origin_replica` carried from v1 so sync won't double-capture.

### `decide` (two classes, same pattern as `terrane-cap-native`) — as shipped

**App-callable** (from `ctx.resource.stt`, default-deny):

- `stt.select <app> <sid> <from> <to> <sink>` — validate session exists+owned,
  `from <= to`, both in range against **folded** segments; **re-derive the text
  by concatenating the folded segments `[from..=to]`** (app-supplied text is
  impossible to supply — there is no text arg — so the record is authoritative);
  mint `selection_id` as a deterministic hash and commit `stt.selection.made`.
- `stt.stop <app> <sid>` — validate session exists; commit `session.closed`
  reason `"stopped"`.

> 🔧 **Deviation:** the app-callable close is **`stt.stop`**, not
> `stt.session.close`. The resource-method name drives the command name
> (`ctx.resource.stt.stop(...)` → `stt.stop`), per the `local-model.ask` pattern.

**Trusted host-only** (gated in `admit_command`, refused from app JS):

- `stt.session.open`, `stt.segment.append`, `stt.session.close-host`,
  `stt.retention.trim`.

**Never returns `Decision::Effect` or `Decision::Runtime`.**

### `fold` (state) — as shipped

```
SttState { sessions: BTreeMap<AppId, BTreeMap<SessionId, SttSession>> }
SttSession { session_id, host_id, executor_host_id, origin_replica: Option<u64>,
             model, sample_rate_hz: u32, status: Open|Closed, closed_reason: Option<String>,
             segments: BTreeMap<u64 /*segment_seq*/, SttSegment>,
             last_segment_seq: u64, dropped_before_seq: u64,
             selections: BTreeMap<SelectionId, SttSelection> }
SttSegment  { segment_seq, start_ms, end_ms, text, confidence_milli: Option<u32>, lang: Option<String> }
SttSelection { selection_id, from_segment_seq, to_segment_seq, text, sink }
```

Fold rules: `session.opened` inserts (Open, first-wins); `segment.appended`
inserts only if session Open **and** `segment_seq > last_segment_seq` **and**
`>= dropped_before_seq`, then bumps `last_segment_seq` (idempotent, first-wins);
`session.closed` sets Closed (first close wins); `selection.made` inserts (first
`selection_id` wins); `retention.trimmed` sets `dropped_before_seq = max(...)` and
removes older segments. **Subscribes to `app.removed`** → drop that app's sessions
(mandatory: revoked apps must not inherit data). Unknown kinds fall through `Ok(())`.

### `describe`, effect boundary, replay-identity

As designed — see the original text. Shipped verbatim + covered by
`replay_matches()` assertions after every state-changing test.

---

## 2. The app — `apps/scribe` (plain JS, no build) ✅

> 🔧 **Deviation:** shipped `manifest.json` declares **`"resources": ["stt"]`**
> only — not `["stt","native","kv"]`. Clipboard delivery (native) and per-app
> prefs (kv) are deferred to Phase 5's sink work; v1 scribe records selections
> (the recorded fact) and the host performs clipboard delivery outside the app.
> The backend is the `actions`-table shape (like `apps/todo`), not a raw `handle`.

`apps/scribe/main.js` actions: `state`, `sessions`, `transcript`, `segments`,
`select`, `selections`, `stop`. `apps/scribe/index.html` is a three-pane UI
(LIVE / TRANSCRIPT / SELECT-USE) with shift-click range selection and Copy/Note/
Field sinks. The app holds no audio and never captures — it reads the folded
transcript and records selections.

---

## 3. Host integration (web + macOS, symmetric) — partial

> ✅ **The `SttSessionRunner` shipped (Phase 2b)** as `terrane-host/src/stt_runner.rs`
> (NOT `asr.rs` — `asr.rs` is reserved for the Phase-3 whisper engine module).
> 🔧 **VAD:** shipped a self-contained **energy-based VAD** (mean-square energy +
> hysteresis hangover) rather than Silero/WebRTC. whisper.cpp's own VAD arrives
> with Phase 3 and can replace or augment it.

### What the runner is (shipped, generic + tested)

`SttRunner<E: AsrEngine, S: SegmentSink>` owns, per session: PCM framing at the
configured `frame_ms`, the VAD, an accumulating utterance buffer, a monotonic
`next_segment_seq` (starts at 1), the audio-clock (`samples_seen` → ms offsets),
and idle tracking. `push_pcm(&[i16])` frames → VAD → on each closed utterance
calls `engine.transcribe(pcm, hz)` → `sink.append(session_id, seq, start_ms,
end_ms, AsrOutput)`. Empty recognitions are skipped without consuming a seq.
`with_ring_cap(n)` enables drop-oldest backpressure. `PcmRing` is a standalone
drop-oldest buffer for the audio-thread→worker-thread handoff.

### What's pending

- **Web (Phase 2c):** ⚠️ the app iframe is sandboxed **without**
  `allow="microphone"`/`allow-same-origin`, so in-iframe `getUserMedia` is NOT
  usable. Capture is a **shell-owned trusted control** in `app_shell.js`:
  `getUserMedia({audio:{sampleRate:16000,channelCount:1}})` → AudioWorklet →
  Int16 PCM → loopback WebSocket to a host service holding an `SttRunner` →
  finalized segments POST to an admin-authenticated route
  (`X-Terrane-Admin: local-admin`, like `/__terrane/admin/...` in `routes.rs`)
  dispatching `stt.segment.append`. Interim text over a nonce-guarded
  `terrane:bridge:progress` side-channel (never recorded). Unload/`ended` →
  `stt.session.close-host`.
- **macOS (Phase 4):** `AVAudioEngine.installTap` → new
  `terrane_stt_push_pcm(handle, session_id, ptr, len)` C ABI in `ffi.rs`
  (`catch_unwind`-wrapped, no serde) → `PcmRing` → worker-thread `SttRunner`
  (audio thread only enqueues). `TerraneBridge.swift` marshals the push.

### Consent gating (always-on mic) — four stacked layers (Phase 5)

1. **App DAC grant** (default-deny): `auth.grant user:local-owner scribe stt`.
   Shipped: `GrantResourceSpec::namespace_v1("stt", &["call","read"], …)`.
   > 🔧 **Deviation:** verbs are **`["call","read"]`**, not `["read","write"]` —
   > `select`/`stop` are `ResourceMethod::Call` (they return a value), and there
   > are no `Write` methods on the stt surface.
2. **Trusted-host boundary**: the four edge verbs are host-only (shipped in
   `admit_command` + `public_authz`).
3. **In-session elicitation** (PENDING): host-owned "listen continuously" dialog;
   approval is an admin decision.
4. **Browser/OS mic prompt** (PENDING): `getUserMedia` / macOS TCC.

Plus always-on hygiene (Phase 5): persistent host-rendered LISTENING indicator,
one-click Stop, idle auto-close (`reason="idle"` after `TERRANE_STT_IDLE_MS` —
the runner exposes `idle_ms()` already), frame nonce, `auth.revoke`/`app.removed`
drop live sessions (the fold subscription shipped).

---

## 4. ASR engine — **whisper.cpp (GGUF) via whisper-rs** (confirmed 2026-07-03; Phase 3, pending)

> **Decision (whisper vs Parakeet, 2026-07-03).** Parakeet (NVIDIA TDT 0.6B v3) is
> the stronger *model* for English ambient dictation now — more accurate + 3–10×
> faster than Whisper large-v3 for English/25 EU langs, native streaming, and (key
> for always-listening) it doesn't hallucinate during silence. **But** its clean
> on-device Mac paths are all worse *fits for Terrane*: MLX reintroduces the
> fragile resident-Python worker already flagged as risky ([[local-model-capability-merged]]);
> the ONNX/CoreML path (`transcribe-rs`) underperforms on Apple Silicon; and
> `parakeet.cpp` (pure C++/Metal, the real contender) is young/single-maintainer.
> whisper.cpp/whisper-rs uniquely matches the existing **no-Python, compile-time
> Metal, `OnceLock`+shutdown** infra (identical to the llama.cpp setup), and our
> **VAD gating already neutralizes Whisper's silence-hallucination weakness** (we
> only ever transcribe VAD-closed speech). So: ship whisper-rs first; **keep the
> `AsrEngine` trait engine-plural** so Parakeet (via `parakeet.cpp` or the
> `transcribe-rs`/ONNX path) is a drop-in fast-follow, not a rewrite. See §7 D10.

Via a new `terrane-asr` crate mirroring `terrane-local-llm`: a process-global
`OnceLock` engine cache with Metal offload (`llama.rs`'s `cached_llama`), a
mandatory Metal-buffer shutdown hook, and `terrane-local-llm/src/download.rs` for
weight pull. whisper.cpp ships its own VAD, so the edge can close utterances with
no extra dependency (the shipped energy VAD can stay as a first-stage gate).

> **Recommended concrete path:** implement `AsrEngine` for
> [whisper-rs](https://crates.io/crates/whisper-rs) (a mature Rust binding to
> whisper.cpp) behind a `whisper` feature on `terrane-asr`. Keep the default
> build green (no feature) so CI stays fast; the feature pulls the C++ compile.
> The `SttRunner` is already generic over `AsrEngine`, so wiring is:
> `SttRunner::new(cfg, WhisperEngine::new(model_path)?, host_sink)`.

**`AsrEngine` trait (already defined in `stt_runner.rs`):**
```rust
pub trait AsrEngine {
    fn transcribe(&self, pcm: &[i16], sample_rate_hz: u32) -> Result<AsrOutput>;
}
pub struct AsrOutput { pub text: String, pub confidence_milli: Option<u32>, pub lang: Option<String> }
```

**Parakeet/whisper-MLX is a deferred second backend** (Phase 6) reusing the
resident Unix-socket worker from `terrane-local-llm/src/server.rs`.

---

## 5. Phase plan (updated to reality)

Each phase ends green on `cargo test --workspace --locked` and
`cargo clippy --workspace --all-targets --locked -- -D warnings`.
Effectful tests are `#[ignore]`d with a reason (needs weights/hardware).

### ✅ Phase 1 — capability + driveable app (DONE, `a3abd38f`)
Scaffolded `terrane-cap-stt`; wired into `State`/`StateStore`/`default_registry`;
`admit_command` trusted-host gate; `apps/scribe`; 13 engine tests in
`terrane-core/tests/cap/stt.rs` (lifecycle, monotonic idempotence, select
re-derivation, retention, `app.removed`, authority gating, fold-without-inference,
JS-backend e2e). Inventory tests + `APP_API.md` updated.

### ✅ Phase 2a — CLI host-edge verbs (DONE, `081315e4`)
`terrane stt open/append/close/trim` → trusted dispatch; real `apps/scribe`
binary e2e in `terrane-host/tests/cap/host.rs` asserting Option-A replay.

### ✅ Phase 2b — session runner (DONE, `0c67d2c6`)
`SttSessionRunner` + `SttVad` + `PcmRing` + `AsrEngine`/`SegmentSink` traits;
10 deterministic tests in `terrane-host/tests/stt_runner.rs`.

### ⏳ Phase 2c — web capture transport (PENDING)
Shell-owned `getUserMedia` + AudioWorklet + loopback WebSocket → host `SttRunner`
service → admin `stt.segment.append` route. See §8 handoff.

### ⏳ Phase 3 — real ASR (PENDING)
`terrane-asr` crate, `whisper` feature (whisper-rs), `WhisperEngine: AsrEngine`,
process-global `OnceLock` + Metal offload + shutdown hook (extend
`local_llm_shutdown`). `#[ignore]` e2e on a short WAV fixture needing a model.

### ⏳ Phase 4 — macOS native mic (PENDING)
`AVAudioEngine.installTap` → `terrane_stt_push_pcm` C ABI → `PcmRing` → worker
`SttRunner`. Bridging header + `TerraneBridge.swift`. Xcode-only build.

### ⏳ Phase 5 — consent + always-on hardening (PENDING)
Host consent dialog; LISTENING indicator; one-click Stop; idle auto-close
watchdog using `SttRunner::idle_ms()`; the four sinks (clipboard via `native`,
`field` via document API, `app:<id>` relay, `note`).

### ⏳ Phase 6 — deferred until forced
Parakeet/whisper-MLX backend; `stt.pull` model management; log compaction/TTL
(see §6 — the biggest open item).

---

## 6. Open questions / risks

### Needs a user decision
- **Log growth & PII (biggest open item).** `retention.trim` bounds live *state*,
  but `stt.segment.appended` events — verbatim spoken text — persist in the durable
  *log* until compaction, which doesn't exist yet. Ship Phase 1–5 with
  retention-of-state only and accept unbounded log growth, or gate always-on use
  behind a compaction/`stt.session.purged` housekeeping event first? Recommend
  shipping without compaction and prioritizing a purge event in Phase 6.
- **Interim-vs-final discipline** is a host-side invariant not enforceable by core.
  Only VAD-closed finals may cross. Accept as a runner invariant + test (shipped:
  the runner only finalizes on `VadEdge::SpeechEnd`), or add a core-side rate/size
  guard on `stt.segment.append`?
- **Web capture transport:** WASM whisper in-browser vs. loopback WebSocket PCM →
  Rust runner. Plan assumes WebSocket for engine reuse; confirm at Phase 2c.

### Risks (mitigated)
- **In-iframe `getUserMedia` not a proven web path** — mitigated by shell-owned
  capture (Phase 2c).
- **whisper.cpp Metal residency assertion** — long-lived hosts abort on ggml
  static destructors unless the shutdown hook clears the ASR `OnceLock`; make it
  Phase 3's definition-of-done (mirror `clear_llama_cache`).
- **macOS audio-thread reentrancy** — the real-time tap must only enqueue;
  VAD/whisper on a worker (the shipped `PcmRing` is the enqueue seam).
- **Sync double-capture** — `executor_host_id`/`origin_replica` are in the payload
  from v1 (shipped), so LAN sync won't double-capture.
- **Wall-clock trap** — session-relative `start_ms`/`end_ms` only; guard with
  `replay_matches()` (shipped). Any `decide`/`fold` branch that reads `now()`
  breaks replay-identity.
- **Consent optics** — always-on is Terrane's highest-trust operation; the
  LISTENING indicator, idle auto-close, and frame nonce are load-bearing.

### Key files
- `rust/crates/terrane-cap-stt/*` ✅
- `rust/crates/terrane-host/src/stt_runner.rs` ✅ (+ `tests/stt_runner.rs`)
- `rust/crates/terrane-host/src/{cli.rs, public_authz.rs, lib.rs}` ✅ (edited)
- `rust/crates/terrane-core/src/lib.rs` ✅ (State + registry + admit_command)
- `apps/scribe/*` ✅
- `rust/crates/terrane-asr/*` (new, Phase 3)
- `rust/crates/terrane-host/src/asr.rs` (new, Phase 3 — the whisper `AsrEngine`)
- `rust/crates/terrane-host/src/ffi.rs` (`terrane_stt_push_pcm` + shutdown, Phase 4)
- `host/web/src/js/app_shell.js` + `host/web/src/routes.rs` (Phase 2c)
- `host/macos/Sources/TerraneBridge.swift` + bridging header (Phase 4)

---

## 7. Deviations from the original design (decisions made during Phase 1–2)

| # | Original draft | Shipped | Why |
|---|---|---|---|
| D1 | app close = `stt.session.close` | **`stt.stop`** | Resource-method name drives command name (`ctx.resource.stt.stop` → `stt.stop`), per `local-model.ask`. |
| D2 | `selection_id = H(app,sid,from,to,sink,**count**)` | **`H(app,sid,from,to,sink)`** (no count) | Including `count` made re-dispatch of the same selection non-idempotent (count changes after first commit). Without count, same range+sink ⇒ same id ⇒ first-wins no-op, which is the correct replay/sync semantics. |
| D3 | state carried `opened_seq`/`next_open_seq` | **dropped** | Unused; `segment_seq` is the only monotonic id and it's edge-minted. Fewer fields = simpler `Eq`. |
| D4 | grant verbs `&["read","write"]` | **`&["call","read"]`** | `select`/`stop` are `ResourceMethod::Call` (return a value); no `Write` methods exist on the stt surface. |
| D5 | scribe `resources: ["stt","native","kv"]` | **`["stt"]`** | Clipboard delivery (native) + prefs (kv) defer to Phase 5 sinks; v1 records the selection fact only. |
| D6 | VAD = Silero/WebRTC or whisper.cpp built-in | **energy-based VAD (mean-square + hysteresis)** shipped in the runner | Dependency-free, deterministic, fully unit-tested; whisper.cpp's VAD can augment in Phase 3. |
| D7 | runner in `asr.rs` | **runner in `stt_runner.rs`**; `asr.rs` reserved for the whisper engine | Separates the transport/sequencing (done) from the ASR backend (Phase 3). |
| D8 | (explore suggested) add `Effect::SttTranscribe` | **not added** | Correct — STT never uses `Decision::Effect`; the edge dispatches `stt.segment.append` directly. |
| D9 | (implied) CLI 3-segment commands `stt.session.open` | **friendly verbs `terrane stt open/append/close/trim`** | The CLI's generic arm only routes 2-segment names; mirrored the `kv.storage.set` special-route precedent. |
| D10 | Phase-3 engine open (whisper vs Parakeet/MLX) | **whisper-rs first; trait kept engine-plural, Parakeet a fast-follow** | Confirmed 2026-07-03 after a 2026 landscape check. Parakeet is the better English ambient model, but every clean-on-Mac Parakeet path is a worse *fit* (MLX=Python risk, ONNX=slow on Apple Silicon, `parakeet.cpp`=immature); whisper-rs matches existing infra and VAD gating covers Whisper's silence weakness. See §4. |

---

## 8. Precise handoff for the remaining phases

### Phase 3 — real ASR (whisper.cpp) — the next, highest-value step
1. New crate `rust/crates/terrane-asr/` (workspace member + dep, mirroring
   `terrane-local-llm`). `[features] default = []; whisper = ["whisper-rs"]` (or
   vendored whisper.cpp via `cc` if you prefer no third-party binding).
2. ⚠️ **Avoid a circular dep.** The `AsrEngine` trait lives in
   `terrane_host::stt_runner`, so `terrane-asr` must NOT try to `impl` it (that
   would force `terrane-asr → terrane-host → terrane-asr`). Mirror the working
   `terrane-local-llm` ↔ `terrane-host/src/local_llm.rs` split instead:
   - `rust/crates/terrane-asr/src/lib.rs` is a **pure engine** (no `terrane-host`
     dep): `WhisperEngine` with an *inherent* `transcribe(pcm: &[i16], hz: u32)
     -> Result<AsrOut, AsrError>` and a `clear_whisper_cache()`, exactly like
     `terrane-local-llm` exposes `cached_llama`/`clear_llama_cache`. Load model
     from `TERRANE_STT_MODEL`, cache in `OnceLock<Mutex<HashMap<PathBuf,
     Arc<Mutex<WhisperContext>>>>>` (the `llama_cache()` pattern), resample i16→
     16 kHz mono f32 (whisper.cpp's required input).
   - `rust/crates/terrane-host/src/asr.rs` holds the **bridge**: `struct
     HostWhisper(...)` `impl AsrEngine` delegating to the pure engine — the same
     shape as `local_llm.rs` wrapping `terrane-local-llm`.
3. Shutdown hook: add `asr_shutdown()` in `terrane-host/src/asr.rs` (feature-gated
   pair like `local_llm.rs`), called from `local_llm_shutdown()` in
   `terrane-host/src/lib.rs:40` and the FFI `terrane_local_model_shutdown`. **This
   is mandatory** — same Metal-residency-assertion hazard as llama.cpp.
4. Weight pull: reuse `terrane-local-llm/src/download.rs` for the GGUF.
5. Tests: a pure describe/construct test (default) + an `#[ignore]`
   `fn whisper_transcribes_short_wav()` that loads a fixture model + WAV and
   asserts non-empty text. `cargo test` stays green without weights.
6. Validation gate: `scripts/with-cargo-cache.sh` (use the **isolated**
   `CARGO_TARGET_DIR` from §0) `cargo test --workspace --locked` +
   `cargo clippy --workspace --all-targets --locked -- -D warnings`.

### Phase 4 — macOS native mic
1. `rust/crates/terrane-host/src/ffi.rs`: add
   `#[no_mangle] pub unsafe extern "C" fn terrane_stt_push_pcm(handle, session_id, ptr, len)`
   mirroring `terrane_dispatch`'s `catch_unwind` + `read_str` shape. It enqueues
   the PCM into a process-global `PcmRing` (the audio thread must only enqueue).
2. A worker thread owns the `SttRunner<WhisperEngine, HostSink>` and drains the
   ring; `HostSink` dispatches `stt.segment.append` via `dispatch_on_core`.
3. `host/macos/Sources/TerraneHost-Bridging-Header.h`: declare
   `terrane_stt_push_pcm` + `terrane_stt_shutdown`.
4. `TerraneBridge.swift`: `AVAudioEngine`, `installTap(on:)` →
   `terrane_stt_push_pcm` with Int16 PCM. Request TCC mic access
   (`AVCaptureDevice.requestAccess(for:.audio)`).
5. Build: Xcode only; no cargo validation here. Document the manual test.

### Phase 2c — web capture transport
1. `host/web/src/routes.rs`: add `POST /__terrane/admin/stt/segment` (admin token
   like the approve routes) → `dispatch_on_core("stt.segment.append", args)`. Add
   a WebSocket upgrade endpoint `/__terrane/stt/pcm` that feeds a host-held
   `SttRunner`.
2. `host/web/src/js/app_shell.js`: shell-owned "Enable microphone" control
   (outside the sandboxed iframe, frame-nonce guarded) → `getUserMedia` →
   AudioWorklet `process()` → Int16 → WS. Interim text via
   `terrane:bridge:progress` (never recorded). Unload → `stt.session.close-host`.
3. Test the Rust route with the existing HTTP test harness (`host/web/tests`);
   the AudioWorklet JS is browser-only (manual test).

### Phase 5 — consent + always-on hardening
1. Host-owned first-run consent dialog (extended timeout, admin approve/deny).
2. Persistent host-rendered LISTENING indicator (web shell + macOS top bar).
3. Idle auto-close watchdog polling `SttRunner::idle_ms()` →
   `stt.session.close-host reason="idle"` over `TERRANE_STT_IDLE_MS`.
4. The four sinks: `clipboard` via `native.clipboardWriteText`; `field` via the
   document API; `app:<id>` relay via `invoke`; `note` saved (kv).

### Build/cache reminder for every remaining phase
Use the **isolated** target dir (§0): the shared cache is corrupted by concurrent
main-repo builds. If you see phantom fields/errors referencing identifiers that
don't exist in this worktree's source, that's the cache — touch the affected
sources or rebuild under the isolated dir.
