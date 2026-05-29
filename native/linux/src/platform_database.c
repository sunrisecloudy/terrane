#include "platform_database.h"

static gchar *repo_root(void) {
  g_autofree gchar *cwd = g_get_current_dir();
  gchar *current = g_strdup(cwd);
  for (int depth = 0; depth < 5; ++depth) {
    g_autofree gchar *prd = g_build_filename(current, "docs", "00_PRD.md", NULL);
    if (g_file_test(prd, G_FILE_TEST_EXISTS)) {
      return current;
    }
    gchar *parent = g_path_get_dirname(current);
    g_free(current);
    current = parent;
  }
  return g_strdup(cwd);
}

static void exec_sql(sqlite3 *db, const gchar *sql, const gchar *label) {
  gchar *error = NULL;
  if (sqlite3_exec(db, sql, NULL, NULL, &error) != SQLITE_OK) {
    g_printerr("PlatformDatabase failed to apply %s: %s\n", label, error != NULL ? error : sqlite3_errmsg(db));
  }
  sqlite3_free(error);
}

static void apply_migration_file(sqlite3 *db, const gchar *path) {
  g_autofree gchar *contents = NULL;
  if (!g_file_get_contents(path, &contents, NULL, NULL)) {
    g_printerr("PlatformDatabase could not read migration: %s\n", path);
    return;
  }
  exec_sql(db, contents, path);
}

static void apply_checked_in_migrations(sqlite3 *db) {
  g_autofree gchar *root = repo_root();
  g_autofree gchar *migrations_dir = g_build_filename(root, "db", "sqlite", NULL);
  GDir *dir = g_dir_open(migrations_dir, 0, NULL);
  if (dir == NULL) {
    exec_sql(
        db,
        "CREATE TABLE IF NOT EXISTS apps (id TEXT PRIMARY KEY, name TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'enabled', data_version INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);"
        "CREATE TABLE IF NOT EXISTS app_storage (app_id TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(app_id, key));"
        "CREATE TABLE IF NOT EXISTS runtime_sessions (session_id TEXT PRIMARY KEY, target TEXT NOT NULL, platform TEXT NOT NULL, runtime_version TEXT NOT NULL, active_app_id TEXT, active_install_id TEXT, started_at TEXT NOT NULL, ended_at TEXT, status TEXT NOT NULL DEFAULT 'running', capabilities_json TEXT, resource_high_water_json TEXT, metadata_json TEXT);"
        "CREATE TABLE IF NOT EXISTS control_sessions (control_session_id TEXT PRIMARY KEY, target TEXT NOT NULL, runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL, actor TEXT NOT NULL DEFAULT 'codex', token_hash TEXT, started_at TEXT NOT NULL, ended_at TEXT, status TEXT NOT NULL DEFAULT 'running', metadata_json TEXT);"
        "CREATE TABLE IF NOT EXISTS control_commands (command_id TEXT PRIMARY KEY, control_session_id TEXT NOT NULL REFERENCES control_sessions(control_session_id) ON DELETE CASCADE, runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL, tool TEXT NOT NULL, http_method TEXT, path TEXT, decision TEXT, error_code TEXT, args_json TEXT, result_json TEXT, error_json TEXT, created_at TEXT NOT NULL, duration_ms INTEGER);"
        "CREATE INDEX IF NOT EXISTS idx_control_commands_session_created ON control_commands(control_session_id, created_at);",
        "fallback schema");
    return;
  }

  GList *names = NULL;
  for (const gchar *name = g_dir_read_name(dir); name != NULL; name = g_dir_read_name(dir)) {
    if (g_str_has_suffix(name, ".sql")) {
      names = g_list_prepend(names, g_strdup(name));
    }
  }
  g_dir_close(dir);
  names = g_list_sort(names, (GCompareFunc)g_strcmp0);

  for (GList *node = names; node != NULL; node = node->next) {
    g_autofree gchar *path = g_build_filename(migrations_dir, (const gchar *)node->data, NULL);
    apply_migration_file(db, path);
  }
  g_list_free_full(names, g_free);
}

static void run_integrity_check(sqlite3 *db) {
  sqlite3_stmt *statement = NULL;
  if (sqlite3_prepare_v2(db, "PRAGMA integrity_check", -1, &statement, NULL) != SQLITE_OK) {
    g_printerr("PlatformDatabase integrity_check prepare failed: %s\n", sqlite3_errmsg(db));
    return;
  }
  if (sqlite3_step(statement) == SQLITE_ROW) {
    const gchar *result = (const gchar *)sqlite3_column_text(statement, 0);
    if (g_strcmp0(result, "ok") != 0) {
      g_printerr("PlatformDatabase integrity_check failed: %s\n", result != NULL ? result : "unknown");
    }
  }
  sqlite3_finalize(statement);
}

sqlite3 *platform_database_open(const gchar *database_path) {
  g_autofree gchar *parent = g_path_get_dirname(database_path);
  g_mkdir_with_parents(parent, 0700);

  sqlite3 *db = NULL;
  if (sqlite3_open(database_path, &db) != SQLITE_OK) {
    g_printerr("PlatformDatabase open failed: %s\n", db != NULL ? sqlite3_errmsg(db) : "unknown");
    return db;
  }

  exec_sql(db, "PRAGMA foreign_keys = ON", "foreign_keys pragma");
  apply_checked_in_migrations(db);
  run_integrity_check(db);
  return db;
}

void platform_database_close(sqlite3 *db) {
  if (db != NULL) {
    sqlite3_close(db);
  }
}
