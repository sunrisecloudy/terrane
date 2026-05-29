#include "PlatformStorage.h"

#include <utility>

namespace nativeai {
namespace json = winrt::Windows::Data::Json;

namespace {

json::JsonObject StorageError(BridgeRequest const& request, sqlite3* db, std::wstring const& operation) {
  json::JsonObject details;
  details.Insert(L"sqliteMessage", json::JsonValue::CreateStringValue(db == nullptr ? L"database unavailable" : Utf8ToWide(sqlite3_errmsg(db))));
  return BridgeResponse::Failure(request.id, request.hasId, L"storage_error", operation + L" failed", details);
}

}  // namespace

PlatformStorage::PlatformStorage(std::filesystem::path databasePath) : database_(std::move(databasePath)) {}

PlatformStorage::~PlatformStorage() = default;

bool PlatformStorage::EnsureAppRow(std::wstring const& appId) {
  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(
      database_.handle(),
      "INSERT OR IGNORE INTO apps (id, name, status, data_version, created_at, updated_at) VALUES (?, ?, 'enabled', 1, datetime('now'), datetime('now'))",
      -1,
      &statement,
      nullptr) != SQLITE_OK) {
    return false;
  }
  sqlite3_bind_text(statement, 1, WideToUtf8(appId).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, WideToUtf8(appId).c_str(), -1, SQLITE_TRANSIENT);
  const bool ok = sqlite3_step(statement) == SQLITE_DONE;
  sqlite3_finalize(statement);
  return ok;
}

json::JsonObject PlatformStorage::Get(BridgeRequest const& request) {
  auto key = std::wstring(request.params.GetNamedString(L"key", L"").c_str());
  if (key.empty()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"storage.get requires key");
  }
  if (!HasStoragePrefix(request, key)) {
    return storagePrefixFailure(request, key);
  }

  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(database_.handle(), "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, nullptr) != SQLITE_OK) {
    return StorageError(request, database_.handle(), L"storage.get");
  }
  sqlite3_bind_text(statement, 1, WideToUtf8(request.context.appId).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, WideToUtf8(key).c_str(), -1, SQLITE_TRANSIENT);

  json::JsonObject result;
  const int step = sqlite3_step(statement);
  if (step == SQLITE_ROW) {
    auto text = reinterpret_cast<char const*>(sqlite3_column_text(statement, 0));
    json::IJsonValue value = json::JsonValue::CreateNullValue();
    json::JsonValue::TryParse(Utf8ToWide(text == nullptr ? "" : text), value);
    result.Insert(L"value", value);
  } else if (step == SQLITE_DONE && request.params.HasKey(L"defaultValue")) {
    result.Insert(L"value", request.params.GetNamedValue(L"defaultValue"));
  } else if (step == SQLITE_DONE) {
    result.Insert(L"value", json::JsonValue::CreateNullValue());
  } else {
    sqlite3_finalize(statement);
    return StorageError(request, database_.handle(), L"storage.get");
  }
  sqlite3_finalize(statement);
  return BridgeResponse::Success(request.id, request.hasId, result);
}

json::JsonObject PlatformStorage::Set(BridgeRequest const& request) {
  auto key = std::wstring(request.params.GetNamedString(L"key", L"").c_str());
  if (key.empty()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"storage.set requires key");
  }
  if (!HasStoragePrefix(request, key)) {
    return storagePrefixFailure(request, key);
  }
  if (!EnsureAppRow(request.context.appId)) {
    return StorageError(request, database_.handle(), L"storage.set");
  }

  auto value = request.params.HasKey(L"value") ? std::wstring(request.params.GetNamedValue(L"value").Stringify().c_str()) : std::wstring(L"null");
  auto valueUtf8 = WideToUtf8(value.c_str());
  if (auto limit = request.context.resourceBudget.find(L"maxStorageBytes");
      limit != request.context.resourceBudget.end()) {
    auto projectedBytes = StorageBytesAfterSet(request.context.appId, key, static_cast<int64_t>(valueUtf8.size()));
    if (!projectedBytes.has_value()) {
      return StorageError(request, database_.handle(), L"storage.set");
    }
    if (projectedBytes.value() > static_cast<int64_t>(limit->second)) {
      json::JsonObject details;
      details.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId));
      details.Insert(L"key", json::JsonValue::CreateStringValue(key));
      details.Insert(L"budget", json::JsonValue::CreateStringValue(L"maxStorageBytes"));
      details.Insert(L"current", json::JsonValue::CreateNumberValue(static_cast<double>(projectedBytes.value())));
      details.Insert(L"max", json::JsonValue::CreateNumberValue(static_cast<double>(limit->second)));
      details.Insert(L"limit", json::JsonValue::CreateNumberValue(static_cast<double>(limit->second)));
      details.Insert(L"projectedBytes", json::JsonValue::CreateNumberValue(static_cast<double>(projectedBytes.value())));
      return BridgeResponse::Failure(
          request.id,
          request.hasId,
          L"resource_budget_exceeded",
          L"Storage write exceeds manifest.resourceBudget.maxStorageBytes",
          details);
    }
  }
  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(
      database_.handle(),
      "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, datetime('now')) "
      "ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
      -1,
      &statement,
      nullptr) != SQLITE_OK) {
    return StorageError(request, database_.handle(), L"storage.set");
  }
  sqlite3_bind_text(statement, 1, WideToUtf8(request.context.appId).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, WideToUtf8(key).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 3, valueUtf8.c_str(), -1, SQLITE_TRANSIENT);
  if (sqlite3_step(statement) != SQLITE_DONE) {
    sqlite3_finalize(statement);
    return StorageError(request, database_.handle(), L"storage.set");
  }
  sqlite3_finalize(statement);

  json::JsonObject result;
  result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
  result.Insert(L"bytesWritten", json::JsonValue::CreateNumberValue(static_cast<double>(valueUtf8.size())));
  return BridgeResponse::Success(request.id, request.hasId, result);
}

std::optional<int64_t> PlatformStorage::StorageBytesAfterSet(std::wstring const& appId, std::wstring const& key, int64_t valueBytes) const {
  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(
      database_.handle(),
      "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ? AND key != ?",
      -1,
      &statement,
      nullptr) != SQLITE_OK) {
    return std::nullopt;
  }
  sqlite3_bind_text(statement, 1, WideToUtf8(appId).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, WideToUtf8(key).c_str(), -1, SQLITE_TRANSIENT);
  const int step = sqlite3_step(statement);
  if (step != SQLITE_ROW) {
    sqlite3_finalize(statement);
    return std::nullopt;
  }
  auto currentOtherBytes = sqlite3_column_int64(statement, 0);
  sqlite3_finalize(statement);
  return currentOtherBytes + valueBytes;
}

json::JsonObject PlatformStorage::Remove(BridgeRequest const& request) {
  auto key = std::wstring(request.params.GetNamedString(L"key", L"").c_str());
  if (key.empty()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"storage.remove requires key");
  }
  if (!HasStoragePrefix(request, key)) {
    return storagePrefixFailure(request, key);
  }

  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(database_.handle(), "DELETE FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, nullptr) != SQLITE_OK) {
    return StorageError(request, database_.handle(), L"storage.remove");
  }
  sqlite3_bind_text(statement, 1, WideToUtf8(request.context.appId).c_str(), -1, SQLITE_TRANSIENT);
  sqlite3_bind_text(statement, 2, WideToUtf8(key).c_str(), -1, SQLITE_TRANSIENT);
  if (sqlite3_step(statement) != SQLITE_DONE) {
    sqlite3_finalize(statement);
    return StorageError(request, database_.handle(), L"storage.remove");
  }
  sqlite3_finalize(statement);

  json::JsonObject result;
  result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
  return BridgeResponse::Success(request.id, request.hasId, result);
}

json::JsonObject PlatformStorage::List(BridgeRequest const& request) {
  auto prefix = std::wstring(request.params.GetNamedString(L"prefix", L"").c_str());
  if (prefix.empty()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"storage.list requires prefix");
  }
  if (!HasStoragePrefix(request, prefix)) {
    return storagePrefixFailure(request, prefix);
  }

  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(database_.handle(), "SELECT key FROM app_storage WHERE app_id = ? AND key LIKE ? ORDER BY key", -1, &statement, nullptr) != SQLITE_OK) {
    return StorageError(request, database_.handle(), L"storage.list");
  }
  sqlite3_bind_text(statement, 1, WideToUtf8(request.context.appId).c_str(), -1, SQLITE_TRANSIENT);
  auto likePrefix = WideToUtf8(prefix) + "%";
  sqlite3_bind_text(statement, 2, likePrefix.c_str(), -1, SQLITE_TRANSIENT);

  json::JsonArray keys;
  int step = SQLITE_ROW;
  while ((step = sqlite3_step(statement)) == SQLITE_ROW) {
    auto text = reinterpret_cast<char const*>(sqlite3_column_text(statement, 0));
    keys.Append(json::JsonValue::CreateStringValue(Utf8ToWide(text == nullptr ? "" : text)));
  }
  if (step != SQLITE_DONE) {
    sqlite3_finalize(statement);
    return StorageError(request, database_.handle(), L"storage.list");
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
