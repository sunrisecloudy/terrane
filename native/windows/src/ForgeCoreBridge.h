#pragma once

#include "BridgeTypes.h"

#include <Windows.h>
#include <cstddef>
#include <filesystem>
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace terrane {

class ForgeCoreBridge {
 public:
  explicit ForgeCoreBridge(std::filesystem::path databasePath);
  ~ForgeCoreBridge();

  ForgeCoreBridge(ForgeCoreBridge const&) = delete;
  ForgeCoreBridge& operator=(ForgeCoreBridge const&) = delete;

  bool IsAvailable() const;
  winrt::Windows::Data::Json::JsonObject Step(BridgeRequest const& request);
  using StepCompletion = std::function<void(winrt::Windows::Data::Json::JsonObject)>;
  void StepAsync(BridgeRequest request, StepCompletion completion);

 private:
  using ForgeCoreOpenFn = void* (*)(char const*, char const*);
  using ForgeCoreHandleCommandFn = char* (*)(void*, char const*);
  using ForgeCoreDrainEventsFn = char* (*)(void*);
  using ForgeCoreLastErrorFn = char* (*)();
  using ForgeCoreCloseFn = void (*)(void*);
  using ForgeStringFreeFn = void (*)(char*);

  struct CoreRuntime;
  struct CoreCommandOutcome {
    enum class Kind {
      Output,
      EmptyOutput,
      WorkerFailed,
    };
    Kind kind = Kind::WorkerFailed;
    std::string output;
  };

  static std::vector<std::filesystem::path> CandidateLibraryPaths();
  static std::filesystem::path CoreDatabasePath(std::filesystem::path const& databasePath);
  static CoreCommandOutcome RunCoreStep(std::shared_ptr<CoreRuntime> runtime, std::string commandJson);
  static winrt::Windows::Data::Json::JsonObject ResponseForOutcome(
      BridgeRequest const& request,
      CoreCommandOutcome const& outcome);
  static winrt::Windows::Data::Json::JsonObject TimeoutFailure(BridgeRequest const& request);
  struct StepCompletionState;
  static void CompleteStep(
      std::shared_ptr<StepCompletionState> state,
      winrt::Windows::Data::Json::JsonObject response);
  bool Load(std::filesystem::path const& path);
  winrt::Windows::Data::Json::JsonObject CoreCommandForRequest(BridgeRequest const& request) const;
  winrt::Windows::Data::Json::JsonObject CorePayloadForRequest(BridgeRequest const& request) const;

  std::shared_ptr<CoreRuntime> runtime_;
  std::filesystem::path loadedPath_;
  std::filesystem::path coreDatabasePath_;
};

}  // namespace terrane
