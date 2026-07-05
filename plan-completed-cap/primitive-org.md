# Primitive: `org` — an organization is a shared home

Mostly composition, not new machinery: **an org is a Terrane home of its own**
— its own event log, apps, and data — that members' replicas sync with under
role grants signed against their [person](primitive-person.md) keys. The `org`
field that already sits in every `ExecutionPrincipal` stops being the constant
`"local"` and starts naming a real thing.

## Locked decision (user, 2026-07-05)

**Org = shared home**, with **Premium hosting as a convenience, never a
limitation**: Premium's offering is "we run your org's always-on host" (same
protocol, same sync); self-hosting an org home on any machine works
identically and is always available.

## Model

- **Org identity** = an org keypair (minted like a person's; held by owners in
  their keychains) → `org_id`. The org home's replica is attested to the org
  key.
- **Membership** = the org home's own auth cap: members are person_ids with
  roles (`owner` / `admin` / `member`), recorded as grants signed by an
  owner/admin key. Joining = redeem an invite ([cap-share-invite.md](cap-share-invite.md))
  with your person key; your attested devices inherit membership.
- **Org-owned apps and data** live in the org home's log — synced to members
  per role ([cap-sync-v2.md](cap-sync-v2.md)); personal homes stay private.
- **Principal wiring:** acting on org content, requests carry
  `{org: <org_id>, subject: user:<person_id>}` — the field finally means what
  it says; the engine stamps it into every event via
  [primitive-actor.md](primitive-actor.md).
- **Permission prompts on org resources** route to admins: v1 = approve on the
  org host itself (admin console); cross-device elicitation over sync is v2.

## Command surface (thin `org` cap on top of existing caps)

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `org.create` | mints org keypair (edge), initializes the org home, founder becomes `owner` — `org.created {org_id, pubkey, founder}` |
| Command | `org.invite` / `org.join` / `org.leave` / `org.role.set` | thin wrappers recording membership facts; enforcement is edge policy over folded state at the sync routes (same stance as share-invite) |
| Query | `org.members` / `org.info` | folded membership + roles |

## Premium offering (convenience plan)

- `terrane org host --premium`: Premium provisions an always-on host carrying
  the org home — storage + sync endpoint + web-publish relay integration; org
  admins hold the keys, Premium holds availability. Migration off Premium =
  `terrane sync` the org home to any machine (it is just a home).
- Work items live in `../terrane-premium`: hosted-home runner, billing hook,
  provisioning API. The Rust side needs nothing Premium-specific — that is
  the "convenience not limitation" test, enforced by design.

## Implementation plan

1. Org keypair + `org.create` flow (reuses person keygen effects and the
   keychain).
2. `org` cap crate (membership facts, folds, doc); role policy at the sync-v2
   routes (extends share-invite's enforcement table with roles).
3. Principal wiring: hosts resolve which home a request targets and stamp
   `{org, subject}` accordingly; `LOCAL_ORG` remains for personal homes.
4. Founder/join CLI + shell flows (invite QR/code reuses share-invite).
5. Admin surface: org member list + pending permission requests on the org
   host (ties into the planned auth admin console).
6. Premium: hosted org-home provisioning (TS repo), documented migration path
   off.
7. Tests: e2e two-person org over loopback sync — create, invite, role
   enforcement (member cannot approve grants; admin can), org-app data
   syncing to members only.

Gate: `cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Depends on

[primitive-person.md](primitive-person.md) (memberships are person grants),
[cap-sync-v2.md](cap-sync-v2.md) + [cap-share-invite.md](cap-share-invite.md)
(transport + invites), [primitive-actor.md](primitive-actor.md) (org-stamped
events).

## Non-goals (v1)

Nested orgs/teams, cross-org federation, per-role app-level custom
permissions beyond owner/admin/member, billing seats (Premium concern).

## Decisions to confirm

- **Role set = owner/admin/member** — recommend as specced — alternative:
  add `guest` (read-only) in v1.
- **Where the org home lives by default** — recommend founder's machine until
  moved (explicit, local-first) — alternative: prompt Premium hosting at
  creation (better availability, softer local-first story).
