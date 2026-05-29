#pragma once

#include <filesystem>
#include <winsqlite/winsqlite3.h>

namespace nativeai {

class PlatformDatabase {
 public:
  explicit PlatformDatabase(std::filesystem::path databasePath);
  ~PlatformDatabase();

  PlatformDatabase(PlatformDatabase const&) = delete;
  PlatformDatabase& operator=(PlatformDatabase const&) = delete;

  sqlite3* handle() const { return db_; }

 private:
  void ApplyCheckedInMigrations();
  void RunIntegrityCheck();
  void ExecSql(char const* sql, char const* label);

  sqlite3* db_ = nullptr;
};

}  // namespace nativeai
