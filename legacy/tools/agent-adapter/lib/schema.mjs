import fs from "node:fs";
import path from "node:path";
import { repoRoot } from "./paths.mjs";

const FALLBACK_INPUT_SCHEMA = {
  type: "object",
  additionalProperties: true,
};

/**
 * Resolve a descriptor payload_schema into an LLM input_schema object.
 */
export function resolveInputSchema(descriptor, { root = repoRoot } = {}) {
  const schemaRef = descriptor.payload_schema;
  if (!schemaRef) {
    return { ...FALLBACK_INPUT_SCHEMA };
  }

  if (typeof schemaRef === "object" && !Array.isArray(schemaRef)) {
    return cloneJson(schemaRef);
  }

  if (typeof schemaRef !== "string") {
    return { ...FALLBACK_INPUT_SCHEMA };
  }

  const trimmed = schemaRef.trim();
  if (trimmed.startsWith("{")) {
    try {
      return JSON.parse(trimmed);
    } catch {
      return { ...FALLBACK_INPUT_SCHEMA };
    }
  }

  const candidates = [
    path.isAbsolute(trimmed) ? trimmed : path.join(root, trimmed),
    path.join(root, "schemas", "commands", path.basename(trimmed)),
  ];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return JSON.parse(fs.readFileSync(candidate, "utf8"));
    }
  }

  return {
    ...FALLBACK_INPUT_SCHEMA,
    description: `Schema file not found (${trimmed}); additionalProperties allowed`,
  };
}

export function validatePayload(payload, inputSchema, { commandName = "command" } = {}) {
  if (payload === null || typeof payload !== "object" || Array.isArray(payload)) {
    return { ok: false, error: `${commandName} payload must be a JSON object` };
  }

  const schema = inputSchema ?? FALLBACK_INPUT_SCHEMA;
  const missing = [];
  for (const property of schema.required ?? []) {
    if (!(property in payload)) {
      missing.push(property);
    }
  }
  if (missing.length > 0) {
    return {
      ok: false,
      error: `${commandName} missing required fields: ${missing.join(", ")}`,
      details: { missing },
    };
  }

  if (schema.anyOf && !schema.anyOf.some((option) => hasRequiredArguments(payload, option.required ?? []))) {
    return {
      ok: false,
      error: `${commandName} missing one required argument group`,
      details: { anyOf: schema.anyOf.map((option) => option.required ?? []) },
    };
  }

  if (schema.additionalProperties === false) {
    const allowed = new Set(Object.keys(schema.properties ?? {}));
    for (const key of Object.keys(payload)) {
      if (!allowed.has(key)) {
        return { ok: false, error: `${commandName} has unexpected field: ${key}` };
      }
    }
  }

  for (const [property, value] of Object.entries(payload)) {
    const propertySchema = schema.properties?.[property];
    if (!propertySchema) continue;
    const error = validateValue(property, value, propertySchema);
    if (error) {
      return { ok: false, error: `${commandName} invalid field ${error}`, details: { property } };
    }
  }

  return { ok: true };
}

function hasRequiredArguments(args, required) {
  return required.every((property) => property in args);
}

function validateValue(property, value, valueSchema) {
  if ("const" in valueSchema && value !== valueSchema.const) {
    return `${property} must equal ${JSON.stringify(valueSchema.const)}`;
  }
  if (valueSchema.enum && !valueSchema.enum.includes(value)) {
    return `${property} must be one of ${valueSchema.enum.join(", ")}`;
  }
  if (valueSchema.anyOf) {
    return valueSchema.anyOf.some((option) => !validateValue(property, value, option))
      ? null
      : `${property} has unsupported JSON shape`;
  }
  if (valueSchema.type && !valueMatchesType(value, valueSchema.type)) {
    return `${property} must be ${valueSchema.type}`;
  }
  if (valueSchema.minLength && typeof value === "string" && value.length < valueSchema.minLength) {
    return `${property} must not be empty`;
  }
  if (valueSchema.minimum !== undefined && typeof value === "number" && value < valueSchema.minimum) {
    return `${property} must be >= ${valueSchema.minimum}`;
  }
  if (valueSchema.maximum !== undefined && typeof value === "number" && value > valueSchema.maximum) {
    return `${property} must be <= ${valueSchema.maximum}`;
  }
  return null;
}

function valueMatchesType(value, type) {
  if (Array.isArray(type)) {
    return type.some((entry) => valueMatchesType(value, entry));
  }
  if (type === "array") return Array.isArray(value);
  if (type === "null") return value === null;
  if (type === "integer") return Number.isInteger(value);
  if (type === "object") return Boolean(value) && typeof value === "object" && !Array.isArray(value);
  return typeof value === type;
}

function cloneJson(value) {
  return JSON.parse(JSON.stringify(value));
}