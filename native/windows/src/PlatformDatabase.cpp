#include "PlatformDatabase.h"

#include "BridgeTypes.h"

#include <Windows.h>
#include <algorithm>
#include <fstream>
#include <iterator>
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
  auto migrationsDir = RepoRoot() / L"db" / L"sqlite";
  if (!std::filesystem::exists(migrationsDir)) {
    ExecSql(
        "CREATE TABLE IF NOT EXISTS apps (id TEXT PRIMARY KEY, name TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'enabled', data_version INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);"
        "CREATE TABLE IF NOT EXISTS app_storage (app_id TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(app_id, key));",
        "fallback schema");
    return;
  }

  std::vector<std::filesystem::path> migrations;
  for (auto const& entry : std::filesystem::directory_iterator(migrationsDir)) {
    if (entry.is_regular_file() && entry.path().extension() == L".sql") {
      migrations.push_back(entry.path());
    }
  }
  std::sort(migrations.begin(), migrations.end());

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
