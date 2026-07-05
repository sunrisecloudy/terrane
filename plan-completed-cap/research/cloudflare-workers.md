# Cloudflare Workers — the full serverless product checklist

The most complete commercial enumeration of "everything app code needs from a
platform". Used as the final completeness sweep; two roadmap caps exist
because of it.

## Product list (2026) → Terrane mapping

| Cloudflare | What it is | Terrane |
| --- | --- | --- |
| Workers | V8-isolate serverless JS/Wasm | js-runtime + wasm-runtime (shipped) |
| Workers KV | key-value | kv (shipped) |
| D1 | SQLite SQL | relational_db (shipped) |
| R2 | object/blob storage | [../cap-blob.md](../cap-blob.md) (locked) |
| Queues | messages, retries, batching | [../cap-job-queue.md](../cap-job-queue.md) |
| Cron Triggers | scheduled invocation | [../cap-scheduler.md](../cap-scheduler.md) |
| **Queues event subscriptions** | react to platform events (R2, KV, AI, Workflows…) | **[../cap-automation.md](../cap-automation.md)** — created from this |
| Workflows | durable multi-step execution, auto-retries | job-queue + scheduler composition (follow-up pattern doc; cap-ify only if demanded) |
| **Browser Rendering** | headless Chromium: DOM, screenshots, PDFs | **[../cap-browser.md](../cap-browser.md)** — created from this |
| Email Workers | send/receive email in code | [../cap-common.md](../cap-common.md) send + receive-via-interop |
| Workers AI | serverless inference (LLMs, embeddings, images) | model + local-model (shipped), [../cap-model-v2.md](../cap-model-v2.md) |
| Vectorize | vector DB for RAG | search + local-model.embed (shipped) |
| AI Gateway | model-call observability | free by construction — model calls are recorded events |
| Agents | stateful AI agents | agent cap + assist loop (shipped) |
| Durable Objects | stateful serverless actors | Terrane apps *are* durable single-writer actors |
| Tail Workers | consume execution logs | [../cap-telemetry.md](../cap-telemetry.md) |
| Hyperdrive | accelerate external Postgres/MySQL | N/A (external DBs via net-v2/mcp-client if ever) |
| Containers | full-runtime workloads | non-goal: JS/Wasm only, deliberately |
| Workers for Platforms | run untrusted customer/AI code in isolated workers | the entire Terrane premise, locally |
| Artifacts (git-native storage) | versioned storage | [../cap-history.md](../cap-history.md) + blob CAS |
| Custom domains / deploys | public URLs | [../cap-web-publish.md](../cap-web-publish.md) (Premium relay) |
| Budget alerts / billable usage | spend visibility | follow-up: model spend limits in model-v2 |

## Notes

- Their "build for the agent era" positioning (2026 homepage) matches the
  audit's agent lens: event subscriptions, browser rendering, and logs are the
  three primitives agents lean on hardest — all three now have Terrane
  answers.
- Snippets / Smart Placement / multi-region concerns don't translate to a
  local-first platform; excluded deliberately, not overlooked.

## Sources

- https://developers.cloudflare.com/workers/
- https://developers.cloudflare.com/workers/platform/storage-options/
- https://www.cloudflare.com/products/workers/
