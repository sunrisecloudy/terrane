# Terrane's resident MLX worker — the same mlx_lm generation loop the
# mlx_lm.generate CLI uses, behind a Unix-socket line protocol. All serving
# logic (lifecycle, protocol, timeouts, idle shutdown) lives in Rust; this
# file stays a thin engine shim and is written to
# $TERRANE_HOME/engines/mlx-worker.py by the Rust side that spawns it.
#
# Protocol (newline-delimited JSON, one request per connection):
#   -> {"ping": true}
#   <- {"pong": true, "models": [...]}
#   -> {"model": "...", "prompt": "...", "maxTokens": 256,
#       "temperature": 0.7, "seed": 42}
#   <- {"t": "<text delta>"}            (repeated, per detokenized segment)
#   <- {"done": true, "tokens": N, "genTps": F, "promptTps": F,
#       "finish": "stop"|"length"}
#   <- {"error": "..."}                 (instead of done, on failure)

import json
import os
import socket
import sys

import mlx.core as mx
from mlx_lm import load, stream_generate
from mlx_lm.models.cache import make_prompt_cache
from mlx_lm.sample_utils import make_sampler

SOCKET_PATH = sys.argv[1]

_models = {}
_ll_tokenizers = {}
_prompt_caches = {}


def get_model(ref):
    if ref not in _models:
        _models[ref] = load(ref)
    return _models[ref]


# Conversation prefix caching. The snapshot is taken at the HISTORY BOUNDARY
# (the rendering without the generation tail): chat templates render past
# turns consistently across requests, while the generation tail (e.g. Qwen3's
# empty `<think/>` block under enable_thinking=False) and the generated reply
# are re-rendered differently next turn — so nothing past the boundary is
# ever stored. MLX arrays are immutable, so a snapshot is a cheap reference
# copy that later generation steps cannot corrupt, and restoring works for
# every cache type including recurrent layers (ArraysCache — Qwen3.5's
# hybrid graph) that cannot rewind.
PREFILL_CHUNK = 2048


def prefill(model, cache, tokens):
    """Feed tokens into the cache without sampling (mirrors generate_step's
    chunked prefill)."""
    i = 0
    while i < len(tokens):
        n = min(PREFILL_CHUNK, len(tokens) - i)
        model(mx.array(tokens[i : i + n])[None], cache=cache)
        mx.eval([c.state for c in cache])
        i += n


def restore_prompt_cache(ref, model, prompt_tokens):
    """Restore the model's snapshot when this prompt extends the snapshotted
    conversation. Returns (cache, suffix_tokens, reused_token_count)."""
    entry = _prompt_caches.get(ref)
    if entry is not None:
        cached = entry["tokens"]
        if 0 < len(cached) < len(prompt_tokens) and prompt_tokens[: len(cached)] == cached:
            cache = make_prompt_cache(model)
            for layer, state, meta in zip(cache, entry["state"], entry["meta"]):
                layer.state = state
                if meta:
                    layer.meta_state = meta
            return cache, prompt_tokens[len(cached) :], len(cached)
    return make_prompt_cache(model), prompt_tokens, 0


def snapshot_prompt_cache(ref, cache, tokens):
    _prompt_caches[ref] = {
        "tokens": list(tokens),
        "state": [layer.state for layer in cache],
        "meta": [getattr(layer, "meta_state", ()) for layer in cache],
    }


class LlgProcessor:
    """Token-mask enforcement for a JSON schema via llguidance: every step,
    tokens that cannot extend a schema-valid document get -inf bias."""

    def __init__(self, ll_tokenizer, schema):
        import llguidance

        grammar = llguidance.LLMatcher.grammar_from_json_schema(schema)
        self.matcher = llguidance.LLMatcher(ll_tokenizer, grammar)
        self.consumed = None

    def __call__(self, tokens, logits):
        count = int(tokens.shape[-1])
        if self.consumed is None:
            # First call: `tokens` is the prompt; the grammar starts fresh.
            self.consumed = count
        elif count > self.consumed:
            self.matcher.consume_tokens(
                [int(t) for t in tokens[self.consumed : count].tolist()]
            )
            self.consumed = count
        bias = self.matcher.compute_logit_bias()
        vocab = int(logits.shape[-1])
        mask = mx.array(bias[:vocab]).astype(logits.dtype)
        if vocab > len(bias):
            pad = mx.full((vocab - len(bias),), float("-inf"), dtype=logits.dtype)
            mask = mx.concatenate([mask, pad])
        return logits + mask[None]


def schema_processor(model_ref, tokenizer, schema):
    """Build a masking processor, or None (guided fallback) when llguidance
    is not installed in this runtime."""
    if model_ref not in _ll_tokenizers:
        try:
            import llguidance.hf

            hf_tokenizer = getattr(tokenizer, "_tokenizer", tokenizer)
            _ll_tokenizers[model_ref] = llguidance.hf.from_tokenizer(hf_tokenizer)
        except Exception:
            _ll_tokenizers[model_ref] = None
    ll_tokenizer = _ll_tokenizers[model_ref]
    if ll_tokenizer is None:
        return None
    return LlgProcessor(ll_tokenizer, schema)


def handle(conn):
    stream = conn.makefile("rwb")

    def send(obj):
        stream.write((json.dumps(obj) + "\n").encode("utf-8"))
        stream.flush()

    try:
        line = stream.readline()
        if not line:
            return
        req = json.loads(line)
        if req.get("ping"):
            send({"pong": True, "models": sorted(_models)})
            return

        model, tokenizer = get_model(req["model"])
        if "seed" in req:
            mx.random.seed(req["seed"])
        messages = []
        if req.get("system"):
            messages.append({"role": "system", "content": req["system"]})
        for user, assistant in req.get("history") or []:
            messages.append({"role": "user", "content": user})
            messages.append({"role": "assistant", "content": assistant})
        messages.append({"role": "user", "content": req["prompt"]})
        # Same template handling as the CLI's --chat-template-config; the
        # thinking flag is ignored by templates that lack it.
        prompt = list(
            tokenizer.apply_chat_template(
                messages,
                add_generation_prompt=True,
                enable_thinking=False,
            )
        )
        bare = list(
            tokenizer.apply_chat_template(
                messages,
                add_generation_prompt=False,
                enable_thinking=False,
            )
        )
        # History boundary: where the consistently-rendered conversation ends
        # and the per-turn generation tail begins.
        boundary = len(bare) if prompt[: len(bare)] == bare else None
        if boundary is not None and boundary >= len(prompt):
            boundary = None
        sampler = make_sampler(temp=req.get("temperature", 0.0))
        constrained = None
        processors = None
        if req.get("schema"):
            processor = schema_processor(req["model"], tokenizer, req["schema"])
            if processor is not None:
                processors = [processor]
                constrained = "mask"
            else:
                constrained = "guided"
        draft_model = None
        if req.get("draftModel"):
            # Speculative decoding: mlx_lm rewinds caches on rejected drafts,
            # so this needs rewindable (standard-attention) caches and manages
            # its own prompt cache — the conversation snapshot is skipped.
            draft_model, _ = get_model(req["draftModel"])
        cache, suffix, reused = (None, prompt, 0)
        if draft_model is None:
            cache, suffix, reused = restore_prompt_cache(req["model"], model, prompt)
            if boundary is not None and boundary > reused:
                # Feed up to the history boundary ourselves, snapshot the
                # pristine state, then let stream_generate handle the tail.
                prefill(model, cache, prompt[reused:boundary])
                snapshot_prompt_cache(req["model"], cache, prompt[:boundary])
                suffix = prompt[boundary:]
        last = None
        pending = 0
        for resp in stream_generate(
            model,
            tokenizer,
            suffix,
            max_tokens=req.get("maxTokens", 256),
            sampler=sampler,
            logits_processors=processors,
            prompt_cache=cache,
            draft_model=draft_model,
        ):
            last = resp
            if resp.text:
                # Buffer the tiny writes and flush in groups: a per-token
                # flush stalls the decode pipeline for a syscall 400×/s.
                stream.write((json.dumps({"t": resp.text}) + "\n").encode("utf-8"))
                pending += 1
                if pending >= 8:
                    stream.flush()
                    pending = 0
        if pending:
            stream.flush()
        send(
            {
                "done": True,
                "tokens": last.generation_tokens if last else 0,
                "genTps": last.generation_tps if last else 0.0,
                "promptTps": last.prompt_tps if last else 0.0,
                "finish": (last.finish_reason if last else None) or "stop",
                "constrained": constrained,
                "cachedTokens": reused,
            }
        )
    except (BrokenPipeError, ConnectionResetError):
        pass  # the caller hit its deadline and hung up; drop this generation
    except Exception as error:  # noqa: BLE001 — keep serving after bad requests
        try:
            send({"error": str(error)})
        except Exception:
            pass
    finally:
        try:
            stream.close()
        except Exception:
            pass


def main():
    try:
        os.unlink(SOCKET_PATH)
    except FileNotFoundError:
        pass
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(SOCKET_PATH)
    os.chmod(SOCKET_PATH, 0o600)
    server.listen(1)
    while True:
        conn, _ = server.accept()
        try:
            handle(conn)
        finally:
            conn.close()


if __name__ == "__main__":
    main()
