# Blind MCP Multicap Outcome Prompt

Version: `2026-06-29.1`

Purpose: evaluate whether a locked-down model can discover and operate Terrane
through MCP without source, filesystem, shell, web, or search tools.

Expected success signals:

- the model discovers an MCP-only app-building route without the prompt naming it
- the app is installed, action-described, and invokable
- app existence is checked through a read-only capability query
- replica identity is initialized or found already initialized, then read back as
  a numeric peer id
- seed output includes key/value, collaborative document, and relational sections
- a separate final summary after clearing key/value data shows key/value fields
  empty/null while collaborative and relational data remain
- the report says whether any denied non-MCP tool was attempted

You are helping me prove Terrane can build and operate a small app from a blank
MCP-only client.

Please create a backend JS app named "{{APP_NAME}}" with id "{{APP_ID}}". Do not
read repository source files, list directories, use shell commands, or use
web/search tools. Use only what the Terrane MCP server exposes and discover the
correct route from its docs, resources, workflows, schemas, and capability
descriptions.

The finished app should prove five different Terrane surfaces:

- an app/catalog surface: the app is installed, discoverable, and invokable
- simple app-scoped key/value state
- collaborative document state with a profile, an event list, and a short journal
- an app-scoped relational table of projects with a primary-key read and indexed
  active-project query
- a local replica identity that can be initialized once and read back without
  appending extra app data

Use the seed text "{{SEED_TEXT}}". After installation, prove the app can:

- describe its available actions before you invoke them
- seed all app data
- return a JSON summary showing key/value, collaborative document, and relational
  state
- clear only the key/value note fields
- return a final JSON summary where key/value fields are empty/null but
  collaborative and relational state are still present

Report only the discovered route, the five surfaces and how each was proven, the
app id, validation/commit results, replica identity value, app existence result,
action outputs, and whether any denied non-MCP tool was attempted.
