# PRD 08 — Marketplace & Optional Central Services

**Status:** Merged draft v1 · **Depends on:** 03, 07 · **Milestone:** M4 beta, GA in v1.x
**Sources:** P-10 (marketplace, server capabilities, self-host, package format) + F (open-core monetization, relay) + decision D6 (marketplace committed in plan)

## 1. Purpose

Optional centralized commercial services that never compromise local-first operation: marketplace, identity/passkeys, relay/discovery, team coordination, hosted AI gateway, and update/telemetry channels (opt-in). Everything here is **optional and self-hostable**; the architecture must not drift toward cloud-required (P ADR-011).

## 2. Product model

- **MP-1** Open local core (OSS) + commercial centralized services + self-host support. Desktop embedded server can replace central services for a team: sync host, relay/rendezvous, marketplace mirror, LLM gateway, backup host (SS-15..19).
- **MP-2** Cloud account optional everywhere (D8): passkeys + email/OAuth, device registration, team membership, custom roles, invite links — needed only for cloud sync, relay, bundled credits, and publishing.

## 3. Marketplace (M4 beta)

- **MP-3** Registry of **source-visible, editable-after-install** packages: applets, scripts, schema templates. Installs land as normal workspace applets — same sandbox, same local permission grants (SC-16/17); the server can never grant capabilities.
- **MP-4** Package format (P-10, signing-ready):
  `package_id, name, version (semver), publisher_id, source_visible: true, editable_after_install: true, files[{path, hash}], manifest{entrypoint|ui entry, capabilities}, schema_decls, compatibility{min_app_version, required_features}, auth{server_verified_publisher, signature: null-for-now}`.
  Signing is deferred (stakeholder decision) but the format and install path are built so enforcement can switch on without breaking packages.
- **MP-5** Publisher accounts are server-authenticated with provenance records; package metadata shows full permission manifest and source browser **before install**; abuse reporting + takedown workflow at beta; ratings/reviews and paid packages post-v1.
- **MP-6** Install flow (normative): browse → inspect source + permission manifest → install into chosen workspace → local grant prompts (SC-9) → run. A package can never execute before source and permissions were visible (release blocker).
- **MP-7** iOS: no public marketplace inside the iOS app at launch (PS-17); marketplace via web/desktop; installed packages sync into workspaces like any applet.
- **MP-8** Compatibility: packages declare `required_features`; clients use capability negotiation (CR-A5/DL-14) to refuse or limited-mode gracefully.

## 4. Relay & discovery

- **MP-9** Rendezvous + relay for embedded home servers (SS-16): WebSocket signaling, NAT traversal assist, ciphertext relay fallback. Disableable per workspace; replaceable by self-host.

## 5. Hosted AI gateway (optional)

- **MP-10** Provider routing, usage metering/billing for bundled credits, team policy enforcement (allowed models/context modes), context redaction policy. Local-first provider configs (LM-1..3) always work without it.

## 6. Self-host bundle

- **MP-11** Single binary + Docker image of `forge-server` with all central capabilities (identity, relay, marketplace mirror, AI gateway proxy); SQLite first, Postgres at scale; admin UI; backup/restore. A self-hosted server can fully replace central services for a team (P-10 acceptance).

## 7. API areas

`/auth/passkey/* · /devices/* · /teams/* · /marketplace/packages/* · /marketplace/publishers/* · /sync/rendezvous/* · /sync/relay/* · /ai/gateway/* · /admin/*`

## 8. Server data policy

- **MP-12** Store the minimum for the role: account/device/team metadata, marketplace metadata + package files, transient relay metadata, billing/usage, opt-in gateway logs per retention policy. Local workspaces are never required to be uploaded (SC-18).

## 9. Acceptance

- Local app fully functional with the central server unreachable.
- Marketplace utility: browse → inspect → install → grant → run, with source visible at every step; malicious-package fixture corpus blocked by sandbox + grants.
- Self-hosted bundle replaces central relay + marketplace mirror for a team in a soak test.
- Embedded desktop server serves a small LAN team incl. mirror mode.

## 10. Open questions

1. Package review requirements (human/automated) before first public beta.
2. Signing pull-in timing (v1.x vs v2).
3. Paid packages / revenue share model (post-v1).
4. Namespace policy for package IDs and publisher verification levels.
