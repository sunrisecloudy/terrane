#define WIN32_LEAN_AND_MEAN

#include <winsock2.h>
#include <ws2tcpip.h>
#include <Windows.h>
#include <bcrypt.h>
#include <ShlObj.h>

#include "DevControlPlane.h"

#include "BridgeTypes.h"
#include "PlatformDatabase.h"
#include "WebViewHost.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <cctype>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <optional>
#include <sstream>
#include <string>
#include <thread>
#include <vector>
#include <winrt/base.h>
#include <winrt/Windows.Data.Json.h>

namespace nativeai {
namespace json = winrt::Windows::Data::Json;
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

std::wstring ColumnText(sqlite3_stmt* statement, int index) {
  auto text = reinterpret_cast<char const*>(sqlite3_column_text(statement, index));
  return text == nullptr ? L"" : Utf8ToWide(text);
}

std::wstring SqliteValueJson(sqlite3_stmt* statement, int index) {
  switch (sqlite3_column_type(statement, index)) {
    case SQLITE_NULL:
      return L"null";
    case SQLITE_INTEGER:
      return std::to_wstring(sqlite3_column_int64(statement, index));
    case SQLITE_FLOAT:
      return std::to_wstring(sqlite3_column_double(statement, index));
    case SQLITE_TEXT:
      return JsonString(ColumnText(statement, index));
    case SQLITE_BLOB:
      return JsonString(L"<blob>");
    default:
      return L"null";
  }
}

std::wstring NowIso() {
  SYSTEMTIME time{};
  GetSystemTime(&time);
  wchar_t buffer[32]{};
  swprintf_s(
      buffer,
      L"%04u-%02u-%02uT%02u:%02u:%02uZ",
      time.wYear,
      time.wMonth,
      time.wDay,
      time.wHour,
      time.wMinute,
      time.wSecond);
  return buffer;
}

std::wstring JsonNullableString(std::wstring const& value) {
  return value.empty() ? L"null" : JsonString(value);
}

bool IsValidAppId(std::wstring const& appId) {
  if (appId.size() < 3 || appId.size() > 64 || appId.front() < L'a' || appId.front() > L'z') {
    return false;
  }
  for (wchar_t ch : appId) {
    if ((ch >= L'a' && ch <= L'z') || (ch >= L'0' && ch <= L'9') || ch == L'-') {
      continue;
    }
    return false;
  }
  return true;
}

bool ParseJsonObject(std::string const& body, json::JsonObject* object, std::wstring* error) {
  auto text = body.empty() ? L"{}" : Utf8ToWide(body);
  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(text, parsed)) {
    if (error != nullptr) {
      *error = L"Control request body must be a JSON object";
    }
    return false;
  }
  *object = parsed;
  return true;
}

std::optional<std::wstring> OptionalStringMember(json::JsonObject const& object, std::wstring const& key) {
  if (!object.HasKey(key)) {
    return std::nullopt;
  }
  auto value = object.GetNamedValue(key);
  if (value.ValueType() == json::JsonValueType::Null) {
    return std::wstring();
  }
  if (value.ValueType() != json::JsonValueType::String) {
    return std::nullopt;
  }
  return std::wstring(value.GetString().c_str());
}

std::wstring StringMemberOr(json::JsonObject const& object, std::wstring const& key, std::wstring const& fallback) {
  auto value = OptionalStringMember(object, key);
  return value.has_value() && !value->empty() ? value.value() : fallback;
}

std::optional<json::JsonObject> OptionalObjectMember(json::JsonObject const& object, std::wstring const& key) {
  if (!object.HasKey(key)) {
    return std::nullopt;
  }
  auto value = object.GetNamedValue(key);
  if (value.ValueType() != json::JsonValueType::Object) {
    return std::nullopt;
  }
  return value.GetObject();
}

std::wstring ObjectMemberJsonOr(json::JsonObject const& object, std::wstring const& key, std::wstring const& fallback) {
  auto member = OptionalObjectMember(object, key);
  return member.has_value() ? std::wstring(member->Stringify().c_str()) : fallback;
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
    case 400:
      return "Bad Request";
    case 401:
      return "Unauthorized";
    case 404:
      return "Not Found";
    case 405:
      return "Method Not Allowed";
    case 500:
      return "Internal Server Error";
    case 503:
      return "Service Unavailable";
    default:
      return "Error";
  }
}

std::string ControlErrorJson(std::wstring const& code, std::wstring const& message) {
  auto body = L"{\"ok\":false,\"error\":{\"code\":" + JsonString(code) +
      L",\"message\":" + JsonString(message) + L",\"details\":{}}}";
  return WideToUtf8(body);
}

std::wstring ControlOkJson(std::wstring const& resultJson) {
  return L"{\"ok\":true,\"result\":" + (resultJson.empty() ? L"{}" : resultJson) + L"}";
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
  static std::atomic_uint64_t sequence{0};
  return prefix + L"-" + std::to_wstring(GetCurrentProcessId()) + L"-" +
      std::to_wstring(GetTickCount64()) + L"-" + std::to_wstring(sequence.fetch_add(1));
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

bool TryParseContentLength(std::string const& value, size_t* length) {
  if (value.empty()) {
    *length = 0;
    return true;
  }
  size_t parsed = 0;
  for (char ch : value) {
    if (ch < '0' || ch > '9') {
      return false;
    }
    parsed = (parsed * 10) + static_cast<size_t>(ch - '0');
    if (parsed > 1024 * 1024) {
      return false;
    }
  }
  *length = parsed;
  return true;
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
  std::atomic<WebViewHost*> host{nullptr};
  std::atomic_bool readyAnnounced{false};

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
    AnnounceReadyIfPossible();
    return true;
#endif
  }

  void SetHost(WebViewHost* nextHost) {
    host.store(nextHost);
    AnnounceReadyIfPossible();
  }

  void AnnounceReadyIfPossible() {
    if (host.load() == nullptr || readyAnnounced.exchange(true)) {
      return;
    }
    WriteControlLine(L"NATIVE_AI_WINDOWS_CONTROL_READY port=" + std::to_wstring(port) + L" token_path=" + tokenPath.wstring());
  }

  void Stop() {
    stopping.store(true);
    host.store(nullptr);
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

  std::wstring ActiveInstallId(sqlite3* db, std::wstring const& appId) {
    if (appId.empty()) {
      return L"";
    }
    sqlite3_stmt* statement = nullptr;
    std::wstring installId;
    if (sqlite3_prepare_v2(db, "SELECT active_install_id FROM apps WHERE id = ?", -1, &statement, nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        installId = ColumnText(statement, 0);
      }
    }
    sqlite3_finalize(statement);
    return installId;
  }

  std::wstring RuntimeCapabilitiesJson(std::wstring const& appId) {
    return L"{\"runtimeVersion\":\"0.1.0\",\"platform\":\"windows\",\"target\":\"windows\",\"appId\":" +
        JsonNullableString(appId) +
        L",\"controlPlane\":{\"port\":" + std::to_wstring(port) +
        L",\"debug\":true,\"routes\":[\"GET /health\",\"POST /sessions\",\"DELETE /sessions/:id\","
        L"\"GET /sessions/:id/snapshot\",\"GET /sessions/:id/events\",\"GET /sessions/:id/capabilities\","
        L"\"POST /sessions/:id/command\"]}" +
        L",\"devMode\":true,\"features\":{\"storage.read\":true,\"storage.write\":true,"
        L"\"storage.get\":true,\"storage.set\":true,\"storage.remove\":true,\"storage.list\":true,"
        L"\"network.request\":true,\"dialog.openFile\":true,\"dialog.saveFile\":true,"
        L"\"notification.toast\":true,\"app.log\":true,\"runtime.capabilities\":true,"
        L"\"core.step\":true}}";
  }

  int64_t CountTableForApp(sqlite3* db, std::wstring const& table, std::wstring const& appId) {
    char const* sql = nullptr;
    if (table == L"bridge_calls") {
      sql = appId.empty() ? "SELECT COUNT(*) FROM bridge_calls" : "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ?";
    } else if (table == L"core_events") {
      sql = appId.empty() ? "SELECT COUNT(*) FROM core_events" : "SELECT COUNT(*) FROM core_events WHERE app_id = ?";
    } else if (table == L"app_storage") {
      sql = appId.empty() ? "SELECT COUNT(*) FROM app_storage" : "SELECT COUNT(*) FROM app_storage WHERE app_id = ?";
    } else {
      return 0;
    }
    sqlite3_stmt* statement = nullptr;
    int64_t count = 0;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK) {
      if (!appId.empty()) {
        BindText(statement, 1, appId);
      }
      if (sqlite3_step(statement) == SQLITE_ROW) {
        count = sqlite3_column_int64(statement, 0);
      }
    }
    sqlite3_finalize(statement);
    return count;
  }

  struct ControlSessionRecord {
    std::wstring controlSessionId;
    std::wstring runtimeSessionId;
    std::wstring target;
    std::wstring appId;
    std::wstring status;
    std::wstring startedAt;
    std::wstring endedAt;
  };

  bool LoadControlSession(sqlite3* db, std::wstring const& sessionId, ControlSessionRecord* record) {
    sqlite3_stmt* statement = nullptr;
    bool found = false;
    if (sqlite3_prepare_v2(
            db,
            "SELECT c.control_session_id, c.runtime_session_id, c.target, c.status, c.started_at, c.ended_at, r.active_app_id "
            "FROM control_sessions c LEFT JOIN runtime_sessions r ON r.session_id = c.runtime_session_id "
            "WHERE c.control_session_id = ?",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, sessionId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        record->controlSessionId = ColumnText(statement, 0);
        record->runtimeSessionId = ColumnText(statement, 1);
        record->target = ColumnText(statement, 2);
        record->status = ColumnText(statement, 3);
        record->startedAt = ColumnText(statement, 4);
        record->endedAt = ColumnText(statement, 5);
        record->appId = ColumnText(statement, 6);
        found = true;
      }
    }
    sqlite3_finalize(statement);
    return found;
  }

  std::wstring CreateControlSession(json::JsonObject const& body, std::wstring* error) {
    std::wstring appId;
    if (body.HasKey(L"appId")) {
      auto value = body.GetNamedValue(L"appId");
      if (value.ValueType() != json::JsonValueType::String) {
        *error = L"Control session appId must be a string";
        return L"";
      }
      appId = value.GetString().c_str();
      if (!appId.empty() && !IsValidAppId(appId)) {
        *error = L"Control session appId is not a valid generated app id";
        return L"";
      }
    }
    auto actor = StringMemberOr(body, L"actor", L"codex");
    auto target = StringMemberOr(body, L"target", L"windows");
    auto metadataJson = ObjectMemberJsonOr(body, L"metadata", L"{}");
    auto childControlSessionId = MakeId(L"control");
    auto runtimeSessionId = appId.empty() ? L"" : MakeId(L"session");
    auto startedAt = NowIso();

    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    auto installId = ActiveInstallId(db, appId);
    auto capabilities = RuntimeCapabilitiesJson(appId);
    auto resourceUsage = L"{\"appId\":" + JsonNullableString(appId) + L",\"bridgeCalls\":0,\"coreEvents\":0}";
    auto runtimeMetadata = L"{\"controlSessionId\":" + JsonString(childControlSessionId) + L",\"source\":\"windows-dev-control\"}";

    char* sqlError = nullptr;
    if (sqlite3_exec(db, "BEGIN IMMEDIATE", nullptr, nullptr, &sqlError) != SQLITE_OK) {
      *error = L"Could not begin control session transaction";
      sqlite3_free(sqlError);
      return L"";
    }

    bool ok = true;
    sqlite3_stmt* statement = nullptr;
    if (!runtimeSessionId.empty()) {
      ok = sqlite3_prepare_v2(
               db,
               "INSERT INTO runtime_sessions "
               "(session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, resource_high_water_json, metadata_json) "
               "VALUES (?, 'windows', 'windows', '0.1.0', ?, ?, ?, 'running', ?, ?, ?)",
               -1,
               &statement,
               nullptr) == SQLITE_OK;
      if (ok) {
        BindText(statement, 1, runtimeSessionId);
        BindText(statement, 2, appId);
        if (installId.empty()) {
          sqlite3_bind_null(statement, 3);
        } else {
          BindText(statement, 3, installId);
        }
        BindText(statement, 4, startedAt);
        BindText(statement, 5, capabilities);
        BindText(statement, 6, resourceUsage);
        BindText(statement, 7, runtimeMetadata);
        ok = sqlite3_step(statement) == SQLITE_DONE;
      }
      sqlite3_finalize(statement);
      statement = nullptr;
    }

    ok = ok &&
        sqlite3_prepare_v2(
             db,
             "INSERT INTO control_sessions "
             "(control_session_id, target, runtime_session_id, actor, token_hash, started_at, status, metadata_json) "
             "VALUES (?, ?, ?, ?, ?, ?, 'running', ?)",
             -1,
             &statement,
             nullptr) == SQLITE_OK;
    if (ok) {
      BindText(statement, 1, childControlSessionId);
      BindText(statement, 2, target);
      if (runtimeSessionId.empty()) {
        sqlite3_bind_null(statement, 3);
      } else {
        BindText(statement, 3, runtimeSessionId);
      }
      BindText(statement, 4, actor);
      BindText(statement, 5, tokenHash);
      BindText(statement, 6, startedAt);
      BindText(statement, 7, metadataJson);
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);

    if (!ok) {
      sqlite3_exec(db, "ROLLBACK", nullptr, nullptr, nullptr);
      *error = L"Could not create control session";
      return L"";
    }
    sqlite3_exec(db, "COMMIT", nullptr, nullptr, nullptr);

    return L"{\"controlSessionId\":" + JsonString(childControlSessionId) +
        L",\"runtimeSessionId\":" + JsonNullableString(runtimeSessionId) +
        L",\"target\":" + JsonString(target) +
        L",\"appId\":" + JsonNullableString(appId) +
        L",\"status\":\"running\",\"startedAt\":" + JsonString(startedAt) + L"}";
  }

  std::wstring EndControlSession(std::wstring const& childControlSessionId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    ControlSessionRecord record;
    if (!LoadControlSession(db, childControlSessionId, &record)) {
      *error = L"Control session not found";
      return L"";
    }
    auto endedAt = NowIso();
    sqlite3_stmt* statement = nullptr;
    bool ok = sqlite3_prepare_v2(db, "UPDATE control_sessions SET status = 'ended', ended_at = ? WHERE control_session_id = ?", -1, &statement, nullptr) == SQLITE_OK;
    if (ok) {
      BindText(statement, 1, endedAt);
      BindText(statement, 2, childControlSessionId);
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
    if (!record.runtimeSessionId.empty()) {
      statement = nullptr;
      if (sqlite3_prepare_v2(db, "UPDATE runtime_sessions SET status = 'ended', ended_at = ? WHERE session_id = ?", -1, &statement, nullptr) == SQLITE_OK) {
        BindText(statement, 1, endedAt);
        BindText(statement, 2, record.runtimeSessionId);
        sqlite3_step(statement);
      }
      sqlite3_finalize(statement);
    }
    if (!ok) {
      *error = L"Could not end control session";
      return L"";
    }
    return L"{\"ok\":true,\"controlSessionId\":" + JsonString(childControlSessionId) +
        L",\"runtimeSessionId\":" + JsonNullableString(record.runtimeSessionId) +
        L",\"status\":\"ended\",\"endedAt\":" + JsonString(endedAt) + L"}";
  }

  std::wstring BridgeCallRowsJson(sqlite3* db, std::wstring const& appId) {
    char const* sql = appId.empty()
        ? "SELECT bridge_call_id, session_id, app_id, method, created_at FROM bridge_calls ORDER BY created_at"
        : "SELECT bridge_call_id, session_id, app_id, method, created_at FROM bridge_calls WHERE app_id = ? ORDER BY created_at";
    sqlite3_stmt* statement = nullptr;
    std::wstring rows = L"[";
    bool first = true;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK) {
      if (!appId.empty()) {
        BindText(statement, 1, appId);
      }
      while (sqlite3_step(statement) == SQLITE_ROW) {
        if (!first) {
          rows += L",";
        }
        first = false;
        rows += L"{\"bridgeCallId\":" + JsonString(ColumnText(statement, 0)) +
            L",\"sessionId\":" + JsonNullableString(ColumnText(statement, 1)) +
            L",\"appId\":" + JsonNullableString(ColumnText(statement, 2)) +
            L",\"method\":" + JsonString(ColumnText(statement, 3)) +
            L",\"createdAt\":" + JsonString(ColumnText(statement, 4)) + L"}";
      }
    }
    sqlite3_finalize(statement);
    rows += L"]";
    return rows;
  }

  std::wstring CoreEventRowsJson(sqlite3* db, std::wstring const& appId) {
    char const* sql = appId.empty()
        ? "SELECT event_id, session_id, app_id, state_version_before, created_at FROM core_events ORDER BY created_at"
        : "SELECT event_id, session_id, app_id, state_version_before, created_at FROM core_events WHERE app_id = ? ORDER BY created_at";
    sqlite3_stmt* statement = nullptr;
    std::wstring rows = L"[";
    bool first = true;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK) {
      if (!appId.empty()) {
        BindText(statement, 1, appId);
      }
      while (sqlite3_step(statement) == SQLITE_ROW) {
        if (!first) {
          rows += L",";
        }
        first = false;
        rows += L"{\"eventId\":" + JsonString(ColumnText(statement, 0)) +
            L",\"sessionId\":" + JsonNullableString(ColumnText(statement, 1)) +
            L",\"appId\":" + JsonNullableString(ColumnText(statement, 2)) +
            L",\"stateVersionBefore\":";
        if (sqlite3_column_type(statement, 3) == SQLITE_NULL) {
          rows += L"null";
        } else {
          rows += std::to_wstring(sqlite3_column_int64(statement, 3));
        }
        rows += L",\"createdAt\":" + JsonString(ColumnText(statement, 4)) + L"}";
      }
    }
    sqlite3_finalize(statement);
    rows += L"]";
    return rows;
  }

  std::wstring SafeTableRowsJson(
      sqlite3* db,
      char const* table,
      std::vector<char const*> const& columns,
      char const* orderBy,
      char const* filterColumn = nullptr,
      std::wstring const& filterValue = L"") {
    if (columns.empty()) {
      return L"[]";
    }
    std::string sql = "SELECT ";
    for (size_t index = 0; index < columns.size(); ++index) {
      if (index > 0) {
        sql += ", ";
      }
      sql += columns[index];
    }
    sql += " FROM ";
    sql += table;
    bool hasFilter = filterColumn != nullptr && !filterValue.empty();
    if (hasFilter) {
      sql += " WHERE ";
      sql += filterColumn;
      sql += " = ?";
    }
    sql += " ORDER BY ";
    sql += orderBy;
    sql += " LIMIT 100";

    sqlite3_stmt* statement = nullptr;
    std::wstring rows = L"[";
    bool first = true;
    if (sqlite3_prepare_v2(db, sql.c_str(), -1, &statement, nullptr) == SQLITE_OK) {
      if (hasFilter) {
        BindText(statement, 1, filterValue);
      }
      while (sqlite3_step(statement) == SQLITE_ROW) {
        if (!first) {
          rows += L",";
        }
        first = false;
        rows += L"{";
        for (size_t index = 0; index < columns.size(); ++index) {
          if (index > 0) {
            rows += L",";
          }
          rows += JsonString(Utf8ToWide(columns[index])) + L":" + SqliteValueJson(statement, static_cast<int>(index));
        }
        rows += L"}";
      }
    }
    sqlite3_finalize(statement);
    rows += L"]";
    return rows;
  }

  std::wstring DbSnapshotJson(std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    return L"{\"apps\":" +
        SafeTableRowsJson(db, "apps", {"id", "name", "status", "active_install_id", "active_version", "data_version", "created_at", "updated_at"}, "id") +
        L",\"app_versions\":" +
        SafeTableRowsJson(db, "app_versions", {"install_id", "app_id", "version", "runtime_version", "data_version", "content_hash", "status", "created_at", "activated_at"}, "created_at") +
        L",\"app_storage\":" +
        SafeTableRowsJson(db, "app_storage", {"app_id", "key", "value_json", "updated_at"}, "updated_at") +
        L",\"bridge_calls\":" +
        SafeTableRowsJson(db, "bridge_calls", {"bridge_call_id", "session_id", "app_id", "install_id", "method", "result_json", "error_json", "duration_ms", "created_at"}, "created_at") +
        L",\"core_events\":" +
        SafeTableRowsJson(db, "core_events", {"event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"}, "created_at") +
        L",\"test_runs\":" +
        SafeTableRowsJson(db, "test_runs", {"test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at"}, "started_at") +
        L",\"control_sessions\":" +
        SafeTableRowsJson(db, "control_sessions", {"control_session_id", "target", "runtime_session_id", "actor", "started_at", "ended_at", "status", "metadata_json"}, "started_at") +
        L",\"control_commands\":" +
        SafeTableRowsJson(db, "control_commands", {"command_id", "control_session_id", "runtime_session_id", "tool", "http_method", "path", "decision", "error_code", "created_at", "duration_ms"}, "created_at") +
        L",\"runtime_sessions\":" +
        SafeTableRowsJson(db, "runtime_sessions", {"session_id", "target", "platform", "runtime_version", "active_app_id", "active_install_id", "started_at", "ended_at", "status"}, "started_at") +
        L",\"runtime_snapshots\":" +
        SafeTableRowsJson(db, "runtime_snapshots", {"snapshot_id", "session_id", "app_id", "install_id", "type", "content_hash", "created_at"}, "created_at") +
        L",\"backup_exports\":" +
        SafeTableRowsJson(db, "backup_exports", {"export_id", "type", "source_platform", "runtime_version", "content_hash", "created_at", "imported_at"}, "created_at") +
        L"}";
  }

  std::wstring DbQueryRowsJson(std::wstring const& tool, std::wstring const& appId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    std::wstring rows;
    if (tool == L"db.query_app_storage") {
      rows = SafeTableRowsJson(db, "app_storage", {"app_id", "key", "value_json", "updated_at"}, "updated_at", "app_id", appId);
    } else if (tool == L"db.query_app_versions") {
      rows = SafeTableRowsJson(db, "app_versions", {"install_id", "app_id", "version", "runtime_version", "data_version", "content_hash", "status", "created_at", "activated_at"}, "created_at", "app_id", appId);
    } else if (tool == L"db.query_bridge_calls") {
      rows = SafeTableRowsJson(db, "bridge_calls", {"bridge_call_id", "session_id", "app_id", "install_id", "method", "result_json", "error_json", "duration_ms", "created_at"}, "created_at", "app_id", appId);
    } else if (tool == L"db.query_core_events") {
      rows = SafeTableRowsJson(db, "core_events", {"event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"}, "created_at", "app_id", appId);
    } else if (tool == L"db.query_test_runs") {
      rows = SafeTableRowsJson(db, "test_runs", {"test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at"}, "started_at", "app_id", appId);
    } else {
      *error = L"Unsupported DB inspection command";
      return L"";
    }
    return L"{\"rows\":" + rows + L"}";
  }

  std::wstring SessionSnapshotJson(std::wstring const& childControlSessionId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    ControlSessionRecord record;
    if (!LoadControlSession(db, childControlSessionId, &record)) {
      *error = L"Control session not found";
      return L"";
    }
    auto capabilities = RuntimeCapabilitiesJson(record.appId);
    auto resourceUsage = L"{\"appId\":" + JsonNullableString(record.appId) +
        L",\"bridgeCalls\":" + std::to_wstring(CountTableForApp(db, L"bridge_calls", record.appId)) +
        L",\"coreEvents\":" + std::to_wstring(CountTableForApp(db, L"core_events", record.appId)) +
        L",\"storageKeys\":" + std::to_wstring(CountTableForApp(db, L"app_storage", record.appId)) + L"}";
    return L"{\"controlSessionId\":" + JsonString(record.controlSessionId) +
        L",\"snapshot\":{\"target\":\"windows\",\"appId\":" + JsonNullableString(record.appId) +
        L",\"runtimeSessionId\":" + JsonNullableString(record.runtimeSessionId) +
        L",\"status\":" + JsonString(record.status) +
        L",\"title\":" + JsonString(record.appId.empty() ? L"Windows Native Runtime" : record.appId) +
        L",\"testIds\":[],\"resourceUsage\":" + resourceUsage +
        L",\"capabilities\":" + capabilities + L"}}";
  }

  std::wstring SessionEventsJson(std::wstring const& childControlSessionId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    ControlSessionRecord record;
    if (!LoadControlSession(db, childControlSessionId, &record)) {
      *error = L"Control session not found";
      return L"";
    }
    return L"{\"controlSessionId\":" + JsonString(record.controlSessionId) +
        L",\"runtimeSessionId\":" + JsonNullableString(record.runtimeSessionId) +
        L",\"appId\":" + JsonNullableString(record.appId) +
        L",\"bridgeCalls\":" + BridgeCallRowsJson(db, record.appId) +
        L",\"coreEvents\":" + CoreEventRowsJson(db, record.appId) + L"}";
  }

  std::wstring SessionCapabilitiesJson(std::wstring const& childControlSessionId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    ControlSessionRecord record;
    if (!LoadControlSession(db, childControlSessionId, &record)) {
      *error = L"Control session not found";
      return L"";
    }
    return RuntimeCapabilitiesJson(record.appId);
  }

  bool ControlSessionAllowsApp(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring* errorCode,
      std::wstring* errorMessage,
      int* status) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not open platform database";
      *status = 500;
      return false;
    }
    ControlSessionRecord record;
    if (!LoadControlSession(db, childControlSessionId, &record)) {
      *errorCode = L"not_found";
      *errorMessage = L"Control session not found";
      *status = 400;
      return false;
    }
    if (record.status != L"running") {
      *errorCode = L"invalid_request";
      *errorMessage = L"Control session is not running";
      *status = 400;
      return false;
    }
    if (!appId.empty() && !record.appId.empty() && record.appId != appId) {
      *errorCode = L"permission_denied";
      *errorMessage = L"Control command appId does not match the control session app";
      *status = 400;
      return false;
    }
    return true;
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
    AuditForSession(L"", tool, method, path, decision, errorCode, resultJson, errorJson, durationMs);
  }

  void AuditForSession(
      std::wstring const& auditSessionId,
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
      BindText(statement, 2, auditSessionId.empty() ? controlSessionId : auditSessionId);
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

  void SendControlRouteError(
      SOCKET client,
      std::wstring const& auditSessionId,
      std::wstring const& tool,
      std::wstring const& method,
      std::wstring const& path,
      uint64_t started,
      std::wstring const& code,
      std::wstring const& message,
      int status) {
    auto body = ControlErrorJson(code, message);
    SendJson(client, status, body);
    AuditForSession(auditSessionId, tool, method, path, L"rejected", code, L"", Utf8ToWide(body), GetTickCount64() - started);
  }

  void SendControlRouteResult(
      SOCKET client,
      std::wstring const& auditSessionId,
      std::wstring const& tool,
      std::wstring const& method,
      std::wstring const& path,
      uint64_t started,
      std::wstring const& resultJson) {
    auto body = ControlOkJson(resultJson);
    SendJson(client, 200, WideToUtf8(body));
    AuditForSession(auditSessionId, tool, method, path, L"accepted", L"", resultJson, L"", GetTickCount64() - started);
  }

  bool IsSessionsCollectionPath(std::string const& path) const {
    return path == "/sessions" || path == "/control/sessions";
  }

  bool IsSessionsRoutePath(std::string const& path) const {
    return IsSessionsCollectionPath(path) ||
        path.rfind("/sessions/", 0) == 0 ||
        path.rfind("/control/sessions/", 0) == 0;
  }

  std::optional<std::wstring> SessionIdFromPath(std::string const& path, std::string const& suffix) const {
    std::string normalized = path.rfind("/control/sessions/", 0) == 0 ? path.substr(std::string("/control").size()) : path;
    if (normalized.rfind("/sessions/", 0) != 0) {
      return std::nullopt;
    }
    auto start = std::string("/sessions/").size();
    if (suffix.empty()) {
      if (normalized.find('/', start) != std::string::npos || normalized.size() == start) {
        return std::nullopt;
      }
      return Utf8ToWide(normalized.substr(start));
    }
    if (normalized.size() <= suffix.size() + start || normalized.substr(normalized.size() - suffix.size()) != suffix) {
      return std::nullopt;
    }
    auto end = normalized.size() - suffix.size();
    if (end <= start || normalized[end] != '/') {
      return std::nullopt;
    }
    return Utf8ToWide(normalized.substr(start, end - start));
  }

  std::wstring BridgeRequestJson(std::wstring const& requestId, std::wstring const& method, std::wstring const& paramsJson) {
    return L"{\"id\":" + JsonString(requestId) +
        L",\"method\":" + JsonString(method) +
        L",\"params\":" + (paramsJson.empty() ? L"{}" : paramsJson) + L"}";
  }

  void SessionCreateHandler(SOCKET client, std::wstring const& method, std::wstring const& path, std::string const& body, uint64_t started) {
    if (method != L"POST") {
      SendControlRouteError(client, L"", L"control.sessions.create", method, path, started, L"not_found", L"Control session route was not found", 404);
      return;
    }
    json::JsonObject object{nullptr};
    std::wstring error;
    if (!ParseJsonObject(body, &object, &error)) {
      SendControlRouteError(client, L"", L"control.sessions.create", method, path, started, L"invalid_request", error, 400);
      return;
    }
    auto result = CreateControlSession(object, &error);
    if (result.empty()) {
      SendControlRouteError(client, L"", L"control.sessions.create", method, path, started, L"invalid_request", error.empty() ? L"Could not create control session" : error, 400);
      return;
    }
    SendControlRouteResult(client, L"", L"control.sessions.create", method, path, started, result);
  }

  void SessionItemHandler(SOCKET client, std::wstring const& method, std::wstring const& path, uint64_t started, std::wstring const& sessionId) {
    if (method != L"DELETE") {
      SendControlRouteError(client, sessionId, L"control.sessions.end", method, path, started, L"not_found", L"Control session route was not found", 404);
      return;
    }
    std::wstring error;
    auto result = EndControlSession(sessionId, &error);
    if (result.empty()) {
      SendControlRouteError(client, L"", L"control.sessions.end", method, path, started, L"not_found", error.empty() ? L"Control session not found" : error, 400);
      return;
    }
    SendControlRouteResult(client, sessionId, L"control.sessions.end", method, path, started, result);
  }

  void SessionReadHandler(
      SOCKET client,
      std::wstring const& method,
      std::wstring const& path,
      uint64_t started,
      std::wstring const& sessionId,
      std::wstring const& tool,
      std::wstring (Impl::*reader)(std::wstring const&, std::wstring*)) {
    if (method != L"GET") {
      SendControlRouteError(client, sessionId, tool, method, path, started, L"not_found", L"Control session route was not found", 404);
      return;
    }
    std::wstring error;
    auto result = (this->*reader)(sessionId, &error);
    if (result.empty()) {
      SendControlRouteError(client, L"", tool, method, path, started, L"not_found", error.empty() ? L"Control session not found" : error, 400);
      return;
    }
    SendControlRouteResult(client, sessionId, tool, method, path, started, result);
  }

  void SessionCommandHandler(SOCKET client, std::wstring const& method, std::wstring const& path, std::string const& body, uint64_t started, std::wstring const& sessionId) {
    if (method != L"POST") {
      SendControlRouteError(client, sessionId, L"control.sessions.command", method, path, started, L"not_found", L"Control session command route was not found", 404);
      return;
    }
    json::JsonObject command{nullptr};
    std::wstring error;
    if (!ParseJsonObject(body, &command, &error)) {
      SendControlRouteError(client, sessionId, L"control.sessions.command", method, path, started, L"invalid_request", error, 400);
      return;
    }
    auto toolValue = OptionalStringMember(command, L"tool");
    if (!toolValue.has_value() || toolValue->empty()) {
      SendControlRouteError(client, sessionId, L"control.sessions.command", method, path, started, L"invalid_request", L"Control command requires tool", 400);
      return;
    }
    auto tool = toolValue.value();
    std::wstring result;
    if (tool == L"platform.health") {
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = Utf8ToWide(HealthJson(port));
    } else if (tool == L"runtime.capabilities") {
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = SessionCapabilitiesJson(sessionId, &error);
      if (result.empty()) {
        SendControlRouteError(client, L"", tool, method, path, started, L"not_found", error.empty() ? L"Control session not found" : error, 400);
        return;
      }
    } else if (tool == L"db.snapshot" ||
        tool == L"db.query_app_storage" ||
        tool == L"db.query_app_versions" ||
        tool == L"db.query_bridge_calls" ||
        tool == L"db.query_core_events" ||
        tool == L"db.query_test_runs") {
      std::wstring appId;
      if (command.HasKey(L"args")) {
        auto args = OptionalObjectMember(command, L"args");
        if (!args.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
          return;
        }
        if (args->HasKey(L"appId")) {
          auto appIdValue = OptionalStringMember(args.value(), L"appId");
          if (!appIdValue.has_value()) {
            SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" appId must be a string", 400);
            return;
          }
          appId = appIdValue.value();
          if (!appId.empty() && !IsValidAppId(appId)) {
            SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" appId is not a valid generated app id", 400);
            return;
          }
        }
      }
      if ((tool == L"db.query_app_storage" || tool == L"db.query_app_versions") && appId.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires appId", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = tool == L"db.snapshot" ? DbSnapshotJson(&error) : DbQueryRowsJson(tool, appId, &error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Could not read platform database" : error, 500);
        return;
      }
    } else if (tool == L"runtime.call_bridge" || tool == L"runtime.core_step") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
        return;
      }
      auto appId = OptionalStringMember(args.value(), L"appId");
      if (!appId.has_value() || appId->empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires appId", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      auto currentHost = host.load();
      if (currentHost == nullptr) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"platform_unsupported", L"Windows dev control bridge is not available", 503);
        return;
      }
      std::wstring requestId;
      std::wstring bridgeRequest;
      if (tool == L"runtime.call_bridge") {
        auto bridgeMethod = OptionalStringMember(args.value(), L"method");
        if (!bridgeMethod.has_value() || bridgeMethod->empty()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.call_bridge requires appId and method", 400);
          return;
        }
        std::wstring paramsJson = L"{}";
        if (args->HasKey(L"params")) {
          auto params = OptionalObjectMember(args.value(), L"params");
          if (!params.has_value()) {
            SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.call_bridge params must be an object", 400);
            return;
          }
          paramsJson = params->Stringify().c_str();
        }
        requestId = StringMemberOr(args.value(), L"id", L"control_call_bridge");
        bridgeRequest = BridgeRequestJson(requestId, bridgeMethod.value(), paramsJson);
      } else {
        if (!args->HasKey(L"event")) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.core_step requires appId and event object", 400);
          return;
        }
        auto eventValue = args->GetNamedValue(L"event");
        if (eventValue.ValueType() != json::JsonValueType::Object) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.core_step requires appId and event object", 400);
          return;
        }
        requestId = StringMemberOr(args.value(), L"id", L"control_core_step");
        bridgeRequest = BridgeRequestJson(requestId, L"core.step", L"{\"event\":" + std::wstring(eventValue.Stringify().c_str()) + L"}");
      }
      result = currentHost->DevControlBridgeCall(appId.value(), sessionId, bridgeRequest);
    } else {
      SendControlRouteError(client, sessionId, tool, method, path, started, L"unsupported_tool", L"Windows dev control session command is not supported yet", 400);
      return;
    }
    SendControlRouteResult(client, sessionId, tool, method, path, started, result);
  }

  void ControlRouteHandler(SOCKET client, std::string const& method, std::string const& path, std::string const& body, uint64_t started) {
    auto methodWide = Utf8ToWide(method);
    auto pathWide = Utf8ToWide(path);
    if (IsSessionsCollectionPath(path)) {
      SessionCreateHandler(client, methodWide, pathWide, body, started);
      return;
    }
    if (auto sessionId = SessionIdFromPath(path, "/snapshot"); sessionId.has_value()) {
      SessionReadHandler(client, methodWide, pathWide, started, sessionId.value(), L"control.sessions.snapshot", &Impl::SessionSnapshotJson);
      return;
    }
    if (auto sessionId = SessionIdFromPath(path, "/events"); sessionId.has_value()) {
      SessionReadHandler(client, methodWide, pathWide, started, sessionId.value(), L"control.sessions.events", &Impl::SessionEventsJson);
      return;
    }
    if (auto sessionId = SessionIdFromPath(path, "/capabilities"); sessionId.has_value()) {
      SessionReadHandler(client, methodWide, pathWide, started, sessionId.value(), L"control.sessions.capabilities", &Impl::SessionCapabilitiesJson);
      return;
    }
    if (auto sessionId = SessionIdFromPath(path, "/command"); sessionId.has_value()) {
      SessionCommandHandler(client, methodWide, pathWide, body, started, sessionId.value());
      return;
    }
    if (auto sessionId = SessionIdFromPath(path, ""); sessionId.has_value()) {
      SessionItemHandler(client, methodWide, pathWide, started, sessionId.value());
      return;
    }
    auto bodyText = ControlErrorJson(L"not_found", L"Control route was not found");
    SendJson(client, 404, bodyText);
    Audit(L"control.route", methodWide, pathWide, L"rejected", L"not_found", L"", Utf8ToWide(bodyText), GetTickCount64() - started);
  }

  void AcceptLoop() {
    winrt::init_apartment(winrt::apartment_type::multi_threaded);
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
    auto headerEnd = request.find("\r\n\r\n");
    if (headerEnd == std::string::npos) {
      return;
    }
    std::istringstream firstLine(request);
    std::string method;
    std::string path;
    std::string version;
    firstLine >> method >> path >> version;
    auto methodWide = Utf8ToWide(method);
    auto pathWide = Utf8ToWide(path);

    size_t contentLength = 0;
    if (!TryParseContentLength(HeaderValue(request, "Content-Length"), &contentLength)) {
      auto body = ControlErrorJson(L"invalid_request", L"Content-Length must be a valid bounded integer");
      SendJson(client, 400, body);
      Audit(
          L"control.route",
          methodWide,
          pathWide,
          L"rejected",
          L"invalid_request",
          L"",
          Utf8ToWide(body),
          GetTickCount64() - started);
      return;
    }
    auto bodyStart = headerEnd + 4;
    while (request.size() < bodyStart + contentLength && request.size() < bodyStart + 1024 * 1024) {
      int count = recv(client, buffer.data(), static_cast<int>(buffer.size()), 0);
      if (count <= 0) {
        return;
      }
      request.append(buffer.data(), count);
    }
    auto requestBody = contentLength == 0 ? std::string() : request.substr(bodyStart, contentLength);

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

    if (path == "/health" && method != "GET") {
      auto body = ControlErrorJson(L"method_not_allowed", L"Only GET /health is supported");
      SendJson(client, 405, body);
      Audit(
          L"platform.health",
          methodWide,
          pathWide,
          L"rejected",
          L"method_not_allowed",
          L"",
          Utf8ToWide(body),
          GetTickCount64() - started);
      return;
    }

    if (path != "/health") {
      ControlRouteHandler(client, method, path, requestBody, started);
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

void DevControlPlane::SetHost(WebViewHost* host) {
  if (impl_ != nullptr) {
    impl_->SetHost(host);
  }
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
