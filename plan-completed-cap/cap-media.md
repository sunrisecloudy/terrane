# Capability: `media` — image/audio/video understanding over the blob CAS

New crate `rust/crates/terrane-cap-media/`, namespace `media`, registered in
`default_registry`. **Depends on `cap-blob.md`** — bytes live in the blob CAS;
`media` adds *understanding* (what is this blob?) and *derivation* (make me a
smaller/rotated/re-encoded one). The headline use case is **thumbnails for the
shell/UI**: an app stores a photo via `blob.put` and asks `media` for a 256 px
thumbnail the shell can serve through `window.terrane.blobUrl`.

## Locked decision

**Record the output hash, never the encoder.** `media.transform` is a
`Decision::Effect`; the edge decodes, transforms, re-encodes, writes the result
bytes into the CAS, and the recorded event carries `{source_hash, ops,
dest_name, dest_hash, dest_size, dest_mime}`. Replay folds that fact and never
re-encodes — **encoders are not bit-stable across library versions or
platforms**, so re-running the transform on replay would produce a different
hash and break replay-identity. The recorded `dest_hash` *is* the result; the
CAS is the verified-by-hash artifact store, exactly as in `cap-blob.md`.

## Capability surface

### Resource reads (transient, never recorded)

| Method (`ctx.resource.media`) | Semantics |
| --- | --- |
| `info(blobName)` | Probe the named blob's bytes at the edge, return JSON: images `{kind:"image", width, height, format}`; audio `{kind:"audio", durationMs, sampleRateHz, channels, codec}`; video `{kind:"video", width, height, durationMs, codec}` or `{kind:"video", probe:"unavailable"}`. Same live-read path as `blob.get` (LiveHost hook); typed `BlobMissing`/`UnrecognizedMedia` errors. |

Probing is pure-Rust where possible: the `image` crate for stills, `symphonia`
for audio container/codec metadata. Video metadata uses **ffprobe if present**
on the host (`PATH` lookup at the edge); graceful absence returns the
`probe:"unavailable"` shape — never an error, never a hard dependency.

### Commands

| Command | Args | Decision |
| --- | --- | --- |
| `media.transform` | `app, source_name, ops_json, dest_name` | Validate (source exists in folded blob state, ops parse, dest name valid) → `Decision::Effect(Effect::MediaTransform { app, source_hash, source_mime, ops_json, dest_name })`. |

`ops_json` is an ordered array; v1 ops (all via the `image` crate):

```jsonc
[
  { "op": "resize",    "maxWidth": 256, "maxHeight": 256 },  // aspect-preserving fit
  { "op": "crop",      "x": 0, "y": 0, "width": 100, "height": 100 },
  { "op": "rotate",    "degrees": 90 },                       // 90 | 180 | 270
  { "op": "thumbnail", "size": 256 },                         // resize+encode jpeg q80 shorthand
  { "op": "encode",    "format": "jpeg", "quality": 80 }      // jpeg | png | webp
]
```

Audio v1 gets exactly one transform: `{ "op": "transcodeAudio", "format":
"wav" }` (symphonia decode → PCM WAV encode — decode-only libraries make WAV
the honest v1 target). **Video transforms are a non-goal** (see below);
`media.transform` on a video source is a typed `UnsupportedMedia` error.

### Events

| Kind | Payload (borsh) | Fold |
| --- | --- | --- |
| `media.transformed` | `{ app, source_hash, ops_json, dest_name, dest_hash, dest_size, dest_mime }` | upsert `app → dest_name → TransformRecord`; keep-last 200 per app |
| (reacts) `app.removed` | — | drop the app's transform records |

The edge runner, after writing result bytes into the CAS, returns **two**
events: `media.transformed` plus a `blob.stored` for `dest_name` (the
`blob.link` pattern from `cap-net-v2.md`'s body offload) — so the derived
bytes are reachable through the normal blob surface (`blob.get`, `blobUrl`,
sync's blob pass) and participate in refcount GC.

## Shell/UI surface

`window.terrane.blobUrl(destName)` serves the thumbnail directly — no new
host route needed; the blob route from `cap-blob.md` covers it. Recommended
app convention: derived names under `__thumb__/<source_name>` so listings can
filter them.

## Replay story

- `media.info` — transient read, no events, nothing to replay.
- `media.transform` — replay folds `media.transformed` + `blob.stored`; the
  CAS already holds `dest_hash` (or read yields typed `BlobMissing`, same
  contract as any blob). No decoder, encoder, or ffprobe runs during replay.

## Security & permissions

- Grant resource: `media` namespace-v1, verbs `call` + `read`, described as
  "inspect and transform this app's stored media". Reading transform *output*
  additionally requires the `blob` grant (it flows through blob reads).
- `media` only ever addresses the calling app's own blob names — cross-app
  sources are a typed error (same app-scoping as `query` sources).
- `describe()` for `media.transformed` prints source hash prefix, op names
  only (no coordinates), dest name and size.

## Limits (in `doc.rs`, enforced in decide/edge)

- Source ≤ 64 MiB (the blob cap); decoded pixel budget ≤ 64 megapixels
  (decompression-bomb guard, checked at the edge before full decode).
- ≤ 8 ops per transform; output must re-encode ≤ 64 MiB.
- Transform is synchronous within the dispatch (thumbnails are fast); a job
  queue is a non-goal until a real need forces it.

## Implementation plan

1. **Interface:** add `Effect::MediaTransform { app, source_hash, source_mime,
   ops_json, dest_name }` to `terrane-cap-interface::abi`.
2. **Crate `terrane-cap-media`:** `lib.rs` (manifest, decide, fold, describe),
   `ops.rs` (ops_json parse/validate — pure, fully unit-testable), `doc.rs`,
   `transformed_event()` constructor. Deps: interface, borsh, serde_json.
3. **Edge:** `terrane-host/src/media_edge.rs` — decode/transform/encode via
   `image`, audio via `symphonia`+WAV writer, ffprobe-if-present for `info`;
   wire `Effect::MediaTransform` into `EdgeRunner::run` (CAS write, then
   `media.transformed` + `blob.stored`). `info` wired through the LiveHost
   read hook. Depends on `cap-blob.md` step 3 (CAS module).
4. **Register** in `default_registry`; `APP_API.md` documents
   `ctx.resource.media` with the thumbnail-for-shell worked example.
5. **Tests:** engine `terrane-core/tests/cap/media.rs` — ops validation,
   decide→Effect shape, fold/replay identity from hand-built events (no
   image bytes needed); e2e `terrane-host/tests/cap/media.rs` — real tiny
   PNG through put→transform→blobUrl-shaped read, hash verification,
   pixel-bomb rejection; all default-run (pure local).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`

## Non-goals (v1)

Video transforms/transcode (ffmpeg is a heavy, non-deterministic edge dep —
revisit when a real app needs it), EXIF editing/stripping beyond what
re-encode drops, streaming/progressive output, animated GIF/APNG frames,
server-side image ML (that's `local-model`'s lane).

## Decisions to confirm

- **ffprobe as optional probe** — *recommendation:* PATH-lookup, graceful
  `probe:"unavailable"`; *alternative:* pure-Rust `mp4parse` for MP4-only
  metadata (narrower but dependency-free).
- **Audio transcode scope** — *recommendation:* WAV-out only v1 (symphonia is
  decode-only); *alternatives:* add an `mp3lame`/`opus` encoder dep now, or
  drop audio transforms from v1 entirely and keep only `info`.
- **EXIF orientation** — *recommendation:* auto-apply orientation on
  transform (thumbnails look right); *alternative:* preserve bytes-faithful
  orientation and expose it in `info`.
