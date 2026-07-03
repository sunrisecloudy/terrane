#!/usr/bin/env node
// Emit terrane's public-contract.json — the artifact terrane-premium pins.
//
// The authoritative *surface* (host API, capabilities, ctx.resource, app
// contract, sync) comes from `terrane contract export`, derived entirely from
// the Rust declarations so it can't drift from the running system. This wrapper
// adds the cross-cutting bits a build tool owns: provenance (git commit),
// license, the conformance commands a consumer must run, and integrity hashes of
// the contract-defining files.
//
//   node --no-warnings tools/export-public-contract.mjs [--out <path>]

import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function arg(name, fallback) {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : fallback;
}

// The files whose bytes define the contract (so a consumer can detect drift).
const CONTRACT_FILES = [
  "docs/SERVER_API.md",
  "docs/APP_API.md",
  "rust/crates/terrane-api/src/lib.rs",
  "rust/crates/terrane-cap-interface/src/lib.rs",
  "rust/crates/terrane-cap-interface/src/manifest.rs",
  "rust/crates/terrane-core/src/lib.rs",
  "rust/crates/terrane-host/src/cap_doc.rs",
  "rust/crates/terrane-host/src/cli.rs",
  "rust/crates/terrane-host/src/mcp.rs",
];

export function buildPublicContract() {
  const surfaceJson = execFileSync(
    "cargo",
    ["run", "-q", "-p", "terrane-host", "--bin", "terrane", "--", "contract", "export"],
    {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
    },
  );
  const surface = JSON.parse(surfaceJson);

  let sourceCommit = "uncommitted";
  try {
    sourceCommit = execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: root,
      encoding: "utf8",
    }).trim();
  } catch {
    /* not a git checkout */
  }

  const files = CONTRACT_FILES.map((rel) => {
    const bytes = fs.readFileSync(path.join(root, rel));
    return {
      path: rel,
      bytes: bytes.length,
      sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
    };
  });

  return {
    schemaVersion: 1,
    contractId: "terrane-public-contract",
    contractVersion: surface.contract_version,
    license: "MIT OR Apache-2.0",
    provenance: {
      sourceRepository: "terrane",
      sourceCommit,
      generatedFrom: "terrane contract export (terrane-api + terrane-core)",
    },
    surface,
    conformance: {
      sourceCheckoutRequired: true,
      commands: [
        "cargo test --workspace --locked",
        "cargo clippy --workspace --all-targets --locked -- -D warnings",
        "node --no-warnings tools/verify-public-contract.mjs",
      ],
    },
    files,
  };
}

// Run as a script (not when imported by the verifier).
if (path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const out = path.resolve(
    arg("--out", path.join(root, "artifacts", "public-contract.json")),
  );
  const contract = buildPublicContract();
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, JSON.stringify(contract, null, 2) + "\n");
  console.error(
    `wrote ${out} (contractVersion ${contract.contractVersion}, ${contract.files.length} files hashed)`,
  );
}
