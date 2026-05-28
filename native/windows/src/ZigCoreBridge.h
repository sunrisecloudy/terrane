#pragma once

#include "BridgeTypes.h"

#include <Windows.h>
#include <cstddef>
#include <filesystem>
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

 private:
  struct ZigCoreBuffer {
    uint8_t* ptr;
    size_t len;
  };

  using CoreCreateFn = void* (*)();
  using CoreDestroyFn = void (*)(void*);
  using CoreStepJsonFn = int32_t (*)(void*, uint8_t const*, size_t, ZigCoreBuffer*);
  using CoreFreeFn = void (*)(ZigCoreBuffer);

  static std::vector<std::filesystem::path> CandidateLibraryPaths();
  bool Load(std::filesystem::path const& path);
  winrt::Windows::Data::Json::JsonObject CoreInputForRequest(BridgeRequest const& request) const;

  HMODULE handle_ = nullptr;
  void* core_ = nullptr;
  CoreDestroyFn destroy_ = nullptr;
  CoreStepJsonFn stepJson_ = nullptr;
  CoreFreeFn freeBuffer_ = nullptr;
  std::filesystem::path loadedPath_;
};

}  // namespace nativeai
