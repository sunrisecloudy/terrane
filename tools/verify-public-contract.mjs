#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

export function verifyPublicContract({
  contractPath = path.join(repoRoot, "artifacts", "public-contract.json"),
  root = repoRoot,
  schemaPath = path.join(root, "forge", "contracts", "public-contract.schema.json"),
  requireProvenanceMatch = true,
} = {}) {
  const contract = readJson(contractPath);
  const schema = readJson(schemaPath);
  const schemaErrors = createSchemaValidator(schema).validate(contract);
  const errors = schemaErrors.map((error) => `schema: ${error}`);

  if (requireProvenanceMatch) {
    const expectedCommit = readGitCommit(root);
    if (expectedCommit && contract.provenance?.sourceCommit !== expectedCommit) {
      errors.push(`provenance: sourceCommit ${contract.provenance?.sourceCommit ?? "null"} does not match ${expectedCommit}`);
    }
  }

  for (const file of allContractFiles(contract)) {
    const filePath = path.join(root, file.path);
    if (!fs.existsSync(filePath)) {
      errors.push(`file: ${file.path} is missing`);
      continue;
    }
    const data = fs.readFileSync(filePath);
    const sha256 = crypto.createHash("sha256").update(data).digest("hex");
    if (data.length !== file.bytes) {
      errors.push(`file: ${file.path} bytes ${data.length} does not match ${file.bytes}`);
    }
    if (sha256 !== file.sha256) {
      errors.push(`file: ${file.path} sha256 ${sha256} does not match ${file.sha256}`);
    }
  }

  return {
    ok: errors.length === 0,
    errors,
    contractPath: path.resolve(contractPath),
    root: path.resolve(root),
    filesChecked: allContractFiles(contract).length,
  };
}

function allContractFiles(contract) {
  return Object.values(contract.files ?? {}).flat();
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function readGitCommit(root) {
  try {
    return execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

function createSchemaValidator(schema) {
  function validate(value, currentSchema = schema, valuePath = "$", rootSchema = schema) {
    if (!currentSchema || Object.keys(currentSchema).length === 0) return [];
    if (currentSchema.$ref) {
      return validate(value, resolveRef(rootSchema, currentSchema.$ref), valuePath, rootSchema);
    }

    const errors = [];
    if (currentSchema.const !== undefined && !sameJson(value, currentSchema.const)) {
      errors.push(`${valuePath} must equal ${JSON.stringify(currentSchema.const)}`);
    }
    if (currentSchema.type && !typeMatches(value, currentSchema.type)) {
      errors.push(`${valuePath} must be ${Array.isArray(currentSchema.type) ? currentSchema.type.join(" or ") : currentSchema.type}`);
      return errors;
    }
    if (typeof value === "string") {
      if (Number.isInteger(currentSchema.minLength) && value.length < currentSchema.minLength) errors.push(`${valuePath} is shorter than ${currentSchema.minLength}`);
      if (currentSchema.pattern && !new RegExp(currentSchema.pattern).test(value)) errors.push(`${valuePath} does not match ${currentSchema.pattern}`);
    }
    if (typeof value === "number" && typeof currentSchema.minimum === "number" && value < currentSchema.minimum) {
      errors.push(`${valuePath} must be >= ${currentSchema.minimum}`);
    }
    if (Array.isArray(value)) {
      if (Number.isInteger(currentSchema.minItems) && value.length < currentSchema.minItems) errors.push(`${valuePath} must contain at least ${currentSchema.minItems} items`);
      if (currentSchema.uniqueItems && new Set(value.map((item) => JSON.stringify(item))).size !== value.length) errors.push(`${valuePath} must contain unique items`);
      if (currentSchema.items) value.forEach((item, index) => errors.push(...validate(item, currentSchema.items, `${valuePath}[${index}]`, rootSchema)));
    }
    if (isPlainObject(value)) {
      const properties = currentSchema.properties ?? {};
      for (const required of currentSchema.required ?? []) {
        if (!Object.hasOwn(value, required)) errors.push(`${valuePath}.${required} is required`);
      }
      for (const [key, item] of Object.entries(value)) {
        if (properties[key]) errors.push(...validate(item, properties[key], `${valuePath}.${key}`, rootSchema));
        else if (currentSchema.additionalProperties === false) errors.push(`${valuePath}.${key} is not allowed`);
      }
    }
    return errors;
  }

  return { validate };
}

function resolveRef(rootSchema, ref) {
  return ref
    .slice(2)
    .split("/")
    .reduce((value, segment) => value?.[segment.replace(/~1/g, "/").replace(/~0/g, "~")], rootSchema);
}

function typeMatches(value, type) {
  const types = Array.isArray(type) ? type : [type];
  return types.some((candidate) => {
    if (candidate === "array") return Array.isArray(value);
    if (candidate === "object") return isPlainObject(value);
    if (candidate === "integer") return Number.isInteger(value);
    if (candidate === "number") return typeof value === "number";
    if (candidate === "null") return value === null;
    return typeof value === candidate;
  });
}

function isPlainObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function sameJson(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function parseCliArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--contract") {
      options.contractPath = path.resolve(argv[(index += 1)]);
    } else if (arg === "--root") {
      options.root = path.resolve(argv[(index += 1)]);
    } else if (arg === "--schema") {
      options.schemaPath = path.resolve(argv[(index += 1)]);
    } else if (arg === "--skip-provenance-match") {
      options.requireProvenanceMatch = false;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return options;
}

const currentFile = fileURLToPath(import.meta.url);
if (process.argv[1] && path.resolve(process.argv[1]) === currentFile) {
  try {
    const result = verifyPublicContract(parseCliArgs(process.argv.slice(2)));
    if (!result.ok) {
      for (const error of result.errors) console.error(error);
      process.exitCode = 1;
    } else {
      console.log(JSON.stringify(result, null, 2));
    }
  } catch (error) {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  }
}
