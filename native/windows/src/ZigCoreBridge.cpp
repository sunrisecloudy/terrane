#include "ZigCoreBridge.h"

#include <cstdlib>

namespace nativeai {
namespace json = winrt::Windows::Data::Json;

namespace {

std::filesystem::path ExecutableDirectory() {
  std::wstring buffer(MAX_PATH, L'\0');
  DWORD length = GetModuleFileNameW(nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
  if (length == 0) {
    return std::filesystem::current_path();
  }
  buffer.resize(length);
  return std::filesystem::path(buffer).parent_path();
}

std::filesystem::path EnvironmentPath(wchar_t const* name) {
  std::wstring value(32767, L'\0');
  DWORD length = GetEnvironmentVariableW(name, value.data(), static_cast<DWORD>(value.size()));
  if (length == 0 || length >= value.size()) {
    return {};
  }
  value.resize(length);
  return std::filesystem::path(value);
}

}  // namespace

ZigCoreBridge::ZigCoreBridge() {
  for (auto const& path : CandidateLibraryPaths()) {
    if (!path.empty() && Load(path)) {
      break;
    }
  }
}

ZigCoreBridge::~ZigCoreBridge() {
  if (destroy_ != nullptr && core_ != nullptr) {
    destroy_(core_);
  }
  if (handle_ != nullptr) {
    FreeLibrary(handle_);
  }
}

bool ZigCoreBridge::IsAvailable() const {
  return handle_ != nullptr && core_ != nullptr && stepJson_ != nullptr && freeBuffer_ != nullptr;
}

winrt::Windows::Data::Json::JsonObject ZigCoreBridge::Step(BridgeRequest const& request) {
  if (!IsAvailable()) {
    return BridgeResponse::Failure(
        request.id,
        request.hasId,
        L"platform_unsupported",
        L"core.step requires loading zig_core.dll into the Windows host");
  }

  if (request.params.HasKey(L"app")) {
    auto appValue = request.params.GetNamedValue(L"app");
    if (appValue.ValueType() != json::JsonValueType::String) {
      return BridgeResponse::Failure(
          request.id,
          request.hasId,
          L"invalid_request",
          L"core.step app field must be a string when present");
    }
    auto requestedApp = std::wstring(appValue.GetString().c_str());
    if (requestedApp != request.context.appId) {
      json::JsonObject details;
      details.Insert(L"requestedApp", json::JsonValue::CreateStringValue(requestedApp));
      details.Insert(L"channelApp", json::JsonValue::CreateStringValue(request.context.appId));
      return BridgeResponse::Failure(
          request.id,
          request.hasId,
          L"permission_denied",
          L"core.step app field does not match the channel-derived app id",
          details);
    }
  }

  std::string input = WideToUtf8(std::wstring(CoreInputForRequest(request).Stringify().c_str()));
  ZigCoreBuffer output{};
  int32_t code = stepJson_(core_, reinterpret_cast<uint8_t const*>(input.data()), input.size(), &output);
  if (code != 0) {
    json::JsonObject details;
    details.Insert(L"status", json::JsonValue::CreateNumberValue(code));
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"core_step_json failed", details);
  }
  if (output.ptr == nullptr) {
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"core.step returned empty output");
  }

  std::string outputText(reinterpret_cast<char const*>(output.ptr), output.len);
  freeBuffer_(output);

  json::JsonObject result{nullptr};
  if (!json::JsonObject::TryParse(Utf8ToWide(outputText), result)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"core.step returned invalid JSON");
  }
  return BridgeResponse::Success(request.id, request.hasId, result);
}

std::vector<std::filesystem::path> ZigCoreBridge::CandidateLibraryPaths() {
  auto cwd = std::filesystem::current_path();
  auto exeDir = ExecutableDirectory();
  return {
      EnvironmentPath(L"NATIVE_AI_ZIG_CORE_DLL"),
      cwd / L"zig-core" / L"zig-out" / L"bin" / L"zig_core.dll",
      cwd / L"zig-core" / L"zig-out" / L"lib" / L"zig_core.dll",
      cwd / L".." / L"zig-core" / L"zig-out" / L"bin" / L"zig_core.dll",
      exeDir / L"zig_core.dll",
  };
}

bool ZigCoreBridge::Load(std::filesystem::path const& path) {
  HMODULE handle = LoadLibraryW(path.c_str());
  if (handle == nullptr) {
    return false;
  }

  auto create = reinterpret_cast<CoreCreateFn>(GetProcAddress(handle, "core_create"));
  auto destroy = reinterpret_cast<CoreDestroyFn>(GetProcAddress(handle, "core_destroy"));
  auto stepJson = reinterpret_cast<CoreStepJsonFn>(GetProcAddress(handle, "core_step_json"));
  auto freeBuffer = reinterpret_cast<CoreFreeFn>(GetProcAddress(handle, "core_free"));
  if (create == nullptr || destroy == nullptr || stepJson == nullptr || freeBuffer == nullptr) {
    FreeLibrary(handle);
    return false;
  }

  void* core = create();
  if (core == nullptr) {
    FreeLibrary(handle);
    return false;
  }

  handle_ = handle;
  core_ = core;
  destroy_ = destroy;
  stepJson_ = stepJson;
  freeBuffer_ = freeBuffer;
  loadedPath_ = path;
  return true;
}

winrt::Windows::Data::Json::JsonObject ZigCoreBridge::CoreInputForRequest(BridgeRequest const& request) const {
  json::JsonObject input;
  for (auto const& entry : request.params) {
    auto key = std::wstring(entry.Key().c_str());
    if (key == L"app") {
      continue;
    }
    input.Insert(key, entry.Value());
  }
  input.Insert(L"app", json::JsonValue::CreateStringValue(request.context.appId));
  return input;
}

}  // namespace nativeai
