# Capability: `capture` — camera photos and raw audio, as native operations

**Not a new crate.** Camera and microphone capture are new operations in the
existing `native` capability's operations registry
(`rust/crates/terrane-cap-native/src/operations/`), reusing its async
`native.requested` → `native.completed`/`failed`/`cancelled` lifecycle and its
`native.supports` probe unchanged. `stt` (see `terrane-cap-stt`) records
*transcripts only* and never audio; this is the complement — it captures the
**media bytes**, which land in the blob CAS (**depends on `cap-blob.md`**).
Downstream, `cap-media.md` gives the captured bytes thumbnails and metadata.

## Locked decision

**Bytes to CAS, hash in the event.** A capture's result never rides in an
event. The executing host writes the captured bytes into the blob CAS, then
dispatches `native.complete` with `result_json = {hash, size, mime, …}` and the
edge also emits a `blob.stored` naming `__capture__/<request_id>` (the
`blob.link` pattern from `cap-net-v2.md`), so captured media is reachable via
the normal blob surface and participates in refcount GC. Replay folds the
request/completion facts and the blob metadata — no camera, no microphone, no
bytes re-acquired. This matches the catalog's existing `blob-ref` result-size
class (already stubbed for `screen.capture` in `operations/desktop.rs`).

## New operations (group `common`)

| Operation id | Input (`input_json`) | Completion `result_json` | Result size | Safety / policy |
| --- | --- | --- | --- | --- |
| `camera.capturePhoto` | `{ "facing": "user"\|"environment"?, "maxWidth"?: int }` | `{ "hash", "size", "mime": "image/jpeg", "width", "height", "blobName": "__capture__/<request_id>" }` | `blob-ref` | `sensitive`, grant-gated |
| `audio.record` | `{ "maxDurationMs": int (≤ 300_000), "sampleRateHz"?: int }` | `{ "hash", "size", "mime": "audio/wav", "durationMs", "sampleRateHz", "blobName": "__capture__/<request_id>" }` | `blob-ref` | `sensitive`, grant-gated |

Both follow the existing request shape exactly: app-callable command
`native.camera.capture-photo <app> <request_id> <input_json>` (and resource
method `cameraCapturePhoto`), committed as `native.requested`; the executing
host performs the capture and calls `native.complete`/`native.fail`. Failure
payloads use the existing `error_json` slot (`{"code": "denied" | "no-device"
| "timeout" | "cancelled-by-user", "message": …}`). `native.cancel` covers
user abort mid-recording.

**Screen capture is NOT here.** `screen.capture` stays in the `desktop` group
where the catalog already stubs it — promoted to v1 in `cap-native-v2.md`.
Cameras/mics are `common` (phones have them); screens are desktop chrome.

## Per-host implementation

| Host | `camera.capturePhoto` | `audio.record` |
| --- | --- | --- |
| mac app | AVFoundation (`AVCaptureSession` still photo; `AVAudioRecorder` → WAV). Host binary must carry `NSCameraUsageDescription` / `NSMicrophoneUsageDescription`. | same session machinery |
| web shell | `getUserMedia` in the **shell** (not the app iframe — the app never touches the stream), canvas → JPEG / MediaRecorder → WAV via shell bridge upload | same |
| CLI | not in its `native.platform.observe` supported-operations list → `decide` rejects before any effect; `native.supports("camera.capturePhoto")` → `false` | same |

## Permission layering (both gates required)

1. **Terrane grant** — default-deny, elicited in-session through the existing
   shell permission flow. Prompt wording must name the hardware: *"<app> wants
   to take photos with your camera"* / *"…record audio from your microphone"*.
2. **OS consent** — macOS TCC camera/mic prompts (or the browser's
   `getUserMedia` permission) stack **on top**; an OS denial completes the
   request as `native.fail {code:"denied"}` — a recorded fact, not a hang.

A Terrane grant never bypasses TCC and TCC never bypasses the grant. Recording
UX rule for `audio.record`: the executing host must show a visible recording
indicator (shell banner / mac menu-bar dot) for the whole duration.

## Replay story

Replay folds `native.requested` → `native.completed` (hash/size/mime metadata)
plus the `blob.stored` link. The CAS holds the bytes by hash (or a read yields
typed `BlobMissing` after a partial sync — the `cap-blob.md` contract). The
existing keep-last-100 terminal retention in `NativeState` applies unchanged;
the *blob* survives retention because `blob.stored` owns its lifecycle.

## Limits

- Photo: longest edge clamped to 4096 px at the edge (re-encode), JPEG q85.
- Audio: `maxDurationMs` ≤ 5 min v1; WAV 16-bit mono default (16 kHz default
  sample rate, matching `stt`'s `DEFAULT_SAMPLE_RATE_HZ`).
- Output must fit the blob cap (64 MiB) — enforced at the edge before
  `native.complete`, else `native.fail {code:"too-large"}`.

## Implementation plan

1. **Catalog:** add `camera.capturePhoto` + `audio.record` entries to
   `operations/common.rs` (`status:"v1"`, `safety:"sensitive"`,
   `result_size:"blob-ref"`); new command/resource constants, `input_json`
   validation arms in `commands.rs`, `result_size_for_operation` arms.
2. **Grant wording:** extend the native grant description; per-operation grant
   selector is a Decision below.
3. **Mac host:** AVFoundation capture module behind the existing native
   request pump; CAS write + `blob.stored` + `native.complete`.
4. **Web shell:** shell-side `getUserMedia` flow + bridge upload → CAS →
   complete. Depends on `cap-blob.md` steps 3/5.
5. **Docs:** `APP_API.md` capture section; `doc.rs` catalog summaries.
6. **Tests:** engine `terrane-core/tests/cap/native.rs` extension — request/
   complete fold with blob-ref result, unsupported-host rejection, replay
   identity; e2e `terrane-host/tests/cap/native.rs` — full lifecycle with a
   stubbed executor writing fixture bytes to the CAS (default-run); real
   camera/mic e2e `#[ignore = "requires hardware + TCC consent"]`.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`

## Non-goals (v1)

Video recording (bytes and duration explode; needs `cap-media.md` video story
first), continuous camera streams / preview frames to apps, multi-shot burst,
camera settings control (flash, zoom), screen capture (→ `cap-native-v2.md`),
transcription (that is `stt`, which stays transcript-only).

## Decisions to confirm

- **Operation-level grant vs namespace grant** — *recommendation:* keep the
  `native` namespace grant but make camera/mic the first users of an
  operation-level selector (`native:camera.capturePhoto`), since the catalog
  already reserves `refuse-until-selector` policy machinery; *alternative:*
  ship v1 on the coarse namespace grant with loud prompt wording, add
  selectors in native-v3.
- **Audio container** — *recommendation:* WAV (dumb, lossless, plays
  everywhere, `cap-media.md` can read it); *alternative:* AAC/Opus at the
  edge for 10× smaller blobs, at the cost of platform-encoder variance.
- **Preview before commit** — *recommendation:* none in v1 (host shows its own
  capture UI; the request completes with the final bytes); *alternative:* a
  two-step confirm flow via a second request.
