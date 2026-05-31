#include "PlatformDatabase.h"

#include "BridgeTypes.h"

#include <Windows.h>
#include <algorithm>
#include <fstream>
#include <iterator>
#include <string>
#include <vector>

namespace nativeai {
namespace {

std::filesystem::path RepoRoot() {
  auto current = std::filesystem::current_path();
  for (int depth = 0; depth < 5; ++depth) {
    if (std::filesystem::exists(current / L"docs" / L"00_PRD.md")) {
      return current;
    }
    current = current.parent_path();
  }
  return std::filesystem::current_path();
}

std::filesystem::path ExecutableDirectory() {
  std::vector<wchar_t> buffer(MAX_PATH);
  while (true) {
    DWORD length = GetModuleFileNameW(nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
    if (length == 0) {
      return std::filesystem::current_path();
    }
    if (length < buffer.size()) {
      return std::filesystem::path(std::wstring(buffer.data(), length)).parent_path();
    }
    buffer.resize(buffer.size() * 2);
  }
}

std::vector<std::filesystem::path> MigrationDirCandidates() {
  return {
      ExecutableDirectory() / L"resources" / L"db" / L"sqlite",
      RepoRoot() / L"db" / L"sqlite",
  };
}

std::string ReadTextFile(std::filesystem::path const& path) {
  std::ifstream file(path, std::ios::binary);
  if (!file) {
    return {};
  }
  return std::string((std::istreambuf_iterator<char>(file)), std::istreambuf_iterator<char>());
}

void DebugLog(std::wstring const& message) {
  OutputDebugStringW((message + L"\n").c_str());
}

}  // namespace

PlatformDatabase::PlatformDatabase(std::filesystem::path databasePath) {
  std::filesystem::create_directories(databasePath.parent_path());
  if (sqlite3_open16(databasePath.c_str(), &db_) != SQLITE_OK) {
    DebugLog(L"PlatformDatabase open failed");
    return;
  }

  ExecSql("PRAGMA foreign_keys = ON", "foreign_keys pragma");
  ApplyCheckedInMigrations();
  RunIntegrityCheck();
}

PlatformDatabase::~PlatformDatabase() {
  if (db_ != nullptr) {
    sqlite3_close(db_);
  }
}

void PlatformDatabase::ExecSql(char const* sql, char const* label) {
  if (db_ == nullptr) {
    return;
  }
  char* error = nullptr;
  if (sqlite3_exec(db_, sql, nullptr, nullptr, &error) != SQLITE_OK) {
    DebugLog(L"PlatformDatabase failed to apply " + Utf8ToWide(label) + L": " + Utf8ToWide(error == nullptr ? sqlite3_errmsg(db_) : error));
  }
  sqlite3_free(error);
}

void PlatformDatabase::ApplyCheckedInMigrations() {
  std::filesystem::path migrationsDir;
  for (auto const& candidate : MigrationDirCandidates()) {
    if (std::filesystem::exists(candidate) && std::filesystem::is_directory(candidate)) {
      migrationsDir = candidate;
      break;
    }
  }

  std::vector<std::filesystem::path> migrations;
  if (!migrationsDir.empty()) {
    for (auto const& entry : std::filesystem::directory_iterator(migrationsDir)) {
      if (entry.is_regular_file() && entry.path().extension() == L".sql") {
        migrations.push_back(entry.path());
      }
    }
    std::sort(migrations.begin(), migrations.end());
  }

  if (migrations.empty()) {
    ExecSql(
        "CREATE TABLE IF NOT EXISTS apps (id TEXT PRIMARY KEY, name TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'enabled', data_version INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);"
        "CREATE TABLE IF NOT EXISTS app_storage (app_id TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(app_id, key));"
        "CREATE TABLE IF NOT EXISTS runtime_sessions (session_id TEXT PRIMARY KEY, target TEXT NOT NULL, platform TEXT NOT NULL, runtime_version TEXT NOT NULL, active_app_id TEXT, active_install_id TEXT, started_at TEXT NOT NULL, ended_at TEXT, status TEXT NOT NULL DEFAULT 'running', capabilities_json TEXT, resource_high_water_json TEXT, metadata_json TEXT);"
        "CREATE TABLE IF NOT EXISTS control_sessions (control_session_id TEXT PRIMARY KEY, target TEXT NOT NULL, runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL, actor TEXT NOT NULL DEFAULT 'codex', token_hash TEXT, started_at TEXT NOT NULL, ended_at TEXT, status TEXT NOT NULL DEFAULT 'running', metadata_json TEXT);"
        "CREATE TABLE IF NOT EXISTS control_commands (command_id TEXT PRIMARY KEY, control_session_id TEXT NOT NULL REFERENCES control_sessions(control_session_id) ON DELETE CASCADE, runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL, tool TEXT NOT NULL, http_method TEXT, path TEXT, decision TEXT, error_code TEXT, args_json TEXT, result_json TEXT, error_json TEXT, created_at TEXT NOT NULL, duration_ms INTEGER);"
        "CREATE INDEX IF NOT EXISTS idx_control_commands_session_created ON control_commands(control_session_id, created_at);",
        "fallback schema");
    return;
  }

  for (auto const& migration : migrations) {
    auto sql = ReadTextFile(migration);
    if (sql.empty()) {
      DebugLog(L"PlatformDatabase could not read migration: " + migration.wstring());
      continue;
    }
    ExecSql(sql.c_str(), migration.string().c_str());
  }
}

void PlatformDatabase::RunIntegrityCheck() {
  if (db_ == nullptr) {
    return;
  }
  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(db_, "PRAGMA integrity_check", -1, &statement, nullptr) != SQLITE_OK) {
    DebugLog(L"PlatformDatabase integrity_check prepare failed");
    return;
  }
  if (sqlite3_step(statement) == SQLITE_ROW) {
    auto result = reinterpret_cast<char const*>(sqlite3_column_text(statement, 0));
    if (result == nullptr || std::string(result) != "ok") {
      DebugLog(L"PlatformDatabase integrity_check failed: " + Utf8ToWide(result == nullptr ? "unknown" : result));
    }
  }
  sqlite3_finalize(statement);
}

}  // namespace nativeai
