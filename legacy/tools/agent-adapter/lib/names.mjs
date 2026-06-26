const TOOL_NAME_PATTERN = /^[a-zA-Z0-9_-]+$/;

/**
 * Map a catalog command name to an LLM-safe tool name (dots → underscores).
 */
export function commandToToolName(commandName) {
  return commandName.replaceAll(".", "_");
}

/**
 * Build bidirectional maps for a filtered command list.
 */
export function buildNameMaps(commands) {
  const toolToCommand = new Map();
  const commandToTool = new Map();

  for (const command of commands) {
    const toolName = commandToToolName(command.name);
    if (toolToCommand.has(toolName) && toolToCommand.get(toolName) !== command.name) {
      throw new Error(`tool name collision after sanitization: ${toolName}`);
    }
    if (!TOOL_NAME_PATTERN.test(toolName)) {
      throw new Error(`sanitized tool name is invalid: ${toolName}`);
    }
    toolToCommand.set(toolName, command.name);
    commandToTool.set(command.name, toolName);
  }

  return { toolToCommand, commandToTool };
}

export function toolNameToCommand(toolName, toolToCommand) {
  if (toolToCommand instanceof Map) {
    return toolToCommand.get(toolName) ?? null;
  }
  return toolToCommand[toolName] ?? null;
}