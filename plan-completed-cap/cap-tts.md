# Capability: `tts` — text-to-speech, the mirror of `stt`

New crate `rust/crates/terrane-cap-tts/`, namespace `tts`, registered in
`default_registry`. `stt` brings speech *in* as recorded transcript facts with
all audio machinery at the edge; `tts` sends speech *out* with the same
discipline: synthesis always runs at the edge, and the log records either
nothing (playback) or a blob fact (render). **Depends on `cap-blob.md`** for
the render path.

## Locked decision

**Playback is transient; render is recorded.** The split is explicit and is
the whole design:

- `tts.speak` — *"say this out loud now"* — is
  `Decision::TransientEffect(Effect::TtsSpeak)`. Live-only, never an event.
  Speech that was played has **no replay value**: replaying a log must never
  make a machine start talking, and recording "we played audio" would be a
  fact about a moment, not about state.
- `tts.render` — *"give me this speech as audio bytes"* — is
  `Decision::Effect(Effect::TtsRender)`. The edge synthesizes, writes bytes to
  the blob CAS, and records `tts.rendered {app, text_hash, voice, rate_milli,
  blob_hash, size, mime, duration_ms}` plus a `blob.stored` named
  `__tts__/<text_hash>` (the `cap-net-v2.md` offload pattern). Replay folds
  the fact and never re-synthesizes — **synthesizers are not bit-stable across
  OS versions**, which is exactly why the output hash is recorded (same
  rationale as `cap-media.md`).

## Capability surface

### Commands / resources

| Surface | Name | Args | Decision |
| --- | --- | --- | --- |
| Resource (call) | `tts.speak` | `text, voice?, rate?` | `TransientEffect(TtsSpeak { app, text, voice, rate_milli })` — never recorded |
| Command | `tts.render` | `app, text, voice?, rate?` | `Effect(TtsRender { … })` — recorded |
| Resource (read) | `tts.voices` | — | transient edge read: JSON `[{id, name, lang, kind}]` from the host synthesizer; never recorded |
| Resource (read) | `tts.renders` | — | this app's folded `tts.rendered` records (pure state read) |

`rate` is a speaking-rate multiplier recorded as integer thousandths
(`rate_milli`, 500–2000, default 1000) — integers in events, same convention
as `stt`'s `confidence_milli`. `text_hash` is SHA-256 of the exact text, so a
re-render of identical text/voice/rate is a state-level overwrite, not a
duplicate name.

### Events

| Kind | Payload (borsh) | Fold |
| --- | --- | --- |
| `tts.rendered` | `{ app, text_hash, voice, rate_milli, blob_hash, size, mime, duration_ms }` | upsert `app → text_hash → RenderRecord`; keep-last 100 per app (deterministic truncation) |
| (reacts) `app.removed` | — | drop the app's render records |

## Edge synthesis backends

| Host | `speak` | `render` |
| --- | --- | --- |
| mac app | `AVSpeechSynthesizer` | `AVSpeechSynthesizer.write(_:toBufferCallback:)` → WAV/CAF → CAS |
| CLI (macOS) | `/usr/bin/say` | `say -o out.wav --data-format=LEI16` → CAS |
| web shell | Web Speech API (`speechSynthesis`) in the shell | **unsupported** — Web Speech has no capture-to-bytes; typed `Unsupported` error naming the mac/CLI hosts |
| non-mac CLI | typed `Unsupported` error (probe below) | same |

Support is discoverable the `native.supports` way: a `tts.supports` query
answering per-verb from the host's observed platform, so apps can hide the
button instead of hitting the error.

## Replay story

`tts.speak` leaves no trace by design. `tts.rendered` folds metadata; the CAS
holds the audio by hash (missing after partial sync ⇒ typed `BlobMissing` on
read — the `cap-blob.md` contract). Replay makes no sound and spawns no
synthesizer. `duration_ms` is recorded because it's a fact about the produced
artifact, not recomputable without decoding.

## Security & permissions

- Grant resource: `tts` namespace-v1, verbs `call` + `read`, description
  "speak text aloud and render speech audio". Low sensitivity, but audible
  playback is user-facing annoyance surface — still default-deny, prompt
  wording: *"<app> wants to speak text aloud on this device"*.
- Reading rendered audio bytes flows through blob reads → also needs the
  `blob` grant (same layering as net v2 body offload).
- `describe()` for `tts.rendered` prints voice + duration + text-hash prefix,
  never the text.

## Limits

- Text ≤ 32 KiB per call; render output ≤ 64 MiB (blob cap).
- One concurrent `speak` per app at the edge; a new `speak` interrupts the
  previous one (documented, matches every OS synthesizer's default).

## Implementation plan

1. **Interface:** add `Effect::TtsSpeak { app, text, voice, rate_milli }` and
   `Effect::TtsRender { … }` to `terrane-cap-interface::abi`.
2. **Crate `terrane-cap-tts`:** `lib.rs` (manifest, decide, fold, describe,
   `resource_call_output` for speak's `{ok}` reply), `doc.rs`,
   `rendered_event()` constructor; validation pure (text size, rate range,
   voice token).
3. **Edge:** `terrane-host/src/tts_edge.rs` — `say`/AVSpeechSynthesizer
   backends behind one trait, `TtsSpeak` (fire-and-forget with interrupt) and
   `TtsRender` (synthesize → CAS write → `tts.rendered` + `blob.stored`) arms
   in `EdgeRunner::run`; typed `Unsupported` off-platform. Depends on
   `cap-blob.md` step 3.
4. **Web shell:** `speak` via `speechSynthesis` behind the bridge; `voices`
   proxied from the shell.
5. **Register** in `default_registry`; `APP_API.md` (`ctx.resource.tts` with a
   speak + render example); CLI help lines (`terrane tts render <app> …`).
6. **Tests:** engine `terrane-core/tests/cap/tts.rs` — decide shapes
   (transient vs recorded — assert the *variant*), fold/replay identity,
   truncation, `app.removed`; e2e `terrane-host/tests/cap/tts.rs` —
   validation paths default-run; real synthesis `#[ignore = "runs real macOS
   speech synthesis"]` (render a short WAV, verify CAS hash, replay identity).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`

## Non-goals (v1)

Full SSML (a plain `text` + rate/voice is v1; SSML is additive later),
streaming/low-latency incremental synthesis, word-boundary callbacks to apps,
voice cloning, audio *input* (that's `stt`), playback of arbitrary blobs
(that's a shell `<audio>` tag over `blobUrl`, no capability needed).

## Decisions to confirm

- **Local neural voices (piper)** — *recommendation:* not in v1; OS voices
  cover the need with zero model management. *Alternative:* a piper backend at
  the edge (better voices, Linux support) selected per-voice-id — the event
  format already carries `voice`, so it slots in without format change.
- **Render container** — *recommendation:* WAV (deterministic-ish, dumb,
  `cap-media.md`-readable); *alternative:* AAC/CAF from AVFoundation (smaller,
  more platform variance — hash-recording tolerates it either way).
- **`speak` as resource-only** — *recommendation:* yes, no `tts.speak`
  *command* (nothing to record ⇒ nothing for the CLI log to hold); the CLI
  still gets `terrane tts speak` as a host-side convenience that calls the
  resource path. *Alternative:* mirror both surfaces like `net.fetch`/`net.get`.
