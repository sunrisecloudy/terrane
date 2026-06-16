# Merged PRD Pack

Merged from `local_first_util_2/` (Forge pack) and `local_first_utility_prd_pack/` (Utility pack) on 2026-06-12, resolving all conflicts via stakeholder decisions recorded in [DECISIONS.md](DECISIONS.md).

**Strategy in one line:** build the complete vertical slice headless first (CLI harness + renderer zero + conformance suites that start with covered engine vectors and expand per platform) so every platform shell is a thin, conformance-tested renderer over one Rust core.

| File | Contents |
|---|---|
| [00-master-prd.md](00-master-prd.md) | Vision, audience, pillars, e2e-template strategy, scope, architecture, monetization, metrics, milestones, risks |
| [01-core-runtime-prd.md](01-core-runtime-prd.md) | Crate layout, command/event shell contract, sandbox, dual JS engines (JSC + QuickJS), applet & script shapes, deterministic runs, offline TS pipeline placement |
| [02-data-layer-prd.md](02-data-layer-prd.md) | Loro CRDT mapping, SQLite KV/oplog physical schema, record envelope, dynamic schema + forward-compat rules, query DSL, time travel, tombstones, export |
| [03-sync-server-prd.md](03-sync-server-prd.md) | Home-server topology (managed cloud + embedded desktop), protocol, server-enforced RBAC, conflict policy, migration, self-host |
| [04-llm-system-prd.md](04-llm-system-prd.md) | Providers (cloud/LM Studio/in-core), context modes, AI modes, offline generation pipeline, repair loop, injection defenses, eval harness, budgets |
| [05-ui-system-prd.md](05-ui-system-prd.md) | Declarative component-tree protocol, headless golden-tree contract, renderer zero, renderer conformance kit, platform app surfaces (editor, schema designer, permission UX) |
| [06-platform-shells-prd.md](06-platform-shells-prd.md) | Shell zero (CLI), macOS, web, Linux headless, Windows fast-follow, iOS (JSC/2.5.2 posture), Android |
| [07-security-prd.md](07-security-prd.md) | Threat model, sandbox guarantees, capability/RBAC grammar, secrets, supply chain, cloud handling, assurance program |
| [08-marketplace-prd.md](08-marketplace-prd.md) | Centralized marketplace (source-visible, signing-ready), relay/discovery, AI gateway, self-host bundle |
| [09-roadmap-quality-gates-prd.md](09-roadmap-quality-gates-prd.md) | Milestones M0–M6 with exit criteria, test layers, compatibility fixtures, perf/reliability gates, release blockers |
| [DECISIONS.md](DECISIONS.md) | What the packs agreed on, the 8 stakeholder decisions, editorial merge resolutions, what was dropped |
