#include "WebViewHost.h"

#include <Windows.h>
#include <memory>
#include <winrt/base.h>

namespace {
std::unique_ptr<nativeai::WebViewHost> g_host;

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
