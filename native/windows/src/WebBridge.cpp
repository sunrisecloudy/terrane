#include "WebBridge.h"

#include <atomic>
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

std::wstring RuntimeSessionId(BridgeRequest const& request) {
  auto token = request.context.mountToken.empty() ? L"native" : request.context.mountToken;
  return L"runtime_windows_" + request.context.appId + L"_" + token;
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

std::wstring JsonMemberString(json::JsonObject const& object, std::wstring const& member) {
  if (!object.HasKey(member)) {
    return L"";
  }
  return std::wstring(object.GetNamedValue(member).Stringify().c_str());
}

}  // namespace

WebBridge::WebBridge(std::filesystem::path databasePath, HWND ownerWindow)
    : storage_(std::move(databasePath)), dialogs_(ownerWindow) {}

std::wstring WebBridge::HandleJson(std::wstring const& body, AppSandboxContext const& context) {
  auto startedAtMs = NowMs();
  BridgeRequest request;
  request.context = context;

  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(body, parsed)) {
    return BridgeResponse::Failure(L"", false, L"invalid_request", L"Bridge message body must be JSON").Stringify().c_str();
  }

  if (parsed.HasKey(L"id")) {
    request.hasId = true;
    request.id = parsed.GetNamedString(L"id", L"").c_str();
  }
  request.method = parsed.GetNamedString(L"method", L"").c_str();
  request.params = parsed.GetNamedObject(L"params", json::JsonObject());

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
    return response.Stringify().c_str();
  }

  auto response = Dispatch(request);
  RecordBridgeCall(request, response, startedAtMs);
  return response.Stringify().c_str();
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
    return dialogs_.OpenFile(request);
  }
  if (request.method == L"dialog.saveFile") {
    return dialogs_.SaveFile(request);
  }
  if (request.method == L"notification.toast") {
    return notifications_.Toast(request);
  }
  if (request.method == L"network.request") {
    return network_.Request(request);
  }
  if (request.method == L"core.step") {
    return core_.Step(request);
  }
  if (request.method == L"runtime.capabilities") {
    return Capabilities(request);
  }
  if (request.method == L"app.log") {
    json::JsonObject result;
    result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
    return BridgeResponse::Success(request.id, request.hasId, result);
  }
  return BridgeResponse::Failure(request.id, request.hasId, L"unknown_method", L"Unknown bridge method: " + request.method);
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

  json::JsonObject result;
  result.Insert(L"platform", json::JsonValue::CreateStringValue(L"windows"));
  result.Insert(L"target", json::JsonValue::CreateStringValue(L"windows"));
  result.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId));
  result.Insert(L"runtimeVersion", json::JsonValue::CreateStringValue(L"0.1.0"));
  result.Insert(L"devMode", json::JsonValue::CreateBooleanValue(true));
  result.Insert(L"features", features);
  result.Insert(L"limits", limits);
  return BridgeResponse::Success(request.id, request.hasId, result);
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
