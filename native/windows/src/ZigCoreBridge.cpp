#include "ZigCoreBridge.h"

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

struct ZigCoreBridge::CoreRuntime {
  ~CoreRuntime() {
    if (destroy != nullptr && core != nullptr) {
      destroy(core);
    }
    if (handle != nullptr) {
      FreeLibrary(handle);
    }
  }

  HMODULE handle = nullptr;
  void* core = nullptr;
  CoreDestroyFn destroy = nullptr;
  CoreStepJsonFn stepJson = nullptr;
  CoreFreeFn freeBuffer = nullptr;
  std::mutex stepMutex;
};

struct ZigCoreBridge::StepCompletionState {
  explicit StepCompletionState(StepCompletion callback) : completion(std::move(callback)) {}

  std::mutex mutex;
  bool completed = false;
  StepCompletion completion;
};

ZigCoreBridge::ZigCoreBridge() {
  for (auto const& path : CandidateLibraryPaths()) {
    if (!path.empty() && Load(path)) {
      break;
    }
  }
}

ZigCoreBridge::~ZigCoreBridge() = default;

bool ZigCoreBridge::IsAvailable() const {
  auto runtime = runtime_;
  return runtime != nullptr && runtime->handle != nullptr && runtime->core != nullptr && runtime->stepJson != nullptr &&
      runtime->freeBuffer != nullptr;
}

winrt::Windows::Data::Json::JsonObject ZigCoreBridge::Step(BridgeRequest const& request) {
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

void ZigCoreBridge::StepAsync(BridgeRequest request, StepCompletion completion) {
  if (!IsAvailable()) {
    completion(BridgeResponse::Failure(
        request.id, request.hasId, L"platform_unsupported", L"core.step requires loading zig_core.dll into the Windows host"));
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

  std::string input = WideToUtf8(std::wstring(CoreInputForRequest(request).Stringify().c_str()));
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
        [state, runtime = runtime_, input = std::move(input), request]() mutable {
          InitializeWorkerApartment();
          CompleteStep(state, ResponseForOutcome(request, RunCoreStep(std::move(runtime), std::move(input))));
        })
        .detach();
  } catch (...) {
    CompleteStep(state, ResponseForOutcome(request, CoreStepOutcome{.kind = CoreStepOutcome::Kind::WorkerFailed}));
  }
}

void ZigCoreBridge::CompleteStep(std::shared_ptr<StepCompletionState> state, json::JsonObject response) {
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

ZigCoreBridge::CoreStepOutcome ZigCoreBridge::RunCoreStep(std::shared_ptr<CoreRuntime> runtime, std::string input) {
  if (runtime == nullptr || runtime->core == nullptr || runtime->stepJson == nullptr || runtime->freeBuffer == nullptr) {
    return CoreStepOutcome{.kind = CoreStepOutcome::Kind::WorkerFailed};
  }

  std::lock_guard<std::mutex> guard(runtime->stepMutex);
  ZigCoreBuffer output{};
  int32_t code = runtime->stepJson(runtime->core, reinterpret_cast<uint8_t const*>(input.data()), input.size(), &output);
  if (code != 0) {
    if (output.ptr != nullptr) {
      runtime->freeBuffer(output);
    }
    return CoreStepOutcome{.kind = CoreStepOutcome::Kind::StepFailed, .status = code};
  }
  if (output.ptr == nullptr) {
    return CoreStepOutcome{.kind = CoreStepOutcome::Kind::EmptyOutput};
  }

  std::string outputText(reinterpret_cast<char const*>(output.ptr), output.len);
  runtime->freeBuffer(output);
  return CoreStepOutcome{.kind = CoreStepOutcome::Kind::Output, .output = std::move(outputText)};
}

winrt::Windows::Data::Json::JsonObject ZigCoreBridge::ResponseForOutcome(
    BridgeRequest const& request,
    CoreStepOutcome const& outcome) {
  if (outcome.kind == CoreStepOutcome::Kind::StepFailed) {
    json::JsonObject details;
    details.Insert(L"status", json::JsonValue::CreateNumberValue(outcome.status));
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"core_step_json failed", details);
  }
  if (outcome.kind == CoreStepOutcome::Kind::EmptyOutput) {
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"core.step returned empty output");
  }
  if (outcome.kind == CoreStepOutcome::Kind::WorkerFailed) {
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"core.step worker failed");
  }

  json::JsonObject result{nullptr};
  if (!json::JsonObject::TryParse(Utf8ToWide(outcome.output), result)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"core_error", L"core.step returned invalid JSON");
  }
  return BridgeResponse::Success(request.id, request.hasId, result);
}

std::vector<std::filesystem::path> ZigCoreBridge::CandidateLibraryPaths() {
  auto cwd = std::filesystem::current_path();
  auto exeDir = ExecutableDirectory();
  return {
      EnvironmentPath(L"TERRANE_ZIG_CORE_DLL"),
      exeDir / L"zig_core.dll",
      cwd / L"zig-core" / L"zig-out" / L"bin" / L"zig_core.dll",
      cwd / L"zig-core" / L"zig-out" / L"lib" / L"zig_core.dll",
      cwd / L".." / L"zig-core" / L"zig-out" / L"bin" / L"zig_core.dll",
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

  auto runtime = std::make_shared<CoreRuntime>();
  runtime->handle = handle;
  runtime->core = core;
  runtime->destroy = destroy;
  runtime->stepJson = stepJson;
  runtime->freeBuffer = freeBuffer;
  runtime_ = std::move(runtime);
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

winrt::Windows::Data::Json::JsonObject ZigCoreBridge::TimeoutFailure(BridgeRequest const& request) {
  json::JsonObject details;
  details.Insert(L"timeoutMs", json::JsonValue::CreateNumberValue(kCoreStepTimeoutMs));
  return BridgeResponse::Failure(request.id, request.hasId, L"timeout", L"core.step timed out", details);
}

}  // namespace terrane
