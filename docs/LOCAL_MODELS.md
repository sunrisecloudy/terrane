# Local models

Locally-run LLM inference as a Terrane capability. Generation is an effect at
the edge: the `local-model` capability validates purely, the host runs the
engine exactly once, and the response is recorded as an ordinary event —
replay rebuilds every transcript without ever re-running inference.

Two engine backends exist. **They are two engine targets, not interchangeable
engines**: quantization, tokenizer/template handling, and sampler differences
all shift output between them, even for the "same" model.

| Backend | Engine | Weights | Platforms |
|---|---|---|---|
| `llama_cpp` | llama.cpp in-process (Metal on macOS) | one `.gguf` file | macOS, Linux, Windows (compiles-by-design, unvalidated) |
| `mlx` | mlx-lm via a resident Python worker | an MLX repo snapshot | Apple silicon only |

## Quick start

```sh
# Fetch the recommended model (Qwen3.5-0.8B, GGUF) and make it the default:
terrane local-model pull

# Or the MLX build (Apple silicon; provision the runtime first):
terrane local-model setup mlx
terrane local-model pull --backend mlx

# Ask (uses the default model):
terrane app add demo Demo
terrane local-model ask demo "say hello"
```

The first registered model becomes the default automatically;
`terrane local-model default <id>` changes it explicitly; removing the default
clears it.

## Commands

- `local-model pull [<id> <hf-repo> [<file>]] [--backend gguf|mlx] [options]`
  — download weights and register the spec. Bare `pull` fetches the
  recommended model for the backend. GGUF pulls reuse the standard Hugging Face
  hub cache when the file already exists there and otherwise download into that
  cache; MLX pulls snapshot the repo into the same HF cache.
- `local-model register <id> <backend> <path> [options]` — record a spec for
  weights already on disk (checked at inference time, not at decide time).
- `local-model rm <id>` — unregister (weights on disk untouched).
- `local-model default <id>` — set the model `ask` uses without `--model`.
- `local-model ask <app> [--model <id>] [--system <text>] [--continue]
  [--schema <json> | --grammar <gbnf>] <prompt…>` — one recorded generation.
- `local-model setup mlx` — provision a pinned, self-contained MLX runtime
  under `$TERRANE_HOME/engines/` (uv → CPython 3.12 → mlx-lm 0.31.3 +
  llguidance; ~400 MB on a fresh machine). Idempotent; a runtime already on
  PATH is just recorded.
- `local-model server status|stop` — resident MLX worker lifecycle.

Spec options for `pull`/`register`: `--context N`, `--template T`,
`--max-tokens N`, `--temp F`, `--draft <ref>` (mlx only).

## Conversations

`--system <text>` renders a system prompt ahead of the conversation.
`--continue` feeds back the app's recorded, successful exchanges with the
same model (most recent 8, oldest first). History lives in the event log —
there is no hidden session state, and a different app shares nothing.

## Typed output

- `--schema '<json object>'` — token-mask enforced JSON on both backends:
  llama.cpp via its built-in llguidance sampler; MLX via an llguidance logits
  processor in the worker (`constrained(schema-mask)` in the log). If the MLX
  runtime lacks llguidance, the worker falls back to prompt-guided decoding
  and says so (`schema-guided`).
- `--grammar '<gbnf>'` — llama.cpp only, for shapes a schema cannot express.

## Apps and agents

Declaring `"local-model"` in an app manifest requests the namespace; an admin
grant (`terrane auth grant user:local-owner <app> local-model`) exposes it:

```js
var lm = ctx.resource["local-model"];
lm.ask(prompt)              // default model
lm.askModel(model, prompt)  // named model
lm.askJson(schema, prompt)  // schema-mask JSON
```

The call runs the effect once and records `local-model.responded`; Option-A
replay never re-runs the JS or the inference. Over MCP, `local-model.ask` is
grant-gated like `kv`/`crdt` writes; `register`/`pull`/`rm`/`default` stay
trusted-admin-only (they configure machine-local weights).

For GGUF specs pulled from Hugging Face, inference resolves the recorded
`hf:<repo>/<file>` source through the standard Hugging Face cache
(`HF_HUB_CACHE` / `HUGGINGFACE_HUB_CACHE`, `HF_HOME`, or
`~/.cache/huggingface/hub`) if the originally recorded path is no longer
present. This lets Terrane share model files with other local tools instead of
requiring a private copy per `TERRANE_HOME`.

## Resident MLX worker

The first MLX ask auto-starts a detached worker (one per `$TERRANE_HOME`,
newline-JSON over a 0600 Unix socket) and a shell watchdog that kills it
after an idle window — an idle machine has **zero** resident processes; the
next ask restarts it lazily. `mlx_lm.server` is deliberately not used: its
continuous-batching engine decodes ~2.5× slower at batch size 1.

The worker keeps a per-model **conversation prefix cache**: it snapshots the
KV state at the history boundary and restores it when the next prompt extends
the conversation — `--continue` turns skip re-processing the whole
transcript (works for every cache type, including Qwen3.5's hybrid graph).

The macOS host shows the worker in the sidebar's Local models panel (status +
Stop Server).

## Performance (Qwen3.5-0.8B, M3 Max, 256-token story)

Recorded end-to-end durations from the event log (release build; includes
model load, prompt processing, and transport — engine-only decode is higher):

| Path | Recorded |
|---|---|
| llama.cpp (GGUF Q4_K_M, Metal), one-shot CLI | 256 tok / 1.63 s ≈ 157 tok/s (incl. ~0.5 s load) |
| MLX 4-bit, cold (worker spawn + model load) | 256 tok / 3.72 s |
| MLX 4-bit, warm resident worker | 256 tok / 1.11 s ≈ 230 tok/s wall (~326–381 engine) |
| MLX `--continue` turn (prefix cache) | 36 tok / 0.64 s; 1.4× wall on an 8-turn conversation |
| llama.cpp engine cache (long-lived hosts) | ~490 ms load → ~3 µs per ask |

Speculative decoding (`--draft`) is wired for MLX but needs a model whose
caches can rewind (standard attention); Qwen3.5's hybrid graph refuses it
with mlx_lm's clear error.

## Environment variables

| Variable | Meaning |
|---|---|
| `TERRANE_HOME` | workspace root (log, models, engines) |
| `TERRANE_LOCAL_MODEL_TIMEOUT_MS` | per-ask wall budget (default 300000) |
| `TERRANE_MLX_IDLE_MS` | worker idle window before auto-exit (default 600000) |
| `TERRANE_MLX_RESIDENT=0` | disable the resident worker (one-shot `mlx_lm.generate` per ask; no `--continue`) |
| `TERRANE_MLX_LM_BIN` / `TERRANE_MLX_SERVER_BIN` | explicit runtime CLIs (overrides manifest/PATH) |
| `TERRANE_MLX_PYTHON` | explicit interpreter for the worker |
| `TERRANE_LOCAL_MODEL_GGUF` | test-only: GGUF path for the `#[ignore]` suites |

## Platform notes

- **macOS (Apple silicon)** — both backends, fully validated.
- **Linux** — llama.cpp validated by design (CPU); the MLX stack compiles but
  mlx-lm has no Linux wheels, so `setup mlx` reports the platform honestly.
- **Windows** — compiles-by-design, unvalidated. llama.cpp is the local
  backend; the MLX worker is never spawned (Unix sockets + Apple silicon),
  and `server status` reports not-running.

## Embedding hosts

Long-lived hosts get a process-global llama engine cache (the ~0.5 s GGUF
load is paid once). **Call `terrane_local_model_shutdown()` (C ABI) or
`terrane_host::local_llm_shutdown()` before a normal exit** — a cached Metal
model alive during ggml's static destructors aborts the process. The CLI and
the macOS app already do.

## Testing

```sh
cargo test                                  # pure suites (default green)
cargo test -p terrane-host -- --ignored     # real inference + downloads
TERRANE_LOCAL_MODEL_GGUF=~/path/to.gguf \
  cargo test -p terrane-local-llm --test engine -- --ignored
```

The `#[ignore]` suites cover: real llama/MLX asks, two-turn recall, schema
enforcement, app-backend calls through the binary, worker hardening
(concurrent asks, kill mid-generation, socket perms, lazy restart), and the
scrubbed-PATH fresh-machine `setup mlx` bootstrap (~400 MB).
