# Codex plugin references

Use these official Codex docs while implementing the plugin:

- Codex plugins overview: https://developers.openai.com/codex/plugins
- Build plugins: https://developers.openai.com/codex/plugins/build
- Model Context Protocol in Codex: https://developers.openai.com/codex/mcp
- Agent skills: https://developers.openai.com/codex/skills
- AGENTS.md: https://developers.openai.com/codex/guides/agents-md
- Config reference: https://developers.openai.com/codex/config-reference
- Rules: https://developers.openai.com/codex/rules

Important details as of this spec version:

- A plugin has `.codex-plugin/plugin.json`.
- A plugin can include `skills/` and `.mcp.json`.
- `SKILL.md` files must include `name` and `description` metadata.
- MCP servers can be configured by `.mcp.json` or user/project Codex config.
- Keep the plugin dev-only and require the user's normal Codex approval/sandbox policies.
