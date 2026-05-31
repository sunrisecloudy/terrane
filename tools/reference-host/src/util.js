import crypto from "node:crypto";
import fs from "node:fs";

export function nowIso() {
  return new Date().toISOString();
}

export function id(prefix) {
  return `${prefix}_${crypto.randomUUID()}`;
}

export function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

export function canonicalJson(value) {
  return JSON.stringify(sortJson(value));
}

export function prettyJson(value) {
  return JSON.stringify(value, null, 2);
}

export function readJsonFile(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function sortJson(value) {
  if (Array.isArray(value)) {
    return value.map(sortJson);
  }

  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, sortJson(value[key])]),
    );
  }

  return value;
}
