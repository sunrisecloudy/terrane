# ChatGPT Apps SDK / MCP Apps — apps inside AI hosts

OpenAI's platform for apps that run *inside* ChatGPT, built on MCP. Relevant
because Terrane's bet is the same sentence reversed: they put apps inside the
agent; Terrane puts agents inside the app platform. The surfaces converge.

## Key ideas

- Every app = **tools** (JSON-Schema functions the model may call) +
  **structured output** + optional **widgets** (HTML iframes rendered inline
  in chat) + an MCP server.
- Widget runtime: `window.openai` bridge — read tool input/output/theme, save
  widget state, **call other tools from the UI**, request display modes, send
  follow-up messages into the conversation.
- Recommended pattern: separate data tools from render tools so the model can
  apply intelligence between fetch and display.
- Auth: OAuth 2.1 + dynamic client registration for connecting user accounts.
- **2026 standardization: "MCP Apps"** — the widget-over-MCP protocol
  (JSON-RPC over postMessage) extracted as an open standard, so one app UI can
  run across compatible hosts; ChatGPT keeps `window.openai` for compat.
- Discovery: apps invoked by @-mention or suggested by relevance; store +
  monetization still partially rolled out.

## What it validated for Terrane

- The verb surface being MCP-shaped was the right call: Terrane apps are
  already tools + structured replies over the host MCP, with iframe UIs and a
  `window.terrane` bridge — structurally an MCP Apps host *and* client before
  the standard existed.
- Their widget→tool calls with per-tool visibility ≈
  [../cap-interop.md](../cap-interop.md)'s grants on the same verb surface.
- OAuth for connecting accounts ≈ [../cap-oauth-connections.md](../cap-oauth-connections.md).

## What it exposed

- **Track MCP Apps compatibility** (agent-readiness follow-up): if
  `window.terrane` grows an MCP-Apps-compatible shim, Terrane apps could
  render inside ChatGPT/Claude and — more strategically — apps built anywhere
  for MCP Apps could install into Terrane. Watch the standard settle before
  building.
- Their "model calls tools mid-conversation with UI sync" flow is the shape
  [../cap-model-v2.md](../cap-model-v2.md)'s tool-use loop should stay
  compatible with.

## Sources

- https://developers.openai.com/apps-sdk
- https://openai.com/index/introducing-apps-in-chatgpt/
- https://github.com/openai/openai-apps-sdk-examples
