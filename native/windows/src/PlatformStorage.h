#pragma once

#include "BridgeTypes.h"
#include "PlatformDatabase.h"

#include <cstdint>
#include <filesystem>
#include <winsqlite/winsqlite3.h>

namespace nativeai {

class PlatformStorage {
 public:
  explicit PlatformStorage(std::filesystem::path databasePath);
  ~PlatformStorage();

  PlatformStorage(PlatformStorage const&) = delete;
  PlatformStorage& operator=(PlatformStorage const&) = delete;

  winrt::Windows::Data::Json::JsonObject Get(BridgeRequest const& request);
  winrt::Windows::Data::Json::JsonObject Set(BridgeRequest const& request);
  winrt::Windows::Data::Json::JsonObject Remove(BridgeRequest const& request);
  winrt::Windows::Data::Json::JsonObject List(BridgeRequest const& request);
  sqlite3* DatabaseHandle() const { return database_.handle(); }

 private:
  void EnsureAppRow(std::wstring const& appId);
  int64_t StorageBytesAfterSet(std::wstring const& appId, std::wstring const& key, int64_t valueBytes) const;
  winrt::Windows::Data::Json::JsonObject storagePrefixFailure(BridgeRequest const& request, std::wstring const& key);
  bool HasStoragePrefix(BridgeRequest const& request, std::wstring const& key) const;

  PlatformDatabase database_;
};

}  // namespace nativeai
