#include "BridgeTypes.h"
#include "PlatformDatabase.h"
#include "WebViewHost.h"

#include <Windows.h>
#include <ShlObj.h>
#include <shellapi.h>
#include <filesystem>
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

std::wstring EnvironmentValue(wchar_t const* name) {
  DWORD required = GetEnvironmentVariableW(name, nullptr, 0);
  if (required == 0) {
    return L"";
  }
  std::wstring value(required, L'\0');
  DWORD written = GetEnvironmentVariableW(name, value.data(), required);
  if (written == 0 || written >= required) {
    return L"";
  }
  value.resize(written);
  return value;
}

std::filesystem::path ProductionGuardDatabasePath() {
  auto smokeDataHome = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_DATA_HOME");
  if (!smokeDataHome.empty()) {
    return std::filesystem::path(smokeDataHome) / L"NativeAIWebappPlatform" / L"platform.sqlite";
  }
  PWSTR localAppData = nullptr;
  if (SUCCEEDED(SHGetKnownFolderPath(FOLDERID_LocalAppData, KF_FLAG_CREATE, nullptr, &localAppData)) && localAppData != nullptr) {
    std::filesystem::path path(localAppData);
    CoTaskMemFree(localAppData);
    return path / L"NativeAIWebappPlatform" / L"platform.sqlite";
  }
  return std::filesystem::current_path() / L"platform.sqlite";
}

std::wstring JsonString(std::wstring_view value) {
  std::wstring escaped = L"\"";
  for (wchar_t ch : value) {
    switch (ch) {
      case L'\\':
        escaped += L"\\\\";
        break;
      case L'"':
        escaped += L"\\\"";
        break;
      case L'\n':
        escaped += L"\\n";
        break;
      case L'\r':
        escaped += L"\\r";
        break;
      case L'\t':
        escaped += L"\\t";
        break;
      default:
        escaped.push_back(ch);
        break;
    }
  }
  escaped += L"\"";
  return escaped;
}

void BindText(sqlite3_stmt* statement, int index, std::wstring const& value) {
  auto text = nativeai::WideToUtf8(value);
  sqlite3_bind_text(statement, index, text.c_str(), -1, SQLITE_TRANSIENT);
}

void RecordProductionGuardAudit(std::wstring const& flag) {
  nativeai::PlatformDatabase database(ProductionGuardDatabasePath());
  sqlite3* db = database.handle();
  if (db == nullptr) {
    return;
  }

  const auto suffix = std::to_wstring(GetCurrentProcessId()) + L"_" + std::to_wstring(GetTickCount64());
  const auto sessionId = L"windows-production-guard-" + suffix;
  const auto commandId = L"command-windows-production-guard-" + suffix;
  const auto argsJson = L"{\"flag\":" + JsonString(flag) + L"}";
  const auto errorJson =
      L"{\"code\":\"dev_only_flag\",\"message\":\"Production build rejects dev-only flag\",\"details\":{\"flag\":" +
      JsonString(flag) +
      L"}}";
  const auto metadataJson = L"{\"reason\":\"dev_only_flag\",\"flag\":" + JsonString(flag) + L"}";

  sqlite3_stmt* statement = nullptr;
  if (sqlite3_prepare_v2(
          db,
          "INSERT OR REPLACE INTO control_sessions "
          "(control_session_id, target, actor, started_at, ended_at, status, metadata_json) "
          "VALUES (?, 'windows', 'native-production-guard', datetime('now'), datetime('now'), 'failed', ?)",
          -1,
          &statement,
          nullptr) == SQLITE_OK) {
    BindText(statement, 1, sessionId);
    BindText(statement, 2, metadataJson);
    sqlite3_step(statement);
  }
  sqlite3_finalize(statement);

  statement = nullptr;
  if (sqlite3_prepare_v2(
          db,
          "INSERT INTO control_commands "
          "(command_id, control_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms) "
          "VALUES (?, ?, 'native.production_guard', NULL, NULL, 'rejected', 'dev_only_flag', ?, NULL, ?, datetime('now'), 0)",
          -1,
          &statement,
          nullptr) == SQLITE_OK) {
    BindText(statement, 1, commandId);
    BindText(statement, 2, sessionId);
    BindText(statement, 3, argsJson);
    BindText(statement, 4, errorJson);
    sqlite3_step(statement);
  }
  sqlite3_finalize(statement);
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
      RecordProductionGuardAudit(argv[index]);
      OutputDebugStringW(L"fatal: production build rejects dev-only startup flag\n");
      LocalFree(argv);
      return true;
    }
  }
  LocalFree(argv);
  return false;
}

LRESULT CALLBACK WindowProc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
  if (g_host) {
    LRESULT result = 0;
    if (g_host->TryHandleWindowMessage(message, wparam, lparam, &result)) {
      return result;
    }
  }
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
