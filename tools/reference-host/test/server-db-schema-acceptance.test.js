import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const logicalJsonColumns = [
  "manifest_json",
  "signature_json",
  "details_json",
  "value_json",
  "capabilities_json",
  "resource_high_water_json",
  "metadata_json",
  "params_json",
  "result_json",
  "error_json",
  "event_json",
  "action_json",
  "spec_json",
  "diagnostics_json",
  "response_json",
  "migration_json",
  "report_json",
  "validation_json",
  "security_json",
  "permissions_json",
  "compatibility_json",
  "smoke_test_json",
  "export_json",
  "snapshot_json",
  "operation_json",
  "frontier_json",
  "policy_json",
  "proposal_json",
];

function readRepoFile(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function sqlText(relativeDir) {
  return fs
    .readdirSync(path.join(repoRoot, relativeDir))
    .filter((fileName) => fileName.endsWith(".sql"))
    .sort()
    .map((fileName) => readRepoFile(path.join(relativeDir, fileName)))
    .join("\n");
}

function serverSchemaText() {
  return readRepoFile("server/src/main.zig").replace(/^[ \t]*\\\\/gm, "");
}

function parseSqlSchema(sql) {
  const schema = new Map();
  const tablePattern = /CREATE\s+TABLE\s+IF\s+NOT\s+EXISTS\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(([\s\S]*?)\);/gi;
  let match;
  while ((match = tablePattern.exec(sql))) {
    const [, table, body] = match;
    const columns = new Set();
    for (const rawLine of body.split("\n")) {
      const line = rawLine.trim().replace(/,$/, "");
      if (!line || line.startsWith("--")) continue;
      const column = line.split(/\s+/)[0]?.replace(/"/g, "");
      if (!column || /^(PRIMARY|FOREIGN|CONSTRAINT|CHECK|UNIQUE|KEY)$/i.test(column)) continue;
      columns.add(column);
    }
    schema.set(table, columns);
  }
  return schema;
}

function assertSameLogicalColumns(actualSchema, expectedSchema, actualName, expectedName) {
  assert.deepEqual(
    [...actualSchema.keys()].sort(),
    [...expectedSchema.keys()].sort(),
    `${actualName} tables must match ${expectedName}`,
  );

  for (const [table, expectedColumns] of expectedSchema) {
    assert.deepEqual(
      [...(actualSchema.get(table) ?? [])].sort(),
      [...expectedColumns].sort(),
      `${actualName}.${table} columns must match ${expectedName}.${table}`,
    );
  }
}

test("server opens the v0.4 schema through SQLite by default", () => {
  const build = readRepoFile("server/build.zig");
  const source = readRepoFile("server/src/main.zig");

  assert.match(build, /server\.linkSystemLibrary\("sqlite3"\)/);
  assert.match(build, /tests\.linkSystemLibrary\("sqlite3"\)/);
  assert.match(source, /@cInclude\("sqlite3\.h"\)/);
  assert.match(source, /getEnvVarOwned\(allocator, "NATIVE_AI_SERVER_DB"\)/);
  assert.match(source, /allocator\.dupe\(u8, "server-platform\.sqlite"\)/);
  assert.match(source, /sqlite3_open\(path_z\.ptr, &db\)/);
  assert.match(source, /PRAGMA foreign_keys = ON/);
  assert.match(source, /\\"db\\":\\"sqlite\\"/);
});

test("server inline SQLite schema matches checked-in SQLite logical schema", () => {
  const checkedInSqlite = parseSqlSchema(sqlText("db/sqlite"));
  const serverSqlite = parseSqlSchema(serverSchemaText());

  assert.equal(checkedInSqlite.size, 30);
  assertSameLogicalColumns(serverSqlite, checkedInSqlite, "server", "db/sqlite");
});

test("Postgres schema mirrors the server logical schema", () => {
  const serverSqlite = parseSqlSchema(serverSchemaText());
  const postgres = parseSqlSchema(sqlText("db/postgres"));
  const postgresText = sqlText("db/postgres");

  assertSameLogicalColumns(postgres, serverSqlite, "db/postgres", "server");
  assert.match(postgresText, /PRIMARY KEY\s*\(\s*app_id\s*,\s*key\s*\)/i);
  for (const column of logicalJsonColumns) {
    if (postgresText.includes(column)) {
      assert.match(postgresText, new RegExp(`${column}\\s+JSONB`, "i"), `${column} must use JSONB in Postgres`);
    }
  }
});
