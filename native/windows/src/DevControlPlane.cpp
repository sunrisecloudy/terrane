#define WIN32_LEAN_AND_MEAN

#include <winsock2.h>
#include <ws2tcpip.h>
#include <Windows.h>
#include <bcrypt.h>
#include <ShlObj.h>

#include "DevControlPlane.h"

#include "BridgeTypes.h"
#include "PlatformDatabase.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <cctype>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>
#include <thread>

namespace nativeai {
namespace {

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

std::filesystem::path DefaultTokenPath() {
  auto overridePath = EnvironmentValue(L"PLATFORM_CONTROL_TOKEN_FILE");
  if (!overridePath.empty()) {
    return overridePath;
  }
  PWSTR localAppData = nullptr;
  if (SUCCEEDED(SHGetKnownFolderPath(FOLDERID_LocalAppData, KF_FLAG_CREATE, nullptr, &localAppData)) && localAppData != nullptr) {
    std::filesystem::path path(localAppData);
    CoTaskMemFree(localAppData);
    return path / L"NativeAIWebappPlatform" / L"control.token";
  }
  return std::filesystem::current_path() / L"control.token";
}

std::wstring JsonString(std::wstring const& value) {
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
  auto text = WideToUtf8(value);
  sqlite3_bind_text(statement, index, text.c_str(), -1, SQLITE_TRANSIENT);
}

std::string ToLowerAscii(std::string value) {
  std::transform(value.begin(), value.end(), value.begin(), [](unsigned char ch) {
    return static_cast<char>(std::tolower(ch));
  });
  return value;
}

std::string TrimAscii(std::string value) {
  while (!value.empty() && std::isspace(static_cast<unsigned char>(value.front()))) {
    value.erase(value.begin());
  }
  while (!value.empty() && std::isspace(static_cast<unsigned char>(value.back()))) {
    value.pop_back();
  }
  return value;
}

std::string HeaderValue(std::string const& request, std::string const& headerName) {
  std::istringstream lines(request);
  std::string line;
  std::string wanted = ToLowerAscii(headerName);
  while (std::getline(lines, line)) {
    if (!line.empty() && line.back() == '\r') {
      line.pop_back();
    }
    auto colon = line.find(':');
    if (colon == std::string::npos) {
      continue;
    }
    auto name = ToLowerAscii(TrimAscii(line.substr(0, colon)));
    if (name == wanted) {
      return TrimAscii(line.substr(colon + 1));
    }
  }
  return {};
}

std::string StatusText(int status) {
  switch (status) {
    case 200:
      return "OK";
    case 401:
      return "Unauthorized";
    case 404:
      return "Not Found";
    case 405:
      return "Method Not Allowed";
    default:
      return "Error";
  }
}

std::string ControlErrorJson(std::wstring const& code, std::wstring const& message) {
  auto body = L"{\"ok\":false,\"error\":{\"code\":" + JsonString(code) +
      L",\"message\":" + JsonString(message) + L",\"details\":{}}}";
  return WideToUtf8(body);
}

std::string HealthJson(uint16_t port) {
  return "{\"ok\":true,\"target\":\"windows\",\"status\":\"ok\",\"controlPlane\":{\"port\":" +
      std::to_string(port) + ",\"debug\":true}}";
}

bool SendAll(SOCKET socket, std::string const& text) {
  const char* cursor = text.data();
  int remaining = static_cast<int>(text.size());
  while (remaining > 0) {
    int sent = send(socket, cursor, remaining, 0);
    if (sent <= 0) {
      return false;
    }
    cursor += sent;
    remaining -= sent;
  }
  return true;
}

void SendJson(SOCKET socket, int status, std::string const& body) {
  std::ostringstream response;
  response << "HTTP/1.1 " << status << " " << StatusText(status) << "\r\n"
           << "Content-Type: application/json\r\n"
           << "Content-Length: " << body.size() << "\r\n"
           << "Connection: close\r\n\r\n"
           << body;
  SendAll(socket, response.str());
}

std::wstring MakeId(std::wstring const& prefix) {
  return prefix + L"-" + std::to_wstring(GetCurrentProcessId()) + L"-" + std::to_wstring(GetTickCount64());
}

std::wstring Base64Url(std::array<unsigned char, 32> const& bytes) {
  static constexpr wchar_t alphabet[] = L"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  std::wstring out;
  int value = 0;
  int bits = -6;
  for (unsigned char byte : bytes) {
    value = (value << 8) + byte;
    bits += 8;
    while (bits >= 0) {
      out.push_back(alphabet[(value >> bits) & 0x3F]);
      bits -= 6;
    }
  }
  if (bits > -6) {
    out.push_back(alphabet[((value << 8) >> (bits + 8)) & 0x3F]);
  }
  return out;
}

std::wstring Hex(std::array<unsigned char, 32> const& bytes) {
  static constexpr wchar_t alphabet[] = L"0123456789abcdef";
  std::wstring out;
  out.reserve(bytes.size() * 2);
  for (unsigned char byte : bytes) {
    out.push_back(alphabet[(byte >> 4) & 0x0F]);
    out.push_back(alphabet[byte & 0x0F]);
  }
  return out;
}

std::wstring Sha256Hex(std::wstring const& text) {
  auto bytes = WideToUtf8(text);
  BCRYPT_ALG_HANDLE algorithm = nullptr;
  BCRYPT_HASH_HANDLE hash = nullptr;
  DWORD objectLength = 0;
  DWORD resultLength = 0;
  std::array<unsigned char, 32> digest{};
  if (BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) < 0) {
    return {};
  }
  if (BCryptGetProperty(algorithm, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&objectLength), sizeof(objectLength), &resultLength, 0) < 0) {
    BCryptCloseAlgorithmProvider(algorithm, 0);
    return {};
  }
  std::string object(objectLength, '\0');
  if (BCryptCreateHash(algorithm, &hash, reinterpret_cast<PUCHAR>(object.data()), objectLength, nullptr, 0, 0) < 0 ||
      BCryptHashData(hash, reinterpret_cast<PUCHAR>(bytes.data()), static_cast<ULONG>(bytes.size()), 0) < 0 ||
      BCryptFinishHash(hash, digest.data(), static_cast<ULONG>(digest.size()), 0) < 0) {
    if (hash != nullptr) {
      BCryptDestroyHash(hash);
    }
    BCryptCloseAlgorithmProvider(algorithm, 0);
    return {};
  }
  BCryptDestroyHash(hash);
  BCryptCloseAlgorithmProvider(algorithm, 0);
  return Hex(digest);
}

bool GenerateToken(std::wstring* token) {
  std::array<unsigned char, 32> bytes{};
  if (BCryptGenRandom(nullptr, bytes.data(), static_cast<ULONG>(bytes.size()), BCRYPT_USE_SYSTEM_PREFERRED_RNG) < 0) {
    return false;
  }
  *token = Base64Url(bytes);
  return token->size() == 43;
}

bool WriteTokenFile(std::filesystem::path const& tokenPath, std::wstring const& token) {
  if (!tokenPath.parent_path().empty()) {
    std::filesystem::create_directories(tokenPath.parent_path());
  }
  HANDLE file = CreateFileW(
      tokenPath.c_str(),
      GENERIC_WRITE,
      0,
      nullptr,
      CREATE_ALWAYS,
      FILE_ATTRIBUTE_NORMAL,
      nullptr);
  if (file == INVALID_HANDLE_VALUE) {
    return false;
  }
  auto line = WideToUtf8(token + L"\n");
  DWORD written = 0;
  bool ok = WriteFile(file, line.data(), static_cast<DWORD>(line.size()), &written, nullptr) &&
      written == line.size();
  CloseHandle(file);
  return ok;
}

void WriteControlLine(std::wstring const& line) {
  auto markerPath = EnvironmentValue(L"NATIVE_AI_WINDOWS_SMOKE_RESULT_FILE");
  if (!markerPath.empty()) {
    std::ofstream file{std::filesystem::path(markerPath), std::ios::binary | std::ios::app};
    file << WideToUtf8(line) << "\n";
  }
  OutputDebugStringW((line + L"\n").c_str());
}

}  // namespace

struct DevControlPlane::Impl {
  SOCKET listenSocket = INVALID_SOCKET;
  std::thread thread;
  std::atomic_bool stopping{false};
  bool winsockStarted = false;
  uint16_t port = 0;
  std::filesystem::path databasePath;
  std::filesystem::path tokenPath;
  std::wstring token;
  std::wstring tokenHash;
  std::wstring controlSessionId;

  bool Start(DevControlPlaneConfig const& config, std::wstring* error) {
#ifndef _DEBUG
    if (error != nullptr) {
      *error = L"Windows dev control plane is disabled in release builds";
    }
    return false;
#else
    if (config.databasePath.empty()) {
      if (error != nullptr) {
        *error = L"Windows dev control requires a database path";
      }
      return false;
    }
    databasePath = config.databasePath;
    tokenPath = DefaultTokenPath();
    if (!GenerateToken(&token) || !WriteTokenFile(tokenPath, token)) {
      if (error != nullptr) {
        *error = L"Could not create Windows dev control token file";
      }
      return false;
    }
    tokenHash = Sha256Hex(token);
    controlSessionId = MakeId(L"windows-control-session");

    WSADATA data{};
    if (WSAStartup(MAKEWORD(2, 2), &data) != 0) {
      if (error != nullptr) {
        *error = L"WSAStartup failed";
      }
      return false;
    }
    winsockStarted = true;

    listenSocket = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (listenSocket == INVALID_SOCKET) {
      if (error != nullptr) {
        *error = L"Could not create Windows dev control socket";
      }
      Stop();
      return false;
    }

    sockaddr_in address{};
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port = htons(config.requestedPort);
    if (bind(listenSocket, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == SOCKET_ERROR ||
        listen(listenSocket, SOMAXCONN) == SOCKET_ERROR) {
      if (error != nullptr) {
        *error = L"Could not bind Windows dev control loopback socket";
      }
      Stop();
      return false;
    }

    sockaddr_in bound{};
    int boundLen = sizeof(bound);
    if (getsockname(listenSocket, reinterpret_cast<sockaddr*>(&bound), &boundLen) == 0) {
      port = ntohs(bound.sin_port);
    }
    InsertControlSession();
    thread = std::thread([this]() { AcceptLoop(); });
    WriteControlLine(L"NATIVE_AI_WINDOWS_CONTROL_READY port=" + std::to_wstring(port) + L" token_path=" + tokenPath.wstring());
    return true;
#endif
  }

  void Stop() {
    stopping.store(true);
    if (listenSocket != INVALID_SOCKET) {
      closesocket(listenSocket);
      listenSocket = INVALID_SOCKET;
    }
    if (thread.joinable()) {
      thread.join();
    }
    FinishControlSession();
    if (winsockStarted) {
      WSACleanup();
      winsockStarted = false;
    }
  }

  void InsertControlSession() {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      return;
    }
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(
            db,
            "INSERT OR REPLACE INTO control_sessions "
            "(control_session_id, target, actor, token_hash, started_at, status, metadata_json) "
            "VALUES (?, 'windows', 'codex', ?, datetime('now'), 'running', '{\"source\":\"native-windows-dev-control\"}')",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, controlSessionId);
      BindText(statement, 2, tokenHash);
      sqlite3_step(statement);
    }
    sqlite3_finalize(statement);
  }

  void FinishControlSession() {
    if (controlSessionId.empty() || databasePath.empty()) {
      return;
    }
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      return;
    }
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(
            db,
            "UPDATE control_sessions SET status = 'ended', ended_at = datetime('now') WHERE control_session_id = ? AND status = 'running'",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, controlSessionId);
      sqlite3_step(statement);
    }
    sqlite3_finalize(statement);
  }

  void Audit(
      std::wstring const& tool,
      std::wstring const& method,
      std::wstring const& path,
      std::wstring const& decision,
      std::wstring const& errorCode,
      std::wstring const& resultJson,
      std::wstring const& errorJson,
      uint64_t durationMs) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      return;
    }
    sqlite3_stmt* statement = nullptr;
    auto commandId = MakeId(L"windows-control-command");
    if (sqlite3_prepare_v2(
            db,
            "INSERT INTO control_commands "
            "(command_id, control_session_id, tool, http_method, path, decision, error_code, args_json, result_json, error_json, created_at, duration_ms) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, '{}', ?, ?, datetime('now'), ?)",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, commandId);
      BindText(statement, 2, controlSessionId);
      BindText(statement, 3, tool);
      BindText(statement, 4, method);
      BindText(statement, 5, path);
      BindText(statement, 6, decision);
      if (errorCode.empty()) {
        sqlite3_bind_null(statement, 7);
      } else {
        BindText(statement, 7, errorCode);
      }
      if (resultJson.empty()) {
        sqlite3_bind_null(statement, 8);
      } else {
        BindText(statement, 8, resultJson);
      }
      if (errorJson.empty()) {
        sqlite3_bind_null(statement, 9);
      } else {
        BindText(statement, 9, errorJson);
      }
      sqlite3_bind_int64(statement, 10, static_cast<sqlite3_int64>(durationMs));
      sqlite3_step(statement);
    }
    sqlite3_finalize(statement);
  }

  void AcceptLoop() {
    while (!stopping.load()) {
      SOCKET client = accept(listenSocket, nullptr, nullptr);
      if (client == INVALID_SOCKET) {
        if (stopping.load()) {
          break;
        }
        continue;
      }
      DWORD timeoutMs = 2000;
      setsockopt(client, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<char const*>(&timeoutMs), static_cast<int>(sizeof(timeoutMs)));
      setsockopt(client, SOL_SOCKET, SO_SNDTIMEO, reinterpret_cast<char const*>(&timeoutMs), static_cast<int>(sizeof(timeoutMs)));
      HandleClient(client);
      closesocket(client);
    }
  }

  void HandleClient(SOCKET client) {
    auto started = GetTickCount64();
    std::string request;
    std::array<char, 4096> buffer{};
    while (request.find("\r\n\r\n") == std::string::npos && request.size() < 16384) {
      int count = recv(client, buffer.data(), static_cast<int>(buffer.size()), 0);
      if (count <= 0) {
        return;
      }
      request.append(buffer.data(), count);
    }

    std::istringstream firstLine(request);
    std::string method;
    std::string path;
    std::string version;
    firstLine >> method >> path >> version;
    auto methodWide = Utf8ToWide(method);
    auto pathWide = Utf8ToWide(path);

    if (HeaderValue(request, "X-Platform-Control-Token") != WideToUtf8(token)) {
      auto body = ControlErrorJson(L"control_auth_required", L"Missing or invalid control token");
      SendJson(client, 401, body);
      Audit(
          L"control.auth",
          methodWide,
          pathWide,
          L"rejected",
          L"control_auth_required",
          L"",
          Utf8ToWide(body),
          GetTickCount64() - started);
      return;
    }

    if (method != "GET" || path != "/health") {
      auto code = method == "GET" ? L"not_found" : L"method_not_allowed";
      auto body = ControlErrorJson(code, method == "GET" ? L"Control route was not found" : L"Only GET /health is supported");
      SendJson(client, method == "GET" ? 404 : 405, body);
      Audit(
          L"control.route",
          methodWide,
          pathWide,
          L"rejected",
          code,
          L"",
          Utf8ToWide(body),
          GetTickCount64() - started);
      return;
    }

    auto body = HealthJson(port);
    SendJson(client, 200, body);
    Audit(L"platform.health", methodWide, pathWide, L"accepted", L"", Utf8ToWide(body), L"", GetTickCount64() - started);
  }
};

DevControlPlane::DevControlPlane() : impl_(std::make_unique<Impl>()) {}

DevControlPlane::~DevControlPlane() {
  Stop();
}

bool DevControlPlane::Start(DevControlPlaneConfig const& config, std::wstring* error) {
  return impl_->Start(config, error);
}

void DevControlPlane::Stop() {
  if (impl_ != nullptr) {
    impl_->Stop();
  }
}

uint16_t DevControlPlane::Port() const {
  return impl_ == nullptr ? 0 : impl_->port;
}

std::filesystem::path DevControlPlane::TokenPath() const {
  return impl_ == nullptr ? std::filesystem::path() : impl_->tokenPath;
}

}  // namespace nativeai
