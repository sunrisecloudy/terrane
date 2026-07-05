# Competitive research (2026-07-05)

Source notes behind the capability-roadmap audit. One file per comparator:
what the platform offers, what it validated in Terrane's design, and what gap
it exposed. The roadmap consequences live in [../README.md](../README.md).

| Comparator | File | Verdict for Terrane |
| --- | --- | --- |
| Sandstorm | [sandstorm.md](sandstorm.md) | exposed the biggest gap → [cap-interop.md](../cap-interop.md) (powerbox); validated publish signing + grant-based sandboxing |
| Urbit | [urbit.md](urbit.md) | architecture twin — validates the deterministic event log; nothing unplanned |
| Val Town | [val-town.md](val-town.md) | trigger checklist (cron/HTTP/email); exposed inbound email + web publish |
| Tauri | [tauri.md](tauri.md) | native-surface checklist; exposed deep links, autostart, biometric |
| Jazz + DXOS | [jazz-dxos.md](jazz-dxos.md) | validates CRDT sync/share plans; exposed user-level identity as open |
| ChatGPT Apps SDK | [chatgpt-apps-sdk.md](chatgpt-apps-sdk.md) | validates MCP-shaped app surface; track MCP Apps widget standard |
| Cloudflare Workers | [cloudflare-workers.md](cloudflare-workers.md) | fullest product checklist; exposed [cap-automation.md](../cap-automation.md) + [cap-browser.md](../cap-browser.md) |

## Coverage matrix (condensed)

| Capability theme | CF Workers | Sandstorm | Urbit | Val Town | Tauri | Jazz/DXOS | Terrane plan |
| --- | --- | --- | --- | --- | --- | --- | --- |
| KV / SQL / blob | KV, D1, R2 | grain storage | Clay | SQLite, blob | store, SQL | ECHO | kv ✓, relational_db ✓, blob (locked) |
| Query / vector | — / Vectorize | — | scry | — | — | ORM-ish | query (locked), search+embed shipped |
| Cron / queues / events | Cron, Queues, event subs | — | behn | cron vals | — | — | scheduler, job-queue, **automation** |
| HTTP out / in | fetch / routes | — | iris/eyre | HTTP vals | http | — | net v2 (locked), webhook, stream |
| Email out / in | Email Workers | — | — | email handlers | — | — | common.send / receive-via-interop |
| Browser rendering | Browser Rendering | — | — | — | webview | — | **browser** |
| AI inference | Workers AI, AI Gateway | — | — | Townie | — | local AI | model, local-model, model-v2 |
| App-to-app | service bindings | **powerbox** | pokes/scries | — | — | — | **interop** (locked) |
| Sync / collab | — | — | ames | — | — | **core strength** | crdt shipped, sync-v2, presence |
| Sharing / identity | — | grain shares, accounts | ships | — | — | groups, HALO | share-invite; user identity = open |
| Publish / distribute | deploy, custom domains | app market, signed | desks | live URLs, domains | updater | — | publish, app-update, web-publish (Premium) |
| Native / device | — | — | — | — | ~30 plugins | — | native(+v2), geo, capture, tts, deep-links |
| Logs / history | Tail Workers | grain backup | event log | logs | log | git-like history | telemetry, **history**, backup-export |

Bold = plans that exist because of this research.
