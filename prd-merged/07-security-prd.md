# PRD 07 — Security & Privacy

**Status:** Merged draft v1 · **Applies to:** all components · GA-gating document
**Sources:** F-07 (threat model, sandbox guarantees, secrets, supply chain, assurance) + P-09 (RBAC, capability grammar, network rules, audit list, iOS posture) — the packs agree almost entirely here; this is a union.

## 1. Threat model (in scope)

| # | Adversary | Vector | Primary mitigations |
|---|---|---|---|
| T1 | Malicious/buggy applet code (LLM or human) | Sandbox escape; resource exhaustion; data/secret exfiltration | §2, §4, §5 |
| T2 | Prompt injection | Adversarial synced content steers generation toward hostile code or permission grabs | §3 |
| T3 | Malicious collaborator | Hostile applets/data in shared workspace; role abuse | §4, SS-7..9 |
| T4 | Network attacker | MITM sync; LAN spoofing of embedded server | SS-21, SC-13 |
| T5 | Cloud compromise / insider | Server-readable data exposure | §8 |
| T6 | Stolen/lost device | Local data exposure | §9 |
| T7 | Marketplace package abuse | Hostile published packages | §7, PRD 08 |
Out of scope v1: nation-state targeted attacks; compromised OS; default E2E encryption (explicit decision; project-level keys supported, DL-25).

## 2. Sandbox guarantees (T1) — cross-ref CR-1..5, CR-13

- **SC-1** Zero ambient capability; every effect passes a capability check at call time. `eval`/`Function`/dynamic import/prototype-pollution-of-bridge blocked at engine level **and** by static policy scan (LM-9) — two independent layers.
- **SC-2** Resource exhaustion containment-tested by a maintained hostile-applet corpus; regressions block release. Limits per CR-5 + manifest limits.
- **SC-3** Realm isolation; no shared mutable state; inter-applet communication only via mutually-granted collections.
- **SC-4** Engine CVE response (QuickJS/rquickjs, JSC, wasm runtime): 7-day patch-to-release SLA for sandbox-relevant CVEs, 48 h if actively exploited.
- **SC-5** Network egress: manifest domain allowlist enforced in the host `net` layer — scheme/hostname/path/method/headers/body size/response size/content type/timeout validated; DNS pinning; redirects re-checked; **localhost and private-network access blocked by default** (explicit grant required; SSRF-hardened in cloud components too) (P-09).

## 3. Prompt injection (T2) — cross-ref LM-14..16

- **SC-6** Invariant: **generation can propose; only human review can grant.** No pipeline path installs code or expands permissions without review (per the workspace's AI-mode policy); server LLM jobs run at requester's scope.
- **SC-7** Injection corpus is a living suite; new in-the-wild patterns get cases within one release.

## 4. Permission system: capabilities + RBAC (T1, T3)

- **SC-8** Capability = action + resource + constraints (P-09 grammar), e.g. `http.fetch {method GET, url https://api.example.com/public/*, max_response_bytes 1MB}`, `files.read {handles[folder_abc], path_patterns[notes/*.md]}`, `ai.generate {providers[lmstudio_local], cloud_context false}`. No wildcard net domains in v1.
- **SC-9** Grants are explicit (shown as a diff at install/upgrade), least-privilege by template (LLM prompted to request minimal scopes), scoped, instantly revocable (CR-4), and **per-workspace-member**: an applet a collaborator installed runs for *you* only after *you* accept its grants.
- **SC-10** A run is allowed only if **all** pass (P-07): actor role permits operation ∧ workspace policy permits capability ∧ manifest requests it ∧ run profile permits it ∧ platform permission granted ∧ resource matches allowlist ∧ rate/resource limit available. Decisions evaluated in the Rust policy engine on every command and every remote sync op (SS-7); shells may tighten, never loosen. **Gate scope by boundary.** All seven gates apply on every **command/run** (a `ctx.*` host call executing applet code). A **remote sync op** is a passive import of a peer's already-authored CRDT chunk — it is *not* a run: it has no run profile, requests no manifest capability, and uses no OS platform permission. So at the remote-sync boundary the SC-10 decision evaluates the **sync-applicable** gates — the SS-7 RBAC role/membership decision **and** the workspace-policy capability gate, applied per the chunk's capability *category* — while the **run-profile, platform-permission, manifest, resource-allowlist, and rate/resource-limit** gates are RUN-scoped and do not apply to a chunk import (DECISIONS I3). A receiver whose workspace policy forbids a category therefore skips an incoming chunk of *that* category even when membership would allow it; a DL-13 schema-migration chunk additionally requires schema-change authority via the membership `schema_write` RBAC grant (the sync-applicable schema gate).
- **SC-11** Roles: customizable RBAC; defaults `Owner, Maintainer, Editor, Runner, Viewer, Auditor, Reviewer` (SS-7, PRD 08). Permission monotonicity property-tested (SS-9).
- **SC-12** Audit (union of F + P lists): permission grants/denials, role changes, secret access attempts, network calls (metadata), filesystem access, AI provider calls + context manifests, marketplace installs, sync peer changes, hard-purge events, runtime crashes/limit violations, membership/admin events. Retention configurable; redaction default; secrets never in logs; per-applet access view for the user (UI-21).

## 5. Secrets (T1, T2)

- **SC-13** OS keychain/keystore/credential vault everywhere; browser uses WebCrypto-backed storage with documented limitations. Stored by `secret_ref`, never in workspace SQLite, never synced, never in LLM context, never readable as strings by applet code — values injected only into `net` headers/params for **allowlisted domains**; secret-into-payload patterns trigger hard review (LM-9). Logs redact values and matching patterns.

## 6. Supply chain (T1)

- **SC-14** Curated stdlib only; no runtime npm. Every `@forge/std` addition is security-reviewed; std versioned with the core and signed.
- **SC-15** Build integrity: reproducible builds where toolchain allows; signed artifacts (notarized macOS, signed MSIX, sigstore server images); SBOM per release; `cargo audit`/`cargo deny` in CI.

## 7. Marketplace (T7) — cross-ref PRD 08

- **SC-16** v1 packages: source-visible, editable after install, no npm, declared manifest + permissions shown before first run, server-authenticated publisher, compatibility metadata, sandbox-only execution, abuse reporting. **A package can never run before its source and permissions are visible** (release blocker). Format is signing-ready; enforcement can switch on without format break.
- **SC-17** Server cannot grant local runtime capabilities; install grants always happen locally (P-10).

## 8. Cloud & data handling (T5)

- **SC-18** Server-readable ≠ server-promiscuous: encryption at rest (KMS), strict tenant isolation with invariant tests (SS-13), break-glass access with dual approval + customer-visible audit entry, content never in logs/telemetry (SS-22, LM-19). Minimal server data: accounts/devices/teams, marketplace metadata + packages, transient relay metadata, billing; **local workspaces are never uploaded to use the app** (P-10).
- **SC-19** Compliance: GDPR/CCPA export (= DL-24 archive) & deletion (propagates to backups ≤ 30 days); DPA; SOC 2 Type II program from M2, report ≤ 12 months post-GA; PII masking before cloud LLM calls (LM-5).

## 9. Device, client & store compliance (T6)

- **SC-20** OS disk encryption + keychain assumed; optional project-level keys and encrypted export (DL-25); optional biometric app-lock on mobile; remote workspace sign-out revokes tokens and purges local copies on next contact (best-effort, honestly documented).
- **SC-21** iOS review-safety mode (documented artifact, P-09 + PS-17): JSC-only execution, user-viewable/editable source, user-requested execution, no hidden functionality changes, no native code download, public APIs only.
- **SC-22** Apple privacy nutrition label + Play Data Safety generated from config in CI per release.

## 10. Assurance program

- **SC-23** Pre-GA external penetration test: sandbox escape, sync auth, embedded-server LAN attacks, marketplace package abuse, injection corpus; all criticals fixed before GA (M5 gate).
- **SC-24** Vulnerability disclosure policy + security.txt at launch; bounty within 6 months of GA; quarterly internal red-team of the LLM pipeline; annual full re-test.
- **SC-25** Every SC item maps to ≥ 1 automated test or recurring audited process; mapping table at `/security/controls.md`, reviewed each release.

## 11. Open questions

1. E2E-encrypted workspace mode timeline (would disable SS-14 server-side features per-workspace).
2. Bounty scope/budget.
3. Package signing pull-in (PRD 08 open question; architecture is ready).
