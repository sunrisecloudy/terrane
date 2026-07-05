# Sandstorm — self-hosted app platform (capability security)

Self-hostable web productivity suite built as a security-hardened app package
manager (Kenton Varda / Cap'n Proto lineage). The closest prior art for
Terrane's *security* model.

## Key ideas

- **Grains** — every document/chat/blog is its own sandboxed instance; a grain
  is the unit of sharing, private until shared. Fine granularity lets the
  platform (not each app) own access control; they claim ~95% of security
  vulnerabilities are mitigated by the containment model. Apps are told to
  implement **no internal user model** — no logins, ACLs, or permission
  systems; the platform handles identity and sharing.
- **Powerbox** — the app-to-app capability picker. A grain requests "a
  capability implementing API X"; the system shows the user a picker of their
  grains that offer X; choosing fulfills the request. No yes/no security
  dialogs — **choosing IS granting**. Connections are auditable and revocable;
  composability like Unix pipes (email → chat-poster without either knowing
  the other).
- Two powerbox modes: request and offer; apps can export multiple APIs at
  different permission levels.
- Cap'n Proto object capabilities as the wire model; sandboxing via Linux
  namespaces/cgroups/seccomp.
- Platform features: one-click grain **backup** (.zip), signed auto-updates,
  publisher identity verification (domains/PGP) on install, account tiers
  (admin/user/visitor), incognito visits.

## What it validated for Terrane

- Grant-based sandboxing where the platform is the security boundary — our
  manifest resources + auth grants + elicitation are the same stance.
- Signed publishing with publisher identity → [../cap-publish.md](../cap-publish.md)
  (ed25519 + TOFU).
- One-click backup/restore → [../cap-backup-export.md](../cap-backup-export.md),
  [../cap-history.md](../cap-history.md).

## What it exposed

- **The powerbox was the single biggest missing idea** → became
  [../cap-interop.md](../cap-interop.md) (locked): interface-based picker over
  `common.*` verbs, choosing-is-granting via the existing elicitation flow.
- **Granularity honesty:** Sandstorm shares *documents* (grains); Terrane v1
  shares *apps* ([../cap-share-invite.md](../cap-share-invite.md)). Noted as a
  known coarser granularity; per-document sharing is a crdt/document v2.
- Deliberately not adopted: transitive capability forwarding (grain X
  introducing Y⇄Z). Terrane keeps the user in the loop per edge — simpler to
  reason about, slightly less composable.

## Sources

- https://sandstorm.io/how-it-works
- https://sandstorm.io/features
- https://docs.sandstorm.io/en/latest/developing/powerbox/
- https://github.com/sandstorm-io/sandstorm
