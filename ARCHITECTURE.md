# Terrane — Architecture

> Terrane is your digital space, truly yours: a local-first home for the apps
> you keep. AI agents can generate HTML apps cheaply and endlessly — Terrane is
> where those apps *live, run, and persist*, owned by you, on your own devices.

This document is the high-level shape. It is intentionally about layers and
responsibilities, not file paths. Implementation lives in
[`terrane-core/`](terrane-core/); see [README.md](README.md) for the one rule.

## The picture

Top to bottom — each layer only talks to the one directly below it.

```
┌──────────────────────────────────────────────────────────────────────┐
│  APPLICATIONS                                                          │
│  any number of personal apps, each = UI (HTML/CSS/JS) + Backend (JS)   │  ← what the user sees
├──────────────────────────────────────────────────────────────────────┤
│  NATIVE HOST                                                           │
│  the thin per-platform shell: macOS · Linux · Windows · iOS ·         │  ← the launch hub
│  Android · Web. Hosts the UI webview and the backend runtime.         │
├──────────────────────────────────────────────────────────────────────┤
│  TERRANE-CORE  (the spine)                                            │
│  Command ▸ Event ▸ State ▸ replay. Connects the host to resources.    │  ← deterministic
├──────────────────────────────────────────────────────────────────────┤
│  RESOURCES  (the capability surface apps consume)                     │
│  storage (SQLite/KV) · files · network/API · model/LLM · CLI & agents │  ← effects, at the edge
└──────────────────────────────────────────────────────────────────────┘
```

## Layer 1 — Applications

A user has many apps. Each app, like an Electron app, has two parts:

- **UI** — HTML, CSS, JavaScript. The visible surface, rendered in a webview.
- **Backend** — JavaScript. The app's own logic.

An app never touches the OS or the network directly. It reaches everything it
needs through the **resource** surface (`ctx.resource.*`) exposed by the layers
below. That indirection is what makes apps portable and safe to run.

### Where the backend JavaScript runs

The backend runtime *can be anywhere* — the app doesn't care. Today:

| Platform                              | Backend JS runtime                          |
| ------------------------------------- | ------------------------------------------- |
| macOS · Linux · Windows · iOS · Android | **QuickJS** embedded inside the native host |
| Web                                   | a **Web Worker** (the host's own JS engine) |

On native platforms we embed QuickJS so the backend runs in-process, isolated,
without a third-party heavyweight runtime. On the web we use a Web Worker
instead — compiling/embedding QuickJS-in-WASM on the web is much harder to do
well, and a Worker already gives us isolated JavaScript for far less cost.

## Layer 2 — Native host

A small native application per platform (macOS, Linux, Windows, iOS, Android,
and Web). Its job is narrow:

- present the app UI (a webview),
- run the app backend (QuickJS, or a Web Worker on web),
- bridge both into terrane-core.

The host is the *launch hub* — it holds the user's apps and is how the user
opens and switches between them. It carries no business logic of its own.

## Layer 3 — terrane-core (the spine)

The deterministic command/event core that connects the native host to the
resources. This is the part this repository is built around, and it obeys one
rule:

```
Command ▸ terrane-core ▸ [Event] ▸ State          (replaying the log → identical State)
```

Apps and hosts issue **Commands**; the core applies them to produce **Events**
and **State**, and the event log replays deterministically. Effects (anything
in the Resources layer) are mediated here and recorded, so that replay stays
deterministic even though the resources themselves are not. The CLI (`terrane`)
is a front door onto this same spine.

## Layer 4 — Resources (the capability surface)

What an app is actually allowed to *do*. Apps call these as `ctx.resource.*`;
the host and core mediate, sandbox, and (where needed) record them:

- **Storage** — a SQLite-backed store and key/value storage.
- **Files** — a scoped filesystem.
- **Network / API** — outbound API access (HTTP and friends).
- **Model / Intelligence** — access to an LLM, so apps can be intelligent.
- **CLI & Agents** — the core can invoke command-line tools. The important
  special case is **agent CLIs** (e.g. Claude, Codex): terrane-core can drive
  them as a supervisor, letting AI agents talk **to each other** and — the main
  point of the whole project — **to the user**.

## North star

AI agents make it trivial to spin up endless little HTML apps. Without a home,
they scatter and disappear. Terrane is that home:

- a place to **save, run, and keep** the apps you and your agents create,
- **truly yours** — local-first, on your devices,
- still open to the outside: an app here can be reachable **via MCP or agent
  skills**, so external agents can call into your apps.

The supervisor role in the Resources layer is what ties it together — Terrane
isn't just storage for apps, it's the place where your agents and your apps and
you all meet.

## Terms I interpreted (correct me)

This was captured from a spoken design pass; a few words were ambiguous and I
made a best guess:

- **"resources"** for the bottom capability layer (you said "result"/"the
  thing we have") — storage, files, API, model, CLI/agents.
- **"launch hub" / native host** for the layer you called the "LAN hub layer."
- **agent CLIs = Claude and Codex** (heard as "fraud"/"codecs").
- **QuickJS** as the embedded native backend runtime ("quick JS").
- Backend runtime placement: QuickJS natively, Web Worker on web.

If any of these are wrong, tell me and I'll fix the doc before we build toward
it.
