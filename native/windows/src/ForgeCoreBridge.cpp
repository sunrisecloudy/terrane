#include "ForgeCoreBridge.h"

#include <chrono>
#include <cstdlib>
#include <future>
#include <mutex>
#include <thread>
#include <utility>
#include <vector>
#include <winrt/base.h>

namespace terrane {
namespace json = winrt::Windows::Data::Json;

namespace {

constexpr uint32_t kCoreStepTimeoutMs = 2000;

std::filesystem::path ExecutableDirectory() {
  std::vector<wchar_t> buffer(MAX_PATH);
  while (true) {
    DWORD length = GetModuleFileNameW(nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
    if (length == 0) {
      return std::filesystem::current_path();
    }
    if (length < buffer.size()) {
      return std::filesystem::path(std::wstring(buffer.data(), length)).parent_path();
    }
    buffer.resize(buffer.size() * 2);
  }
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

void InitializeWorkerApartment() {
  try {
    winrt::init_apartment(winrt::apartment_type::multi_threaded);
  } catch (...) {
  }
}

}  // namespace

struct ForgeCoreBridge::CoreRuntime {
  ~CoreRuntime() {
    if (closeCore != nullptr && core != nullptr) {
      closeCore(core);
    }
    if (handle != nullptr) {
      FreeLibrary(handle);
    }
  }

  HMODULE handle = nullptr;
  void* core = nullptr;
  ForgeCoreCloseFn closeCore = nullptr;
  ForgeCoreHandleCommandFn handleCommand = nullptr;
  ForgeCoreDrainEventsFn drainEvents = nullptr;
  ForgeCoreLastErrorFn lastError = nullptr;
  ForgeStringFreeFn freeString = nullptr;
  std::mutex stepMutex;
};

struct ForgeCoreBridge::StepCompletionState {
  explicit StepCompletionState(StepCompletion callback) : completion(std::move(callback)) {}

  std::mutex mutex;
  bool completed = false;
  StepCompletion completion;
};

ForgeCoreBridge::ForgeCoreBridge(std::filesystem::path databasePath)
    : coreDatabasePath_(CoreDatabasePath(databasePath)) {
  for (auto const& path : CandidateLibraryPaths()) {
    if (!path.empty() && Load(path)) {
      break;
    }
  }
}

ForgeCoreBridge::~ForgeCoreBridge() = default;

bool ForgeCoreBridge::IsAvailable() const {
  auto runtime = runtime_;
  return runtime != nullptr && runtime->handle != nullptr && runtime->core != nullptr && runtime->handleCommand != nullptr &&
      runtime->freeString != nullptr;
}

winrt::Windows::Data::Json::JsonObject ForgeCoreBridge::Step(BridgeRequest const& request) {
  auto promise = std::make_shared<std::promise<json::JsonObject>>();
  auto future = promise->get_future();
  StepAsync(request, [promise](json::JsonObject response) {
    try {
      promise->set_value(response);
    } catch (...) {
    }
  });
  return future.get();
}

void ForgeCoreBridge::StepAsync(BridgeRequest request, StepCompletion completion) {
  if (!IsAvailable()) {
    completion(BridgeResponse::Failure(
        request.id, request.hasId, L"platform_unsupported", L"core.step requires loading forge_ffi.dll into the Windows host"));
    return;
  }

  if (request.params.HasKey(L"app")) {
    auto appValue = request.params.GetNamedValue(L"app");
    if (appValue.ValueType() != json::JsonValueType::String) {
      completion(BridgeResponse::Failure(
          request.id, request.hasId, L"invalid_request", L"core.step app field must be a string when present"));
      return;
    }
    auto requestedApp = std::wstring(appValue.GetString().c_str());
    if (requestedApp != request.context.appId) {
      json::JsonObject details;
      details.Insert(L"requestedApp", json::JsonValue::CreateStringValue(requestedApp));
      details.Insert(L"channelApp", json::JsonValue::CreateStringValue(request.context.appId));
      completion(BridgeResponse::Failure(
          request.id,
          request.hasId,
          L"permission_denied",
          L"core.step app field does not match the channel-derived app id",
          details));
      return;
    }
  }

  std::string command = WideToUtf8(std::wstring(CoreCommandForRequest(request).Stringify().c_str()));
  auto state = std::make_shared<StepCompletionState>(std::move(completion));
  try {
    std::thread(
        [state, request]() {
          InitializeWorkerApartment();
          std::this_thread::sleep_for(std::chrono::milliseconds(kCoreStepTimeoutMs));
          CompleteStep(state, TimeoutFailure(request));
        })
        .detach();
    std::thread(
        [state, runtime = runtime_, command = std::move(command), request]() mutable {
          InitializeWorkerApartment();
          CompleteStep(state, ResponseForOutcome(request, RunCoreStep(std::move(runtime), std::move(command))));
        })
        .detach();
  } catch (...) {
    CompleteStep(state, ResponseForOutcome(request, CoreCommandOutcome{.kind = CoreCommandOutcome::Kind::WorkerFailed}));
  }
}

void ForgeCoreBridge::CompleteStep(std::shared_ptr<StepCompletionState> state, json::JsonObject response) {
  StepCompletion completion;
  {
    std::lock_guard<std::mutex> guard(state->mutex);
    if (state->completed) {
      return;
    }
    state->completed = true;
    completion = std::move(state->completion);
  }
  if (completion) {
    completion(std::move(response));
  }
}

ForgeCoreBridge::CoreCommandOutcome ForgeCoreBridge::RunCoreStep(std::shared_ptr<CoreRuntime> runtime, std::string commandJson) {
  if (runtime == nullptr || runtime->core == nullptr || runtime->handleCommand == nullptr || runtime->freeString == nullptr) {
    return CoreCommandOutcome{.kind = CoreCommandOutcome::Kind::WorkerFailed};
  }

  std::lock_guard<std::mutex> guard(runtime->stepMutex);
  char* output = runtime->handleCommand(runtime->core, commandJson.c_str());
  if (output == nullptr) {
    return CoreCommandOutcome{.kind = CoreCommandOutcome::Kind::EmptyOutput};
  }

  std::string outputText(output);
  runtime->freeString(output);
  return CoreCommandOutcome{.kind = CoreCommandOutcome::Kind::Output, .output = std::move(outputText)};
}

winrt::Windows::Data::Json::JsonObject ForgeCoreBridge::ResponseForOutcome(
    BridgeRequest const& request,
    CoreCommandOutcome const& outcome) {
  if (outcome.kind == CoreCommandOutcome::Kind::EmptyOutput) {
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"forge_core_handle_command returned empty output");
  }
  if (outcome.kind == CoreCommandOutcome::Kind::WorkerFailed) {
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"core.step worker failed");
  }

  json::JsonObject response{nullptr};
  if (!json::JsonObject::TryParse(Utf8ToWide(outcome.output), response)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"forge_core_handle_command returned invalid JSON");
  }
  if (!response.HasKey(L"ok") || response.GetNamedValue(L"ok").ValueType() != json::JsonValueType::Boolean) {
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"forge_core_handle_command returned a malformed CoreResponse");
  }
  if (response.GetNamedBoolean(L"ok", false)) {
    return BridgeResponse::Success(
        request.id,
        request.hasId,
        response.GetNamedValue(L"payload", json::JsonValue::CreateNullValue()));
  }

  json::JsonObject details;
  details.Insert(L"response", response);
  return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"legacy.core_step failed", details);
}

std::vector<std::filesystem::path> ForgeCoreBridge::CandidateLibraryPaths() {
  auto cwd = std::filesystem::current_path();
  auto exeDir = ExecutableDirectory();
  return {
      EnvironmentPath(L"TERRANE_FORGE_FFI_DLL"),
      exeDir / L"forge_ffi.dll",
      cwd / L"forge" / L"target" / L"debug" / L"forge_ffi.dll",
      cwd / L"forge" / L"target" / L"release" / L"forge_ffi.dll",
      cwd / L"forge" / L"target" / L"x86_64-pc-windows-msvc" / L"debug" / L"forge_ffi.dll",
      cwd / L"forge" / L"target" / L"x86_64-pc-windows-msvc" / L"release" / L"forge_ffi.dll",
      cwd / L".." / L"forge" / L"target" / L"debug" / L"forge_ffi.dll",
      cwd / L".." / L"forge" / L"target" / L"release" / L"forge_ffi.dll",
  };
}

std::filesystem::path ForgeCoreBridge::CoreDatabasePath(std::filesystem::path const& databasePath) {
  auto parent = databasePath.parent_path();
  if (parent.empty()) {
    parent = std::filesystem::current_path();
  }
  return parent / L"forge-workspace.sqlite";
}

bool ForgeCoreBridge::Load(std::filesystem::path const& path) {
  HMODULE handle = LoadLibraryW(path.c_str());
  if (handle == nullptr) {
    return false;
  }

  auto openCore = reinterpret_cast<ForgeCoreOpenFn>(GetProcAddress(handle, "forge_core_open"));
  auto handleCommand = reinterpret_cast<ForgeCoreHandleCommandFn>(GetProcAddress(handle, "forge_core_handle_command"));
  auto drainEvents = reinterpret_cast<ForgeCoreDrainEventsFn>(GetProcAddress(handle, "forge_core_drain_events"));
  auto lastError = reinterpret_cast<ForgeCoreLastErrorFn>(GetProcAddress(handle, "forge_core_last_error"));
  auto closeCore = reinterpret_cast<ForgeCoreCloseFn>(GetProcAddress(handle, "forge_core_close"));
  auto freeString = reinterpret_cast<ForgeStringFreeFn>(GetProcAddress(handle, "forge_string_free"));
  if (openCore == nullptr ||
      handleCommand == nullptr ||
      drainEvents == nullptr ||
      lastError == nullptr ||
      closeCore == nullptr ||
      freeString == nullptr) {
    FreeLibrary(handle);
    return false;
  }

  std::filesystem::create_directories(coreDatabasePath_.parent_path());
  auto coreDatabasePath = WideToUtf8(coreDatabasePath_.wstring());
  void* core = openCore(coreDatabasePath.c_str(), "windows-native");
  if (core == nullptr) {
    char* error = lastError();
    if (error != nullptr) {
      freeString(error);
    }
    FreeLibrary(handle);
    return false;
  }

  auto runtime = std::make_shared<CoreRuntime>();
  runtime->handle = handle;
  runtime->core = core;
  runtime->closeCore = closeCore;
  runtime->handleCommand = handleCommand;
  runtime->drainEvents = drainEvents;
  runtime->lastError = lastError;
  runtime->freeString = freeString;
  runtime_ = std::move(runtime);
  loadedPath_ = path;
  return true;
}

winrt::Windows::Data::Json::JsonObject ForgeCoreBridge::CorePayloadForRequest(BridgeRequest const& request) const {
  json::JsonObject payload;
  for (auto const& entry : request.params) {
    auto key = std::wstring(entry.Key().c_str());
    if (key == L"app") {
      continue;
    }
    payload.Insert(key, entry.Value());
  }
  payload.Insert(L"app", json::JsonValue::CreateStringValue(request.context.appId));
  return payload;
}

std::optional<winrt::Windows::Data::Json::IJsonValue> ForgeCoreBridge::PayloadFromCommandOutcome(
    CoreCommandOutcome const& outcome) const {
  if (outcome.kind != CoreCommandOutcome::Kind::Output) {
    return std::nullopt;
  }
  json::JsonObject response{nullptr};
  if (!json::JsonObject::TryParse(Utf8ToWide(outcome.output), response)) {
    return std::nullopt;
  }
  if (!response.HasKey(L"ok") || !response.GetNamedBoolean(L"ok", false)) {
    return std::nullopt;
  }
  return response.GetNamedValue(L"payload", json::JsonValue::CreateNullValue());
}

std::optional<winrt::Windows::Data::Json::IJsonValue> ForgeCoreBridge::Command(
    std::wstring const& name,
    winrt::Windows::Data::Json::IJsonValue const& payload,
    std::wstring const& requestId) {
  if (!IsAvailable()) {
    return std::nullopt;
  }
  auto command = CommandEnvelope(name, payload, requestId);
  auto outcome = RunCoreStep(runtime_, WideToUtf8(std::wstring(command.Stringify().c_str())));
  return PayloadFromCommandOutcome(outcome);
}

std::optional<winrt::Windows::Data::Json::IJsonValue> ForgeCoreBridge::ControlCommand(
    std::wstring const& name,
    winrt::Windows::Data::Json::IJsonValue const& payload) {
  static std::atomic_uint64_t sequence{0};
  auto requestId = L"windows-control-" + std::to_wstring(GetCurrentProcessId()) + L"_" +
      std::to_wstring(NowMs()) + L"_" + std::to_wstring(sequence.fetch_add(1));
  return Command(name, payload, requestId);
}

std::optional<winrt::Windows::Data::Json::JsonObject> ForgeCoreBridge::BridgeCommandDictionary(
    std::wstring const& name,
    winrt::Windows::Data::Json::IJsonValue const& payload,
    std::wstring const& requestId) {
  auto result = Command(name, payload, requestId);
  if (!result.has_value() || result->ValueType() != json::JsonValueType::Object) {
    return std::nullopt;
  }
  return result->GetObject();
}

winrt::Windows::Data::Json::JsonObject ForgeCoreBridge::CommandEnvelope(
    std::wstring const& name,
    winrt::Windows::Data::Json::IJsonValue const& payload,
    std::wstring const& requestId) const {
  json::JsonObject actor;
  actor.Insert(L"actor", json::JsonValue::CreateStringValue(L"windows-host"));
  actor.Insert(L"role", json::JsonValue::CreateStringValue(L"owner"));

  json::JsonObject command;
  command.Insert(L"request_id", json::JsonValue::CreateStringValue(requestId));
  command.Insert(L"actor", actor);
  command.Insert(L"workspace_id", json::JsonValue::CreateStringValue(L"windows-native"));
  command.Insert(L"name", json::JsonValue::CreateStringValue(name));
  command.Insert(L"payload", payload);
  return command;
}

winrt::Windows::Data::Json::JsonObject ForgeCoreBridge::CoreCommandForRequest(BridgeRequest const& request) const {
  auto requestId = request.hasId && !request.id.empty() ? request.id : L"windows-core-step";
  return CommandEnvelope(L"legacy.core_step", CorePayloadForRequest(request), requestId);
}

winrt::Windows::Data::Json::JsonObject ForgeCoreBridge::TimeoutFailure(BridgeRequest const& request) {
  json::JsonObject details;
  details.Insert(L"timeoutMs", json::JsonValue::CreateNumberValue(kCoreStepTimeoutMs));
  return BridgeResponse::Failure(request.id, request.hasId, L"timeout", L"core.step timed out", details);
}

}  // namespace terrane
