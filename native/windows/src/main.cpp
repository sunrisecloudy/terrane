#include "WebViewHost.h"

#include <Windows.h>
#include <shellapi.h>
#include <memory>
#include <string_view>
#include <winrt/base.h>

namespace {
std::unique_ptr<nativeai::WebViewHost> g_host;

bool DebugBuildAllowsDevFlags() {
#ifdef _DEBUG
  return true;
#else
  return false;
#endif
}

bool IsForbiddenDevFlag(std::wstring_view argument) {
  constexpr std::wstring_view flags[] = {
      L"--control-plane-port",
      L"--allow-runtime-mismatch",
      L"--allow-unsigned-dev",
  };
  for (const auto flag : flags) {
    if (argument == flag) {
      return true;
    }
    if (argument.size() > flag.size() && argument.substr(0, flag.size()) == flag &&
        argument[flag.size()] == L'=') {
      return true;
    }
  }
  return false;
}

bool RejectDevOnlyFlagsIfNeeded() {
  if (DebugBuildAllowsDevFlags()) {
    return false;
  }
  int argc = 0;
  LPWSTR *argv = CommandLineToArgvW(GetCommandLineW(), &argc);
  if (!argv) {
    return false;
  }
  for (int index = 1; index < argc; ++index) {
    if (IsForbiddenDevFlag(argv[index])) {
      OutputDebugStringW(L"fatal: production build rejects dev-only startup flag\n");
      LocalFree(argv);
      return true;
    }
  }
  LocalFree(argv);
  return false;
}

LRESULT CALLBACK WindowProc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
  switch (message) {
    case WM_SIZE:
      return 0;
    case WM_DESTROY:
      PostQuitMessage(0);
      return 0;
    default:
      return DefWindowProc(window, message, wparam, lparam);
  }
}
}  // namespace

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE, PWSTR, int showCommand) {
  if (RejectDevOnlyFlagsIfNeeded()) {
    return 1;
  }

  winrt::init_apartment(winrt::apartment_type::single_threaded);

  WNDCLASS windowClass{};
  windowClass.lpfnWndProc = WindowProc;
  windowClass.hInstance = instance;
  windowClass.lpszClassName = L"NativeAIWebappHostWindow";
  RegisterClass(&windowClass);

  HWND window = CreateWindowEx(
      0,
      windowClass.lpszClassName,
      L"Native AI Webapp Platform",
      WS_OVERLAPPEDWINDOW,
      CW_USEDEFAULT,
      CW_USEDEFAULT,
      1200,
      820,
      nullptr,
      nullptr,
      instance,
      nullptr);

  ShowWindow(window, showCommand);
  g_host = std::make_unique<nativeai::WebViewHost>(window);
  g_host->Initialize();

  MSG message{};
  while (GetMessage(&message, nullptr, 0, 0)) {
    TranslateMessage(&message);
    DispatchMessage(&message);
  }

  g_host.reset();
  return 0;
}
