#include "WebViewHost.h"

#include <objbase.h>
#include <ShlObj.h>
#include <algorithm>
#include <chrono>
#include <cwctype>
#include <fstream>
#include <future>
#include <limits>
#include <winrt/Windows.Data.Json.h>
#include <wrl/event.h>

namespace terrane {
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

bool HasOnlyRuntimeEnvelopeFields(json::JsonObject const& body) {
  for (auto const& entry : body) {
    auto key = std::wstring(entry.Key().c_str());
    if (key != L"appId" && key != L"mountToken" && key != L"request") {
      return false;
    }
  }
  return true;
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
  if (!HasOnlyRuntimeEnvelopeFields(body)) {
    return false;
  }
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

bool HasOnlyMountRequestFields(json::JsonObject const& body) {
  for (auto const& entry : body) {
    auto key = std::wstring(entry.Key().c_str());
    if (key != L"type" && key != L"id" && key != L"appId") {
      return false;
    }
  }
  return true;
}

bool IsRuntimeMountRequest(json::JsonObject const& body) {
  if (!HasOnlyMountRequestFields(body)) {
    return false;
  }
  if (!body.HasKey(L"type") || body.GetNamedValue(L"type").ValueType() != json::JsonValueType::String) {
    return false;
  }
  if (std::wstring(body.GetNamedString(L"type", L"").c_str()) != L"runtime.mount_request") {
    return false;
  }
  if (!body.HasKey(L"id") || !body.HasKey(L"appId")) {
    return false;
  }
  auto id = std::wstring(body.GetNamedString(L"id", L"").c_str());
  auto appId = std::wstring(body.GetNamedString(L"appId", L"").c_str());
  return !id.empty() && !appId.empty();
}

std::wstring NewRuntimeMountToken() {
  GUID guid{};
  if (FAILED(CoCreateGuid(&guid))) {
    return L"";
  }
  wchar_t buffer[39]{};
  if (StringFromGUID2(guid, buffer, 39) == 0) {
    return L"";
  }
  std::wstring token(buffer);
  token.erase(std::remove(token.begin(), token.end(), L'{'), token.end());
  token.erase(std::remove(token.begin(), token.end(), L'}'), token.end());
  return L"windows-" + token;
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
  auto markerPath = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_RESULT_FILE");
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

bool JsonResponseErrorDetailStringMatches(
    std::wstring const& response,
    std::wstring const& member,
    std::wstring const& expected) {
  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(response, parsed)) {
    return false;
  }
  auto error = parsed.GetNamedObject(L"error", json::JsonObject());
  auto details = error.GetNamedObject(L"details", json::JsonObject());
  return std::wstring(details.GetNamedString(member, L"").c_str()) == expected;
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

bool ReadVersionPart(std::wstring const& version, size_t& offset, uint32_t& part) {
  if (offset >= version.size() || !std::iswdigit(version[offset])) {
    return false;
  }
  uint64_t value = 0;
  while (offset < version.size() && std::iswdigit(version[offset])) {
    value = (value * 10) + static_cast<uint64_t>(version[offset] - L'0');
    if (value > std::numeric_limits<uint32_t>::max()) {
      return false;
    }
    ++offset;
  }
  part = static_cast<uint32_t>(value);
  return true;
}

bool WebView2RuntimeMeetsMinimum(std::wstring const& version) {
  size_t offset = 0;
  uint32_t major = 0;
  uint32_t minor = 0;
  uint32_t build = 0;
  if (!ReadVersionPart(version, offset, major) || offset >= version.size() || version[offset++] != L'.') {
    return false;
  }
  if (!ReadVersionPart(version, offset, minor) || offset >= version.size() || version[offset++] != L'.') {
    return false;
  }
  if (!ReadVersionPart(version, offset, build)) {
    return false;
  }
  if (major != 1) {
    return major > 1;
  }
  if (minor != 0) {
    return minor > 0;
  }
  return build >= 2592;
}

bool StorageNotesResponseContainsSmokeValue(std::wstring const& response, std::wstring const& value) {
  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(response, parsed)) {
    return false;
  }
  auto result = parsed.GetNamedObject(L"result", json::JsonObject());
  auto storedValue = result.GetNamedValue(L"value", json::JsonValue::CreateNullValue());
  if (storedValue.ValueType() != json::JsonValueType::Array) {
    return false;
  }
  for (auto const& noteValue : storedValue.GetArray()) {
    if (noteValue.ValueType() != json::JsonValueType::Object) {
      continue;
    }
    auto note = noteValue.GetObject();
    if (std::wstring(note.GetNamedString(L"title", L"").c_str()) == L"Windows smoke " + value &&
        std::wstring(note.GetNamedString(L"body", L"").c_str()) == L"Seeded by Windows runtime-app storage smoke") {
      return true;
    }
  }
  return false;
}
}  // namespace

WebViewHost::WebViewHost(HWND window) : window_(window), bridge_(std::make_unique<WebBridge>(DatabasePath(), window)) {}

struct WebViewHost::DevControlBridgeCallRequest {
  std::wstring appId;
  std::wstring controlSessionId;
  std::wstring requestJson;
  std::promise<std::wstring> result;
};

std::wstring WebViewHost::DevControlBridgeCall(
    std::wstring const& appId,
    std::wstring const& controlSessionId,
    std::wstring const& bridgeRequestJson) {
  auto payload = std::make_unique<DevControlBridgeCallRequest>();
  payload->appId = appId;
  payload->controlSessionId = controlSessionId;
  payload->requestJson = bridgeRequestJson;
  auto future = payload->result.get_future();
  if (!PostMessageW(window_, kDevControlBridgeCallMessage, 0, reinterpret_cast<LPARAM>(payload.release()))) {
    return BridgeResponse::Failure(L"", false, L"platform_unsupported", L"Windows dev control bridge could not post to the host thread")
        .Stringify()
        .c_str();
  }
  if (future.wait_for(std::chrono::seconds(10)) != std::future_status::ready) {
    return BridgeResponse::Failure(L"", false, L"timeout", L"Windows dev control bridge call timed out waiting for the host thread")
        .Stringify()
        .c_str();
  }
  return future.get();
}

struct WebViewHost::AsyncBridgeResponse {
  std::wstring response;
  std::wstring smokeRequestId;
  std::wstring smokeAppId;
  std::wstring smokeMethod;
};

bool WebViewHost::TryHandleWindowMessage(UINT message, WPARAM, LPARAM lparam, LRESULT* result) {
  if (message == kDevControlBridgeCallMessage) {
    std::shared_ptr<DevControlBridgeCallRequest> payload(reinterpret_cast<DevControlBridgeCallRequest*>(lparam));
    if (!payload || payload->appId.empty() || bridge_ == nullptr) {
      if (payload) {
        payload->result.set_value(
            BridgeResponse::Failure(L"", false, L"invalid_request", L"Windows dev control bridge call requires appId")
                .Stringify()
                .c_str());
      }
    } else {
      try {
        bridge_->HandleJsonAsync(
            payload->requestJson,
            SandboxContextForApp(payload->appId, payload->controlSessionId),
            [payload](std::wstring response) {
              try {
                payload->result.set_value(std::move(response));
              } catch (...) {
              }
            });
      } catch (...) {
        try {
          payload->result.set_value(
              BridgeResponse::Failure(L"", false, L"platform_error", L"Windows dev control bridge dispatch failed")
                  .Stringify()
                  .c_str());
        } catch (...) {
        }
      }
    }
    if (result != nullptr) {
      *result = 0;
    }
    return true;
  }
  if (message != kAsyncBridgeResponseMessage) {
    return false;
  }
  std::unique_ptr<AsyncBridgeResponse> payload(reinterpret_cast<AsyncBridgeResponse*>(lparam));
  if (payload) {
    PostWebBridgeResponse(payload->response, payload->smokeRequestId, payload->smokeAppId, payload->smokeMethod);
  }
  if (result != nullptr) {
    *result = 0;
  }
  return true;
}

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
            if (!EnsureSupportedWebView2Runtime(environment)) {
              return E_FAIL;
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

bool WebViewHost::EnsureSupportedWebView2Runtime(ICoreWebView2Environment* environment) {
  PWSTR browserVersion = nullptr;
  HRESULT versionResult = environment->get_BrowserVersionString(&browserVersion);
  std::wstring versionText = browserVersion == nullptr ? L"" : browserVersion;
  CoTaskMemFree(browserVersion);

  if (FAILED(versionResult) || !WebView2RuntimeMeetsMinimum(versionText)) {
    SmokeFailure(L"WebView2 runtime version 1.0.2592 or later is required; found " + versionText);
    return false;
  }
  if (!EnvironmentValue(L"TERRANE_WINDOWS_SMOKE").empty()) {
    WriteSmokeLine(L"TERRANE_WINDOWS_SMOKE_WEBVIEW2_VERSION_OK " + versionText);
  }
  return true;
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
  bool parsedOk = json::JsonObject::TryParse(body, parsed);
  if (parsedOk &&
      parsed.HasKey(L"type") &&
      parsed.GetNamedValue(L"type").ValueType() == json::JsonValueType::String &&
      std::wstring(parsed.GetNamedString(L"type", L"").c_str()) == L"windows_smoke_runtime_load_ready") {
    auto okValue = parsed.GetNamedValue(L"ok", json::JsonValue::CreateBooleanValue(false));
    auto ok = okValue.ValueType() == json::JsonValueType::Boolean && okValue.GetBoolean();
    if (ok) {
      WriteSmokeLine(L"TERRANE_WINDOWS_SMOKE_RUNTIME_JS_READY");
      SmokeSuccess(L"TERRANE_WINDOWS_SMOKE_RUNTIME_LOADED");
    } else {
      SmokeFailure(L"WebView2 runtime readiness check failed: " + std::wstring(parsed.GetNamedString(L"detail", L"").c_str()));
    }
    return;
  }

  if (parsedOk && IsRuntimeMountRequest(parsed)) {
    auto requestId = std::wstring(parsed.GetNamedString(L"id", L"").c_str());
    auto appId = std::wstring(parsed.GetNamedString(L"appId", L"").c_str());
    json::JsonObject mountResponse;
    mountResponse.Insert(L"type", json::JsonValue::CreateStringValue(L"runtime.mount_response"));
    mountResponse.Insert(L"id", json::JsonValue::CreateStringValue(requestId));
    mountResponse.Insert(L"appId", json::JsonValue::CreateStringValue(appId));
    auto mountToken = CreateHostOwnedRuntimeMount(appId);
    if (mountToken.has_value()) {
      mountResponse.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
      mountResponse.Insert(L"mountToken", json::JsonValue::CreateStringValue(mountToken.value()));
    } else {
      json::JsonObject error;
      error.Insert(L"code", json::JsonValue::CreateStringValue(L"invalid_request"));
      error.Insert(L"message", json::JsonValue::CreateStringValue(L"Runtime mount request references an unknown app"));
      error.Insert(L"details", json::JsonObject());
      mountResponse.Insert(L"ok", json::JsonValue::CreateBooleanValue(false));
      mountResponse.Insert(L"error", error);
    }
    auto responseText = std::wstring(mountResponse.Stringify().c_str());
    webview_->PostWebMessageAsString(responseText.c_str());
    return;
  }

  if (parsedOk && IsRuntimeEnvelope(parsed)) {
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
        auto context = SandboxContextForRegisteredMount(appId, mountToken);
        if (context.has_value()) {
          bridge_->HandleJsonAsync(
              requestJson,
              context.value(),
              [this, smokeRequestId, smokeAppId, smokeMethod](std::wstring bridgeResponse) {
                PostAsyncBridgeResponse(std::move(bridgeResponse), smokeRequestId, smokeAppId, smokeMethod);
              });
          return;
        }
        response = BridgeResponse::Failure(
                       requestId,
                       !requestId.empty(),
                       L"invalid_request",
                       L"Runtime bridge envelope does not match a host-owned mount channel")
                       .Stringify()
                       .c_str();
      }
    }
  } else {
    response = BridgeResponse::Failure(
                   L"",
                   false,
                   L"invalid_request",
                   parsedOk ? L"Runtime bridge envelope is required" : L"Runtime bridge envelope must be JSON")
                   .Stringify()
                   .c_str();
  }

  PostWebBridgeResponse(response, smokeRequestId, smokeAppId, smokeMethod);
}

void WebViewHost::PostAsyncBridgeResponse(
    std::wstring response,
    std::wstring smokeRequestId,
    std::wstring smokeAppId,
    std::wstring smokeMethod) {
  auto payload = std::make_unique<AsyncBridgeResponse>(
      AsyncBridgeResponse{std::move(response), std::move(smokeRequestId), std::move(smokeAppId), std::move(smokeMethod)});
  if (!PostMessageW(window_, kAsyncBridgeResponseMessage, 0, reinterpret_cast<LPARAM>(payload.get()))) {
    PostWebBridgeResponse(payload->response, payload->smokeRequestId, payload->smokeAppId, payload->smokeMethod);
    return;
  }
  payload.release();
}

void WebViewHost::PostWebBridgeResponse(
    std::wstring const& response,
    std::wstring const& smokeRequestId,
    std::wstring const& smokeAppId,
    std::wstring const& smokeMethod) {
  HandleWebBridgeSmokeResponse(smokeRequestId, response);
  HandleRuntimeAppBridgeSmokeResponse(smokeAppId, smokeMethod, response);
  if (webview_ != nullptr) {
    webview_->PostWebMessageAsString(response.c_str());
  }
}

std::optional<std::wstring> WebViewHost::CreateHostOwnedRuntimeMount(std::wstring const& appId) {
  if (!IsKnownExampleAppId(appId)) {
    return std::nullopt;
  }
  auto mountToken = NewRuntimeMountToken();
  if (mountToken.empty()) {
    return std::nullopt;
  }
  RegisterHostOwnedRuntimeMount(appId, mountToken);
  return mountToken;
}

void WebViewHost::RegisterHostOwnedRuntimeMount(std::wstring const& appId, std::wstring const& mountToken) {
  if (!IsKnownExampleAppId(appId) || mountToken.empty()) {
    return;
  }
  registeredMountsByToken_.clear();
  registeredMountsByToken_[mountToken] = appId;
}

void WebViewHost::RunSmoke() {
  if (smokeRan_) {
    return;
  }
  auto action = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE");
  if (action.empty()) {
    return;
  }
  smokeRan_ = true;
  WriteSmokeLine(L"TERRANE_WINDOWS_SMOKE_STARTED_" + action);
  if (action == L"runtime-load") {
    RunRuntimeLoadSmoke();
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

void WebViewHost::RunRuntimeLoadSmoke() {
  if (webview_ == nullptr) {
    SmokeFailure(L"WebView2 is not initialized");
    return;
  }

  constexpr wchar_t kScript[] = LR"JS((function () {
  if (!window.chrome || !window.chrome.webview || typeof window.chrome.webview.postMessage !== "function") {
    return "missing-webview2-bridge";
  }
  var deadline = Date.now() + 5000;
  function send(ok, detail) {
    window.chrome.webview.postMessage(JSON.stringify({
      type: "windows_smoke_runtime_load_ready",
      ok: ok,
      detail: detail || ""
    }));
  }
  function check() {
    var status = document.querySelector('[data-testid="runtime-status"]');
    var appList = document.querySelector('[data-testid="app-list"]');
    var openButton = document.querySelector('[data-testid="open-notes-lite-button"]');
    var ready = status && status.textContent.trim() === "Ready" && appList && appList.children.length > 0 && openButton;
    if (ready) {
      send(true, "ready");
      return;
    }
    if (Date.now() < deadline) {
      window.setTimeout(check, 50);
      return;
    }
    send(false, JSON.stringify({
      status: status ? status.textContent.trim() : "",
      appCount: appList ? appList.children.length : 0,
      hasOpenButton: Boolean(openButton)
    }));
  }
  check();
  return "started";
})())JS";

  webview_->ExecuteScript(
      kScript,
      Callback<ICoreWebView2ExecuteScriptCompletedHandler>(
          [this](HRESULT errorCode, PCWSTR resultObjectAsJson) -> HRESULT {
            if (FAILED(errorCode)) {
              SmokeFailure(L"WebView2 runtime readiness script failed");
              return S_OK;
            }
            auto result = ScriptStringResult(resultObjectAsJson);
            if (result != L"started") {
              SmokeFailure(L"WebView2 runtime readiness script did not start: " + result);
            }
            return S_OK;
          })
          .Get());
}

void WebViewHost::RunStorageSmoke(bool setValue) {
  auto key = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_STORAGE_KEY");
  auto value = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
  if (key.empty() || value.empty()) {
    SmokeFailure(L"storage smoke requires TERRANE_WINDOWS_SMOKE_STORAGE_KEY and TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
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

  SmokeSuccess(setValue ? L"TERRANE_WINDOWS_SMOKE_STORAGE_SET_OK" : L"TERRANE_WINDOWS_SMOKE_STORAGE_GET_OK");
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
  SmokeSuccess(L"TERRANE_WINDOWS_SMOKE_CORE_STEP_OK");
}

void WebViewHost::RunFixedBridgeSurfaceSmoke() {
  auto key = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_STORAGE_KEY");
  auto value = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
  if (key.empty() || value.empty()) {
    SmokeFailure(L"fixed bridge surface smoke requires TERRANE_WINDOWS_SMOKE_STORAGE_KEY and TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
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
  notificationParams.Insert(L"message", json::JsonValue::CreateStringValue(L"Fixed bridge surface smoke"));
  notificationParams.Insert(L"level", json::JsonValue::CreateStringValue(L"success"));
  if (!requireOk(BridgeCall(L"notes-lite", L"windows_smoke_fixed_notification", L"notification.toast", notificationParams))) {
    return;
  }

  json::JsonObject notificationBadParams;
  notificationBadParams.Insert(L"message", json::JsonValue::CreateStringValue(L"Saved"));
  notificationBadParams.Insert(L"level", json::JsonValue::CreateStringValue(L"warn"));
  auto notificationBadResponse = BridgeCall(
      L"notes-lite",
      L"windows_smoke_fixed_notification_bad_level",
      L"notification.toast",
      notificationBadParams);
  if (!JsonResponseErrorCodeMatches(notificationBadResponse, L"invalid_request") ||
      !JsonResponseErrorDetailStringMatches(notificationBadResponse, L"level", L"warn")) {
    SmokeFailure(notificationBadResponse);
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

  SmokeSuccess(L"TERRANE_WINDOWS_SMOKE_FIXED_BRIDGE_SURFACE_OK");
}

void WebViewHost::RunWebBridgeStorageSmoke(bool setValue) {
  auto key = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_STORAGE_KEY");
  auto value = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
  if (key.empty() || value.empty()) {
    SmokeFailure(L"web bridge storage smoke requires TERRANE_WINDOWS_SMOKE_STORAGE_KEY and TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
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
  auto value = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
  if (value.empty()) {
    SmokeFailure(L"runtime app storage smoke requires TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
    return;
  }

  json::JsonObject note;
  note.Insert(L"id", json::JsonValue::CreateStringValue(L"windows-smoke-note"));
  note.Insert(L"title", json::JsonValue::CreateStringValue(L"Windows smoke " + value));
  note.Insert(L"body", json::JsonValue::CreateStringValue(L"Seeded by Windows runtime-app storage smoke"));
  note.Insert(L"updatedAt", json::JsonValue::CreateNumberValue(static_cast<double>(GetTickCount64())));
  json::JsonArray notes;
  notes.Append(note);

  json::JsonObject setParams;
  setParams.Insert(L"key", json::JsonValue::CreateStringValue(L"notes-lite:notes"));
  setParams.Insert(L"value", notes);
  auto setResponse = BridgeCall(L"notes-lite", L"windows_smoke_runtime_app_seed_storage", L"storage.set", setParams);
  if (!JsonResponseOk(setResponse)) {
    SmokeFailure(setResponse);
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

  RegisterHostOwnedRuntimeMount(appId, L"windows-webview-smoke");

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
        ? SmokeSuccess(L"TERRANE_WINDOWS_SMOKE_BRIDGE_STORAGE_SET_OK")
        : SmokeFailure(response);
  } else if (requestId == L"windows_smoke_bridge_storage_get") {
    auto value = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
    JsonResponseOk(response) && StorageSmokeResponseMatches(response, value)
        ? SmokeSuccess(L"TERRANE_WINDOWS_SMOKE_BRIDGE_STORAGE_GET_OK")
        : SmokeFailure(response);
  } else if (requestId == L"windows_smoke_bridge_core_step") {
    JsonResponseOk(response)
        ? SmokeSuccess(L"TERRANE_WINDOWS_SMOKE_BRIDGE_CORE_STEP_OK")
        : SmokeFailure(response);
  }
}

void WebViewHost::HandleRuntimeAppBridgeSmokeResponse(
    std::wstring const& appId,
    std::wstring const& method,
    std::wstring const& response) {
  if (EnvironmentValue(L"TERRANE_WINDOWS_SMOKE") != L"runtime-app-storage-get") {
    return;
  }
  if (appId == L"notes-lite" && method == L"storage.get") {
    auto value = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_STORAGE_VALUE");
    JsonResponseOk(response) && StorageNotesResponseContainsSmokeValue(response, value)
        ? SmokeSuccess(L"TERRANE_WINDOWS_SMOKE_RUNTIME_APP_STORAGE_GET_OK")
        : SmokeFailure(response);
  }
}

void WebViewHost::SmokeSuccess(std::wstring const& marker) {
  WriteSmokeLine(marker);
  if (EnvironmentIsOne(L"TERRANE_WINDOWS_SMOKE_EXIT_AFTER")) {
    PostQuitMessage(0);
  }
}

void WebViewHost::SmokeFailure(std::wstring const& message) {
  WriteSmokeLine(L"TERRANE_WINDOWS_SMOKE_FAILED: " + message);
  if (EnvironmentIsOne(L"TERRANE_WINDOWS_SMOKE_EXIT_AFTER")) {
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

std::optional<AppSandboxContext> WebViewHost::SandboxContextForRegisteredMount(
    std::wstring const& appId,
    std::wstring const& mountToken) const {
  auto mount = registeredMountsByToken_.find(mountToken);
  if (mount == registeredMountsByToken_.end() || mount->second != appId) {
    return std::nullopt;
  }
  return SandboxContextForApp(mount->second, mountToken);
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
    rule.pathPrefix = raw.GetNamedString(L"pathPrefix", L"").c_str();
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
  auto smokeDataHome = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_DATA_HOME");
  if (!smokeDataHome.empty()) {
    return std::filesystem::path(smokeDataHome) / L"Terrane" / L"platform.sqlite";
  }

  PWSTR localAppData = nullptr;
  SHGetKnownFolderPath(FOLDERID_LocalAppData, KF_FLAG_CREATE, nullptr, &localAppData);
  std::filesystem::path path(localAppData == nullptr ? L"." : localAppData);
  CoTaskMemFree(localAppData);
  return path / L"Terrane" / L"platform.sqlite";
}

}  // namespace terrane
