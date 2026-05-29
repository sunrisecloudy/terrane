#pragma once

#include "BridgeTypes.h"
#include "PlatformDialogs.h"
#include "PlatformNetwork.h"
#include "PlatformNotifications.h"
#include "PlatformStorage.h"
#include "ZigCoreBridge.h"

#include <Windows.h>
#include <optional>

namespace nativeai {

class WebBridge {
 public:
  WebBridge(std::filesystem::path databasePath, HWND ownerWindow);

  std::wstring HandleJson(std::wstring const& body, AppSandboxContext const& context);
  sqlite3* DatabaseHandle() const { return storage_.DatabaseHandle(); }

 private:
  std::optional<std::wstring> permissionForBridgeMethod(std::wstring const& method) const;
  winrt::Windows::Data::Json::JsonObject Dispatch(BridgeRequest const& request);
  winrt::Windows::Data::Json::JsonObject Capabilities(BridgeRequest const& request) const;
  void RecordBridgeCall(
      BridgeRequest const& request,
      winrt::Windows::Data::Json::JsonObject const& response,
      uint64_t startedAtMs);
  void EnsureRuntimeSession(BridgeRequest const& request);

  PlatformStorage storage_;
  PlatformDialogs dialogs_;
  PlatformNotifications notifications_;
  PlatformNetwork network_;
  ZigCoreBridge core_;
};

}  // namespace nativeai
