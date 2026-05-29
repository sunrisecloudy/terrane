import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

const targetAssertions = [
  {
    name: "iOS",
    files: [
      {
        path: "native/ios/Package.swift",
        contains: ['.linkedLibrary("sqlite3")'],
      },
      {
        path: "native/ios/Sources/NativeAIHostIOS/PlatformStorage.swift",
        contains: [
          "import SQLite3",
          "sqlite3_open",
          "CREATE TABLE IF NOT EXISTS app_storage",
          "platform.sqlite",
          "WHERE app_id = ? AND key = ?",
          "request.context.appId",
        ],
      },
    ],
  },
  {
    name: "macOS",
    files: [
      {
        path: "native/macos/Package.swift",
        contains: ['.linkedLibrary("sqlite3")'],
      },
      {
        path: "native/macos/Sources/NativeAIHostMac/PlatformDatabase.swift",
        contains: [
          "final class PlatformDatabase",
          "sqlite3_open",
          "PRAGMA foreign_keys = ON",
          "PRAGMA integrity_check",
          'appendingPathComponent("db/sqlite")',
          "CREATE TABLE IF NOT EXISTS apps",
        ],
      },
      {
        path: "native/macos/Sources/NativeAIHostMac/PlatformStorage.swift",
        contains: [
          "import SQLite3",
          "PlatformDatabase(databaseURL: databaseURL)",
          "INSERT OR IGNORE INTO apps",
          "WHERE app_id = ? AND key = ?",
          "request.context.appId",
        ],
      },
    ],
  },
  {
    name: "Android",
    files: [
      {
        path: "native/android/app/build.gradle.kts",
        contains: ['from(repoRoot.resolve("db/sqlite"))', 'into("db/sqlite")'],
      },
      {
        path: "native/android/app/src/main/java/com/nativeai/platform/PlatformDatabase.kt",
        contains: [
          "class PlatformDatabase",
          'SQLiteOpenHelper(context, "platform.sqlite"',
          "PRAGMA foreign_keys = ON",
          "PRAGMA integrity_check",
          'assets.list("db/sqlite")',
          "CREATE TABLE IF NOT EXISTS apps",
        ],
      },
      {
        path: "native/android/app/src/main/java/com/nativeai/platform/PlatformStorage.kt",
        contains: [
          "import android.database.sqlite.SQLiteDatabase",
          "private val database = PlatformDatabase(context)",
          "insertWithOnConflict(\"apps\"",
          "SQLiteDatabase.CONFLICT_REPLACE",
          "arrayOf(request.context.appId, key)",
        ],
      },
    ],
  },
  {
    name: "Windows",
    files: [
      {
        path: "native/windows/src/PlatformDatabase.h",
        contains: ["class PlatformDatabase", "sqlite3* handle() const"],
      },
      {
        path: "native/windows/src/PlatformDatabase.cpp",
        contains: [
          "PRAGMA foreign_keys = ON",
          "PRAGMA integrity_check",
          'L"db" / L"sqlite"',
          "CREATE TABLE IF NOT EXISTS apps",
        ],
      },
      {
        path: "native/windows/src/PlatformStorage.h",
        contains: ['#include "PlatformDatabase.h"', "PlatformDatabase database_"],
      },
      {
        path: "native/windows/src/PlatformStorage.cpp",
        contains: [
          "database_(std::move(databasePath))",
          "INSERT OR IGNORE INTO apps",
          "WHERE app_id = ? AND key = ?",
          "request.context.appId",
        ],
      },
      {
        path: "native/windows/src/WebViewHost.cpp",
        contains: ["platform.sqlite"],
      },
      {
        path: "native/windows/CMakeLists.txt",
        contains: ["winsqlite3", "src/PlatformDatabase.cpp"],
      },
    ],
  },
  {
    name: "Linux",
    files: [
      {
        path: "native/linux/src/platform_database.h",
        contains: ["sqlite3 *platform_database_open", "platform_database_close"],
      },
      {
        path: "native/linux/src/platform_database.c",
        contains: [
          "PRAGMA foreign_keys = ON",
          "PRAGMA integrity_check",
          '"db", "sqlite"',
          "CREATE TABLE IF NOT EXISTS apps",
        ],
      },
      {
        path: "native/linux/src/platform_storage.h",
        contains: ['#include "platform_database.h"', "sqlite3 *db"],
      },
      {
        path: "native/linux/src/platform_storage.c",
        contains: [
          "platform_database_open",
          "INSERT OR IGNORE INTO apps",
          "WHERE app_id = ? AND key = ?",
          "request->context.app_id",
        ],
      },
      {
        path: "native/linux/src/webkit_host.c",
        contains: ["platform.sqlite"],
      },
      {
        path: "native/linux/meson.build",
        contains: ["dependency('sqlite3')", "sqlite_dep", "'src/platform_database.c'"],
      },
    ],
  },
];

const forbiddenStorageDefaults = [
  { pattern: /\bSharedPreferences\b/, label: "Android SharedPreferences" },
  { pattern: /\bUserDefaults\b/, label: "Apple UserDefaults" },
  { pattern: /\bNSUserDefaults\b/, label: "Apple NSUserDefaults" },
  { pattern: /\bstorage\.json\b/i, label: "JSON storage file" },
  { pattern: /\bapp_storage\.json\b/i, label: "JSON app storage file" },
  { pattern: /\bjson[_-]?file[_-]?storage\b/i, label: "JSON-file storage helper" },
];

function readRepoFile(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function assertContains(relativePath, text, expected, targetName) {
  assert.equal(
    text.includes(expected),
    true,
    `${targetName}: ${relativePath} must contain ${JSON.stringify(expected)}`,
  );
}

function collectNativeSourceFiles(directory) {
  const files = [];
  const entries = fs.readdirSync(directory, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.name.startsWith(".") || entry.name === "build" || entry.name === "DerivedData") {
      continue;
    }
    const absolutePath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectNativeSourceFiles(absolutePath));
      continue;
    }
    if (/\.(c|cpp|h|swift|kt|kts|xml|txt|properties|gradle|cmake)$/i.test(entry.name) || entry.name === "CMakeLists.txt") {
      files.push(absolutePath);
    }
  }
  return files;
}

test("native host scaffolds use SQLite-backed storage by default", () => {
  for (const target of targetAssertions) {
    for (const file of target.files) {
      const text = readRepoFile(file.path);
      for (const expected of file.contains) {
        assertContains(file.path, text, expected, target.name);
      }
    }
  }
});

test("native source does not use forbidden persistent storage defaults", () => {
  const nativeRoot = path.join(repoRoot, "native");
  const sourceFiles = collectNativeSourceFiles(nativeRoot);
  assert.equal(sourceFiles.length > 0, true, "native source files are present");

  for (const absolutePath of sourceFiles) {
    const relativePath = path.relative(repoRoot, absolutePath);
    const text = fs.readFileSync(absolutePath, "utf8");
    for (const forbidden of forbiddenStorageDefaults) {
      assert.equal(
        forbidden.pattern.test(text),
        false,
        `${relativePath} must not use ${forbidden.label}`,
      );
    }
  }
});
