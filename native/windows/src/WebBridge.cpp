#include "WebBridge.h"

#include <atomic>
#include <cmath>
#include <future>
#include <memory>
#include <string>

namespace nativeai {
namespace json = winrt::Windows::Data::Json;

namespace {

uint64_t NowMs() {
  return GetTickCount64();
}

std::wstring NewBridgeCallId() {
  static std::atomic_uint64_t sequence{0};
  return L"bridge_windows_" + std::to_wstring(GetCurrentProcessId()) + L"_" +
      std::to_wstring(NowMs()) + L"_" + std::to_wstring(sequence.fetch_add(1));
}

std::wstring NewCoreId(std::wstring const& prefix) {
  static std::atomic_uint64_t sequence{0};
  return prefix + L"_windows_" + std::to_wstring(GetCurrentProcessId()) + L"_" +
      std::to_wstring(NowMs()) + L"_" + std::to_wstring(sequence.fetch_add(1));
}

std::wstring RuntimeSessionId(BridgeRequest const& request) {
  auto token = request.context.mountToken.empty() ? L"native" : request.context.mountToken;
  return L"runtime_windows_" + request.context.appId + L"_" + token;
}

bool NativeDevMode() {
#ifdef _DEBUG
  return true;
#else
  return false;
#endif
}

void BindText(sqlite3_stmt* statement, int index, std::wstring const& value) {
  auto text = WideToUtf8(value);
  sqlite3_bind_text(statement, index, text.c_str(), -1, SQLITE_TRANSIENT);
}

void BindNullableText(sqlite3_stmt* statement, int index, std::wstring const& value) {
  if (value.empty()) {
    sqlite3_bind_null(statement, index);
    return;
  }
  BindText(statement, index, value);
}

void BindNullableInt64(sqlite3_stmt* statement, int index, std::optional<int64_t> value) {
  if (!value.has_value()) {
    sqlite3_bind_null(statement, index);
    return;
  }
  sqlite3_bind_int64(statement, index, static_cast<sqlite3_int64>(value.value()));
}

std::wstring JsonMemberString(json::JsonObject const& object, std::wstring const& member) {
  if (!object.HasKey(member)) {
    return L"";
  }
  return std::wstring(object.GetNamedValue(member).Stringify().c_str());
}

std::wstring ColumnText(sqlite3_stmt* statement, int index) {
  auto text = reinterpret_cast<char const*>(sqlite3_column_text(statement, index));
  return text == nullptr ? L"" : Utf8ToWide(text);
}

std::optional<int64_t> StateVersionBefore(json::JsonObject const& result) {
  if (!result.HasKey(L"stateVersion")) {
    return std::nullopt;
  }
  auto value = result.GetNamedValue(L"stateVersion");
  if (value.ValueType() != json::JsonValueType::Number) {
    return std::nullopt;
  }
  auto number = static_cast<int64_t>(value.GetNumber());
  return number > 0 ? number - 1 : 0;
}

bool HasOnlyBridgeRequestFields(json::JsonObject const& object, json::JsonArray& extraFields) {
  for (auto const& entry : object) {
    auto key = std::wstring(entry.Key().c_str());
    if (key != L"id" && key != L"method" && key != L"params" && key != L"timestamp") {
      extraFields.Append(json::JsonValue::CreateStringValue(key));
    }
  }
  return extraFields.Size() == 0;
}

}  // namespace

WebBridge::WebBridge(std::filesystem::path databasePath, HWND ownerWindow)
    : storage_(std::move(databasePath)), dialogs_(ownerWindow) {}

std::wstring WebBridge::HandleJson(std::wstring const& body, AppSandboxContext const& context) {
  auto promise = std::make_shared<std::promise<std::wstring>>();
  auto future = promise->get_future();
  HandleJsonAsync(body, context, [promise](std::wstring response) {
    try {
      promise->set_value(std::move(response));
    } catch (...) {
    }
  });
  return future.get();
}

void WebBridge::HandleJsonAsync(std::wstring body, AppSandboxContext context, BridgeCompletion completion) {
  auto startedAtMs = NowMs();
  BridgeRequest request;
  request.context = std::move(context);

  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(body, parsed)) {
    completion(BridgeResponse::Failure(L"", false, L"invalid_request", L"Bridge message body must be JSON").Stringify().c_str());
    return;
  }

  json::JsonArray extraFields;
  if (!HasOnlyBridgeRequestFields(parsed, extraFields)) {
    json::JsonObject details;
    details.Insert(L"fields", extraFields);
    completion(BridgeResponse::Failure(
                   L"",
                   false,
                   L"invalid_request",
                   L"Bridge request contains unknown top-level fields",
                   details)
                   .Stringify()
                   .c_str());
    return;
  }

  if (parsed.HasKey(L"timestamp")) {
    auto timestamp = parsed.GetNamedValue(L"timestamp");
    if (timestamp.ValueType() != json::JsonValueType::Number || !std::isfinite(timestamp.GetNumber())) {
      completion(BridgeResponse::Failure(
                     L"",
                     false,
                     L"invalid_request",
                     L"Bridge request timestamp must be a finite number")
                     .Stringify()
                     .c_str());
      return;
    }
  }
  if (!parsed.HasKey(L"id") ||
      parsed.GetNamedValue(L"id").ValueType() != json::JsonValueType::String ||
      std::wstring(parsed.GetNamedString(L"id", L"").c_str()).empty()) {
    completion(BridgeResponse::Failure(L"", false, L"invalid_request", L"Bridge request id must be a non-empty string")
                   .Stringify()
                   .c_str());
    return;
  }
  if (!parsed.HasKey(L"method") || parsed.GetNamedValue(L"method").ValueType() != json::JsonValueType::String) {
    completion(BridgeResponse::Failure(L"", false, L"invalid_request", L"Bridge request method must be a string").Stringify().c_str());
    return;
  }
  if (!parsed.HasKey(L"params") || parsed.GetNamedValue(L"params").ValueType() != json::JsonValueType::Object) {
    completion(BridgeResponse::Failure(L"", false, L"invalid_request", L"Bridge request params must be an object").Stringify().c_str());
    return;
  }

  request.hasId = true;
  request.id = parsed.GetNamedString(L"id", L"").c_str();
  request.method = parsed.GetNamedString(L"method", L"").c_str();
  request.params = parsed.GetNamedObject(L"params", json::JsonObject());

  if (request.params.HasKey(L"appId")) {
    json::JsonObject details;
    details.Insert(L"field", json::JsonValue::CreateStringValue(L"appId"));
    auto response = BridgeResponse::Failure(
        request.id,
        request.hasId,
        L"invalid_request",
        L"Bridge params must not include appId; app id is channel-derived",
        details);
    RecordBridgeCall(request, response, startedAtMs);
    completion(response.Stringify().c_str());
    return;
  }

  if (auto permission = permissionForBridgeMethod(request.method);
      permission.has_value() && !request.context.approvedPermissions.contains(permission.value())) {
    json::JsonObject details;
    details.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId));
    details.Insert(L"method", json::JsonValue::CreateStringValue(request.method));
    details.Insert(L"requiredPermission", json::JsonValue::CreateStringValue(permission.value()));
    auto response = BridgeResponse::Failure(
        request.id,
        request.hasId,
        L"permission_denied",
        L"App " + request.context.appId + L" cannot call " + request.method,
        details);
    RecordBridgeCall(request, response, startedAtMs);
    completion(response.Stringify().c_str());
    return;
  }
  if (auto budgetResponse = ResourceBudgetFailure(request); budgetResponse.has_value()) {
    RecordBridgeCall(request, budgetResponse.value(), startedAtMs);
    completion(budgetResponse.value().Stringify().c_str());
    return;
  }

  if (request.method == L"core.step") {
    core_.StepAsync(request, [this, request, startedAtMs, completion = std::move(completion)](json::JsonObject response) mutable {
      RecordBridgeCall(request, response, startedAtMs);
      RecordCoreStep(request, response);
      completion(response.Stringify().c_str());
    });
    return;
  }

  auto response = Dispatch(request);
  RecordBridgeCall(request, response, startedAtMs);
  RecordCoreStep(request, response);
  completion(response.Stringify().c_str());
}

std::optional<std::wstring> WebBridge::permissionForBridgeMethod(std::wstring const& method) const {
  if (method == L"storage.get" || method == L"storage.list") {
    return L"storage.read";
  }
  if (method == L"storage.set" || method == L"storage.remove") {
    return L"storage.write";
  }
  if (method == L"dialog.openFile" || method == L"dialog.saveFile" || method == L"notification.toast" ||
      method == L"network.request" || method == L"core.step") {
    return method;
  }
  return std::nullopt;
}

json::JsonObject WebBridge::Dispatch(BridgeRequest const& request) {
  if (request.method == L"storage.get") {
    return storage_.Get(request);
  }
  if (request.method == L"storage.set") {
    return storage_.Set(request);
  }
  if (request.method == L"storage.remove") {
    return storage_.Remove(request);
  }
  if (request.method == L"storage.list") {
    return storage_.List(request);
  }
  if (request.method == L"dialog.openFile") {
    if (auto mock = MockedDialogResponse(request, L"openFile")) {
      return mock.value();
    }
    return dialogs_.OpenFile(request);
  }
  if (request.method == L"dialog.saveFile") {
    if (auto mock = MockedDialogResponse(request, L"saveFile")) {
      return mock.value();
    }
    return dialogs_.SaveFile(request);
  }
  if (request.method == L"notification.toast") {
    return notifications_.Toast(request);
  }
  if (request.method == L"network.request") {
    return network_.Request(request, DatabaseHandle());
  }
  if (request.method == L"core.step") {
    return core_.Step(request);
  }
  if (request.method == L"runtime.capabilities") {
    return Capabilities(request);
  }
  if (request.method == L"app.log") {
    return AppLog(request);
  }
  return BridgeResponse::Failure(request.id, request.hasId, L"unknown_method", L"Unknown bridge method: " + request.method);
}

std::optional<json::JsonObject> WebBridge::MockedDialogResponse(
    BridgeRequest const& request,
    std::wstring const& dialogType) const {
  sqlite3* db = DatabaseHandle();
  if (db == nullptr || request.context.appId.empty()) {
    return std::nullopt;
  }
  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(
          db,
          "SELECT response_json FROM dialog_mocks "
          "WHERE enabled = 1 AND dialog_type = ? AND (app_id IS NULL OR app_id = ?) AND (session_id IS NULL OR session_id = ?) "
          "ORDER BY created_at DESC LIMIT 1",
          -1,
          &statement,
          nullptr) != SQLITE_OK) {
    return std::nullopt;
  }
  BindText(statement, 1, dialogType);
  BindText(statement, 2, request.context.appId);
  BindText(statement, 3, RuntimeSessionId(request));
  std::optional<json::JsonObject> response;
  if (sqlite3_step(statement) == SQLITE_ROW) {
    json::JsonValue parsed{nullptr};
    if (json::JsonValue::TryParse(ColumnText(statement, 0), parsed) && parsed.ValueType() == json::JsonValueType::Object) {
      response = BridgeResponse::Success(request.id, request.hasId, parsed.GetObject());
    }
  }
  sqlite3_finalize(statement);
  return response;
}

std::optional<json::JsonObject> WebBridge::ResourceBudgetFailure(BridgeRequest const& request) const {
  if (auto limit = request.context.resourceBudget.find(L"maxBridgeCallsPerMinute");
      limit != request.context.resourceBudget.end()) {
    auto current = BridgeCallCountSince(request.context.appId, 60);
    if (current >= static_cast<int>(limit->second)) {
      json::JsonObject details;
      details.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId));
      details.Insert(L"budget", json::JsonValue::CreateStringValue(L"maxBridgeCallsPerMinute"));
      details.Insert(L"current", json::JsonValue::CreateNumberValue(current));
      details.Insert(L"max", json::JsonValue::CreateNumberValue(limit->second));
      details.Insert(L"limit", json::JsonValue::CreateNumberValue(limit->second));
      return BridgeResponse::Failure(
          request.id,
          request.hasId,
          L"resource_budget_exceeded",
          L"Bridge call rate exceeds manifest.resourceBudget.maxBridgeCallsPerMinute",
          details);
    }
  }
  if (request.method == L"network.request") {
    if (auto limit = request.context.resourceBudget.find(L"maxNetworkRequestsPerMinute");
        limit != request.context.resourceBudget.end()) {
      auto current = BridgeCallCountSince(request.context.appId, L"network.request", 60);
      if (current >= static_cast<int>(limit->second)) {
        json::JsonObject details;
        details.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId));
        details.Insert(L"budget", json::JsonValue::CreateStringValue(L"maxNetworkRequestsPerMinute"));
        details.Insert(L"current", json::JsonValue::CreateNumberValue(current));
        details.Insert(L"max", json::JsonValue::CreateNumberValue(limit->second));
        details.Insert(L"limit", json::JsonValue::CreateNumberValue(limit->second));
        return BridgeResponse::Failure(
            request.id,
            request.hasId,
            L"resource_budget_exceeded",
            L"Network request rate exceeds manifest.resourceBudget.maxNetworkRequestsPerMinute",
            details);
      }
    }
  }
  return std::nullopt;
}

json::JsonObject WebBridge::Capabilities(BridgeRequest const& request) const {
  json::JsonObject features;
  features.Insert(L"storage.read", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"storage.write", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"storage.get", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"storage.set", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"storage.remove", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"storage.list", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"dialog.openFile", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"dialog.saveFile", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"notification.toast", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"network.request", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"core.step", json::JsonValue::CreateBooleanValue(core_.IsAvailable()));
  features.Insert(L"runtime.capabilities", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"app.log", json::JsonValue::CreateBooleanValue(true));

  json::JsonObject limits;
  limits.Insert(L"maxPackageBytes", json::JsonValue::CreateNumberValue(1048576));
  limits.Insert(L"maxFileBytes", json::JsonValue::CreateNumberValue(524288));
  for (auto const& [key, value] : request.context.resourceBudget) {
    limits.Insert(key, json::JsonValue::CreateNumberValue(value));
  }

  json::JsonObject result;
  result.Insert(L"platform", json::JsonValue::CreateStringValue(L"windows"));
  result.Insert(L"target", json::JsonValue::CreateStringValue(L"windows"));
  result.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId));
  result.Insert(L"runtimeVersion", json::JsonValue::CreateStringValue(L"0.1.0"));
  result.Insert(L"devMode", json::JsonValue::CreateBooleanValue(NativeDevMode()));
  result.Insert(L"features", features);
  result.Insert(L"limits", limits);
  return BridgeResponse::Success(request.id, request.hasId, result);
}

json::JsonObject WebBridge::AppLog(BridgeRequest const& request) const {
  auto level = std::wstring(request.params.GetNamedString(L"level", L"").c_str());
  if (level != L"debug" && level != L"info" && level != L"warn" && level != L"error") {
    return BridgeResponse::Failure(
        request.id,
        request.hasId,
        L"invalid_request",
        L"app.log level must be debug, info, warn, or error");
  }
  auto message = std::wstring(request.params.GetNamedString(L"message", L"").c_str());
  if (message.empty()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"app.log requires message");
  }
  if (auto limit = request.context.resourceBudget.find(L"maxLogLinesPerMinute");
      limit != request.context.resourceBudget.end()) {
    auto current = BridgeCallCountSince(request.context.appId, L"app.log", 60);
    if (current >= static_cast<int>(limit->second)) {
      json::JsonObject details;
      details.Insert(L"budget", json::JsonValue::CreateStringValue(L"maxLogLinesPerMinute"));
      details.Insert(L"current", json::JsonValue::CreateNumberValue(current));
      details.Insert(L"max", json::JsonValue::CreateNumberValue(limit->second));
      details.Insert(L"limit", json::JsonValue::CreateNumberValue(limit->second));
      return BridgeResponse::Failure(
          request.id,
          request.hasId,
          L"resource_budget_exceeded",
          L"Log rate exceeds manifest.resourceBudget.maxLogLinesPerMinute",
          details);
    }
  }
  OutputDebugStringW((L"Generated app log [" + request.context.appId + L"] " + message + L"\n").c_str());
  json::JsonObject result;
  result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
  return BridgeResponse::Success(request.id, request.hasId, result);
}

int WebBridge::BridgeCallCountSince(std::wstring const& appId, int seconds) const {
  auto db = storage_.DatabaseHandle();
  if (db == nullptr) {
    return 0;
  }
  sqlite3_stmt* statement = nullptr;
  constexpr char const* sql =
      "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND datetime(created_at) >= datetime('now', ?)";
  if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
    return 0;
  }
  BindText(statement, 1, appId);
  BindText(statement, 2, L"-" + std::to_wstring(seconds) + L" seconds");
  int count = sqlite3_step(statement) == SQLITE_ROW ? sqlite3_column_int(statement, 0) : 0;
  sqlite3_finalize(statement);
  return count;
}

int WebBridge::BridgeCallCountSince(std::wstring const& appId, std::wstring const& method, int seconds) const {
  auto db = storage_.DatabaseHandle();
  if (db == nullptr) {
    return 0;
  }
  sqlite3_stmt* statement = nullptr;
  constexpr char const* sql =
      "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ? AND datetime(created_at) >= datetime('now', ?)";
  if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
    return 0;
  }
  BindText(statement, 1, appId);
  BindText(statement, 2, method);
  BindText(statement, 3, L"-" + std::to_wstring(seconds) + L" seconds");
  int count = sqlite3_step(statement) == SQLITE_ROW ? sqlite3_column_int(statement, 0) : 0;
  sqlite3_finalize(statement);
  return count;
}

void WebBridge::EnsureRuntimeSession(BridgeRequest const& request) {
  auto db = storage_.DatabaseHandle();
  if (db == nullptr || request.context.appId.empty()) {
    return;
  }

  sqlite3_stmt* statement = nullptr;
  constexpr char const* sql =
      "INSERT INTO runtime_sessions "
      "(session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, metadata_json) "
      "VALUES (?, 'windows', 'windows', '0.1.0', ?, NULL, datetime('now'), 'running', '{}', '{\"source\":\"native-windows-bridge\"}') "
      "ON CONFLICT(session_id) DO UPDATE SET active_app_id = excluded.active_app_id, status = 'running'";
  if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
    return;
  }
  BindText(statement, 1, RuntimeSessionId(request));
  BindText(statement, 2, request.context.appId);
  sqlite3_step(statement);
  sqlite3_finalize(statement);
}

void WebBridge::RecordCoreStep(BridgeRequest const& request, json::JsonObject const& response) {
  auto db = storage_.DatabaseHandle();
  if (db == nullptr || request.context.appId.empty() || request.method != L"core.step" || !request.params.HasKey(L"event")) {
    return;
  }
  auto ok = response.GetNamedValue(L"ok", json::JsonValue::CreateBooleanValue(false));
  if (ok.ValueType() != json::JsonValueType::Boolean || !ok.GetBoolean() || !response.HasKey(L"result")) {
    return;
  }
  auto resultValue = response.GetNamedValue(L"result");
  if (resultValue.ValueType() != json::JsonValueType::Object) {
    return;
  }
  auto result = resultValue.GetObject();
  EnsureRuntimeSession(request);

  auto sessionId = RuntimeSessionId(request);
  auto eventId = NewCoreId(L"core_event");
  sqlite3_stmt* statement = nullptr;
  constexpr char const* sql =
      "INSERT INTO core_events "
      "(event_id, session_id, app_id, install_id, state_version_before, event_json, created_at) "
      "VALUES (?, ?, ?, NULL, ?, ?, datetime('now'))";
  if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
    return;
  }
  BindText(statement, 1, eventId);
  BindText(statement, 2, sessionId);
  BindText(statement, 3, request.context.appId);
  BindNullableInt64(statement, 4, StateVersionBefore(result));
  BindText(statement, 5, std::wstring(request.params.GetNamedValue(L"event").Stringify().c_str()));
  auto inserted = sqlite3_step(statement) == SQLITE_DONE;
  sqlite3_finalize(statement);
  if (!inserted || !result.HasKey(L"actions")) {
    return;
  }

  auto actionsValue = result.GetNamedValue(L"actions");
  if (actionsValue.ValueType() != json::JsonValueType::Array) {
    return;
  }
  for (auto const& action : actionsValue.GetArray()) {
    RecordCoreAction(eventId, sessionId, request.context.appId, action);
  }
}

void WebBridge::RecordCoreAction(
    std::wstring const& eventId,
    std::wstring const& sessionId,
    std::wstring const& appId,
    json::IJsonValue const& action) {
  auto db = storage_.DatabaseHandle();
  if (db == nullptr) {
    return;
  }
  sqlite3_stmt* statement = nullptr;
  constexpr char const* sql =
      "INSERT INTO core_actions (action_id, event_id, session_id, app_id, action_json, created_at) "
      "VALUES (?, ?, ?, ?, ?, datetime('now'))";
  if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
    return;
  }
  BindText(statement, 1, NewCoreId(L"core_action"));
  BindText(statement, 2, eventId);
  BindText(statement, 3, sessionId);
  BindText(statement, 4, appId);
  BindText(statement, 5, std::wstring(action.Stringify().c_str()));
  sqlite3_step(statement);
  sqlite3_finalize(statement);
}

void WebBridge::RecordBridgeCall(
    BridgeRequest const& request,
    json::JsonObject const& response,
    uint64_t startedAtMs) {
  auto db = storage_.DatabaseHandle();
  if (db == nullptr || request.context.appId.empty()) {
    return;
  }
  EnsureRuntimeSession(request);

  sqlite3_stmt* statement = nullptr;
  constexpr char const* sql =
      "INSERT INTO bridge_calls "
      "(bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at) "
      "VALUES (?, ?, ?, NULL, ?, ?, ?, ?, ?, datetime('now'))";
  if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
    return;
  }
  BindText(statement, 1, NewBridgeCallId());
  BindText(statement, 2, RuntimeSessionId(request));
  BindText(statement, 3, request.context.appId);
  BindText(statement, 4, request.method);
  BindText(statement, 5, std::wstring(request.params.Stringify().c_str()));
  BindNullableText(statement, 6, JsonMemberString(response, L"result"));
  BindNullableText(statement, 7, JsonMemberString(response, L"error"));
  sqlite3_bind_int64(statement, 8, static_cast<sqlite3_int64>(NowMs() - startedAtMs));
  sqlite3_step(statement);
  sqlite3_finalize(statement);
}

}  // namespace nativeai
