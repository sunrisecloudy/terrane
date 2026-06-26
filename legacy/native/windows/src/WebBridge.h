#pragma once

#include "BridgeTypes.h"
#include "PlatformDialogs.h"
#include "PlatformNetwork.h"
#include "PlatformNotifications.h"
#include "PlatformStorage.h"
#include "ForgeCoreBridge.h"

#include <Windows.h>
#include <functional>
#include <optional>

namespace terrane {

class WebBridge {
 public:
  WebBridge(std::filesystem::path databasePath, HWND ownerWindow);

  std::wstring HandleJson(std::wstring const& body, AppSandboxContext const& context);
  using BridgeCompletion = std::function<void(std::wstring)>;
  void HandleJsonAsync(std::wstring body, AppSandboxContext context, BridgeCompletion completion);
  sqlite3* DatabaseHandle() const { return storage_.DatabaseHandle(); }

 private:
  std::optional<std::wstring> permissionForBridgeMethod(std::wstring const& method) const;
  winrt::Windows::Data::Json::JsonObject Dispatch(BridgeRequest const& request);
  std::optional<winrt::Windows::Data::Json::JsonObject> FaultInjectionFailure(BridgeRequest const& request) const;
  void DisableFaultInjection(std::wstring const& faultId) const;
  std::optional<winrt::Windows::Data::Json::JsonObject> MockedDialogResponse(
      BridgeRequest const& request,
      std::wstring const& dialogType) const;
  std::optional<winrt::Windows::Data::Json::JsonObject> ResourceBudgetFailure(BridgeRequest const& request) const;
  winrt::Windows::Data::Json::JsonObject Capabilities(BridgeRequest const& request) const;
  winrt::Windows::Data::Json::JsonObject AppLog(BridgeRequest const& request) const;
  int BridgeCallCountSince(std::wstring const& appId, int seconds) const;
  int BridgeCallCountSince(std::wstring const& appId, std::wstring const& method, int seconds) const;
  void RecordBridgeCall(
      BridgeRequest const& request,
      winrt::Windows::Data::Json::JsonObject const& response,
      uint64_t startedAtMs);
  void RecordCoreStep(
      BridgeRequest const& request,
      winrt::Windows::Data::Json::JsonObject const& response);
  void RecordCoreAction(
      std::wstring const& eventId,
      std::wstring const& sessionId,
      std::wstring const& appId,
      winrt::Windows::Data::Json::IJsonValue const& action);
  void EnsureRuntimeSession(BridgeRequest const& request);

  PlatformStorage storage_;
  PlatformDialogs dialogs_;
  PlatformNotifications notifications_;
  PlatformNetwork network_;
  ForgeCoreBridge core_;
};

}  // namespace terrane
