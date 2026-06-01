# PRD: Local-First OSS Server and Private SaaS Split

## 0. Document status

This document defines the product and architecture split for a local-first Terrane platform where the open-source server is embedded inside the client as a local platform engine, while the hosted SaaS remains a separate private product surface.

This PRD is a product slice layered on top of the v0.4 platform baseline. The following documents remain normative and are not replaced by this PRD:

- `docs/00_PRD.md` for platform scope and milestone baseline.
- `docs/03_RUNTIME_API_SPEC.md` for bridge methods and request shape.
- `docs/04_WEBAPP_PACKAGE_SPEC.md` for generated app package format.
- `docs/07_SECURITY_MODEL.md` for sandbox and trust boundary.
- `docs/17_APP_SIGNING_AND_TRUST.md` for package canonicalization and signing.
- `docs/27_DATABASE_SCHEMA.md` for platform persistence.
- `docs/32_REFERENCE_HOST_SPEC.md` for reference-host conformance.

## 1. Product summary

Terrane should ship an open-source local platform engine that can run inside the desktop client and optionally run as a standalone local server for development. The same product should offer a private hosted SaaS for accounts, teams, sync, publishing, billing, fleet operations, and managed distribution.

The open-source server is not the SaaS backend. It is the local runtime engine:

```text
Desktop client
  owns native UI, app lifecycle, and OS integration
  starts bundled local server on loopback
  loads runtime-web in a WebView
  talks to local server or native services through the bridge
  stores local state in SQLite

Private SaaS
  owns accounts, tenants, sync, billing, publishing, admin, and fleet operations
  never exposes generated apps to direct platform credentials
  composes around the public runtime contract
```

The product story is:

- Open local runtime.
- Local-first data and generated apps.
- Public bridge/package contract.
- Paid cloud coordination, collaboration, distribution, governance, and managed backup.

Public artifacts should use the Terrane name directly. "Local platform engine" and "OSS local server" are role descriptions, not separate product brands.

## 2. Problem

The platform needs developer trust and a credible local-first story without open-sourcing the operational SaaS backend too early.

If the production SaaS server is open-sourced as-is, it risks exposing business logic, operational details, and security-sensitive admin surfaces before the hosted product is stable. If the server is fully private, developers cannot inspect or verify the runtime behavior that generated apps depend on.

The right split is to open-source the local engine and contract implementation while keeping the multi-tenant hosted control plane private.

## 3. Product thesis

Generated apps should be portable local packages, not hosted-only code. The platform should make the local engine trustworthy and inspectable, while the SaaS should monetize coordination and service guarantees.

This creates two distinct products:

```text
OSS product:
  "Run AI-generated apps locally and safely."

SaaS product:
  "Sync, share, govern, publish, back up, and operate those apps across people and devices."
```

## 4. Users

### Primary user: local-first builder

A technical builder who wants to generate and run small tools locally, inspect the runtime, keep data local by default, and avoid vendor lock-in for basic execution.

### Primary user: team operator

A founder, team lead, or IT owner who wants shared app distribution, team permissions, audit history, managed backups, billing, and central policy.

### Primary user: AI coding agent

Codex or another agent that validates, installs, smokes, repairs, and snapshots generated apps through a stable local control plane.

### Secondary user: platform contributor

A developer improving runtime-web, native hosts, server bridge behavior, package validation, tests, or docs.

### Internal user: SaaS operator

The Terrane team operating hosted sync, billing, signing, distribution, moderation, abuse controls, and incident response.

## 5. Current codebase state

The current repo already has the core ingredients for this split:

- `server/` is a partial Zig HTTP server with `/health`, `/core/step`, `/bridge`, package validation/install/signing helpers, token-gated control commands, safe DB inspection, SQLite persistence, snapshots, rollback, and production-mode dev-control disabling.
- `tools/reference-host/` is the reference implementation of the bridge contract and remains the conformance oracle.
- `runtime-web/` loads generated apps into sandboxed frames and routes bridge calls through `AppRuntime.call`.
- Native hosts already own platform services, SQLite persistence, WebView bridge dispatch, and dev-control surfaces.
- Release packaging already includes a server artifact path.

What does not exist yet is the private SaaS layer:

- no account or organization model;
- no billing;
- no hosted tenant isolation;
- no managed cloud sync service;
- no customer admin console;
- no public/private marketplace service;
- no hosted signing-key custody workflow;
- no cloud fleet deployment and operations model.

## 6. Goals

### G1. Make the OSS server the local platform engine

The open-source server must be positioned and implemented as a local engine for single-user or single-device execution. It must be safe to embed in the desktop client and useful as a standalone developer target.

Required capabilities:

- package validation, policy audit, canonicalization, signing, install, rollback, quarantine, and uninstall;
- bridge dispatch for documented runtime methods;
- SQLite-backed app registry, app files, app permissions, app storage, runtime logs, install reports, snapshots, migrations, and backups;
- token-gated local control commands for Codex and developer tooling;
- deterministic behavior against reference-host contract fixtures;
- release artifact suitable for bundling in desktop clients.

### G2. Embed the OSS server in desktop clients

On macOS, Windows, and Linux, the native client should be able to bundle and launch the local server as a child process.

The desktop client owns:

- creating a per-user data directory;
- choosing a random loopback port or internal IPC endpoint;
- creating and protecting a per-launch control token;
- launching the server;
- waiting for `/health`;
- restarting after crash when safe;
- shutting down the server when the client exits;
- migrating local data on app upgrade;
- presenting user-visible recovery flows when the local engine fails.

### G3. Keep mobile native bridge parity

iOS and Android must not depend on a long-running embedded HTTP server as their primary runtime architecture. Mobile hosts should keep direct native bridge dispatch and share behavior through contract tests, schemas, database migrations, and common core libraries.

Mobile may use the server only for simulator/dev workflows when platform rules permit it.

### G4. Keep the private SaaS separate

The private SaaS must be a separate repository or separately versioned package that composes around the public platform contract. It must not require private code to run the local engine.

The private SaaS owns:

- user identity and sessions;
- organization and team membership;
- billing and entitlements;
- hosted app registry and publishing workflows;
- cross-device sync and backup;
- cloud signing and key custody;
- admin audit and compliance views;
- abuse prevention and rate limiting;
- operational observability and incident response.

### G5. Preserve public contract trust

The bridge contract, generated app package rules, schemas, reference host, fixtures, and local engine behavior must remain public and testable. Private SaaS code must not define hidden runtime semantics required by generated apps.

### G6. Avoid long-lived private forks

The private SaaS should consume the public repo as a dependency, submodule, package, or pinned artifact. It should not permanently fork and modify the local engine's core semantics.

### G7. Support SaaS value without local lock-in

Users must be able to run generated apps locally without signing in. SaaS features should add value through sync, collaboration, distribution, governance, and service guarantees.

## 7. Non-goals

- Do not make the production SaaS backend open-source in this slice.
- Do not add auth, billing, tenant isolation, or cloud admin logic to the OSS server.
- Do not create a separate public self-hosted team server; anyone may run the Terrane app/local server as their own server.
- Do not require generated apps to call the SaaS directly.
- Do not expose SaaS access tokens to generated apps.
- Do not add React, TypeScript, Vite, Next.js, npm dependencies, or build steps to generated app packages.
- Do not make mobile hosts depend on a local HTTP server for normal runtime behavior.
- Do not support arbitrary SQL from generated apps, Codex, or SaaS clients.
- Do not make the reference host a production SaaS backend.

## 8. Architecture

### 8.1 Local desktop architecture

```text
Desktop app process
  native shell and WebView
  launches bundled local server
  loads runtime-web
  derives appId from mount/channel

Generated app iframe
  HTML/CSS/vanilla JS
  calls AppRuntime.call(...)

runtime-web
  validates request shape, permission, budgets, and mount channel
  forwards bridge calls to native bridge or local server endpoint

local server
  validates again as the security boundary
  dispatches storage/network/dialog/mock/core/package/control methods
  persists to SQLite
  calls Zig core via FFI where required

SQLite data directory
  app registry
  app versions/files/permissions
  app storage
  runtime logs
  snapshots and backups
```

### 8.2 Mobile architecture

```text
Mobile app process
  native shell and WebView
  loads runtime-web
  handles bridge dispatch directly in native code
  uses SQLite through PlatformDatabase
  shares contract fixtures with reference host and server
```

Mobile hosts must match the same bridge responses and persistence contracts, but they do not need to run the local server process.

### 8.3 SaaS architecture

```text
Private SaaS
  API gateway
  account/org service
  billing service
  sync service
  package registry
  publishing workflow
  signing/key service
  policy service
  admin/audit service
  observability and abuse controls

Desktop/mobile clients
  authenticate with SaaS through platform-owned client code
  sync platform-owned records
  never expose SaaS credentials to generated apps
```

## 9. OSS local server requirements

### R1. Local binding

The OSS local server must bind to loopback only by default. Non-loopback binding is a development-only mode and must be rejected in production/local-client mode unless an explicit unsafe flag is used in a developer environment.

### R2. Per-launch control token

Every local server launch must create or consume a control token stored in a user-private file. Control commands and safe DB inspection endpoints must require the token.

### R3. Single-user data model

The OSS server targets a single local user profile. It may support multiple installed apps, multiple generated app versions, and multiple runtime sessions, but it must not implement SaaS tenant isolation.

### R4. SQLite-first persistence

The OSS server must use SQLite for local persistence. Postgres is not required for local embedded mode. Any future Postgres adapter belongs to the private SaaS or a separately reviewed private/enterprise deployment slice, not the public local server.

### R5. Local signing

The OSS server may sign local installs with a local platform key. Local signatures prove package integrity inside the local profile. They do not imply cloud publication, marketplace trust, or team approval.

### R6. Package lifecycle

The OSS server must support:

- validate;
- policy audit;
- canonicalize;
- sign;
- install in one DB transaction;
- run smoke checks;
- enable or quarantine;
- rollback;
- uninstall with confirmation;
- create and restore snapshots;
- export and import backups.

### R7. Bridge behavior

The OSS server must implement documented bridge methods only. It must not invent private bridge methods for generated apps. Experimental controls must live under token-gated dev/control commands and must be disabled or unavailable in production/local-client mode as specified by security docs.

### R8. Network behavior

Generated apps must never use direct `fetch`. The server must enforce `manifest.networkPolicy` for `network.request`. SaaS sync traffic is platform-owned client traffic, not generated app network traffic.

### R9. Data directory

The desktop client must start the server with an explicit per-user data directory. The server must not write user data into the repo, current working directory, or shared temp paths in packaged client mode.

### R10. Upgrade behavior

The local server must apply append-only database migrations on upgrade. Failed migrations must leave the prior DB usable or restoreable from a pre-migration snapshot.

## 10. Desktop embedding requirements

### R11. Server lifecycle

Desktop clients must treat the local server as an owned child service:

- launch on app start or first platform use;
- health-check before mounting generated apps;
- restart only when no install/migration/backup transaction is active;
- terminate on app exit;
- record crash metadata for user support and diagnostics.

### R12. Port and origin policy

The first public desktop embedding path must use HTTP loopback. The desktop client must choose a random loopback port. Runtime origins, CORS, and bridge forwarding must allow only the owning client runtime. IPC may be considered later as an internal optimization, but it is not required before public release.

### R13. Token custody

The control token must be stored outside generated app reach. Runtime-web may forward bridge requests, but generated apps must never read control tokens or call control endpoints directly.

### R14. User-visible recovery

If the local engine cannot start, the client must show a recovery path:

- retry;
- inspect logs;
- reset local engine state after snapshot;
- restore from backup;
- continue in limited mode when possible.

### R15. Packaging

Desktop release artifacts must include:

- native host binary;
- runtime-web assets;
- generated example apps where applicable;
- SQLite migrations;
- Zig core library;
- local server executable;
- version manifest with hashes.

## 11. Private SaaS requirements

### R16. Account and organization model

The SaaS must model users, organizations, workspaces, roles, devices, and sessions. These identities are platform-owned and must never be delegated to generated apps.

### R17. Entitlements and billing

The SaaS owns plan limits, billing state, trials, invoices, seats, and feature entitlements. The local engine may cache entitlement snapshots for offline UX, but the SaaS remains the authority.

### R18. Cloud app registry

The SaaS owns published app packages, team-approved packages, marketplace metadata, trust levels, review status, and revocation lists.

### R19. Managed signing

The SaaS owns cloud signing keys and package publication signatures. Local signing keys must not be accepted as marketplace or organization-trusted signatures unless explicitly approved by cloud policy.

### R20. Sync and backup

The SaaS owns cross-device sync, hosted backup, restore, retention, and device authorization. Sync must operate on platform-owned records, not raw generated app filesystem access.

The first paid sync and backup release must use end-to-end encryption for synced user app data and backup payloads. SaaS operational metadata required for billing, routing, device authorization, abuse prevention, or support must be minimized and documented separately.

### R21. Collaboration

The SaaS owns collaboration primitives that cross devices or users:

- shared app installs;
- team app catalogs;
- shared notebooks or CRDT streams;
- comments and approvals;
- device presence where supported;
- audit trails.

### R22. Admin and governance

The SaaS must support admin views for team policy, audit log, package approvals, data export, member lifecycle, and organization-level restrictions.

### R23. Operations

The private SaaS owns metrics, traces, logs, alerts, incident tooling, abuse detection, rate limiting, WAF/CDN rules, deployment automation, and secret management.

## 12. Boundary rules

### B1. Generated apps never call SaaS platform APIs directly

Generated apps can call third-party APIs only through `AppRuntime.call("network.request", ...)` and only when allowed by `manifest.networkPolicy`. They must not receive Terrane account tokens, sync tokens, billing tokens, admin tokens, or signing keys.

### B2. SaaS cannot define hidden bridge semantics

If generated apps depend on a bridge behavior, that behavior must be documented in public specs and covered by public fixtures. Private SaaS APIs may orchestrate installs and sync, but they must not add hidden generated-app capabilities.

### B3. Public tests guard private behavior

The private SaaS CI must run public contract tests for any public runtime behavior it consumes or proxies.

### B4. Private SaaS data stays outside OSS repo

Do not commit private schemas, cloud infra configs, internal admin API definitions, billing flows, production observability config, or abuse heuristics into the OSS repo.

### B5. OSS local server remains useful offline

A signed-in SaaS session may enhance the client, but local install/open/storage/snapshot flows must work without a network connection.

## 13. Sync model

### 13.1 Syncable records

After sign-in, the first SaaS sync slice syncs all safe syncable local records by default:

- installed app identity and version metadata;
- published package references;
- approved team package references;
- `app_storage` records;
- runtime snapshots and backups;
- CRDT notebook updates;
- user settings;
- device metadata.

### 13.2 Non-syncable records by default

Never sync:

- local control tokens;
- raw local debug bundles;
- dev-control command logs;
- unredacted bridge logs;
- local signing private keys;
- OS file paths;
- generated app temporary data;
- app data excluded by user or org policy.

### 13.3 Conflict policy

The sync service must not silently merge records without a deterministic conflict policy.

Initial policy:

- CRDT notebook data uses CRDT merge rules from the notebook PRD.
- Immutable package versions merge by content hash and version id.
- App install state uses server-assigned revisions and may require user/admin resolution on conflict.
- Generic app storage sync uses per-key revisions. Concurrent edits to the same key create a conflict record unless the app declares a supported merge strategy in a future manifest extension.

### 13.4 Privacy policy

Local data remains local unless the user signs in. After sign-in, all safe syncable records sync by default. Users or organization policy may exclude categories from sync, and team-managed devices may require sync by organization policy, but that must be visible to the user.

## 14. Security requirements

### S1. Local attack surface

The embedded local server must assume hostile local web content exists on the same machine. It must defend with loopback-only binding, origin checks, CORS restrictions, per-launch tokens, derived app identity, bridge permissions, and manifest resource budgets.

### S2. Production guard

Production/local-client mode must reject dev-only startup flags and disable dev/control routes that are not required for normal client operation.

### S3. App identity

Generated apps must not send `appId` in request bodies. The runtime and host derive app identity from mount/channel/session context.

### S4. Secret isolation

Generated apps must not access:

- local control tokens;
- SaaS auth tokens;
- signing keys;
- local server data directory paths;
- OS credential stores;
- raw SQLite connections.

### S5. Tenant isolation

Tenant isolation is private SaaS responsibility. It must be enforced in the SaaS database, API authorization layer, sync service, package registry, signing service, and admin tooling.

### S6. Audit

The local server audits security-relevant local actions. The SaaS audits account, organization, publishing, signing, billing, sync, and admin actions.

### S7. Revocation

The SaaS may publish revocation lists for malicious packages, compromised signing keys, or disallowed app versions. The local engine may consume those lists when signed in, but local offline execution policy must be explicit and user-visible.

## 15. Developer experience

### DX1. Public local development

Contributors must be able to run the public reference host and local server without private SaaS access.

Expected public workflows:

```text
run reference host
run local server
validate example app packages
run bridge fixtures
run local server API smoke tests
package desktop artifacts
```

### DX2. Private SaaS development

Private SaaS developers must be able to pin the public platform repo and run the public conformance suite from private CI.

The private repo should consume public artifacts through one of:

- git submodule;
- pinned source checkout;
- published package artifact;
- release tarball with manifest hashes.

### DX3. No private code required for OSS tests

Public tests must not require private SaaS credentials. Tests that require SaaS must live in the private repo or be skipped unless explicit SaaS fixtures are configured.

## 16. Repository and release topology

### 16.1 Public repo

The public repo license is MIT.

The public repo should contain:

```text
docs/
schemas/
runtime-web/
server/
tools/reference-host/
tools/codex-platform-mcp/
zig-core/
zig-crdt/
native/
webapps/examples/
tests/
db/sqlite/
db/postgres logical schema docs and parity checks
```

The public repo should not contain:

```text
saas/auth-service
saas/billing-service
saas/admin-console
saas/sync-production-service
saas/signing-key-custody
saas/deployment-infra
saas/abuse-heuristics
saas/customer-data-migrations
production-secrets
```

### 16.2 Private repo

The private SaaS repo should contain:

```text
services/api-gateway
services/auth
services/billing
services/sync
services/package-registry
services/signing
services/admin
services/observability
infra/
private-ci/
```

It should import or pin the public platform repo for:

- bridge schemas;
- package schemas;
- runtime capability schemas;
- DB logical contract fixtures;
- reference-host tests;
- local engine artifacts.

### 16.3 Release artifacts

Public release artifacts:

- local server executable;
- runtime-web archive;
- example app archive;
- Zig core libraries;
- desktop client artifacts if open-sourced;
- schemas and fixtures;
- release manifest with hashes.

Private release artifacts:

- SaaS service containers;
- private migrations;
- admin UI;
- deployment manifests;
- signing service configuration;
- operational dashboards.

## 17. Monetization boundary

The OSS product should be valuable on its own for local execution. The SaaS should charge for hosted value that is expensive, collaborative, or operational:

- multi-device sync;
- team workspaces;
- managed backups;
- organization app catalogs;
- package publishing and review;
- collaboration and approvals;
- admin audit and compliance;
- hosted browser access;
- SLA and support;
- policy enforcement across devices.

The first paid SKU should bundle all four initial paid surfaces:

- sync and backup;
- team catalogs;
- marketplace publishing;
- enterprise governance.

The local runtime must not be intentionally crippled to force SaaS adoption. Trust in local execution is part of the product value.

## 18. Milestones

### M1. Boundary PRD and status update

Acceptance:

- this PRD exists;
- implementation status lists it as spec-only;
- docs consistently call the OSS server a local platform engine, not the SaaS backend.

### M2. OSS local server hardening

Acceptance:

- local-client mode exists or is specified as the default packaged mode;
- server binds loopback only;
- data directory is explicit;
- token-gated control works;
- dev-only controls are unavailable in production/local-client mode;
- contract fixtures pass against reference host and server.

### M3. Desktop embedded server alpha

Acceptance:

- macOS, Windows, and Linux clients can bundle the local server artifact;
- client launches server on random loopback port;
- health check gates runtime mount;
- client can install and open an example app through the local engine;
- app data persists in per-user SQLite;
- crash/restart behavior is tested.

### M4. Private SaaS skeleton

Acceptance:

- private repo imports or pins the public platform repo;
- auth, orgs, billing placeholder, package registry, and sync skeleton exist;
- private CI runs public conformance tests;
- no generated app receives SaaS credentials.

### M5. Sync beta

Acceptance:

- user signs in from desktop client;
- all safe syncable local records can sync to a second desktop client by default;
- synced user app data and backup payloads are end-to-end encrypted;
- conflicts are represented explicitly;
- backup and restore are available;
- local-only mode remains functional.

### M6. Public OSS release

Acceptance:

- local server release artifact is published;
- reference host and server tests are green;
- docs describe what is OSS and what is SaaS;
- contributors can run local examples without private access;
- MIT license and contribution policy are explicit.

## 19. Acceptance criteria

The product split is successful when:

- a developer can clone the public repo, run the reference host, run the local server, validate/install/open example apps, and inspect the bridge contract without SaaS credentials;
- a desktop user can run generated apps locally without signing in;
- a signed-in user syncs all safe syncable local records by default without generated apps receiving platform secrets;
- private SaaS code can evolve without redefining public generated-app runtime behavior;
- public conformance tests catch bridge/package/runtime drift before private SaaS release;
- mobile hosts continue to match the same bridge contract without depending on a local server process.

## 20. Metrics

Product metrics:

- local install success rate;
- local app open success rate;
- time from desktop launch to local engine ready;
- generated app smoke-test pass rate;
- sync activation rate after sign-in;
- successful restore rate;
- team package publish success rate;
- free local user to paid SaaS conversion.

Engineering metrics:

- bridge fixture pass rate across reference host, local server, and native hosts;
- local server crash rate;
- local DB migration failure rate;
- local engine startup p50/p95;
- sync conflict rate;
- private SaaS conformance failure rate;
- number of private patches that modify public runtime semantics.

## 21. Risks and mitigations

### Risk: private SaaS drifts from public local behavior

Mitigation: private CI must run public conformance tests and pin public platform versions. Runtime behavior required by generated apps must be promoted to public specs.

### Risk: local server increases desktop attack surface

Mitigation: loopback-only bind, per-launch token, strict origin/CORS checks, local-client production guard, no generated-app access to control endpoints, and native ownership of token custody.

### Risk: mobile behavior diverges from desktop

Mitigation: mobile remains a first-class native bridge target with the same fixtures, schemas, and database contracts. Do not force a desktop-only local server model onto mobile.

### Risk: OSS server competes with SaaS

Mitigation: position OSS as local execution and trust. SaaS value comes from sync, teams, governance, distribution, backup, service reliability, and support.

### Risk: self-hosting expectations expand too early

Mitigation: explicitly call the OSS server single-user/local-first and do not create a separate public self-hosted team server. Anyone may run the Terrane app/local server as a server for their own use; multi-tenant hosted operations remain the private SaaS responsibility.

## 22. Resolved product decisions

- The OSS repo uses the MIT license.
- The local server is branded as Terrane, not as a separate "Terrane Local Engine" product.
- Desktop clients use HTTP loopback for the embedded server path before public release.
- SaaS sync and backups are end-to-end encrypted in the first paid release.
- After sign-in, all safe syncable local records sync by default.
- There is no separate public self-hosted team server. Anyone may run the Terrane app/local server as their own server.
- The first paid SKU includes sync/backup, team catalogs, marketplace publishing, and enterprise governance.
