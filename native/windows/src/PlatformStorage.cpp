#include "PlatformStorage.h"

#include <chrono>

namespace nativeai {
namespace json = winrt::Windows::Data::Json;

PlatformStorage::PlatformStorage(std::filesystem::path databasePath) {
  std::filesystem::create_directories(databasePath.parent_path());
  sqlite3_open16(databasePath.c_str(), &db_);
  sqlite3_exec(
      db_,
      "CREATE TABLE IF NOT EXISTS app_storage (app_id TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(app_id, key));",
      nullptr,
      nullptr,
      nullptr);
}

PlatformStorage::~PlatformStorage() {
  if (db_ != nullptr) {
    sqlite3_close(db_);
  }
}

json::JsonObject PlatformStorage::Get(BridgeRequest const& request) {
  auto key = request.params.GetNamedString(L"key", L"").c_str();
  if (key.empty()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"storage.get requires key");
  }
  if (!HasStoragePrefix(request, key)) {
    return storagePrefixFailure(request, key);
  }

  sqlite3_stmt* statement = nullptr;
  sqlite3_prepare_v2(db_, "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, nullptr);
  sqlite3_bind_text(statement, 1, WideToUtf8(request.context.appId).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, WideToUtf8(key).c_str(), -1, SQLITE_TRANSIENT);

  json::JsonObject result;
  if (sqlite3_step(statement) == SQLITE_ROW) {
    auto text = reinterpret_cast<char const*>(sqlite3_column_text(statement, 0));
    json::IJsonValue value = json::JsonValue::CreateNullValue();
    json::JsonValue::TryParse(Utf8ToWide(text == nullptr ? "" : text), value);
    result.Insert(L"value", value);
  } else if (request.params.HasKey(L"defaultValue")) {
    result.Insert(L"value", request.params.GetNamedValue(L"defaultValue"));
  } else {
    result.Insert(L"value", json::JsonValue::CreateNullValue());
  }
  sqlite3_finalize(statement);
  return BridgeResponse::Success(request.id, request.hasId, result);
}

json::JsonObject PlatformStorage::Set(BridgeRequest const& request) {
  auto key = request.params.GetNamedString(L"key", L"").c_str();
  if (key.empty()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"storage.set requires key");
  }
  if (!HasStoragePrefix(request, key)) {
    return storagePrefixFailure(request, key);
  }

  auto value = request.params.HasKey(L"value") ? request.params.GetNamedValue(L"value").Stringify() : L"null";
  sqlite3_stmt* statement = nullptr;
  sqlite3_prepare_v2(
      db_,
      "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, datetime('now')) "
      "ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
      -1,
      &statement,
      nullptr);
  sqlite3_bind_text(statement, 1, WideToUtf8(request.context.appId).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, WideToUtf8(key).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 3, WideToUtf8(value.c_str()).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_step(statement);
  sqlite3_finalize(statement);

  json::JsonObject result;
  result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
  result.Insert(L"bytesWritten", json::JsonValue::CreateNumberValue(static_cast<double>(value.size())));
  return BridgeResponse::Success(request.id, request.hasId, result);
}

json::JsonObject PlatformStorage::Remove(BridgeRequest const& request) {
  auto key = request.params.GetNamedString(L"key", L"").c_str();
  if (key.empty()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"storage.remove requires key");
  }
  if (!HasStoragePrefix(request, key)) {
    return storagePrefixFailure(request, key);
  }

  sqlite3_stmt* statement = nullptr;
  sqlite3_prepare_v2(db_, "DELETE FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, nullptr);
  sqlite3_bind_text(statement, 1, WideToUtf8(request.context.appId).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, WideToUtf8(key).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_step(statement);
  sqlite3_finalize(statement);

  json::JsonObject result;
  result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
  return BridgeResponse::Success(request.id, request.hasId, result);
}

json::JsonObject PlatformStorage::List(BridgeRequest const& request) {
  auto prefix = request.params.GetNamedString(L"prefix", L"").c_str();
  if (prefix.empty()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"storage.list requires prefix");
  }
  if (!HasStoragePrefix(request, prefix)) {
    return storagePrefixFailure(request, prefix);
  }

  sqlite3_stmt* statement = nullptr;
  sqlite3_prepare_v2(db_, "SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key", -1, &statement, nullptr);
  sqlite3_bind_text(statement, 1, WideToUtf8(request.context.appId).c_str(), -1, SQLITE_TRANSIENT);
  auto likePrefix = WideToUtf8(prefix) + "%";
  sqlite3_bind_text(statement, 2, likePrefix.c_str(), -1, SQLITE_TRANSIENT);

  json::JsonArray keys;
  while (sqlite3_step(statement) == SQLITE_ROW) {
    auto text = reinterpret_cast<char const*>(sqlite3_column_text(statement, 0));
    keys.Append(json::JsonValue::CreateStringValue(Utf8ToWide(text == nullptr ? "" : text)));
  }
  sqlite3_finalize(statement);

  json::JsonObject result;
  result.Insert(L"keys", keys);
  return BridgeResponse::Success(request.id, request.hasId, result);
}

json::JsonObject PlatformStorage::storagePrefixFailure(BridgeRequest const& request, std::wstring const& key) {
  json::JsonObject details;
  details.Insert(L"key", json::JsonValue::CreateStringValue(key));
  details.Insert(L"prefix", json::JsonValue::CreateStringValue(request.context.storagePrefix));
  details.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId));
  return BridgeResponse::Failure(
      request.id,
      request.hasId,
      L"permission_denied",
      L"Storage key must begin with " + request.context.storagePrefix,
      details);
}

bool PlatformStorage::HasStoragePrefix(BridgeRequest const& request, std::wstring const& key) const {
  return key.rfind(request.context.storagePrefix, 0) == 0;
}

}  // namespace nativeai
