#pragma once

#include "BridgeTypes.h"

#include <Windows.h>
#include <cstddef>
#include <filesystem>
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace nativeai {

class ZigCoreBridge {
 public:
  ZigCoreBridge();
  ~ZigCoreBridge();

  ZigCoreBridge(ZigCoreBridge const&) = delete;
  ZigCoreBridge& operator=(ZigCoreBridge const&) = delete;

  bool IsAvailable() const;
  winrt::Windows::Data::Json::JsonObject Step(BridgeRequest const& request);
  using StepCompletion = std::function<void(winrt::Windows::Data::Json::JsonObject)>;
  void StepAsync(BridgeRequest request, StepCompletion completion);

 private:
  struct ZigCoreBuffer {
    uint8_t* ptr;
    size_t len;
  };

  using CoreCreateFn = void* (*)();
  using CoreDestroyFn = void (*)(void*);
  using CoreStepJsonFn = int32_t (*)(void*, uint8_t const*, size_t, ZigCoreBuffer*);
  using CoreFreeFn = void (*)(ZigCoreBuffer);

  struct CoreRuntime;
  struct CoreStepOutcome {
    enum class Kind {
      Output,
      StepFailed,
      EmptyOutput,
      WorkerFailed,
    };
    Kind kind = Kind::WorkerFailed;
    int32_t status = 0;
    std::string output;
  };

  static std::vector<std::filesystem::path> CandidateLibraryPaths();
  static CoreStepOutcome RunCoreStep(std::shared_ptr<CoreRuntime> runtime, std::string input);
  static winrt::Windows::Data::Json::JsonObject ResponseForOutcome(
      BridgeRequest const& request,
      CoreStepOutcome const& outcome);
  static winrt::Windows::Data::Json::JsonObject TimeoutFailure(BridgeRequest const& request);
  struct StepCompletionState;
  static void CompleteStep(
      std::shared_ptr<StepCompletionState> state,
      winrt::Windows::Data::Json::JsonObject response);
  bool Load(std::filesystem::path const& path);
  winrt::Windows::Data::Json::JsonObject CoreInputForRequest(BridgeRequest const& request) const;

  std::shared_ptr<CoreRuntime> runtime_;
  std::filesystem::path loadedPath_;
};

}  // namespace nativeai
