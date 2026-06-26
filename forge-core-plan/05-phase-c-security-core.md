# Phase C — Security / policy core (steps C8–C11)

**Theme:** the highest-leverage *app-visible* win — collapse the duplicated security/policy logic
into the core, because divergence here is a **correctness AND security hazard**. These steps are
**HIGH risk and replay-sensitive**, so they come after data extraction (A) and the debug-only
consolidation (B), and each one is migrated macOS-first against **conformance vectors**, gets a
**security review**, and re-exports the **public contract** before fanning out.

**Common pattern for every step in C:**
1. Implement the decision as a pure function in `forge-policy`/`forge-domain`, `wasm32`-clean.
2. Expose it as a new `handle_command` command.
3. Author conformance vectors (a matrix of inputs → expected allow/deny/normalized output).
4. Migrate macOS to **delegate the decision** to the core; keep transport in the shell.
5. Security-review the single authoritative implementation; export + verify public contract.
6. Fan out one shell per commit; cross-platform parity test against the same vectors.

---

## C8 — Private-IP detection + network-policy matching → `forge-policy`

**The worst duplication in the repo: ~3,069 LOC of security-critical logic copied 5×.**

**Moves (decision):** `isPrivateNetworkHost` (IPv4 `10/172.16-31/192.168/127/169.254/100.64-127`;
IPv6 `::1`, `fc/fd/fe8-feb`, `::ffff:` mapped) and `NetworkPolicyRule.matchesTarget` (origin parse,
method/path match, header normalization, credential check, `maxRequestBytes`, timeout).
- macOS: `PlatformNetwork.swift:281-356` (private IP), policy match across `:1-357`.
- Siblings: iOS `PlatformNetwork.swift`, Windows `PlatformNetwork.cpp` (762 LOC), Linux
  `platform_network.c` (715 LOC), Android `PlatformNetwork.kt` (470 LOC).

**Stays (transport):** `URLSession` / WinHTTP / Soup / `HttpClient` actually performing the fetch.

**New command:** `bridge.validate_network_request` (or gate at the runtime `ctx.net.fetch`
boundary). Input: target URL + the applet's network policy. Output: allow/deny + reason.

**Validation:** `cargo test -p forge-policy` against an IP/policy conformance matrix; security review
of the one implementation; export/verify contract (app-visible network gate); per-shell parity test.

**Risk:** high. **App-visible:** yes. **Effort:** L.

---

## C9 — Manifest parsing → `forge-domain` (+ `applet.get_manifest` / `get_permissions`)

**Goal:** stop every shell re-reading and re-parsing `manifest.json` for runtime decisions.

**Moves:** extend `forge-domain` `Manifest`/`AppletManifest` to deserialize + validate
`permissions[]`, `networkPolicy`, `resourceBudget`, `denyPrivateNetwork`. Add commands
`applet.get_manifest` and `applet.get_permissions` returning the **trusted, installed** manifest so
shells stop reading from disk / `app_versions`.
- macOS `AppSandboxContext` (lives in `WebBridge.swift:546-600`); Linux `app_sandbox.c`; siblings in
  iOS/Windows/Android `WebBridge`/`NativeBridge`.

**Stays:** the shell deserializes the core's trusted result into its native `AppSandboxContext`
struct.

**New commands:** `applet.get_manifest`, `applet.get_permissions`.

**Validation:** `cargo test -p forge-domain` (manifest parse/validate vectors); export/verify
contract; per-shell test that `AppSandboxContext` built from the core matches the old direct-parse.

**Risk:** high. **App-visible:** yes. **Effort:** L. **Depends on:** A3 (enums).

---

## C10 — Bridge envelope validation + permission routing + rate-budget → core

**~2,825 LOC of bridge dispatch/validation + 150 LOC rate-budget + 80 LOC storage-prefix, copied 5×.**

**Moves (decision):**
- Envelope field validation (`isFiniteJSONNumber`, field whitelists, `request_id`/`method`/`params`
  extraction) — `WebBridge.swift:22-183, 289-303`; siblings iOS/Win/Lin/Android.
- `permissionForBridgeMethod` map (`storage.read/write`, `network.request`, `dialog.openFile`, …) —
  `WebBridge.swift:456-467`.
- Rate-budget windows (`maxBridgeCallsPerMinute`/`maxNetworkRequestsPerMinute`/`maxLogLinesPerMinute`)
  — `WebBridge.swift:214-251`; `appLog` budget `:194-211`.
- Storage key-prefix (`appId:`) enforcement — `PlatformStorage.swift:168-175` + siblings.

**Stays (transport):** receiving `WKScriptMessage`, posting the reply, and the `COUNT(*)` SQL (the
shell supplies counts to the core, or the core reads via the Store).

**New command(s):** `bridge.validate_envelope` (+ a quota extension returning rate counts /
`quarantine_eligible`).

**Validation:** `cargo test -p forge-core + forge-policy` (envelope/permission/budget vectors);
security review; export/verify contract; native host parity test on accept/reject of crafted
envelopes; replay-determinism check (bridge-call recording unchanged).

**Risk:** high. **App-visible:** yes. **Effort:** L. **Depends on:** C9 (manifest/permissions).

---

## C11 — Unify bridge-call / core-event / session recording (close the replay gap)

**Goal:** make the audit/recording schema and IDs core-owned, and capture the crash-recovery
decision inputs so replay is deterministic.

**Moves:** `recordBridgeCall` / `recordCoreStep` / `recordCoreAction` / `ensureRuntimeSession`
(`WebBridge.swift:332-415`) and crash recovery (`RuntimeCrashRecovery.swift:21-84`) → `forge-runtime`
via the FFI boundary, taking platform-id + target-id as parameters (replacing the per-platform raw
INSERTs and `bridge_macos_`/`bridge_ios_` ID prefixes). Capture `canAutoRemount` / `reloadOffered`
into the run record so replay is deterministic.

**Stays:** the schema lives in the shared migrations (A5); the shell still owns when a
`WKScriptMessage` arrives.

**Validation:** `cargo test -p forge-runtime` (record + replay determinism vectors **including**
crash-recovery inputs); replay-identical gate; per-shell parity that recorded rows match the old
schema/IDs.

**Risk:** high. **App-visible:** no (audit schema) but replay-sensitive. **Effort:** L.
**Depends on:** A5 (schema) + C10.

---

## Phase C exit criteria

- One authoritative implementation of: private-IP/network-policy, manifest parsing, bridge
  envelope/permission/rate-budget, and call/session recording — all in the core.
- Conformance vectors for each, green; security review passed for the network and bridge gates.
- Public contract re-exported + verified; Premium pin refreshed intentionally (see [10](10-validation-and-sequencing.md)).
- Replay-identical gate green (recording + crash-recovery captured).
- The shells' `PlatformNetwork`/`WebBridge`/`PlatformStorage` reduced to transport + delegation.
