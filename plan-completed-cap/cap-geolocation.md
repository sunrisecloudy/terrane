# Capability: `geo` — location as a permissioned recorded read

New crate `rust/crates/terrane-cap-geo/`, namespace `geo`, registered in
`default_registry`. A location fix is an observation of the world, exactly like
an HTTP response: the edge acquires it once, the log records it, replay folds
the recorded fix and never touches a location service again — the `net.fetch`
pattern applied to CoreLocation/browser geolocation.

## Locked decision

**Round before record, per grant tier.** Every `geo` grant carries a precision
tier — `exact` or `coarse` (~1 km) — chosen at approval time. Coarse rounding
happens **at the edge, before the event is written**: the log physically never
contains more precision than the user granted (the same "redact on record"
posture as `cap-net-v2.md` headers). Downgrading later can't leak
already-recorded exact fixes because they were never recorded.

## Capability surface

| Surface | Name | Decision / semantics |
| --- | --- | --- |
| Command | `geo.locate` | `app` → `Decision::Effect(Effect::GeoLocate { app, precision })` — **recorded** as `geo.observed` |
| Resource (call) | `geo.current()` | routes to `geo.locate` for the calling app; `resource_call_output` returns the fix JSON from the committed event |
| Resource (call) | `geo.peek()` | same effect wrapped in `Decision::TransientEffect` — display-only ("show a map centered here"), never recorded |
| Resource (read) | `geo.last()` | newest folded fix for this app (pure state read, no hardware) |
| Query | `geo.supports` | `true`/`false` from the host's observed platform (the `native.supports` pattern) — lets apps hide the button on the CLI host |

`precision` in the effect is the granted tier, resolved by the auth layer at
dispatch time — the edge applies it mechanically.

### Events

| Kind | Payload (borsh) | Fold |
| --- | --- | --- |
| `geo.observed` | `{ app, lat_e7: i64, lon_e7: i64, accuracy_m: u32, precision: String, observed_at: u64 }` | append to `app → fixes`, keep-last 20 (deterministic truncation); newest is `geo.last()` |
| (reacts) `app.removed` | — | drop the app's fixes |

Coordinates are **integer e7 degrees** (degrees × 10⁷ ≈ 1 cm resolution) —
integers in borsh events, no float drift across platforms, matching the
integer-timings discipline in `stt`. `coarse` rounds lat/lon to the nearest
0.01° (~1.1 km) and floors `accuracy_m` to ≥ 1000. `observed_at` is edge
wall-clock ms (a fact about the observation, recorded like `net`'s response —
never re-read on replay).

## Edge sources

| Host | Source |
| --- | --- |
| mac app | CoreLocation one-shot `requestLocation()`; host carries `NSLocationUsageDescription`; macOS TCC location prompt stacks on the Terrane grant (both required) |
| web shell | `navigator.geolocation.getCurrentPosition` **in the shell** (browser permission prompt is the OS layer), fix delivered over the shell bridge |
| CLI | typed `Unsupported` error at the edge; `geo.supports` answers `false` so well-behaved apps never hit it |

Timeout 15 s at the edge; a failed/denied/timed-out acquisition is a typed
error surfaced to the caller — **no event** (there is no observation to
record; failure-as-fact is a Decision below).

## Replay story

Replay folds `geo.observed` payloads verbatim — no CoreLocation, no browser
API, no permission prompts fire during replay. `geo.peek` leaves no trace by
design. This is byte-for-byte the `net.fetched` replay contract.

## Security & permissions

- Grant resource: `geo` namespace-v1, verbs `call` + `read`. The grant
  carries the precision tier (**exact | coarse**); prompt wording is explicit
  and honest: *"<app> wants your location — Exact position / Approximate
  (within ~1 km)"*. Default-deny; elicited through the existing in-session
  shell permission flow.
- **No background tracking.** Every fix is app-initiated, one-shot, and
  recorded (or transient-by-name via `peek`). Continuous watch is a non-goal
  (below).
- `describe()` for `geo.observed` prints precision tier and accuracy only —
  never coordinates (event dumps and MCP listings shouldn't be a location
  diary).

## Limits

- Keep-last 20 fixes per app in state (the log retains history until
  compaction, as everywhere).
- Rate: ≤ 1 recorded fix per app per 10 s, enforced in decide against the
  newest folded `observed_at` (typed error naming the window).

## Implementation plan

1. **Interface:** add `Effect::GeoLocate { app, precision }` to
   `terrane-cap-interface::abi`.
2. **Crate `terrane-cap-geo`:** `lib.rs` (manifest, decide for
   locate/peek routing, fold, describe, `resource_call_output`, `supports`
   query), `types.rs` (`GeoState`, e7 conversion + coarse rounding as pure
   functions), `doc.rs`, `observed_event()` constructor.
3. **Grant tier:** extend the `geo` grant spec with the precision selector;
   permission prompt UI (web + mac shells) renders the two-option wording.
4. **Edge:** `terrane-host/src/geo_edge.rs` — CoreLocation one-shot (mac),
   bridge round-trip (web shell), typed `Unsupported` (CLI); rounding applied
   here, before `observed_event()` is built.
5. **Register** in `default_registry`; `APP_API.md`
   (`ctx.resource.geo.current/peek/last` + the tier semantics).
6. **Tests:** engine `terrane-core/tests/cap/geo.rs` — rounding math (pure,
   table-driven: exact vs coarse e7 values), fold/replay identity, keep-last
   truncation, rate limit, `app.removed`; e2e `terrane-host/tests/cap/geo.rs`
   — CLI unsupported path + stubbed-edge lifecycle default-run; real
   CoreLocation `#[ignore = "requires location services + TCC consent"]`.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`

## Non-goals (v1)

Continuous watch / background tracking (if a real need appears it ties into a
future scheduler capability, with its own grant), geofencing, reverse
geocoding (that's a `net` call to a service the app chooses, under the `net`
grant — not a hidden network dependency here), heading/speed/altitude,
location *history* queries beyond keep-last (use `query` over recorded events
if ever needed).

## Decisions to confirm

- **Failure as fact** — *recommendation:* denied/timeout acquisitions are
  typed errors, not events (nothing observed ⇒ nothing recorded);
  *alternative:* record `geo.failed {app, code}` for auditability of how often
  apps *try* — adds noise, auth's request log already covers intent.
- **Coarse definition** — *recommendation:* 0.01° snap (~1.1 km), simple and
  explainable; *alternatives:* random-offset jitter within 1 km (resists
  corner-snapping inference but is non-deterministic — it would have to be
  recorded anyway, which it is; still harder to explain), or a 3-tier
  exact/~1 km/~10 km ladder.
- **`peek` in v1** — *recommendation:* ship it (map-centering shouldn't force
  a recorded fact); *alternative:* recorded-only surface, smaller v1.
