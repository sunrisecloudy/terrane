#include "WebViewHost.h"

#include <ShlObj.h>
#include <algorithm>
#include <cwctype>
#include <fstream>
#include <winrt/Windows.Data.Json.h>
#include <wrl/event.h>

namespace nativeai {
namespace json = winrt::Windows::Data::Json;
using Microsoft::WRL::Callback;
using Microsoft::WRL::ComPtr;

namespace {
constexpr wchar_t kRuntimeHost[] = L"runtime.local.platform";
constexpr wchar_t kRuntimeOrigin[] = L"https://runtime.local.platform/";

std::wstring ReadTextFile(std::filesystem::path const& path) {
  std::ifstream file(path, std::ios::binary);
  if (!file) {
    return {};
  }
  std::string text((std::istreambuf_iterator<char>(file)), std::istreambuf_iterator<char>());
  return Utf8ToWide(text);
}

std::filesystem::path ExecutableDirectory() {
  std::wstring buffer(MAX_PATH, L'\0');
  DWORD length = GetModuleFileNameW(nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
  if (length == 0) {
    return std::filesystem::current_path();
  }
  buffer.resize(length);
  return std::filesystem::path(buffer).parent_path();
}

json::JsonObject ManifestForApp(std::filesystem::path const& repoRoot, std::wstring const& appId) {
  auto manifest = ReadTextFile(repoRoot / L"webapps" / L"examples" / appId / L"manifest.json");
  json::JsonObject parsed{nullptr};
  if (manifest.empty() || !json::JsonObject::TryParse(manifest, parsed)) {
    return json::JsonObject();
  }
  return parsed;
}

bool IsRuntimeEnvelope(json::JsonObject const& body) {
  return body.HasKey(L"appId") || body.HasKey(L"mountToken") || body.HasKey(L"request");
}

bool IsKnownExampleAppId(std::wstring const& appId) {
  for (auto const* candidate : {L"notes-lite", L"task-workbench", L"file-transformer", L"api-dashboard", L"core-replay-lab"}) {
    if (appId == candidate) {
      return true;
    }
  }
  return false;
}

std::wstring RuntimeEnvelopeRequestId(json::JsonObject const& body) {
  if (!body.HasKey(L"request")) {
    return L"";
  }
  auto requestValue = body.GetNamedValue(L"request");
  if (requestValue.ValueType() != json::JsonValueType::Object) {
    return L"";
  }
  return std::wstring(requestValue.GetObject().GetNamedString(L"id", L"").c_str());
}

bool HasValidRuntimeEnvelope(json::JsonObject const& body) {
  if (!body.HasKey(L"appId") || !body.HasKey(L"mountToken") || !body.HasKey(L"request")) {
    return false;
  }
  auto appId = std::wstring(body.GetNamedString(L"appId", L"").c_str());
  auto mountToken = std::wstring(body.GetNamedString(L"mountToken", L"").c_str());
  if (appId.empty() || mountToken.empty()) {
    return false;
  }
  return body.GetNamedValue(L"request").ValueType() == json::JsonValueType::Object;
}

std::wstring ToUpper(std::wstring value) {
  std::transform(value.begin(), value.end(), value.begin(), [](wchar_t ch) { return static_cast<wchar_t>(std::towupper(ch)); });
  return value;
}

std::wstring ToLower(std::wstring value) {
  std::transform(value.begin(), value.end(), value.begin(), [](wchar_t ch) { return static_cast<wchar_t>(std::towlower(ch)); });
  return value;
}

std::wstring EnvironmentValue(wchar_t const* name) {
  DWORD required = GetEnvironmentVariableW(name, nullptr, 0);
  if (required == 0) {
    return L"";
  }
  std::wstring value(required, L'\0');
  DWORD written = GetEnvironmentVariableW(name, value.data(), required);
  if (written == 0 || written >= required) {
    return L"";
  }
  value.resize(written);
  return value;
}

bool EnvironmentIsOne(wchar_t const* name) {
  return EnvironmentValue(name) == L"1";
}

void BindSmokeSqlText(sqlite3_stmt* statement, int index, std::wstring const& value) {
  auto text = WideToUtf8(value);
  sqlite3_bind_text(statement, index, text.c_str(), -1, SQLITE_TRANSIENT);
}

void WriteSmokeLine(std::wstring const& line) {
  auto markerPath = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_RESULT_FILE");
  if (!markerPath.empty()) {
    std::ofstream file{std::filesystem::path(markerPath), std::ios::binary | std::ios::app};
    file << WideToUtf8(line) << "\n";
  }
  OutputDebugStringW((line + L"\n").c_str());
}

bool JsonResponseOk(std::wstring const& response) {
  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(response, parsed) || !parsed.HasKey(L"ok")) {
    return false;
  }
  auto ok = parsed.GetNamedValue(L"ok");
  return ok.ValueType() == json::JsonValueType::Boolean && ok.GetBoolean();
}

bool StorageSmokeResponseMatches(std::wstring const& response, std::wstring const& value) {
  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(response, parsed)) {
    return false;
  }
  auto result = parsed.GetNamedObject(L"result", json::JsonObject());
  auto storedValue = result.GetNamedValue(L"value", json::JsonValue::CreateNullValue());
  if (storedValue.ValueType() != json::JsonValueType::Object) {
    return false;
  }
  auto stored = storedValue.GetObject();
  return std::wstring(stored.GetNamedString(L"smokeValue", L"").c_str()) == value;
}

bool StorageListResponseContains(std::wstring const& response, std::wstring const& key) {
  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(response, parsed)) {
    return false;
  }
  auto result = parsed.GetNamedObject(L"result", json::JsonObject());
  auto keysValue = result.GetNamedValue(L"keys", json::JsonValue::CreateNullValue());
  if (keysValue.ValueType() != json::JsonValueType::Array) {
    return false;
  }
  for (auto const& item : keysValue.GetArray()) {
    if (item.ValueType() == json::JsonValueType::String && std::wstring(item.GetString().c_str()) == key) {
      return true;
    }
  }
  return false;
}

bool StorageGetResponseIsNull(std::wstring const& response) {
  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(response, parsed)) {
    return false;
  }
  auto result = parsed.GetNamedObject(L"result", json::JsonObject());
  auto storedValue = result.GetNamedValue(L"value", json::JsonValue::CreateBooleanValue(false));
  return storedValue.ValueType() == json::JsonValueType::Null;
}

bool JsonResponseErrorCodeMatches(std::wstring const& response, std::wstring const& code) {
  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(response, parsed)) {
    return false;
  }
  auto error = parsed.GetNamedObject(L"error", json::JsonObject());
  return std::wstring(error.GetNamedString(L"code", L"").c_str()) == code;
}

std::wstring ScriptStringResult(PCWSTR resultObjectAsJson) {
  if (resultObjectAsJson == nullptr) {
    return L"";
  }
  json::JsonValue parsed{nullptr};
  if (!json::JsonValue::TryParse(resultObjectAsJson, parsed)) {
    return resultObjectAsJson;
  }
  return parsed.ValueType() == json::JsonValueType::String
      ? std::wstring(parsed.GetString().c_str())
      : std::wstring(resultObjectAsJson);
}
}  // namespace

WebViewHost::WebViewHost(HWND window) : window_(window), bridge_(std::make_unique<WebBridge>(DatabasePath(), window)) {}

void WebViewHost::Initialize() {
  CreateCoreWebView2EnvironmentWithOptions(
      nullptr,
      nullptr,
      nullptr,
      Callback<ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler>(
          [this](HRESULT result, ICoreWebView2Environment* environment) -> HRESULT {
            if (FAILED(result) || environment == nullptr) {
              return result;
            }
            environment->CreateCoreWebView2Controller(
                window_,
                Callback<ICoreWebView2CreateCoreWebView2ControllerCompletedHandler>(
                    [this](HRESULT controllerResult, ICoreWebView2Controller* controller) -> HRESULT {
                      if (FAILED(controllerResult) || controller == nullptr) {
                        return controllerResult;
                      }
                      controller_ = controller;
                      controller_->get_CoreWebView2(&webview_);

                      RECT bounds{};
                      GetClientRect(window_, &bounds);
                      controller_->put_Bounds(bounds);

                      webview_->SetVirtualHostNameToFolderMapping(
                          kRuntimeHost,
                          RuntimeRoot().wstring().c_str(),
                          COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS);

                      EventRegistrationToken token{};
                      webview_->add_WebMessageReceived(
                          Callback<ICoreWebView2WebMessageReceivedEventHandler>(
                              [this](ICoreWebView2*, ICoreWebView2WebMessageReceivedEventArgs* args) -> HRESULT {
                                OnWebMessage(args);
                                return S_OK;
                              })
                              .Get(),
                          &token);

                      EventRegistrationToken navigationToken{};
                      webview_->add_NavigationCompleted(
                          Callback<ICoreWebView2NavigationCompletedEventHandler>(
                              [this](ICoreWebView2*, ICoreWebView2NavigationCompletedEventArgs* args) -> HRESULT {
                                OnNavigationCompleted(args);
                                return S_OK;
                              })
                              .Get(),
                          &navigationToken);

                      webview_->Navigate(L"https://runtime.local.platform/runtime/index.html");
                      return S_OK;
                    })
                    .Get());
            return S_OK;
          })
          .Get());
}

void WebViewHost::OnNavigationCompleted(ICoreWebView2NavigationCompletedEventArgs* args) {
  BOOL success = FALSE;
  args->get_IsSuccess(&success);
  if (!success || webview_ == nullptr) {
    return;
  }

  PWSTR source = nullptr;
  webview_->get_Source(&source);
  std::wstring sourceText = source == nullptr ? L"" : source;
  CoTaskMemFree(source);
  if (sourceText == L"https://runtime.local.platform/runtime/index.html") {
    RunSmoke();
  }
}

void WebViewHost::OnWebMessage(ICoreWebView2WebMessageReceivedEventArgs* args) {
  PWSTR source = nullptr;
  args->get_Source(&source);
  std::wstring sourceText = source == nullptr ? L"" : source;
  CoTaskMemFree(source);
  if (sourceText.rfind(kRuntimeOrigin, 0) != 0) {
    return;
  }

  PWSTR rawMessage = nullptr;
  args->TryGetWebMessageAsString(&rawMessage);
  std::wstring body = rawMessage == nullptr ? L"" : rawMessage;
  CoTaskMemFree(rawMessage);

  std::wstring response;
  std::wstring smokeRequestId;
  std::wstring smokeAppId;
  std::wstring smokeMethod;
  json::JsonObject parsed{nullptr};
  if (json::JsonObject::TryParse(body, parsed) && IsRuntimeEnvelope(parsed)) {
    auto requestId = RuntimeEnvelopeRequestId(parsed);
    smokeRequestId = requestId;
    if (!HasValidRuntimeEnvelope(parsed)) {
      response = BridgeResponse::Failure(
                     requestId,
                     !requestId.empty(),
                     L"invalid_request",
                     L"Runtime bridge envelope requires appId, mountToken, and request")
                     .Stringify()
                     .c_str();
    } else {
      auto appId = std::wstring(parsed.GetNamedString(L"appId", L"").c_str());
      if (!IsKnownExampleAppId(appId)) {
        response = BridgeResponse::Failure(
                       requestId,
                       !requestId.empty(),
                       L"invalid_request",
                       L"Runtime bridge envelope references an unknown app")
                       .Stringify()
                       .c_str();
      } else {
        auto mountToken = std::wstring(parsed.GetNamedString(L"mountToken", L"").c_str());
        auto requestObject = parsed.GetNamedObject(L"request");
        smokeAppId = appId;
        smokeMethod = std::wstring(requestObject.GetNamedString(L"method", L"").c_str());
        auto requestJson = std::wstring(requestObject.Stringify().c_str());
        response = bridge_->HandleJson(requestJson, SandboxContextForApp(appId, mountToken));
      }
    }
  } else {
    response = bridge_->HandleJson(body, SandboxContextFromSource(sourceText));
  }

  HandleWebBridgeSmokeResponse(smokeRequestId, response);
  HandleRuntimeAppBridgeSmokeResponse(smokeAppId, smokeMethod, response);
  webview_->PostWebMessageAsString(response.c_str());
}

void WebViewHost::RunSmoke() {
  if (smokeRan_) {
    return;
  }
  auto action = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE");
  if (action.empty()) {
    return;
  }
  smokeRan_ = true;
  WriteSmokeLine(L"NATIVE_AI_WINDOWS_SMOKE_STARTED_" + action);
  if (action == L"runtime-load") {
    SmokeSuccess(L"NATIVE_AI_WINDOWS_SMOKE_RUNTIME_LOADED");
  } else if (action == L"storage-set") {
    RunStorageSmoke(true);
  } else if (action == L"storage-get") {
    RunStorageSmoke(false);
  } else if (action == L"core-step") {
    RunCoreSmoke();
  } else if (action == L"fixed-bridge-surface") {
    RunFixedBridgeSurfaceSmoke();
  } else if (action == L"bridge-storage-set") {
    RunWebBridgeStorageSmoke(true);
  } else if (action == L"bridge-storage-get") {
    RunWebBridgeStorageSmoke(false);
  } else if (action == L"bridge-core-step") {
    RunWebBridgeCoreSmoke();
  } else if (action == L"runtime-app-storage-get") {
    RunRuntimeAppBridgeSmoke();
  } else {
    SmokeFailure(L"unknown smoke action");
  }
}

void WebViewHost::RunStorageSmoke(bool setValue) {
  auto key = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY");
  auto value = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE");
  if (key.empty() || value.empty()) {
    SmokeFailure(L"storage smoke requires NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY and NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE");
    return;
  }

  json::JsonObject params;
  params.Insert(L"key", json::JsonValue::CreateStringValue(key));
  if (setValue) {
    json::JsonObject stored;
    stored.Insert(L"smokeValue", json::JsonValue::CreateStringValue(value));
    params.Insert(L"value", stored);
  }

  auto response = BridgeCall(
      L"notes-lite",
      setValue ? L"windows_smoke_storage_set" : L"windows_smoke_storage_get",
      setValue ? L"storage.set" : L"storage.get",
      params);
  if (!JsonResponseOk(response)) {
    SmokeFailure(response);
    return;
  }

  if (!setValue) {
    json::JsonObject parsed{nullptr};
    bool matches = false;
    if (json::JsonObject::TryParse(response, parsed)) {
      auto result = parsed.GetNamedObject(L"result", json::JsonObject());
      auto storedValue = result.GetNamedValue(L"value", json::JsonValue::CreateNullValue());
      if (storedValue.ValueType() == json::JsonValueType::Object) {
        auto stored = storedValue.GetObject();
        matches = std::wstring(stored.GetNamedString(L"smokeValue", L"").c_str()) == value;
      }
    }
    if (!matches) {
      SmokeFailure(response);
      return;
    }
  }

  SmokeSuccess(setValue ? L"NATIVE_AI_WINDOWS_SMOKE_STORAGE_SET_OK" : L"NATIVE_AI_WINDOWS_SMOKE_STORAGE_GET_OK");
}

void WebViewHost::RunCoreSmoke() {
  json::JsonObject payload;
  payload.Insert(L"title", json::JsonValue::CreateStringValue(L"Windows smoke task"));

  json::JsonObject event;
  event.Insert(L"type", json::JsonValue::CreateStringValue(L"CreateTask"));
  event.Insert(L"payload", payload);

  json::JsonObject params;
  params.Insert(L"event", event);

  auto response = BridgeCall(L"task-workbench", L"windows_smoke_core_step", L"core.step", params);
  if (!JsonResponseOk(response)) {
    SmokeFailure(response);
    return;
  }
  if (CoreEventLogCount(L"task-workbench") <= 0 || CoreActionLogCount(L"task-workbench") <= 0) {
    SmokeFailure(L"core smoke did not persist core_events/core_actions rows");
    return;
  }
  SmokeSuccess(L"NATIVE_AI_WINDOWS_SMOKE_CORE_STEP_OK");
}

void WebViewHost::RunFixedBridgeSurfaceSmoke() {
  auto key = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY");
  auto value = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE");
  if (key.empty() || value.empty()) {
    SmokeFailure(L"fixed bridge surface smoke requires NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY and NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE");
    return;
  }

  auto requireOk = [this](std::wstring const& response) -> bool {
    if (!JsonResponseOk(response)) {
      SmokeFailure(response);
      return false;
    }
    return true;
  };

  json::JsonObject setParams;
  setParams.Insert(L"key", json::JsonValue::CreateStringValue(key));
  json::JsonObject stored;
  stored.Insert(L"smokeValue", json::JsonValue::CreateStringValue(value));
  setParams.Insert(L"value", stored);
  if (!requireOk(BridgeCall(L"notes-lite", L"windows_smoke_fixed_storage_set", L"storage.set", setParams))) {
    return;
  }

  json::JsonObject listParams;
  listParams.Insert(L"prefix", json::JsonValue::CreateStringValue(L"notes-lite:"));
  auto listResponse = BridgeCall(L"notes-lite", L"windows_smoke_fixed_storage_list", L"storage.list", listParams);
  if (!JsonResponseOk(listResponse) || !StorageListResponseContains(listResponse, key)) {
    SmokeFailure(listResponse);
    return;
  }

  json::JsonObject removeParams;
  removeParams.Insert(L"key", json::JsonValue::CreateStringValue(key));
  if (!requireOk(BridgeCall(L"notes-lite", L"windows_smoke_fixed_storage_remove", L"storage.remove", removeParams))) {
    return;
  }

  json::JsonObject getParams;
  getParams.Insert(L"key", json::JsonValue::CreateStringValue(key));
  auto getResponse = BridgeCall(L"notes-lite", L"windows_smoke_fixed_storage_get_removed", L"storage.get", getParams);
  if (!JsonResponseOk(getResponse) || !StorageGetResponseIsNull(getResponse)) {
    SmokeFailure(getResponse);
    return;
  }

  json::JsonObject notificationParams;
  notificationParams.Insert(L"title", json::JsonValue::CreateStringValue(L"Native AI smoke"));
  notificationParams.Insert(L"body", json::JsonValue::CreateStringValue(L"Fixed bridge surface smoke"));
  if (!requireOk(BridgeCall(L"notes-lite", L"windows_smoke_fixed_notification", L"notification.toast", notificationParams))) {
    return;
  }

  json::JsonObject logParams;
  logParams.Insert(L"level", json::JsonValue::CreateStringValue(L"info"));
  logParams.Insert(L"message", json::JsonValue::CreateStringValue(L"Fixed bridge surface smoke"));
  if (!requireOk(BridgeCall(L"notes-lite", L"windows_smoke_fixed_app_log", L"app.log", logParams))) {
    return;
  }

  json::JsonObject capabilitiesParams;
  if (!requireOk(BridgeCall(L"notes-lite", L"windows_smoke_fixed_capabilities", L"runtime.capabilities", capabilitiesParams))) {
    return;
  }

  json::JsonObject networkParams;
  networkParams.Insert(L"url", json::JsonValue::CreateStringValue(L"https://blocked.example.com/status"));
  networkParams.Insert(L"method", json::JsonValue::CreateStringValue(L"GET"));
  auto networkResponse = BridgeCall(L"api-dashboard", L"windows_smoke_fixed_network_denied", L"network.request", networkParams);
  if (!JsonResponseErrorCodeMatches(networkResponse, L"network_policy_denied")) {
    SmokeFailure(networkResponse);
    return;
  }

  if (BridgeLogCount(L"notes-lite", L"storage.set") <= 0 || BridgeLogCount(L"api-dashboard", L"network.request") <= 0) {
    SmokeFailure(L"fixed bridge surface smoke did not persist bridge_calls rows");
    return;
  }

  SmokeSuccess(L"NATIVE_AI_WINDOWS_SMOKE_FIXED_BRIDGE_SURFACE_OK");
}

void WebViewHost::RunWebBridgeStorageSmoke(bool setValue) {
  auto key = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY");
  auto value = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE");
  if (key.empty() || value.empty()) {
    SmokeFailure(L"web bridge storage smoke requires NATIVE_AI_WINDOWS_SMOKE_STORAGE_KEY and NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE");
    return;
  }

  json::JsonObject params;
  params.Insert(L"key", json::JsonValue::CreateStringValue(key));
  if (setValue) {
    json::JsonObject stored;
    stored.Insert(L"smokeValue", json::JsonValue::CreateStringValue(value));
    params.Insert(L"value", stored);
  }

  StartWebBridgeSmoke(
      L"notes-lite",
      setValue ? L"windows_smoke_bridge_storage_set" : L"windows_smoke_bridge_storage_get",
      setValue ? L"storage.set" : L"storage.get",
      params);
}

void WebViewHost::RunWebBridgeCoreSmoke() {
  json::JsonObject payload;
  payload.Insert(L"title", json::JsonValue::CreateStringValue(L"Windows WebView bridge smoke task"));

  json::JsonObject event;
  event.Insert(L"type", json::JsonValue::CreateStringValue(L"CreateTask"));
  event.Insert(L"payload", payload);

  json::JsonObject params;
  params.Insert(L"event", event);

  StartWebBridgeSmoke(L"task-workbench", L"windows_smoke_bridge_core_step", L"core.step", params);
}

void WebViewHost::RunRuntimeAppBridgeSmoke() {
  if (webview_ == nullptr) {
    SmokeFailure(L"WebView2 is not initialized");
    return;
  }

  constexpr wchar_t kScript[] = LR"JS((function () {
  var deadline = Date.now() + 5000;
  function openNotesLiteWhenReady() {
    var button = document.querySelector('[data-testid="open-notes-lite-button"]');
    if (button) {
      button.click();
      return;
    }
    if (Date.now() < deadline) {
      window.setTimeout(openNotesLiteWhenReady, 50);
    }
  }
  openNotesLiteWhenReady();
  return "started";
})())JS";

  webview_->ExecuteScript(
      kScript,
      Callback<ICoreWebView2ExecuteScriptCompletedHandler>(
          [this](HRESULT errorCode, PCWSTR resultObjectAsJson) -> HRESULT {
            if (FAILED(errorCode)) {
              SmokeFailure(L"WebView2 runtime app smoke script failed");
              return S_OK;
            }
            auto result = ScriptStringResult(resultObjectAsJson);
            if (result != L"started") {
              SmokeFailure(L"WebView2 runtime app smoke script did not start: " + result);
            }
            return S_OK;
          })
          .Get());
}

void WebViewHost::StartWebBridgeSmoke(
    std::wstring const& appId,
    std::wstring const& id,
    std::wstring const& method,
    json::JsonObject const& params) {
  if (webview_ == nullptr) {
    SmokeFailure(L"WebView2 is not initialized");
    return;
  }

  json::JsonObject request;
  request.Insert(L"id", json::JsonValue::CreateStringValue(id));
  request.Insert(L"method", json::JsonValue::CreateStringValue(method));
  request.Insert(L"params", params);

  json::JsonObject envelope;
  envelope.Insert(L"appId", json::JsonValue::CreateStringValue(appId));
  envelope.Insert(L"mountToken", json::JsonValue::CreateStringValue(L"windows-webview-smoke"));
  envelope.Insert(L"request", request);

  std::wstring script =
      LR"JS((function () {
  if (!window.chrome || !window.chrome.webview || typeof window.chrome.webview.postMessage !== "function") {
    return "missing-webview2-bridge";
  }
  window.chrome.webview.postMessage(JSON.stringify()JS" +
      std::wstring(envelope.Stringify().c_str()) +
      LR"JS());
  return "posted";
})())JS";

  webview_->ExecuteScript(
      script.c_str(),
      Callback<ICoreWebView2ExecuteScriptCompletedHandler>(
          [this](HRESULT errorCode, PCWSTR resultObjectAsJson) -> HRESULT {
            if (FAILED(errorCode)) {
              SmokeFailure(L"WebView2 bridge smoke script failed");
              return S_OK;
            }
            auto result = ScriptStringResult(resultObjectAsJson);
            if (result != L"posted") {
              SmokeFailure(L"WebView2 bridge smoke script did not post: " + result);
            }
            return S_OK;
          })
          .Get());
}

void WebViewHost::HandleWebBridgeSmokeResponse(std::wstring const& requestId, std::wstring const& response) {
  if (requestId == L"windows_smoke_bridge_storage_set") {
    JsonResponseOk(response)
        ? SmokeSuccess(L"NATIVE_AI_WINDOWS_SMOKE_BRIDGE_STORAGE_SET_OK")
        : SmokeFailure(response);
  } else if (requestId == L"windows_smoke_bridge_storage_get") {
    auto value = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_STORAGE_VALUE");
    JsonResponseOk(response) && StorageSmokeResponseMatches(response, value)
        ? SmokeSuccess(L"NATIVE_AI_WINDOWS_SMOKE_BRIDGE_STORAGE_GET_OK")
        : SmokeFailure(response);
  } else if (requestId == L"windows_smoke_bridge_core_step") {
    JsonResponseOk(response)
        ? SmokeSuccess(L"NATIVE_AI_WINDOWS_SMOKE_BRIDGE_CORE_STEP_OK")
        : SmokeFailure(response);
  }
}

void WebViewHost::HandleRuntimeAppBridgeSmokeResponse(
    std::wstring const& appId,
    std::wstring const& method,
    std::wstring const& response) {
  if (EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE") != L"runtime-app-storage-get") {
    return;
  }
  if (appId == L"notes-lite" && method == L"storage.get") {
    JsonResponseOk(response)
        ? SmokeSuccess(L"NATIVE_AI_WINDOWS_SMOKE_RUNTIME_APP_STORAGE_GET_OK")
        : SmokeFailure(response);
  }
}

void WebViewHost::SmokeSuccess(std::wstring const& marker) {
  WriteSmokeLine(marker);
  if (EnvironmentIsOne(L"NATIVE_AI_WINDOWS_SMOKE_EXIT_AFTER")) {
    PostQuitMessage(0);
  }
}

void WebViewHost::SmokeFailure(std::wstring const& message) {
  WriteSmokeLine(L"NATIVE_AI_WINDOWS_SMOKE_FAILED: " + message);
  if (EnvironmentIsOne(L"NATIVE_AI_WINDOWS_SMOKE_EXIT_AFTER")) {
    PostQuitMessage(0);
  }
}

int WebViewHost::BridgeLogCount(std::wstring const& appId, std::wstring const& method) const {
  auto db = bridge_ == nullptr ? nullptr : bridge_->DatabaseHandle();
  if (db == nullptr) {
    return 0;
  }
  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(
          db,
          "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = ?",
          -1,
          &statement,
          nullptr) != SQLITE_OK) {
    return 0;
  }
  BindSmokeSqlText(statement, 1, appId);
  BindSmokeSqlText(statement, 2, method);
  int count = sqlite3_step(statement) == SQLITE_ROW ? sqlite3_column_int(statement, 0) : 0;
  sqlite3_finalize(statement);
  return count;
}

int WebViewHost::CoreEventLogCount(std::wstring const& appId) const {
  auto db = bridge_ == nullptr ? nullptr : bridge_->DatabaseHandle();
  if (db == nullptr) {
    return 0;
  }
  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM core_events WHERE app_id = ?", -1, &statement, nullptr) != SQLITE_OK) {
    return 0;
  }
  BindSmokeSqlText(statement, 1, appId);
  int count = sqlite3_step(statement) == SQLITE_ROW ? sqlite3_column_int(statement, 0) : 0;
  sqlite3_finalize(statement);
  return count;
}

int WebViewHost::CoreActionLogCount(std::wstring const& appId) const {
  auto db = bridge_ == nullptr ? nullptr : bridge_->DatabaseHandle();
  if (db == nullptr) {
    return 0;
  }
  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM core_actions WHERE app_id = ?", -1, &statement, nullptr) != SQLITE_OK) {
    return 0;
  }
  BindSmokeSqlText(statement, 1, appId);
  int count = sqlite3_step(statement) == SQLITE_ROW ? sqlite3_column_int(statement, 0) : 0;
  sqlite3_finalize(statement);
  return count;
}

std::wstring WebViewHost::BridgeCall(
    std::wstring const& appId,
    std::wstring const& id,
    std::wstring const& method,
    json::JsonObject const& params) {
  json::JsonObject request;
  request.Insert(L"id", json::JsonValue::CreateStringValue(id));
  request.Insert(L"method", json::JsonValue::CreateStringValue(method));
  request.Insert(L"params", params);
  return bridge_->HandleJson(std::wstring(request.Stringify().c_str()), SandboxContextForApp(appId, L"windows-smoke"));
}

AppSandboxContext WebViewHost::SandboxContextFromSource(std::wstring const& source) const {
  auto appId = AppIdFromSource(source);
  return SandboxContextForApp(appId, L"");
}

AppSandboxContext WebViewHost::SandboxContextForApp(std::wstring const& appId, std::wstring const& mountToken) const {
  return AppSandboxContext{
      .appId = appId,
      .storagePrefix = appId + L":",
      .approvedPermissions = PermissionsForApp(appId),
      .networkPolicy = NetworkPolicyForApp(appId),
      .resourceBudget = ResourceBudgetForApp(appId),
      .denyPrivateNetwork = DenyPrivateNetworkForApp(appId),
      .mountToken = mountToken,
  };
}

std::wstring WebViewHost::AppIdFromSource(std::wstring const& source) const {
  for (std::wstring marker : {L"/webapps/examples/", L"/examples/"}) {
    auto markerIndex = source.find(marker);
    if (markerIndex == std::wstring::npos) {
      continue;
    }
    auto start = markerIndex + marker.size();
    auto end = source.find(L"/", start);
    return source.substr(start, end == std::wstring::npos ? std::wstring::npos : end - start);
  }
  return L"unknown";
}

std::set<std::wstring> WebViewHost::PermissionsForApp(std::wstring const& appId) const {
  auto parsed = ManifestForApp(RuntimeRoot(), appId);
  if (!parsed.HasKey(L"permissions")) {
    return {};
  }

  std::set<std::wstring> permissions;
  for (auto const& value : parsed.GetNamedArray(L"permissions")) {
    permissions.insert(std::wstring(value.GetString().c_str()));
  }
  return permissions;
}

bool WebViewHost::DenyPrivateNetworkForApp(std::wstring const& appId) const {
  auto parsed = ManifestForApp(RuntimeRoot(), appId);
  if (!parsed.HasKey(L"networkPolicy")) {
    return true;
  }
  auto policy = parsed.GetNamedObject(L"networkPolicy", json::JsonObject());
  auto value = policy.GetNamedValue(L"denyPrivateNetwork", json::JsonValue::CreateBooleanValue(true));
  return value.ValueType() == json::JsonValueType::Boolean ? value.GetBoolean() : true;
}

std::map<std::wstring, uint32_t> WebViewHost::ResourceBudgetForApp(std::wstring const& appId) const {
  auto parsed = ManifestForApp(RuntimeRoot(), appId);
  if (!parsed.HasKey(L"resourceBudget")) {
    return {};
  }
  auto budget = parsed.GetNamedObject(L"resourceBudget", json::JsonObject());
  std::map<std::wstring, uint32_t> limits;
  for (auto const& entry : budget) {
    auto value = entry.Value();
    if (value.ValueType() == json::JsonValueType::Number) {
      limits.emplace(std::wstring(entry.Key().c_str()), static_cast<uint32_t>(value.GetNumber()));
    }
  }
  return limits;
}

std::vector<NetworkPolicyRule> WebViewHost::NetworkPolicyForApp(std::wstring const& appId) const {
  auto parsed = ManifestForApp(RuntimeRoot(), appId);
  if (!parsed.HasKey(L"networkPolicy")) {
    return {};
  }
  auto policy = parsed.GetNamedObject(L"networkPolicy", json::JsonObject());
  if (!policy.HasKey(L"allow")) {
    return {};
  }

  std::vector<NetworkPolicyRule> rules;
  for (auto const& rawValue : policy.GetNamedArray(L"allow", json::JsonArray())) {
    if (rawValue.ValueType() != json::JsonValueType::Object) {
      continue;
    }
    auto raw = rawValue.GetObject();
    NetworkPolicyRule rule;
    rule.origin = raw.GetNamedString(L"origin", L"").c_str();
    if (rule.origin.empty()) {
      continue;
    }
    for (auto const& method : raw.GetNamedArray(L"methods", json::JsonArray())) {
      rule.methods.insert(ToUpper(std::wstring(method.GetString().c_str())));
    }
    for (auto const& header : raw.GetNamedArray(L"allowedHeaders", json::JsonArray())) {
      rule.allowedHeaders.insert(ToLower(std::wstring(header.GetString().c_str())));
    }
    rule.maxRequestBytes = static_cast<uint32_t>(raw.GetNamedNumber(L"maxRequestBytes", 0));
    rule.maxResponseBytes = static_cast<uint32_t>(raw.GetNamedNumber(L"maxResponseBytes", 0));
    rule.timeoutMs = static_cast<uint32_t>(raw.GetNamedNumber(L"timeoutMs", 10000));
    rules.push_back(std::move(rule));
  }
  return rules;
}

std::filesystem::path WebViewHost::RepoRoot() {
  auto current = std::filesystem::current_path();
  for (int depth = 0; depth < 5; ++depth) {
    if (std::filesystem::exists(current / L"docs" / L"00_PRD.md")) {
      return current;
    }
    current = current.parent_path();
  }
  return std::filesystem::current_path();
}

std::filesystem::path WebViewHost::RuntimeRoot() {
  auto resourceRoot = ExecutableDirectory() / L"resources";
  if (std::filesystem::exists(resourceRoot / L"runtime" / L"index.html") &&
      std::filesystem::exists(resourceRoot / L"webapps" / L"examples")) {
    return resourceRoot;
  }
  return RepoRoot();
}

std::filesystem::path WebViewHost::DatabasePath() {
  auto smokeDataHome = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_DATA_HOME");
  if (!smokeDataHome.empty()) {
    return std::filesystem::path(smokeDataHome) / L"NativeAIWebappPlatform" / L"platform.sqlite";
  }

  PWSTR localAppData = nullptr;
  SHGetKnownFolderPath(FOLDERID_LocalAppData, KF_FLAG_CREATE, nullptr, &localAppData);
  std::filesystem::path path(localAppData == nullptr ? L"." : localAppData);
  CoTaskMemFree(localAppData);
  return path / L"NativeAIWebappPlatform" / L"platform.sqlite";
}

}  // namespace nativeai
