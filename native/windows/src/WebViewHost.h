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
  void OnWebMessage(ICoreWebView2WebMessageReceivedEventArgs* args);
  AppSandboxContext SandboxContextFromSource(std::wstring const& source) const;
  std::set<std::wstring> PermissionsForApp(std::wstring const& appId) const;
  std::wstring AppIdFromSource(std::wstring const& source) const;

  static std::filesystem::path RepoRoot();
  static std::filesystem::path RuntimeRoot();
  static std::filesystem::path DatabasePath();

  HWND window_ = nullptr;
  Microsoft::WRL::ComPtr<ICoreWebView2Controller> controller_;
  Microsoft::WRL::ComPtr<ICoreWebView2> webview_;
  std::unique_ptr<WebBridge> bridge_;
};

}  // namespace nativeai
