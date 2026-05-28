#pragma once

#include "BridgeTypes.h"
#include "PlatformDialogs.h"
#include "PlatformNetwork.h"
#include "PlatformNotifications.h"
#include "PlatformStorage.h"
#include "ZigCoreBridge.h"

#include <optional>

namespace nativeai {

class WebBridge {
 public:
  explicit WebBridge(std::filesystem::path databasePath);

  std::wstring HandleJson(std::wstring const& body, AppSandboxContext const& context);

 private:
  std::optional<std::wstring> permissionForBridgeMethod(std::wstring const& method) const;
  winrt::Windows::Data::Json::JsonObject Dispatch(BridgeRequest const& request);
  winrt::Windows::Data::Json::JsonObject Capabilities(BridgeRequest const& request) const;

  PlatformStorage storage_;
  PlatformDialogs dialogs_;
  PlatformNotifications notifications_;
  PlatformNetwork network_;
  ZigCoreBridge core_;
};

}  // namespace nativeai
