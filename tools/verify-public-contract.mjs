#!/usr/bin/env node
// Self-check that terrane's public contract still matches the repo. A consumer
// (terrane-premium) runs this as a conformance command after pinning.
//
// 1. Re-derive the surface and confirm it equals the surface in the contract.
// 2. Re-hash the contract-defining files and confirm they're unchanged.
//
//   node --no-warnings tools/verify-public-contract.mjs [--contract <path>]
//
// With no --contract, it regenerates the contract and checks internal
// consistency (the surface is freshly derived, so this verifies the toolchain
// and file hashes line up).

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { buildPublicContract } from "./export-public-contract.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function arg(name, fallback) {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : fallback;
}

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}

const fresh = buildPublicContract();

const contractPath = arg("--contract", null);
const contract = contractPath
  ? JSON.parse(fs.readFileSync(path.resolve(contractPath), "utf8"))
  : fresh;

// 1. The pinned surface must equal the freshly derived one.
if (JSON.stringify(contract.surface) !== JSON.stringify(fresh.surface)) {
  fail(
    "contract.surface is stale — regenerate with tools/export-public-contract.mjs",
  );
}

// 2. Every listed file must still hash to its recorded value.
for (const file of contract.files ?? []) {
  const abs = path.join(root, file.path);
  if (!fs.existsSync(abs)) fail(`${file.path} is missing`);
  const bytes = fs.readFileSync(abs);
  const sha = crypto.createHash("sha256").update(bytes).digest("hex");
  if (bytes.length !== file.bytes || sha !== file.sha256) {
    fail(`${file.path} changed since the contract was generated`);
  }
}

console.log(
  `ok: terrane public contract verified (contractVersion ${contract.contractVersion}, ` +
    `${contract.surface.capabilities.length} capabilities, ${
      contract.files?.length ?? 0
    } files)`,
);
