#include "platform_storage.h"

static void ensure_app_row(PlatformStorage *storage, const gchar *app_id) {
  sqlite3_stmt *statement = NULL;
  sqlite3_prepare_v2(
      storage->db,
      "INSERT OR IGNORE INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, 'enabled', 1, datetime('now'), datetime('now'))",
      -1,
      &statement,
      NULL);
  sqlite3_bind_text(statement, 1, app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, app_id, -1, SQLITE_TRANSIENT);
  sqlite3_step(statement);
  sqlite3_finalize(statement);
}

static gboolean has_storage_prefix(const BridgeRequest *request, const gchar *key) {
  return g_str_has_prefix(key, request->context.storage_prefix);
}

static JsonNode *storage_prefix_failure(const BridgeRequest *request, const gchar *key) {
  JsonObject *details = json_object_new();
  json_object_set_string_member(details, "key", key);
  json_object_set_string_member(details, "prefix", request->context.storage_prefix);
  json_object_set_string_member(details, "appId", request->context.app_id);
  return bridge_failure(request, "permission_denied", "Storage key must begin with app storage prefix", details);
}

PlatformStorage *platform_storage_new(const gchar *database_path) {
  PlatformStorage *storage = g_new0(PlatformStorage, 1);
  storage->db = platform_database_open(database_path);
  return storage;
}

void platform_storage_free(PlatformStorage *storage) {
  if (storage == NULL) {
    return;
  }
  platform_database_close(storage->db);
  g_free(storage);
}

JsonNode *platform_storage_get(PlatformStorage *storage, const BridgeRequest *request) {
  const gchar *key = json_object_get_string_member_with_default(request->params, "key", "");
  if (*key == '\0') {
    return bridge_failure(request, "invalid_request", "storage.get requires key", NULL);
  }
  if (!has_storage_prefix(request, key)) {
    return storage_prefix_failure(request, key);
  }

  sqlite3_stmt *statement = NULL;
  sqlite3_prepare_v2(storage->db, "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, NULL);
  sqlite3_bind_text(statement, 1, request->context.app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, key, -1, SQLITE_TRANSIENT);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "value");
  if (sqlite3_step(statement) == SQLITE_ROW) {
    const gchar *json_text = (const gchar *)sqlite3_column_text(statement, 0);
    JsonParser *parser = json_parser_new();
    if (json_text != NULL && json_parser_load_from_data(parser, json_text, -1, NULL)) {
      json_builder_add_value(builder, json_node_copy(json_parser_get_root(parser)));
    } else {
      json_builder_add_null_value(builder);
    }
    g_object_unref(parser);
  } else {
    json_builder_add_null_value(builder);
  }
  json_builder_end_object(builder);
  sqlite3_finalize(statement);
  return bridge_success(request, json_builder_get_root(builder));
}

JsonNode *platform_storage_set(PlatformStorage *storage, const BridgeRequest *request) {
  const gchar *key = json_object_get_string_member_with_default(request->params, "key", "");
  if (*key == '\0') {
    return bridge_failure(request, "invalid_request", "storage.set requires key", NULL);
  }
  if (!has_storage_prefix(request, key)) {
    return storage_prefix_failure(request, key);
  }
  ensure_app_row(storage, request->context.app_id);

  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, json_object_get_member(request->params, "value"));
  g_autofree gchar *value_json = json_generator_to_data(generator, NULL);
  sqlite3_stmt *statement = NULL;
  sqlite3_prepare_v2(
      storage->db,
      "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, datetime('now')) "
      "ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
      -1,
      &statement,
      NULL);
  sqlite3_bind_text(statement, 1, request->context.app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, key, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 3, value_json, -1, SQLITE_TRANSIENT);
  sqlite3_step(statement);
  sqlite3_finalize(statement);
  g_object_unref(generator);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_set_member_name(builder, "bytesWritten");
  json_builder_add_int_value(builder, strlen(value_json));
  json_builder_end_object(builder);
  return bridge_success(request, json_builder_get_root(builder));
}

JsonNode *platform_storage_remove(PlatformStorage *storage, const BridgeRequest *request) {
  const gchar *key = json_object_get_string_member_with_default(request->params, "key", "");
  if (*key == '\0') {
    return bridge_failure(request, "invalid_request", "storage.remove requires key", NULL);
  }
  if (!has_storage_prefix(request, key)) {
    return storage_prefix_failure(request, key);
  }

  sqlite3_stmt *statement = NULL;
  sqlite3_prepare_v2(storage->db, "DELETE FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, NULL);
  sqlite3_bind_text(statement, 1, request->context.app_id, -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, key, -1, SQLITE_TRANSIENT);
  sqlite3_step(statement);
  sqlite3_finalize(statement);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_end_object(builder);
  return bridge_success(request, json_builder_get_root(builder));
}

JsonNode *platform_storage_list(PlatformStorage *storage, const BridgeRequest *request) {
  const gchar *prefix = json_object_get_string_member_with_default(request->params, "prefix", "");
  if (*prefix == '\0') {
    return bridge_failure(request, "invalid_request", "storage.list requires prefix", NULL);
  }
  if (!has_storage_prefix(request, prefix)) {
    return storage_prefix_failure(request, prefix);
  }

  sqlite3_stmt *statement = NULL;
  sqlite3_prepare_v2(storage->db, "SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key", -1, &statement, NULL);
  sqlite3_bind_text(statement, 1, request->context.app_id, -1, SQLITE_TRANSIENT);
  g_autofree gchar *like_prefix = g_strdup_printf("%s%%", prefix);
  sqlite3_bind_text(statement, 2, like_prefix, -1, SQLITE_TRANSIENT);

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "keys");
  json_builder_begin_array(builder);
  while (sqlite3_step(statement) == SQLITE_ROW) {
    json_builder_add_string_value(builder, (const gchar *)sqlite3_column_text(statement, 0));
  }
  json_builder_end_array(builder);
  json_builder_end_object(builder);
  sqlite3_finalize(statement);
  return bridge_success(request, json_builder_get_root(builder));
}
