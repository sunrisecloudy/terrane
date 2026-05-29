#pragma once

#include "WebBridge.h"

#include <WebView2.h>
#include <filesystem>
#include <memory>
#include <wrl.h>

namespace nativeai {

class WebViewHost {
 public:
  explicit WebViewHost(HWND window);

  void Initialize();

 private:
  void OnNavigationCompleted(ICoreWebView2NavigationCompletedEventArgs* args);
  void OnWebMessage(ICoreWebView2WebMessageReceivedEventArgs* args);
  void RunSmoke();
  void RunStorageSmoke(bool setValue);
  void RunCoreSmoke();
  void RunFixedBridgeSurfaceSmoke();
  void RunWebBridgeStorageSmoke(bool setValue);
  void RunWebBridgeCoreSmoke();
  void RunRuntimeAppBridgeSmoke();
  void StartWebBridgeSmoke(
      std::wstring const& appId,
      std::wstring const& id,
      std::wstring const& method,
      winrt::Windows::Data::Json::JsonObject const& params);
  void HandleWebBridgeSmokeResponse(std::wstring const& requestId, std::wstring const& response);
  void HandleRuntimeAppBridgeSmokeResponse(
      std::wstring const& appId,
      std::wstring const& method,
      std::wstring const& response);
  void SmokeSuccess(std::wstring const& marker);
  void SmokeFailure(std::wstring const& message);
  std::wstring BridgeCall(
      std::wstring const& appId,
      std::wstring const& id,
      std::wstring const& method,
      winrt::Windows::Data::Json::JsonObject const& params);
  AppSandboxContext SandboxContextFromSource(std::wstring const& source) const;
  AppSandboxContext SandboxContextForApp(std::wstring const& appId, std::wstring const& mountToken) const;
  std::set<std::wstring> PermissionsForApp(std::wstring const& appId) const;
  std::vector<NetworkPolicyRule> NetworkPolicyForApp(std::wstring const& appId) const;
  bool DenyPrivateNetworkForApp(std::wstring const& appId) const;
  std::wstring AppIdFromSource(std::wstring const& source) const;

  static std::filesystem::path RepoRoot();
  static std::filesystem::path RuntimeRoot();
  static std::filesystem::path DatabasePath();

  HWND window_ = nullptr;
  Microsoft::WRL::ComPtr<ICoreWebView2Controller> controller_;
  Microsoft::WRL::ComPtr<ICoreWebView2> webview_;
  std::unique_ptr<WebBridge> bridge_;
  bool smokeRan_ = false;
};

}  // namespace nativeai
