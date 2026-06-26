import fs from "node:fs";
import { defaultCatalogPath, repoRoot } from "./paths.mjs";
import { fetchViaDescribe } from "./transport.mjs";

export const VISIBILITY_TIERS = ["public", "operator", "admin", "debug"];

const TIER_RANK = new Map(VISIBILITY_TIERS.map((tier, index) => [tier, index]));

const ROLE_ALIASES = new Map([
  ["owner", "owner"],
  ["maintainer", "maintainer"],
  ["editor", "editor"],
  ["runner", "runner"],
  ["viewer", "viewer"],
  ["auditor", "auditor"],
  ["reviewer", "reviewer"],
]);

/**
 * Normalize role strings to snake_case catalog form.
 */
export function normalizeRole(role) {
  if (!role) return null;
  const key = String(role).trim().toLowerCase();
  return ROLE_ALIASES.get(key) ?? null;
}

export function tierRank(tier) {
  const normalized = String(tier ?? "public").trim().toLowerCase();
  if (!TIER_RANK.has(normalized)) {
    throw new Error(`unknown tier: ${tier}. Expected one of ${VISIBILITY_TIERS.join(", ")}`);
  }
  return TIER_RANK.get(normalized);
}

/**
 * Accept a raw catalog file, system.describe response, or descriptor array.
 */
export function normalizeCatalog(input) {
  if (Array.isArray(input)) {
    return {
      catalogVersion: null,
      commands: input.map(normalizeDescriptor),
    };
  }

  if (!input || typeof input !== "object") {
    throw new Error("catalog must be an array or object");
  }

  if (Array.isArray(input.commands)) {
    return {
      catalogVersion: input.catalogVersion ?? null,
      commands: input.commands.map(normalizeDescriptor),
    };
  }

  if (input.ok === true && input.payload && Array.isArray(input.payload.commands)) {
    return {
      catalogVersion: input.payload.catalogVersion ?? null,
      commands: input.payload.commands.map(normalizeDescriptor),
    };
  }

  if (input.payload && Array.isArray(input.payload.commands)) {
    return {
      catalogVersion: input.payload.catalogVersion ?? null,
      commands: input.payload.commands.map(normalizeDescriptor),
    };
  }

  throw new Error("catalog input is missing a commands array");
}

function normalizeDescriptor(descriptor) {
  if (!descriptor?.name) {
    throw new Error("catalog entry is missing name");
  }
  return {
    name: descriptor.name,
    namespace: descriptor.namespace ?? descriptor.name.split(".")[0],
    summary: descriptor.summary ?? "",
    surface: String(descriptor.surface ?? "outer").toLowerCase(),
    mutates: Boolean(descriptor.mutates),
    effectful: Boolean(descriptor.effectful),
    visibility: String(descriptor.visibility ?? "operator").toLowerCase(),
    required_roles: normalizeRoles(descriptor.required_roles),
    capabilities: Array.isArray(descriptor.capabilities) ? descriptor.capabilities : [],
    payload_schema: descriptor.payload_schema ?? null,
    response_schema: descriptor.response_schema ?? null,
    events: Array.isArray(descriptor.events) ? descriptor.events : [],
    stability: descriptor.stability ?? "stable",
    since: descriptor.since ?? "",
  };
}

function normalizeRoles(roles) {
  if (!Array.isArray(roles)) return [];
  return roles
    .map((role) => normalizeRole(role) ?? String(role).trim().toLowerCase())
    .filter(Boolean);
}

export function isInnerSurface(command) {
  return command.surface === "inner" || command.name.startsWith("ctx.");
}

export function commandVisibleTo(command, { tier = "public", role = "owner" } = {}) {
  const normalizedRole = normalizeRole(role);
  if (!normalizedRole) {
    return false;
  }
  const maxTier = tierRank(tier);
  const commandTier = tierRank(command.visibility);
  if (commandTier > maxTier) {
    return false;
  }
  if (!command.required_roles.length) {
    return true;
  }
  return command.required_roles.includes(normalizedRole);
}

/**
 * Filter catalog entries for an agent surface.
 */
export function filterCatalog(commands, { tier = "public", role = "owner", includeInner = false } = {}) {
  return commands.filter((command) => {
    if (!includeInner && isInnerSurface(command)) {
      return false;
    }
    return commandVisibleTo(command, { tier, role });
  });
}

export function loadCatalogFromFile(catalogPath) {
  const raw = fs.readFileSync(catalogPath, "utf8");
  return normalizeCatalog(JSON.parse(raw));
}

export async function loadCatalog({
  catalogPath = defaultCatalogPath,
  describe = false,
  server = null,
  token = null,
  tier = "public",
  role = "owner",
  workspaceId = "agent-adapter",
  actor = "agent-adapter",
} = {}) {
  if (describe || server) {
    const response = await fetchViaDescribe({
      server,
      token,
      tier,
      role,
      workspaceId,
      actor,
    });
    return normalizeCatalog(response);
  }

  if (!fs.existsSync(catalogPath)) {
    throw new Error(
      `catalog not found at ${catalogPath}. Pass --catalog <path>, or --describe / --server <url> to fetch via system.describe`,
    );
  }

  return loadCatalogFromFile(catalogPath);
}

export function findCommand(commands, commandName) {
  return commands.find((command) => command.name === commandName) ?? null;
}