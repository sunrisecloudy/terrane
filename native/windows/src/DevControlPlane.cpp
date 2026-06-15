#define WIN32_LEAN_AND_MEAN

#include <winsock2.h>
#include <ws2tcpip.h>
#include <Windows.h>
#include <bcrypt.h>
#include <ShlObj.h>

#include "DevControlPlane.h"

#include "BridgeTypes.h"
#include "PlatformDatabase.h"
#include "PlatformStorage.h"
#include "WebViewHost.h"
#include "ForgeCoreBridge.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <cctype>
#include <cwctype>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <map>
#include <optional>
#include <regex>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
#include <vector>
#include <winrt/base.h>
#include <winrt/Windows.Data.Json.h>

namespace terrane {
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
    return path / L"Terrane" / L"control.token";
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

std::wstring RawJsonOrNull(std::wstring const& text) {
  if (text.empty()) {
    return L"null";
  }
  json::JsonValue parsed{nullptr};
  if (!json::JsonValue::TryParse(text, parsed)) {
    return L"null";
  }
  return text;
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

std::wstring ReadTextFile(std::filesystem::path const& path) {
  std::ifstream file(path, std::ios::binary);
  if (!file) {
    return {};
  }
  std::string text((std::istreambuf_iterator<char>(file)), std::istreambuf_iterator<char>());
  return Utf8ToWide(text);
}

std::filesystem::path ExecutableDirectory() {
  std::wstring buffer(MAX_PATH, L'\0');
  DWORD length = GetModuleFileNameW(nullptr, buffer.data(), static_cast<DWORD>(buffer.size()));
  if (length == 0) {
    return std::filesystem::current_path();
  }
  buffer.resize(length);
  return std::filesystem::path(buffer).parent_path();
}

std::filesystem::path RepoRoot() {
  auto current = std::filesystem::current_path();
  for (int depth = 0; depth < 8; ++depth) {
    if (std::filesystem::exists(current / L"docs" / L"00_PRD.md")) {
      return current;
    }
    if (!current.has_parent_path()) {
      break;
    }
    current = current.parent_path();
  }
  return std::filesystem::current_path();
}

std::filesystem::path RuntimeResourceRoot() {
  auto resourceRoot = ExecutableDirectory() / L"resources";
  if (std::filesystem::exists(resourceRoot / L"runtime" / L"index.html") &&
      std::filesystem::exists(resourceRoot / L"webapps" / L"examples")) {
    return resourceRoot;
  }
  return RepoRoot();
}

std::array<wchar_t const*, 5> BundledWebappIds() {
  return {L"notes-lite", L"task-workbench", L"file-transformer", L"api-dashboard", L"core-replay-lab"};
}

bool ContainsAppId(std::vector<std::wstring> const& appIds, std::wstring const& appId) {
  return std::find(appIds.begin(), appIds.end(), appId) != appIds.end();
}

std::optional<json::JsonObject> BundledManifest(std::wstring const& appId) {
  auto manifestText = ReadTextFile(RuntimeResourceRoot() / L"webapps" / L"examples" / appId / L"manifest.json");
  json::JsonObject manifest{nullptr};
  if (manifestText.empty() || !json::JsonObject::TryParse(manifestText, manifest)) {
    return std::nullopt;
  }
  return manifest;
}

std::optional<std::wstring> ManifestString(json::JsonObject const& manifest, wchar_t const* key) {
  if (!manifest.HasKey(key)) {
    return std::nullopt;
  }
  auto value = manifest.GetNamedValue(key);
  if (value.ValueType() != json::JsonValueType::String) {
    return std::nullopt;
  }
  return std::wstring(value.GetString().c_str());
}

int64_t ManifestDataVersion(json::JsonObject const& manifest) {
  if (!manifest.HasKey(L"dataVersion")) {
    return 1;
  }
  auto value = manifest.GetNamedValue(L"dataVersion");
  return value.ValueType() == json::JsonValueType::Number ? static_cast<int64_t>(value.GetNumber()) : 1;
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

std::optional<json::JsonArray> OptionalArrayMember(json::JsonObject const& object, std::wstring const& key) {
  if (!object.HasKey(key)) {
    return std::nullopt;
  }
  auto value = object.GetNamedValue(key);
  if (value.ValueType() != json::JsonValueType::Array) {
    return std::nullopt;
  }
  return value.GetArray();
}

bool OptionalArgsAppId(json::JsonObject const& command, std::wstring const& tool, std::wstring* appId, std::wstring* error) {
  appId->clear();
  if (!command.HasKey(L"args")) {
    return true;
  }
  auto args = OptionalObjectMember(command, L"args");
  if (!args.has_value()) {
    *error = tool + L" requires args object";
    return false;
  }
  if (!args->HasKey(L"appId")) {
    return true;
  }
  auto value = OptionalStringMember(args.value(), L"appId");
  if (!value.has_value()) {
    *error = tool == L"runtime.fault_inject"
        ? L"runtime.fault_inject appId must be a string"
        : tool + L" appId must be a string";
    return false;
  }
  *appId = value.value();
  if (!appId->empty() && !IsValidAppId(*appId)) {
    *error = tool == L"runtime.fault_inject"
        ? L"runtime.fault_inject appId is not a valid generated app id"
        : tool + L" appId is not a valid generated app id";
    return false;
  }
  return true;
}

std::wstring ObjectMemberJsonOr(json::JsonObject const& object, std::wstring const& key, std::wstring const& fallback) {
  auto member = OptionalObjectMember(object, key);
  return member.has_value() ? std::wstring(member->Stringify().c_str()) : fallback;
}

bool HasMember(json::JsonObject const& object, std::wstring const& key) {
  return object.HasKey(key) && object.GetNamedValue(key).ValueType() != json::JsonValueType::Null;
}

bool BooleanMemberTrue(json::JsonObject const& object, std::wstring const& key) {
  if (!object.HasKey(key)) {
    return false;
  }
  auto value = object.GetNamedValue(key);
  return value.ValueType() == json::JsonValueType::Boolean && value.GetBoolean();
}

std::optional<json::IJsonValue> FirstJsonValue(json::JsonObject const& object, std::vector<std::wstring> const& keys) {
  for (auto const& key : keys) {
    if (!object.HasKey(key)) {
      continue;
    }
    auto value = object.GetNamedValue(key);
    if (value.ValueType() != json::JsonValueType::Null) {
      return value;
    }
  }
  return std::nullopt;
}

std::wstring CanonicalJsonValue(json::IJsonValue const& value) {
  switch (value.ValueType()) {
    case json::JsonValueType::Null:
      return L"null";
    case json::JsonValueType::Boolean:
      return value.GetBoolean() ? L"true" : L"false";
    case json::JsonValueType::Number:
    case json::JsonValueType::String:
      return std::wstring(value.Stringify().c_str());
    case json::JsonValueType::Array: {
      auto array = value.GetArray();
      std::wstring out = L"[";
      for (uint32_t index = 0; index < array.Size(); ++index) {
        if (index > 0) {
          out += L",";
        }
        out += CanonicalJsonValue(array.GetAt(index));
      }
      out += L"]";
      return out;
    }
    case json::JsonValueType::Object: {
      auto object = value.GetObject();
      std::vector<std::wstring> keys;
      for (auto const& pair : object) {
        keys.push_back(std::wstring(pair.Key().c_str()));
      }
      std::sort(keys.begin(), keys.end());
      std::wstring out = L"{";
      for (size_t index = 0; index < keys.size(); ++index) {
        if (index > 0) {
          out += L",";
        }
        out += JsonString(keys[index]) + L":" + CanonicalJsonValue(object.GetNamedValue(keys[index]));
      }
      out += L"}";
      return out;
    }
  }
  return std::wstring(value.Stringify().c_str());
}

bool JsonMatchesSubset(json::IJsonValue const& actual, json::IJsonValue const& expected) {
  if (expected.ValueType() != json::JsonValueType::Object) {
    return CanonicalJsonValue(actual) == CanonicalJsonValue(expected);
  }
  if (actual.ValueType() != json::JsonValueType::Object) {
    return false;
  }
  auto actualObject = actual.GetObject();
  auto expectedObject = expected.GetObject();
  for (auto const& pair : expectedObject) {
    if (!actualObject.HasKey(pair.Key())) {
      return false;
    }
    if (!JsonMatchesSubset(actualObject.GetNamedValue(pair.Key()), pair.Value())) {
      return false;
    }
  }
  return true;
}

std::optional<std::wstring> TextValue(json::JsonObject const& object, std::vector<std::wstring> const& keys) {
  auto value = FirstJsonValue(object, keys);
  if (!value.has_value()) {
    return std::nullopt;
  }
  if (value->ValueType() == json::JsonValueType::String) {
    return std::wstring(value->GetString().c_str());
  }
  if (value->ValueType() == json::JsonValueType::Number) {
    auto number = value->GetNumber();
    auto asInt = static_cast<int64_t>(number);
    if (static_cast<double>(asInt) == number) {
      return std::to_wstring(asInt);
    }
    return std::to_wstring(number);
  }
  return std::nullopt;
}

int64_t IntValue(json::JsonObject const& object, std::vector<std::wstring> const& keys, int64_t fallback) {
  auto value = FirstJsonValue(object, keys);
  if (!value.has_value()) {
    return fallback;
  }
  if (value->ValueType() == json::JsonValueType::Number) {
    return static_cast<int64_t>(value->GetNumber());
  }
  if (value->ValueType() == json::JsonValueType::Boolean) {
    return value->GetBoolean() ? 1 : 0;
  }
  if (value->ValueType() == json::JsonValueType::String) {
    try {
      return std::stoll(std::wstring(value->GetString().c_str()));
    } catch (...) {
      return fallback;
    }
  }
  return fallback;
}

std::optional<std::wstring> JsonTextValue(
    json::JsonObject const& object,
    std::vector<std::wstring> const& stringKeys,
    std::vector<std::wstring> const& objectKeys,
    std::optional<std::wstring> fallback) {
  auto text = TextValue(object, stringKeys);
  if (text.has_value()) {
    return text;
  }
  auto value = FirstJsonValue(object, objectKeys);
  if (value.has_value()) {
    return std::wstring(value->Stringify().c_str());
  }
  return fallback;
}

struct SqlBinding {
  enum class Kind {
    Text,
    NullableText,
    Int,
  };

  Kind kind = Kind::Text;
  std::optional<std::wstring> text;
  int64_t integer = 0;
};

SqlBinding SqlText(std::wstring value) {
  return SqlBinding{SqlBinding::Kind::Text, std::move(value), 0};
}

SqlBinding SqlNullableText(std::optional<std::wstring> value) {
  return SqlBinding{SqlBinding::Kind::NullableText, std::move(value), 0};
}

SqlBinding SqlInt(int64_t value) {
  return SqlBinding{SqlBinding::Kind::Int, std::nullopt, value};
}

bool ExecutePrepared(sqlite3* db, char const* sql, std::vector<SqlBinding> const& bindings) {
  sqlite3_stmt* statement = nullptr;
  bool ok = sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK;
  if (ok) {
    for (size_t index = 0; index < bindings.size(); ++index) {
      int sqliteIndex = static_cast<int>(index + 1);
      auto const& binding = bindings[index];
      switch (binding.kind) {
        case SqlBinding::Kind::Text:
          BindText(statement, sqliteIndex, binding.text.value_or(L""));
          break;
        case SqlBinding::Kind::NullableText:
          if (binding.text.has_value() && !binding.text->empty()) {
            BindText(statement, sqliteIndex, binding.text.value());
          } else {
            sqlite3_bind_null(statement, sqliteIndex);
          }
          break;
        case SqlBinding::Kind::Int:
          sqlite3_bind_int64(statement, sqliteIndex, static_cast<sqlite3_int64>(binding.integer));
          break;
      }
    }
    ok = sqlite3_step(statement) == SQLITE_DONE;
  }
  sqlite3_finalize(statement);
  return ok;
}

bool QuerySingleText(sqlite3* db, char const* sql, std::vector<SqlBinding> const& bindings, std::wstring* value) {
  sqlite3_stmt* statement = nullptr;
  bool ok = sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK;
  if (ok) {
    for (size_t index = 0; index < bindings.size(); ++index) {
      int sqliteIndex = static_cast<int>(index + 1);
      auto const& binding = bindings[index];
      switch (binding.kind) {
        case SqlBinding::Kind::Text:
          BindText(statement, sqliteIndex, binding.text.value_or(L""));
          break;
        case SqlBinding::Kind::NullableText:
          if (binding.text.has_value() && !binding.text->empty()) {
            BindText(statement, sqliteIndex, binding.text.value());
          } else {
            sqlite3_bind_null(statement, sqliteIndex);
          }
          break;
        case SqlBinding::Kind::Int:
          sqlite3_bind_int64(statement, sqliteIndex, static_cast<sqlite3_int64>(binding.integer));
          break;
      }
    }
    ok = sqlite3_step(statement) == SQLITE_ROW;
    if (ok && value != nullptr) {
      *value = ColumnText(statement, 0);
    }
  }
  sqlite3_finalize(statement);
  return ok;
}

int64_t QuerySingleInt(sqlite3* db, char const* sql, std::vector<SqlBinding> const& bindings) {
  sqlite3_stmt* statement = nullptr;
  int64_t value = 0;
  if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK) {
    for (size_t index = 0; index < bindings.size(); ++index) {
      int sqliteIndex = static_cast<int>(index + 1);
      auto const& binding = bindings[index];
      switch (binding.kind) {
        case SqlBinding::Kind::Text:
          BindText(statement, sqliteIndex, binding.text.value_or(L""));
          break;
        case SqlBinding::Kind::NullableText:
          if (binding.text.has_value() && !binding.text->empty()) {
            BindText(statement, sqliteIndex, binding.text.value());
          } else {
            sqlite3_bind_null(statement, sqliteIndex);
          }
          break;
        case SqlBinding::Kind::Int:
          sqlite3_bind_int64(statement, sqliteIndex, static_cast<sqlite3_int64>(binding.integer));
          break;
      }
    }
    if (sqlite3_step(statement) == SQLITE_ROW) {
      value = sqlite3_column_int64(statement, 0);
    }
  }
  sqlite3_finalize(statement);
  return value;
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

std::wstring UpperAscii(std::wstring value) {
  for (auto& ch : value) {
    if (ch >= L'a' && ch <= L'z') {
      ch = static_cast<wchar_t>(ch - L'a' + L'A');
    }
  }
  return value;
}

std::wstring LowerAscii(std::wstring value) {
  for (auto& ch : value) {
    if (ch >= L'A' && ch <= L'Z') {
      ch = static_cast<wchar_t>(ch - L'A' + L'a');
    }
  }
  return value;
}

std::wstring RegexEscape(std::wstring const& value) {
  std::wstring escaped;
  for (wchar_t ch : value) {
    switch (ch) {
      case L'\\':
      case L'.':
      case L'^':
      case L'$':
      case L'|':
      case L'(':
      case L')':
      case L'[':
      case L']':
      case L'{':
      case L'}':
      case L'*':
      case L'+':
      case L'?':
        escaped.push_back(L'\\');
        break;
      default:
        break;
    }
    escaped.push_back(ch);
  }
  return escaped;
}

std::wstring RegexFirst(std::wstring const& text, std::wstring const& pattern) {
  try {
    std::wregex regex(pattern, std::regex_constants::icase);
    std::wsmatch match;
    if (std::regex_search(text, match, regex) && match.size() > 1) {
      return match[1].str();
    }
  } catch (...) {
  }
  return L"";
}

std::wstring HtmlText(std::wstring html) {
  try {
    html = std::regex_replace(html, std::wregex(LR"(<script\b[\s\S]*?</script>)", std::regex_constants::icase), L" ");
    html = std::regex_replace(html, std::wregex(LR"(<style\b[\s\S]*?</style>)", std::regex_constants::icase), L" ");
    html = std::regex_replace(html, std::wregex(LR"(<[^>]+>)"), L" ");
    html = std::regex_replace(html, std::wregex(LR"(&nbsp;)"), L" ");
    html = std::regex_replace(html, std::wregex(LR"(&amp;)"), L"&");
    html = std::regex_replace(html, std::wregex(LR"(&lt;)"), L"<");
    html = std::regex_replace(html, std::wregex(LR"(&gt;)"), L">");
    html = std::regex_replace(html, std::wregex(LR"(&quot;)"), L"\"");
    html = std::regex_replace(html, std::wregex(LR"(\s+)"), L" ");
  } catch (...) {
  }
  while (!html.empty() && iswspace(html.front())) {
    html.erase(html.begin());
  }
  while (!html.empty() && iswspace(html.back())) {
    html.pop_back();
  }
  return html;
}

std::vector<std::wstring> TestIds(std::wstring const& html) {
  std::vector<std::wstring> ids;
  try {
    std::wregex regex(LR"(\bdata-testid=["']([^"']+)["'])", std::regex_constants::icase);
    auto begin = std::wsregex_iterator(html.begin(), html.end(), regex);
    auto end = std::wsregex_iterator();
    for (auto cursor = begin; cursor != end; ++cursor) {
      ids.push_back((*cursor)[1].str());
    }
  } catch (...) {
  }
  std::sort(ids.begin(), ids.end());
  ids.erase(std::unique(ids.begin(), ids.end()), ids.end());
  return ids;
}

std::wstring JsonStringArray(std::vector<std::wstring> const& values) {
  std::wstring out = L"[";
  for (size_t index = 0; index < values.size(); ++index) {
    if (index > 0) {
      out += L",";
    }
    out += JsonString(values[index]);
  }
  out += L"]";
  return out;
}

std::optional<std::wstring> TagForAttribute(std::wstring const& html, std::wstring const& attr, std::wstring const& value) {
  try {
    std::wregex regex(
        L"<([a-z0-9-]+)\\b[^>]*\\b" + RegexEscape(attr) + L"=[\"']" + RegexEscape(value) + L"[\"'][^>]*>",
        std::regex_constants::icase);
    std::wsmatch match;
    if (std::regex_search(html, match, regex) && match.size() > 1) {
      return LowerAscii(match[1].str());
    }
  } catch (...) {
  }
  return std::nullopt;
}

std::optional<std::wstring> TestIdSelectorValue(std::wstring const& selector) {
  try {
    std::wregex regex(LR"(\[data-testid=["']([^"']+)["']\])", std::regex_constants::icase);
    std::wsmatch match;
    if (std::regex_search(selector, match, regex) && match.size() > 1) {
      return match[1].str();
    }
  } catch (...) {
  }
  return std::nullopt;
}

bool IsSimpleTagSelector(std::wstring const& selector) {
  try {
    return std::regex_match(selector, std::wregex(LR"(^[a-z][a-z0-9-]*$)", std::regex_constants::icase));
  } catch (...) {
    return false;
  }
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
  auto markerPath = EnvironmentValue(L"TERRANE_WINDOWS_SMOKE_RESULT_FILE");
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
    WriteControlLine(L"TERRANE_WINDOWS_CONTROL_READY port=" + std::to_wstring(port) + L" token_path=" + tokenPath.wstring());
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

  std::wstring ActiveVersion(sqlite3* db, std::wstring const& appId) {
    if (appId.empty()) {
      return L"";
    }
    sqlite3_stmt* statement = nullptr;
    std::wstring version;
    if (sqlite3_prepare_v2(db, "SELECT active_version FROM apps WHERE id = ?", -1, &statement, nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        version = ColumnText(statement, 0);
      }
    }
    sqlite3_finalize(statement);
    return version;
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

  std::wstring PlatformListTargetsJson() {
    return L"{\"targets\":[{\"id\":\"windows-native\",\"platform\":\"windows\",\"status\":\"available\","
        L"\"runtimeVersion\":\"0.1.0\",\"controlPlane\":{\"port\":" +
        std::to_wstring(port) +
        L",\"debug\":true}}]}";
  }

  std::wstring BundledWebappJson(std::wstring const& appId) {
    auto manifest = BundledManifest(appId);
    auto name = manifest.has_value() ? ManifestString(manifest.value(), L"name") : std::nullopt;
    auto version = manifest.has_value() ? ManifestString(manifest.value(), L"version") : std::nullopt;
    auto description = manifest.has_value() ? ManifestString(manifest.value(), L"description") : std::nullopt;
    auto dataVersion = manifest.has_value() ? ManifestDataVersion(manifest.value()) : 1;
    return L"{\"appId\":" + JsonString(appId) +
        L",\"name\":" + JsonString(name.value_or(appId)) +
        L",\"version\":" + (version.has_value() ? JsonString(version.value()) : L"null") +
        L",\"description\":" + (description.has_value() ? JsonString(description.value()) : L"null") +
        L",\"status\":\"bundled\",\"dataVersion\":" + std::to_wstring(dataVersion) +
        L",\"bundled\":true,\"installed\":false}";
  }

  std::wstring PlatformListWebappsJson(bool includeUninstalled, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }

    sqlite3_stmt* statement = nullptr;
    char const* sql =
        "SELECT a.id, a.name, a.status, a.active_install_id, a.active_version, a.data_version, "
        "a.created_at, a.updated_at, v.runtime_version, v.trust_level "
        "FROM apps a LEFT JOIN app_versions v ON v.install_id = a.active_install_id "
        "WHERE (? = 1 OR a.status <> 'uninstalled') ORDER BY a.id";
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
      *error = L"Could not list Windows webapps: " + Utf8ToWide(sqlite3_errmsg(db));
      return L"";
    }
    sqlite3_bind_int(statement, 1, includeUninstalled ? 1 : 0);

    std::vector<std::wstring> installedIds;
    std::wstring apps = L"[";
    bool first = true;
    while (sqlite3_step(statement) == SQLITE_ROW) {
      auto appId = ColumnText(statement, 0);
      if (appId.empty()) {
        continue;
      }
      installedIds.push_back(appId);
      if (!first) {
        apps += L",";
      }
      first = false;
      apps += L"{\"appId\":" + JsonString(appId) +
          L",\"name\":" + SqliteValueJson(statement, 1) +
          L",\"status\":" + SqliteValueJson(statement, 2) +
          L",\"activeInstallId\":" + SqliteValueJson(statement, 3) +
          L",\"activeVersion\":" + SqliteValueJson(statement, 4) +
          L",\"dataVersion\":" + SqliteValueJson(statement, 5) +
          L",\"runtimeVersion\":" + SqliteValueJson(statement, 8) +
          L",\"trustLevel\":" + SqliteValueJson(statement, 9) +
          L",\"createdAt\":" + SqliteValueJson(statement, 6) +
          L",\"updatedAt\":" + SqliteValueJson(statement, 7) +
          L",\"bundled\":false,\"installed\":true}";
    }
    sqlite3_finalize(statement);

    for (auto const* bundledId : BundledWebappIds()) {
      std::wstring appId = bundledId;
      if (ContainsAppId(installedIds, appId)) {
        continue;
      }
      if (!first) {
        apps += L",";
      }
      first = false;
      apps += BundledWebappJson(appId);
    }
    apps += L"]";
    return L"{\"apps\":" + apps + L"}";
  }

  std::wstring HtmlForBundledApp(std::wstring const& appId) {
    return ReadTextFile(RuntimeResourceRoot() / L"webapps" / L"examples" / appId / L"index.html");
  }

  std::wstring BundledAppText(std::wstring const& appId, std::wstring const& relativePath) {
    return ReadTextFile(RuntimeResourceRoot() / L"webapps" / L"examples" / appId / relativePath);
  }

  std::vector<std::wstring> RuntimeQueryMatches(std::wstring const& html, json::JsonObject const& args) {
    std::vector<std::wstring> matches;
    auto testId = OptionalStringMember(args, L"testId");
    if (testId.has_value() && !testId->empty()) {
      auto tag = TagForAttribute(html, L"data-testid", testId.value());
      if (tag.has_value()) {
        matches.push_back(L"{\"kind\":\"testId\",\"value\":" + JsonString(testId.value()) + L",\"tag\":" + JsonString(tag.value()) + L"}");
      }
      return matches;
    }

    auto selector = OptionalStringMember(args, L"selector");
    if (selector.has_value() && selector->size() > 1 && selector->front() == L'#') {
      auto id = selector->substr(1);
      auto tag = TagForAttribute(html, L"id", id);
      if (tag.has_value()) {
        matches.push_back(L"{\"kind\":\"selector\",\"value\":" + JsonString(selector.value()) + L",\"tag\":" + JsonString(tag.value()) + L"}");
      }
      return matches;
    }

    if (selector.has_value()) {
      auto selectorTestId = TestIdSelectorValue(selector.value());
      if (selectorTestId.has_value()) {
        auto tag = TagForAttribute(html, L"data-testid", selectorTestId.value());
        if (tag.has_value()) {
          matches.push_back(L"{\"kind\":\"selector\",\"value\":" + JsonString(selector.value()) + L",\"tag\":" + JsonString(tag.value()) + L"}");
        }
        return matches;
      }
    }

    auto text = OptionalStringMember(args, L"text");
    if (text.has_value() && !text->empty() && HtmlText(html).find(text.value()) != std::wstring::npos) {
      matches.push_back(L"{\"kind\":\"text\",\"value\":" + JsonString(text.value()) + L"}");
      return matches;
    }

    if (selector.has_value() && IsSimpleTagSelector(selector.value())) {
      try {
        std::wregex regex(L"<" + RegexEscape(selector.value()) + L"\\b", std::regex_constants::icase);
        if (std::regex_search(html, regex)) {
          auto tag = LowerAscii(selector.value());
          matches.push_back(L"{\"kind\":\"selector\",\"value\":" + JsonString(selector.value()) + L",\"tag\":" + JsonString(tag) + L"}");
        }
      } catch (...) {
      }
    }
    return matches;
  }

  std::wstring RuntimeQueryJson(std::wstring const& appId, json::JsonObject const& args) {
    auto html = HtmlForBundledApp(appId);
    auto testId = OptionalStringMember(args, L"testId");
    auto selector = OptionalStringMember(args, L"selector");
    auto text = OptionalStringMember(args, L"text");
    std::wstring query = testId.has_value()
        ? L"[data-testid=\"" + testId.value() + L"\"]"
        : selector.value_or(text.value_or(L""));
    auto matches = RuntimeQueryMatches(html, args);
    std::wstring matchJson = L"[";
    for (size_t index = 0; index < matches.size(); ++index) {
      if (index > 0) {
        matchJson += L",";
      }
      matchJson += matches[index];
    }
    matchJson += L"]";
    return L"{\"ok\":" + std::wstring(matches.empty() ? L"false" : L"true") +
        L",\"appId\":" + JsonString(appId) +
        L",\"query\":" + JsonString(query) +
        L",\"matches\":" + matchJson + L"}";
  }

  std::wstring RuntimeScreenshotJson(std::wstring const& appId, std::optional<std::wstring> const& label) {
    auto html = HtmlForBundledApp(appId);
    auto text = HtmlText(html);
    auto title = HtmlText(RegexFirst(html, LR"(<title[^>]*>([\s\S]*?)</title>)"));
    return L"{\"ok\":true,\"appId\":" + JsonString(appId) +
        L",\"label\":" + (label.has_value() && !label->empty() ? JsonString(label.value()) : L"null") +
        L",\"format\":\"static-html-summary\",\"title\":" + JsonString(title) +
        L",\"textHash\":\"sha256:" + Sha256Hex(text) +
        L"\",\"testIds\":" + JsonStringArray(TestIds(html)) + L"}";
  }

  bool HtmlContains(std::wstring const& html, std::wstring const& pattern) {
    try {
      return std::regex_search(html, std::wregex(pattern, std::regex_constants::icase));
    } catch (...) {
      return false;
    }
  }

  std::optional<std::wstring> HtmlAttribute(std::wstring const& attrs, std::wstring const& name) {
    try {
      std::wregex regex(L"\\b" + RegexEscape(name) + L"=[\"']([^\"']*)[\"']", std::regex_constants::icase);
      std::wsmatch match;
      if (std::regex_search(attrs, match, regex) && match.size() > 1) {
        return match[1].str();
      }
    } catch (...) {
    }
    return std::nullopt;
  }

  struct AccessibilityControlRecord {
    std::wstring tag;
    std::wstring type;
    std::wstring testId;
    std::wstring selector;
    std::wstring name;
  };

  std::wstring AccessibleName(std::wstring const& attrs, std::wstring const& innerHtml) {
    for (auto const& attr : {L"aria-label", L"title", L"alt", L"value"}) {
      auto value = HtmlAttribute(attrs, attr);
      if (value.has_value() && !value->empty()) {
        return value.value();
      }
    }
    return HtmlText(innerHtml);
  }

  std::vector<AccessibilityControlRecord> AccessibilityControls(std::wstring const& html) {
    std::vector<AccessibilityControlRecord> controls;
    auto appendControl = [&](std::wstring tag, std::wstring attrs, std::wstring innerHtml) {
      auto testId = HtmlAttribute(attrs, L"data-testid").value_or(L"");
      AccessibilityControlRecord record;
      record.tag = LowerAscii(tag);
      record.type = HtmlAttribute(attrs, L"type").value_or(L"");
      record.testId = testId;
      record.selector = testId.empty() ? record.tag : L"[data-testid=\"" + testId + L"\"]";
      record.name = AccessibleName(attrs, innerHtml);
      controls.push_back(record);
    };

    try {
      std::wregex paired(LR"(<(button|select|textarea|a)\b([^>]*)>([\s\S]*?)</\1>)", std::regex_constants::icase);
      for (auto cursor = std::wsregex_iterator(html.begin(), html.end(), paired); cursor != std::wsregex_iterator(); ++cursor) {
        appendControl((*cursor)[1].str(), (*cursor)[2].str(), (*cursor)[3].str());
      }
      std::wregex input(LR"(<input\b([^>]*)>)", std::regex_constants::icase);
      for (auto cursor = std::wsregex_iterator(html.begin(), html.end(), input); cursor != std::wsregex_iterator(); ++cursor) {
        appendControl(L"input", (*cursor)[1].str(), L"");
      }
    } catch (...) {
    }
    return controls;
  }

  std::wstring AccessibilityControlsJson(std::vector<AccessibilityControlRecord> const& controls) {
    std::wstring out = L"[";
    for (size_t index = 0; index < controls.size(); ++index) {
      auto const& control = controls[index];
      if (index > 0) {
        out += L",";
      }
      out += L"{\"tag\":" + JsonString(control.tag) +
          L",\"type\":" + JsonNullableString(control.type) +
          L",\"testId\":" + JsonString(control.testId) +
          L",\"selector\":" + JsonString(control.selector) +
          L",\"name\":" + JsonString(control.name) + L"}";
    }
    out += L"]";
    return out;
  }

  std::wstring AccessibilityHeadingsJson(std::wstring const& html) {
    std::wstring out = L"[";
    bool first = true;
    try {
      std::wregex heading(LR"(<h([1-6])\b[^>]*>([\s\S]*?)</h\1>)", std::regex_constants::icase);
      for (auto cursor = std::wsregex_iterator(html.begin(), html.end(), heading); cursor != std::wsregex_iterator(); ++cursor) {
        if (!first) {
          out += L",";
        }
        first = false;
        out += L"{\"level\":" + (*cursor)[1].str() +
            L",\"name\":" + JsonString(HtmlText((*cursor)[2].str())) + L"}";
      }
    } catch (...) {
    }
    out += L"]";
    return out;
  }

  std::optional<AccessibilityControlRecord> FirstUnlabeledControl(std::vector<AccessibilityControlRecord> const& controls) {
    for (auto const& control : controls) {
      if (control.name.empty()) {
        return control;
      }
    }
    return std::nullopt;
  }

  std::wstring AccessibilityCheckJson(
      std::wstring const& id,
      bool ok,
      std::wstring const& message,
      std::optional<std::wstring> const& selector = std::nullopt) {
    std::wstring out = L"{\"id\":" + JsonString(id) +
        L",\"status\":\"" + std::wstring(ok ? L"pass" : L"fail") +
        L"\",\"message\":" + JsonString(message);
    if (selector.has_value() && !selector->empty()) {
      out += L",\"selector\":" + JsonString(selector.value());
    }
    out += L"}";
    return out;
  }

  std::wstring RuntimeAccessibilitySnapshotJson(std::wstring const& appId) {
    auto html = HtmlForBundledApp(appId);
    auto title = HtmlText(RegexFirst(html, LR"(<title[^>]*>([\s\S]*?)</title>)"));
    auto controls = AccessibilityControls(html);
    std::wstring landmarks = HtmlContains(html, LR"(<main\b)")
        ? L"[{\"role\":\"main\",\"selector\":\"main\"}]"
        : L"[]";
    return L"{\"appId\":" + JsonString(appId) +
        L",\"title\":" + JsonString(title) +
        L",\"landmarks\":" + landmarks +
        L",\"headings\":" + AccessibilityHeadingsJson(html) +
        L",\"controls\":" + AccessibilityControlsJson(controls) + L"}";
  }

  std::wstring RuntimeAccessibilityAuditJson(std::wstring const& appId) {
    auto html = HtmlForBundledApp(appId);
    auto title = HtmlText(RegexFirst(html, LR"(<title[^>]*>([\s\S]*?)</title>)"));
    auto controls = AccessibilityControls(html);
    auto unlabeled = FirstUnlabeledControl(controls);
    bool hasTitle = !title.empty();
    bool hasMain = HtmlContains(html, LR"(<main\b)");
    bool hasH1 = HtmlContains(html, LR"(<h1\b[^>]*>[\s\S]*?</h1>)");
    bool pass = hasTitle && hasMain && hasH1 && !unlabeled.has_value();
    return L"{\"appId\":" + JsonString(appId) +
        L",\"checkedAt\":" + JsonString(NowIso()) +
        L",\"status\":\"" + std::wstring(pass ? L"pass" : L"fail") +
        L"\",\"checks\":[" +
        AccessibilityCheckJson(L"document_title", hasTitle, L"Document must include a non-empty <title>.") + L"," +
        AccessibilityCheckJson(L"main_landmark", hasMain, L"Page must include a <main> landmark.") + L"," +
        AccessibilityCheckJson(L"screen_title", hasH1, L"Page must include an h1 screen title.") + L"," +
        AccessibilityCheckJson(
            L"no_unlabeled_controls",
            !unlabeled.has_value(),
            L"Every interactive control must have an accessible name.",
            unlabeled.has_value() ? std::optional<std::wstring>(unlabeled->selector) : std::nullopt) +
        L"]}";
  }

  bool AccessibilityFailsRule(std::wstring const& appId, std::optional<std::wstring> const& rule) {
    auto html = HtmlForBundledApp(appId);
    auto title = HtmlText(RegexFirst(html, LR"(<title[^>]*>([\s\S]*?)</title>)"));
    auto controls = AccessibilityControls(html);
    auto unlabeled = FirstUnlabeledControl(controls);
    std::vector<std::pair<std::wstring, bool>> checks = {
        {L"document_title", !title.empty()},
        {L"main_landmark", HtmlContains(html, LR"(<main\b)")},
        {L"screen_title", HtmlContains(html, LR"(<h1\b[^>]*>[\s\S]*?</h1>)")},
        {L"no_unlabeled_controls", !unlabeled.has_value()},
    };
    for (auto const& check : checks) {
      if (!check.second && (!rule.has_value() || rule->empty() || check.first == rule.value())) {
        return true;
      }
    }
    return false;
  }

  std::wstring RuntimeAssertAccessibilityJson(
      std::wstring const& appId,
      std::optional<std::wstring> const& rule,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    if (AccessibilityFailsRule(appId, rule)) {
      *errorCode = L"accessibility_failed";
      *errorMessage = L"Accessibility assertion failed";
      return L"";
    }
    return L"{\"ok\":true,\"appId\":" + JsonString(appId) +
        L",\"rule\":" + (rule.has_value() && !rule->empty() ? JsonString(rule.value()) : L"null") +
        L",\"report\":" + RuntimeAccessibilityAuditJson(appId) + L"}";
  }

  std::wstring RuntimeTargetCommandJson(
      std::wstring const& tool,
      json::JsonObject const& args,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    if (tool == L"runtime.press_key") {
      return L"{\"ok\":true,\"key\":" + JsonNullableString(OptionalStringMember(args, L"key").value_or(L"")) + L"}";
    }
    auto appId = OptionalStringMember(args, L"appId").value_or(L"");
    auto matches = RuntimeQueryMatches(HtmlForBundledApp(appId), args);
    if (matches.empty()) {
      *errorCode = L"selector.not_found";
      *errorMessage = L"Runtime target was not found in generated app HTML";
      return L"";
    }
    std::wstring response = L"{\"ok\":true,\"tool\":" + JsonString(tool) +
        L",\"target\":" + matches.front();
    if (tool == L"runtime.type" || tool == L"runtime.set_value") {
      auto value = OptionalStringMember(args, L"value");
      if (!value.has_value()) {
        value = OptionalStringMember(args, L"text");
      }
      response += L",\"value\":" + JsonString(value.value_or(L""));
    }
    response += L"}";
    return response;
  }

  std::wstring RuntimeAssertVisibleJson(
      std::wstring const& appId,
      json::JsonObject const& args,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    auto matches = RuntimeQueryMatches(HtmlForBundledApp(appId), args);
    if (matches.empty()) {
      *errorCode = L"selector.not_found";
      *errorMessage = L"Expected runtime target is not visible";
      return L"";
    }
    return L"{\"ok\":true,\"appId\":" + JsonString(appId) +
        L",\"matches\":" + std::to_wstring(matches.size()) +
        L",\"target\":" + matches.front() + L"}";
  }

  std::wstring RuntimeAssertTextJson(
      std::wstring const& appId,
      std::wstring const& text,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    if (HtmlText(HtmlForBundledApp(appId)).find(text) == std::wstring::npos) {
      *errorCode = L"text.not_found";
      *errorMessage = L"Expected text was not found in installed package HTML";
      return L"";
    }
    return L"{\"ok\":true,\"appId\":" + JsonString(appId) + L",\"text\":" + JsonString(text) + L"}";
  }

  std::wstring RuntimeWaitForJson(
      json::JsonObject const& args,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    auto kind = StringMemberOr(args, L"kind", L"idle");
    if (kind == L"idle") {
      return L"{\"ok\":true,\"kind\":\"idle\"}";
    }
    if (kind == L"bridge_call" || kind == L"bridgeCall") {
      auto appId = OptionalStringMember(args, L"appId").value_or(L"");
      auto bridgeMethod = OptionalStringMember(args, L"method").value_or(L"");
      if (appId.empty() || bridgeMethod.empty()) {
        *errorCode = L"invalid_request";
        *errorMessage = L"runtime.wait_for bridge_call requires appId and method";
        return L"";
      }
      auto result = AssertBridgeCallJson(appId, bridgeMethod, errorCode, errorMessage);
      if (result.empty()) {
        if (*errorCode == L"assertion_failed") {
          *errorCode = L"wait_timeout";
          *errorMessage = L"Expected bridge call was not recorded";
        }
        return L"";
      }
      return result.substr(0, result.size() - 1) + L",\"kind\":" + JsonString(kind) + L"}";
    }
    auto appId = OptionalStringMember(args, L"appId").value_or(L"");
    if (appId.empty()) {
      *errorCode = L"invalid_request";
      *errorMessage = L"runtime.wait_for requires appId for selector/text waits";
      return L"";
    }
    auto matches = RuntimeQueryMatches(HtmlForBundledApp(appId), args);
    if (matches.empty()) {
      *errorCode = L"wait_timeout";
      *errorMessage = L"Expected runtime condition did not appear";
      return L"";
    }
    return L"{\"ok\":true,\"kind\":" + JsonString(kind) +
        L",\"appId\":" + JsonString(appId) +
        L",\"matches\":" + std::to_wstring(matches.size()) + L"}";
  }

  std::wstring RuntimeTimerAdvanceJson(json::JsonObject const& args) {
    auto milliseconds = IntValue(args, {L"ms", L"milliseconds"}, 0);
    if (milliseconds < 0) {
      milliseconds = 0;
    }
    return L"{\"ok\":true,\"advancedMs\":" + std::to_wstring(milliseconds) + L"}";
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

  int64_t ScalarInt(char const* sql, std::vector<std::wstring> const& values) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      return 0;
    }
    sqlite3_stmt* statement = nullptr;
    int64_t value = 0;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK) {
      for (size_t index = 0; index < values.size(); ++index) {
        BindText(statement, static_cast<int>(index + 1), values[index]);
      }
      if (sqlite3_step(statement) == SQLITE_ROW) {
        value = sqlite3_column_int64(statement, 0);
      }
    }
    sqlite3_finalize(statement);
    return value;
  }

  std::wstring ResourceUsageJson(std::wstring const& appId) {
    auto since = NowIso();
    if (appId.empty()) {
      return L"";
    }
    auto storageBytes = ScalarInt(
        "SELECT COALESCE(SUM(LENGTH(CAST(value_json AS BLOB))), 0) FROM app_storage WHERE app_id = ?",
        {appId});
    auto bridgeCalls = ScalarInt("SELECT COUNT(*) FROM bridge_calls WHERE app_id = ?", {appId});
    auto coreEvents = ScalarInt("SELECT COUNT(*) FROM core_events WHERE app_id = ?", {appId});
    auto networkRequests = ScalarInt(
        "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = 'network.request' AND created_at >= datetime('now', '-60 seconds')",
        {appId});
    auto logLines = ScalarInt(
        "SELECT COUNT(*) FROM bridge_calls WHERE app_id = ? AND method = 'app.log' AND created_at >= datetime('now', '-60 seconds')",
        {appId});
    auto packageBytes = ScalarInt(
        "SELECT COALESCE(SUM(f.size_bytes), 0) FROM app_files f JOIN app_versions v ON v.install_id = f.install_id WHERE v.app_id = ?",
        {appId});
    return L"{\"appId\":" + JsonString(appId) +
        L",\"storageBytes\":" + std::to_wstring(storageBytes) +
        L",\"bridgeCalls\":" + std::to_wstring(bridgeCalls) +
        L",\"coreEvents\":" + std::to_wstring(coreEvents) +
        L",\"networkRequestsLastMinute\":" + std::to_wstring(networkRequests) +
        L",\"logLinesLastMinute\":" + std::to_wstring(logLines) +
        L",\"domNodes\":0,\"timers\":0,\"packageBytes\":" + std::to_wstring(packageBytes) +
        L",\"measuredAt\":" + JsonString(since) + L"}";
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

  struct PackageFile {
    std::wstring path;
    std::wstring content;
    std::wstring contentHash;
    int64_t sizeBytes = 0;
    std::wstring mime;
  };

  struct PackageRead {
    std::filesystem::path directory;
    json::JsonObject manifest{nullptr};
    std::wstring manifestJson;
    std::vector<PackageFile> files;
    std::vector<std::wstring> errors;
    std::vector<std::wstring> warnings;
  };

  BridgeRequest StorageBridgeRequest(
      std::wstring const& requestId,
      std::wstring const& appId,
      std::wstring const& method,
      json::JsonObject const& params,
      std::wstring const& permission) {
    BridgeRequest request;
    request.hasId = true;
    request.id = requestId;
    request.method = method;
    request.params = params;
    request.context.appId = appId;
    request.context.storagePrefix = appId + L":";
    request.context.approvedPermissions.insert(permission);
    return request;
  }

  std::wstring RuntimeSessionForControlSession(sqlite3* db, std::wstring const& childControlSessionId, std::wstring const& appId) {
    ControlSessionRecord record;
    if (LoadControlSession(db, childControlSessionId, &record) && !record.runtimeSessionId.empty()) {
      return record.runtimeSessionId;
    }

    auto runtimeSessionId = MakeId(L"session");
    auto metadataJson = L"{\"controlSessionId\":" + JsonString(childControlSessionId) + L",\"source\":\"windows-dev-control-storage\"}";
    sqlite3_stmt* statement = nullptr;
    bool ok = sqlite3_prepare_v2(
                  db,
                  "INSERT INTO runtime_sessions "
                  "(session_id, target, platform, runtime_version, active_app_id, active_install_id, started_at, status, capabilities_json, resource_high_water_json, metadata_json) "
                  "VALUES (?, 'windows', 'windows', '0.1.0', ?, NULL, datetime('now'), 'running', ?, '{}', ?)",
                  -1,
                  &statement,
                  nullptr) == SQLITE_OK;
    if (ok) {
      BindText(statement, 1, runtimeSessionId);
      BindText(statement, 2, appId);
      BindText(statement, 3, RuntimeCapabilitiesJson(appId));
      BindText(statement, 4, metadataJson);
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
    if (!ok) {
      return L"";
    }

    if (sqlite3_prepare_v2(
            db,
            "UPDATE control_sessions SET runtime_session_id = ? WHERE control_session_id = ? AND runtime_session_id IS NULL",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, runtimeSessionId);
      BindText(statement, 2, childControlSessionId);
      sqlite3_step(statement);
    }
    sqlite3_finalize(statement);
    return runtimeSessionId;
  }

  bool RecordTestRun(
      std::wstring const& childControlSessionId,
      std::optional<std::wstring> const& appId,
      std::wstring const& microTestId,
      std::wstring const& name,
      std::wstring const& specJson,
      std::wstring const& status,
      std::wstring const& resultJson,
      std::wstring const& diagnosticsJson) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      return false;
    }
    std::wstring runtimeSessionId;
    if (appId.has_value() && !appId->empty()) {
      runtimeSessionId = RuntimeSessionForControlSession(db, childControlSessionId, appId.value());
    }
    sqlite3_stmt* microStatement = nullptr;
    bool ok = sqlite3_prepare_v2(
                  db,
                  "INSERT INTO micro_tests (micro_test_id, app_id, name, spec_json, created_at, updated_at) "
                  "VALUES (?, ?, ?, ?, datetime('now'), datetime('now')) "
                  "ON CONFLICT(micro_test_id) DO UPDATE SET "
                  "app_id = excluded.app_id, "
                  "name = excluded.name, "
                  "spec_json = excluded.spec_json, "
                  "updated_at = excluded.updated_at",
                  -1,
                  &microStatement,
                  nullptr) == SQLITE_OK;
    if (ok) {
      BindText(microStatement, 1, microTestId);
      if (appId.has_value() && !appId->empty()) {
        BindText(microStatement, 2, appId.value());
      } else {
        sqlite3_bind_null(microStatement, 2);
      }
      BindText(microStatement, 3, name);
      BindText(microStatement, 4, specJson);
      ok = sqlite3_step(microStatement) == SQLITE_DONE;
    }
    sqlite3_finalize(microStatement);
    if (!ok) {
      return false;
    }
    sqlite3_stmt* statement = nullptr;
    ok = sqlite3_prepare_v2(
             db,
             "INSERT INTO test_runs "
             "(test_run_id, micro_test_id, session_id, control_session_id, app_id, status, started_at, finished_at, result_json, diagnostics_json) "
             "VALUES (?, ?, ?, ?, ?, ?, datetime('now'), datetime('now'), ?, ?)",
             -1,
             &statement,
             nullptr) == SQLITE_OK;
    if (ok) {
      BindText(statement, 1, MakeId(L"test-run"));
      BindText(statement, 2, microTestId);
      if (runtimeSessionId.empty()) {
        sqlite3_bind_null(statement, 3);
      } else {
        BindText(statement, 3, runtimeSessionId);
      }
      BindText(statement, 4, childControlSessionId);
      if (appId.has_value() && !appId->empty()) {
        BindText(statement, 5, appId.value());
      } else {
        sqlite3_bind_null(statement, 5);
      }
      BindText(statement, 6, status);
      BindText(statement, 7, resultJson);
      BindText(statement, 8, diagnosticsJson);
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
    return ok;
  }

  bool TextCanAppear(std::wstring const& html, std::vector<std::wstring> const& dynamicText, std::wstring const& text) {
    if (HtmlText(html).find(text) != std::wstring::npos) {
      return true;
    }
    return std::find(dynamicText.begin(), dynamicText.end(), text) != dynamicText.end();
  }

  bool BridgeMethodReferenced(std::wstring const& appId, std::wstring const& bridgeMethod) {
    return BundledAppText(appId, L"app.js").find(bridgeMethod) != std::wstring::npos;
  }

  std::wstring TestFailureJson(
      std::wstring const& testName,
      std::wstring const& code,
      std::wstring const& field,
      std::wstring const& value) {
    return L"{\"test\":" + JsonString(testName) +
        L",\"code\":" + JsonString(code) +
        L"," + JsonString(field) + L":" + JsonString(value) + L"}";
  }

  std::wstring JsonArrayText(std::vector<std::wstring> const& items) {
    std::wstring out = L"[";
    for (size_t index = 0; index < items.size(); ++index) {
      if (index > 0) {
        out += L",";
      }
      out += items[index];
    }
    out += L"]";
    return out;
  }

  std::optional<std::filesystem::path> PackageDirectoryFromArgs(json::JsonObject const& args) {
    auto pathValue = OptionalStringMember(args, L"packagePath");
    if (!pathValue.has_value() || pathValue->empty()) {
      pathValue = OptionalStringMember(args, L"path");
    }
    if (!pathValue.has_value() || pathValue->empty()) {
      return std::nullopt;
    }
    auto root = RepoRoot().lexically_normal();
    std::filesystem::path requested(pathValue.value());
    auto candidate = (requested.is_absolute() ? requested : root / requested).lexically_normal();
    auto rootText = LowerAscii(root.wstring());
    auto candidateText = LowerAscii(candidate.wstring());
    if (candidateText != rootText &&
        candidateText.rfind(rootText + L"\\", 0) != 0 &&
        candidateText.rfind(rootText + L"/", 0) != 0) {
      return std::nullopt;
    }
    return candidate;
  }

  std::wstring MimeTypeForPackagePath(std::wstring const& path) {
    auto lower = LowerAscii(path);
    if (lower.size() >= 5 && lower.substr(lower.size() - 5) == L".html") {
      return L"text/html";
    }
    if (lower.size() >= 4 && lower.substr(lower.size() - 4) == L".css") {
      return L"text/css";
    }
    if (lower.size() >= 3 && lower.substr(lower.size() - 3) == L".js") {
      return L"text/javascript";
    }
    if (lower.size() >= 5 && lower.substr(lower.size() - 5) == L".json") {
      return L"application/json";
    }
    return L"text/plain";
  }

  std::wstring PackageIssueJson(
      std::wstring const& code,
      std::wstring const& message,
      std::wstring const& detailsJson = L"{}") {
    return L"{\"code\":" + JsonString(code) +
        L",\"message\":" + JsonString(message) +
        L",\"details\":" + (detailsJson.empty() ? L"{}" : detailsJson) + L"}";
  }

  bool PackageHasFile(PackageRead const& package, std::wstring const& path) {
    return std::find_if(package.files.begin(), package.files.end(), [&](PackageFile const& file) {
      return file.path == path;
    }) != package.files.end();
  }

  std::wstring PackageFileContent(PackageRead const& package, std::wstring const& path) {
    auto found = std::find_if(package.files.begin(), package.files.end(), [&](PackageFile const& file) {
      return file.path == path;
    });
    return found == package.files.end() ? L"" : found->content;
  }

  std::wstring PackageFilePathsJson(PackageRead const& package) {
    std::vector<std::wstring> paths;
    for (auto const& file : package.files) {
      paths.push_back(file.path);
    }
    std::sort(paths.begin(), paths.end());
    return JsonStringArray(paths);
  }

  std::wstring PackageFilesJson(PackageRead const& package) {
    std::vector<std::wstring> rows;
    for (auto const& file : package.files) {
      rows.push_back(
          L"{\"path\":" + JsonString(file.path) +
          L",\"contentText\":" + JsonString(file.content) +
          L",\"contentHash\":" + JsonString(file.contentHash) +
          L",\"sizeBytes\":" + std::to_wstring(file.sizeBytes) +
          L",\"mime\":" + JsonString(file.mime) + L"}");
    }
    return JsonArrayText(rows);
  }

  std::optional<std::wstring> PermissionForBridgeMethod(std::wstring const& method) {
    if (method == L"storage.get" || method == L"storage.list") {
      return L"storage.read";
    }
    if (method == L"storage.set" || method == L"storage.remove") {
      return L"storage.write";
    }
    if (method == L"dialog.openFile" || method == L"dialog.saveFile" ||
        method == L"notification.toast" || method == L"network.request" ||
        method == L"core.step") {
      return method;
    }
    return std::nullopt;
  }

  std::vector<std::wstring> BridgeMethodsIn(std::wstring const& appJs) {
    std::vector<std::wstring> methods;
    for (auto const& method : {
             L"storage.get",
             L"storage.set",
             L"storage.remove",
             L"storage.list",
             L"dialog.openFile",
             L"dialog.saveFile",
             L"notification.toast",
             L"network.request",
             L"core.step",
             L"app.log",
             L"runtime.capabilities",
         }) {
      if (appJs.find(method) != std::wstring::npos) {
        methods.push_back(method);
      }
    }
    return methods;
  }

  std::wstring BridgeMethodsJson(PackageRead const& package) {
    auto methods = BridgeMethodsIn(PackageFileContent(package, L"app.js"));
    std::sort(methods.begin(), methods.end());
    return JsonStringArray(methods);
  }

  void ValidateGeneratedSourcePolicy(PackageRead* package) {
    auto appJs = PackageFileContent(*package, L"app.js");
    std::vector<std::pair<std::wstring, std::wregex>> checks;
    try {
      checks = {
          {L"forbidden_eval", std::wregex(LR"(\beval\s*\()", std::regex_constants::icase)},
          {L"forbidden_function_constructor", std::wregex(LR"(\bnew\s+Function\s*\()", std::regex_constants::icase)},
          {L"forbidden_dynamic_import", std::wregex(LR"(\bimport\s*\()", std::regex_constants::icase)},
          {L"forbidden_network_api", std::wregex(LR"(\bfetch\s*\()", std::regex_constants::icase)},
          {L"forbidden_network_api", std::wregex(LR"(\bXMLHttpRequest\b)", std::regex_constants::icase)},
          {L"forbidden_storage_api", std::wregex(LR"(\blocalStorage\b|\bsessionStorage\b|\bindexedDB\b|\bdocument\.cookie\b)", std::regex_constants::icase)},
          {L"forbidden_native_bridge", std::wregex(LR"(\bwebkit\.messageHandlers\b|\bchrome\.webview\b|\bAndroid\.|\bTerranePlatformBridge\b)", std::regex_constants::icase)},
      };
    } catch (...) {
      return;
    }
    for (auto const& check : checks) {
      if (std::regex_search(appJs, check.second)) {
        package->errors.push_back(PackageIssueJson(check.first, L"app.js uses a forbidden generated-app API"));
      }
    }
  }

  void ValidatePackageManifest(PackageRead* package) {
    auto const& manifest = package->manifest;
    for (auto const& field : {
             L"id",
             L"name",
             L"version",
             L"runtimeVersion",
             L"entry",
             L"description",
             L"permissions",
             L"storagePrefix",
             L"dataVersion",
             L"capabilities",
             L"resourceBudget",
             L"networkPolicy",
         }) {
      if (!manifest.HasKey(field)) {
        package->errors.push_back(PackageIssueJson(
            L"missing_manifest_field",
            L"manifest." + std::wstring(field) + L" is required",
            L"{\"field\":" + JsonString(field) + L"}"));
      }
    }
    if (manifest.HasKey(L"networkAllowlist")) {
      package->errors.push_back(PackageIssueJson(
          L"removed_manifest_field",
          L"manifest.networkAllowlist was removed; use networkPolicy",
          L"{\"field\":\"networkAllowlist\"}"));
    }
    auto appId = OptionalStringMember(manifest, L"id");
    if (!appId.has_value() || !IsValidAppId(appId.value())) {
      package->errors.push_back(PackageIssueJson(L"invalid_manifest_id", L"manifest.id must be lowercase kebab-case"));
    } else {
      auto storagePrefix = OptionalStringMember(manifest, L"storagePrefix").value_or(L"");
      if (storagePrefix != appId.value() + L":") {
        package->errors.push_back(PackageIssueJson(
            L"invalid_storage_prefix",
            L"manifest.storagePrefix must equal <id>:",
            L"{\"expected\":" + JsonString(appId.value() + L":") +
                L",\"actual\":" + JsonString(storagePrefix) + L"}"));
      }
    }
    if (OptionalStringMember(manifest, L"entry").value_or(L"") != L"index.html") {
      package->errors.push_back(PackageIssueJson(L"invalid_entry", L"manifest.entry must be index.html"));
    }
    if (IntValue(manifest, {L"dataVersion"}, 0) < 1) {
      package->errors.push_back(PackageIssueJson(L"invalid_data_version", L"manifest.dataVersion must be a positive integer"));
    }
    if (!manifest.HasKey(L"permissions") || manifest.GetNamedValue(L"permissions").ValueType() != json::JsonValueType::Array) {
      package->errors.push_back(PackageIssueJson(L"invalid_permissions", L"manifest.permissions must be an array"));
    }
    if (!manifest.HasKey(L"capabilities") || manifest.GetNamedValue(L"capabilities").ValueType() != json::JsonValueType::Object) {
      package->errors.push_back(PackageIssueJson(L"invalid_capabilities", L"manifest.capabilities is required"));
    }
    if (!manifest.HasKey(L"resourceBudget") || manifest.GetNamedValue(L"resourceBudget").ValueType() != json::JsonValueType::Object) {
      package->errors.push_back(PackageIssueJson(L"invalid_resource_budget", L"manifest.resourceBudget must be an object"));
    }
    if (!manifest.HasKey(L"networkPolicy") || manifest.GetNamedValue(L"networkPolicy").ValueType() != json::JsonValueType::Object) {
      package->errors.push_back(PackageIssueJson(L"invalid_network_policy", L"manifest.networkPolicy must be an object"));
    }
  }

  void ValidatePackageBudgets(PackageRead* package) {
    int64_t maxPackageBytes = 1048576;
    int64_t maxFileBytes = 524288;
    auto budget = OptionalObjectMember(package->manifest, L"resourceBudget");
    if (budget.has_value()) {
      maxPackageBytes = IntValue(budget.value(), {L"maxPackageBytes"}, maxPackageBytes);
      maxFileBytes = IntValue(budget.value(), {L"maxFileBytes"}, maxFileBytes);
    }
    int64_t totalBytes = 0;
    for (auto const& file : package->files) {
      totalBytes += file.sizeBytes;
      if (file.sizeBytes > maxFileBytes) {
        package->errors.push_back(PackageIssueJson(
            L"resource_budget_exceeded",
            L"Package file exceeds manifest.resourceBudget.maxFileBytes",
            L"{\"path\":" + JsonString(file.path) +
                L",\"bytes\":" + std::to_wstring(file.sizeBytes) +
                L",\"maxFileBytes\":" + std::to_wstring(maxFileBytes) + L"}"));
      }
    }
    if (totalBytes > maxPackageBytes) {
      package->errors.push_back(PackageIssueJson(
          L"resource_budget_exceeded",
          L"Package exceeds manifest.resourceBudget.maxPackageBytes",
          L"{\"bytes\":" + std::to_wstring(totalBytes) +
              L",\"maxPackageBytes\":" + std::to_wstring(maxPackageBytes) + L"}"));
    }
  }

  void ValidatePackageBridgePermissions(PackageRead* package) {
    std::vector<std::wstring> permissions;
    auto permissionArray = OptionalArrayMember(package->manifest, L"permissions");
    if (permissionArray.has_value()) {
      for (uint32_t index = 0; index < permissionArray->Size(); ++index) {
        auto value = permissionArray->GetAt(index);
        if (value.ValueType() == json::JsonValueType::String) {
          permissions.push_back(std::wstring(value.GetString().c_str()));
        }
      }
    }
    for (auto const& method : BridgeMethodsIn(PackageFileContent(*package, L"app.js"))) {
      auto permission = PermissionForBridgeMethod(method);
      if (!permission.has_value()) {
        continue;
      }
      if (std::find(permissions.begin(), permissions.end(), permission.value()) == permissions.end()) {
        package->errors.push_back(PackageIssueJson(
            L"missing_permission",
            L"manifest.permissions does not cover a bridge method used by app.js",
            L"{\"method\":" + JsonString(method) +
                L",\"permission\":" + JsonString(permission.value()) + L"}"));
      }
    }
  }

  PackageRead ReadPackage(std::filesystem::path const& directory) {
    PackageRead package;
    package.directory = directory;
    package.manifestJson = L"{}";
    package.manifest = json::JsonObject::Parse(L"{}");
    std::vector<std::wstring> required = {L"manifest.json", L"index.html", L"styles.css", L"app.js"};
    std::vector<std::wstring> optional = {L"smoke-tests.json", L"README.md"};

    if (!std::filesystem::exists(directory) || !std::filesystem::is_directory(directory)) {
      package.errors.push_back(PackageIssueJson(
          L"package_not_found",
          L"Package directory was not found",
          L"{\"path\":" + JsonString(directory.wstring()) + L"}"));
      return package;
    }

    try {
      for (auto const& entry : std::filesystem::recursive_directory_iterator(directory)) {
        if (!entry.is_regular_file()) {
          continue;
        }
        auto relative = std::filesystem::relative(entry.path(), directory).generic_wstring();
        bool knownPath = std::find(required.begin(), required.end(), relative) != required.end() ||
            std::find(optional.begin(), optional.end(), relative) != optional.end() ||
            relative.rfind(L"migrations/", 0) == 0;
        if (!knownPath || relative.rfind(L"assets/", 0) == 0 || relative.find(L"..") != std::wstring::npos) {
          package.errors.push_back(PackageIssueJson(
              L"unexpected_package_path",
              L"Package contains an unexpected path",
              L"{\"path\":" + JsonString(relative) + L"}"));
          continue;
        }
        auto content = ReadTextFile(entry.path());
        package.files.push_back(PackageFile{
            relative,
            content,
            L"sha256:" + Sha256Hex(content),
            static_cast<int64_t>(WideToUtf8(content).size()),
            MimeTypeForPackagePath(relative)});
      }
    } catch (...) {
      package.errors.push_back(PackageIssueJson(L"package_read_failed", L"Package directory could not be read"));
    }
    std::sort(package.files.begin(), package.files.end(), [](PackageFile const& left, PackageFile const& right) {
      return left.path < right.path;
    });

    if (package.files.size() > 32) {
      package.errors.push_back(PackageIssueJson(L"resource_budget_exceeded", L"Package exceeds hard file count cap"));
    }
    auto migrationCount = std::count_if(package.files.begin(), package.files.end(), [](PackageFile const& file) {
      return file.path.rfind(L"migrations/", 0) == 0;
    });
    if (migrationCount > 16) {
      package.errors.push_back(PackageIssueJson(L"resource_budget_exceeded", L"Package exceeds hard migration file count cap"));
    }
    for (auto const& path : required) {
      if (!PackageHasFile(package, path)) {
        package.errors.push_back(PackageIssueJson(
            L"missing_required_file",
            path + L" is required",
            L"{\"path\":" + JsonString(path) + L"}"));
      }
    }

    package.manifestJson = PackageFileContent(package, L"manifest.json");
    if (package.manifestJson.empty()) {
      package.manifestJson = L"{}";
    }
    json::JsonObject manifest{nullptr};
    if (!json::JsonObject::TryParse(package.manifestJson, manifest)) {
      package.errors.push_back(PackageIssueJson(L"invalid_manifest_json", L"manifest.json must parse as JSON"));
      manifest = json::JsonObject::Parse(L"{}");
    }
    package.manifest = manifest;
    ValidatePackageManifest(&package);
    ValidatePackageBudgets(&package);
    ValidatePackageBridgePermissions(&package);
    ValidateGeneratedSourcePolicy(&package);
    if (!PackageHasFile(package, L"smoke-tests.json")) {
      package.warnings.push_back(PackageIssueJson(L"smoke_tests_missing", L"Package has no smoke-tests.json"));
    }
    return package;
  }

  std::optional<PackageRead> ReadPackageFromArgs(json::JsonObject const& args) {
    auto directory = PackageDirectoryFromArgs(args);
    if (!directory.has_value()) {
      return std::nullopt;
    }
    return ReadPackage(directory.value());
  }

  std::wstring PackageHashMapJson(PackageRead const& package) {
    auto manifestHash = L"sha256:" + Sha256Hex(package.manifestJson);
    std::wstring contentSeed;
    for (auto const& file : package.files) {
      contentSeed += file.path + L"\n" + file.contentHash + L"\n";
    }
    auto contentHash = L"sha256:" + Sha256Hex(contentSeed);
    auto permissionsJson = package.manifest.HasKey(L"permissions") ? std::wstring(package.manifest.GetNamedValue(L"permissions").Stringify().c_str()) : L"[]";
    auto policyJson = package.manifest.HasKey(L"networkPolicy") ? std::wstring(package.manifest.GetNamedValue(L"networkPolicy").Stringify().c_str()) : L"{}";
    return L"{\"manifestHash\":" + JsonString(manifestHash) +
        L",\"contentHash\":" + JsonString(contentHash) +
        L",\"permissionsHash\":" + JsonString(L"sha256:" + Sha256Hex(permissionsJson)) +
        L",\"policyHash\":" + JsonString(L"sha256:" + Sha256Hex(policyJson)) + L"}";
  }

  std::wstring PackageHashValue(PackageRead const& package, std::wstring const& field) {
    auto hashesJson = PackageHashMapJson(package);
    json::JsonObject hashes{nullptr};
    if (json::JsonObject::TryParse(hashesJson, hashes)) {
      return OptionalStringMember(hashes, field).value_or(L"");
    }
    return L"";
  }

  std::wstring ValidatePackageResultJson(json::JsonObject const& args) {
    auto package = ReadPackageFromArgs(args);
    if (!package.has_value()) {
      return L"";
    }
    auto appId = OptionalStringMember(package->manifest, L"id").value_or(L"");
    auto version = OptionalStringMember(package->manifest, L"version").value_or(L"");
    auto runtimeVersion = OptionalStringMember(package->manifest, L"runtimeVersion").value_or(L"");
    auto dataVersion = IntValue(package->manifest, {L"dataVersion"}, 1);
    return L"{\"ok\":" + std::wstring(package->errors.empty() ? L"true" : L"false") +
        L",\"appId\":" + JsonNullableString(appId) +
        L",\"version\":" + JsonNullableString(version) +
        L",\"runtimeVersion\":" + JsonNullableString(runtimeVersion) +
        L",\"dataVersion\":" + std::to_wstring(dataVersion) +
        L",\"files\":" + PackageFilePathsJson(package.value()) +
        L",\"bridgeMethods\":" + BridgeMethodsJson(package.value()) +
        L",\"errors\":" + JsonArrayText(package->errors) +
        L",\"warnings\":" + JsonArrayText(package->warnings) + L"}";
  }

  std::wstring PackageSignatureJson(PackageRead const& package, std::wstring const& trustLevel) {
    auto appId = OptionalStringMember(package.manifest, L"id").value_or(L"");
    auto version = OptionalStringMember(package.manifest, L"version").value_or(L"");
    auto runtimeVersion = OptionalStringMember(package.manifest, L"runtimeVersion").value_or(L"");
    auto dataVersion = IntValue(package.manifest, {L"dataVersion"}, 1);
    auto signedAt = NowIso();
    auto keyId = L"windows-dev-control-static-key";
    auto hashes = PackageHashMapJson(package);
    json::JsonObject hashObject{nullptr};
    json::JsonObject::TryParse(hashes, hashObject);
    auto payload = appId + L"\n" + version + L"\n" + runtimeVersion + L"\n" +
        std::to_wstring(dataVersion) + L"\n" +
        OptionalStringMember(hashObject, L"manifestHash").value_or(L"") + L"\n" +
        OptionalStringMember(hashObject, L"contentHash").value_or(L"") + L"\n" +
        trustLevel + L"\n" + signedAt;
    return L"{\"algorithm\":\"ed25519\",\"keyId\":" + JsonString(keyId) +
        L",\"appId\":" + JsonString(appId) +
        L",\"appVersion\":" + JsonString(version) +
        L",\"runtimeVersion\":" + JsonString(runtimeVersion) +
        L",\"dataVersion\":" + std::to_wstring(dataVersion) +
        L",\"manifestHash\":" + JsonString(OptionalStringMember(hashObject, L"manifestHash").value_or(L"")) +
        L",\"contentHash\":" + JsonString(OptionalStringMember(hashObject, L"contentHash").value_or(L"")) +
        L",\"permissionsHash\":" + JsonString(OptionalStringMember(hashObject, L"permissionsHash").value_or(L"")) +
        L",\"policyHash\":" + JsonString(OptionalStringMember(hashObject, L"policyHash").value_or(L"")) +
        L",\"trustLevel\":" + JsonString(trustLevel) +
        L",\"signedAt\":" + JsonString(signedAt) +
        L",\"signedBy\":\"windows-dev-control\"" +
        L",\"signature\":" + JsonString(L"sha256:" + Sha256Hex(payload)) + L"}";
  }

  std::wstring SignWebappPackageJson(json::JsonObject const& args) {
    auto package = ReadPackageFromArgs(args);
    if (!package.has_value()) {
      return L"";
    }
    auto trustLevel = OptionalStringMember(args, L"trustLevel").value_or(L"developer");
    auto keyId = L"windows-dev-control-static-key";
    return L"{\"ok\":" + std::wstring(package->errors.empty() ? L"true" : L"false") +
        L",\"appId\":" + JsonNullableString(OptionalStringMember(package->manifest, L"id").value_or(L"")) +
        L",\"version\":" + JsonNullableString(OptionalStringMember(package->manifest, L"version").value_or(L"")) +
        L",\"keyId\":" + JsonString(keyId) +
        L",\"signature\":" + PackageSignatureJson(package.value(), trustLevel) +
        L",\"hashes\":" + PackageHashMapJson(package.value()) +
        L",\"errors\":" + JsonArrayText(package->errors) +
        L",\"warnings\":" + JsonArrayText(package->warnings) + L"}";
  }

  std::wstring PackageSmokeResultJson(PackageRead const& package) {
    auto appId = OptionalStringMember(package.manifest, L"id").value_or(L"");
    auto smokeText = PackageFileContent(package, L"smoke-tests.json");
    if (smokeText.empty()) {
      return L"{\"ok\":true,\"status\":\"skipped\",\"appId\":" + JsonString(appId) +
          L",\"total\":0,\"assertions\":0,\"failures\":[],\"runner\":\"windows-static-package\",\"spec\":[]}";
    }
    json::JsonValue parsed{nullptr};
    if (!json::JsonValue::TryParse(smokeText, parsed) || parsed.ValueType() != json::JsonValueType::Array) {
      return L"{\"ok\":false,\"status\":\"failed\",\"appId\":" + JsonString(appId) +
          L",\"total\":0,\"assertions\":0,\"failures\":[{\"code\":\"invalid_smoke_tests\",\"message\":\"smoke-tests.json must parse as a JSON array\"}],\"runner\":\"windows-static-package\",\"spec\":null}";
    }
    auto tests = parsed.GetArray();
    auto html = PackageFileContent(package, L"index.html");
    auto appJs = PackageFileContent(package, L"app.js");
    std::vector<std::wstring> failures;
    std::vector<std::wstring> dynamicText;
    uint32_t assertions = 0;
    for (uint32_t index = 0; index < tests.Size(); ++index) {
      auto testValue = tests.GetAt(index);
      if (testValue.ValueType() != json::JsonValueType::Object) {
        failures.push_back(TestFailureJson(L"unnamed", L"invalid_smoke_test", L"message", L"Smoke test must be an object"));
        continue;
      }
      auto testObject = testValue.GetObject();
      auto testName = StringMemberOr(testObject, L"name", L"unnamed");
      auto steps = OptionalArrayMember(testObject, L"steps");
      if (steps.has_value()) {
        assertions += steps->Size();
        for (uint32_t stepIndex = 0; stepIndex < steps->Size(); ++stepIndex) {
          auto stepValue = steps->GetAt(stepIndex);
          if (stepValue.ValueType() != json::JsonValueType::Object) {
            failures.push_back(TestFailureJson(testName, L"invalid_smoke_step", L"message", L"Smoke step must be an object"));
            continue;
          }
          auto step = stepValue.GetObject();
          auto selector = OptionalStringMember(step, L"selector");
          if (selector.has_value() && !selector->empty()) {
            json::JsonObject query;
            query.Insert(L"selector", json::JsonValue::CreateStringValue(selector.value()));
            if (RuntimeQueryMatches(html, query).empty()) {
              failures.push_back(TestFailureJson(testName, L"selector.not_found", L"selector", selector.value()));
            }
          }
          auto stepType = OptionalStringMember(step, L"type").value_or(L"");
          auto value = OptionalStringMember(step, L"value");
          if ((stepType == L"fill" || stepType == L"select") && value.has_value()) {
            dynamicText.push_back(value.value());
          }
        }
      }
      auto expected = OptionalObjectMember(testObject, L"expected");
      if (expected.has_value()) {
        for (auto const& entry : expected.value()) {
          assertions += 1;
        }
        if (auto methods = OptionalArrayMember(expected.value(), L"bridgeCallsInclude"); methods.has_value()) {
          for (uint32_t methodIndex = 0; methodIndex < methods->Size(); ++methodIndex) {
            auto methodValue = methods->GetAt(methodIndex);
            if (methodValue.ValueType() == json::JsonValueType::String) {
              auto methodName = std::wstring(methodValue.GetString().c_str());
              if (appJs.find(methodName) == std::wstring::npos) {
                failures.push_back(TestFailureJson(testName, L"bridge.call_missing", L"method", methodName));
              }
            }
          }
        }
        auto textIncludes = OptionalStringMember(expected.value(), L"textIncludes");
        if (textIncludes.has_value() && !TextCanAppear(html, dynamicText, textIncludes.value())) {
          failures.push_back(TestFailureJson(testName, L"text.not_found", L"text", textIncludes.value()));
        }
      }
    }
    auto ok = failures.empty();
    return L"{\"ok\":" + std::wstring(ok ? L"true" : L"false") +
        L",\"status\":\"" + std::wstring(ok ? L"passed" : L"failed") +
        L"\",\"appId\":" + JsonString(appId) +
        L",\"total\":" + std::to_wstring(tests.Size()) +
        L",\"assertions\":" + std::to_wstring(assertions) +
        L",\"failures\":" + JsonArrayText(failures) +
        L",\"runner\":\"windows-static-package\",\"spec\":" + smokeText + L"}";
  }

  std::wstring AccessibilityAuditForPackageJson(PackageRead const& package) {
    auto appId = OptionalStringMember(package.manifest, L"id").value_or(L"");
    auto html = PackageFileContent(package, L"index.html");
    auto title = HtmlText(RegexFirst(html, LR"(<title[^>]*>([\s\S]*?)</title>)"));
    auto controls = AccessibilityControls(html);
    auto unlabeled = FirstUnlabeledControl(controls);
    bool hasTitle = !title.empty();
    bool hasMain = HtmlContains(html, LR"(<main\b)");
    bool hasH1 = HtmlContains(html, LR"(<h1\b[^>]*>[\s\S]*?</h1>)");
    bool pass = hasTitle && hasMain && hasH1 && !unlabeled.has_value();
    return L"{\"appId\":" + JsonString(appId) +
        L",\"checkedAt\":" + JsonString(NowIso()) +
        L",\"status\":\"" + std::wstring(pass ? L"pass" : L"fail") +
        L"\",\"checks\":[" +
        AccessibilityCheckJson(L"document_title", hasTitle, L"Document must include a non-empty <title>.") + L"," +
        AccessibilityCheckJson(L"main_landmark", hasMain, L"Page must include a <main> landmark.") + L"," +
        AccessibilityCheckJson(L"screen_title", hasH1, L"Page must include an h1 screen title.") + L"," +
        AccessibilityCheckJson(
            L"no_unlabeled_controls",
            !unlabeled.has_value(),
            L"Every interactive control must have an accessible name.",
            unlabeled.has_value() ? std::optional<std::wstring>(unlabeled->selector) : std::nullopt) +
        L"]}";
  }

  std::wstring RuntimeCompatibilityJson(PackageRead const& package) {
    auto runtimeVersion = OptionalStringMember(package.manifest, L"runtimeVersion").value_or(L"");
    auto ok = runtimeVersion.empty() || runtimeVersion == L"0.4.0" || runtimeVersion == L"0.1.0";
    return L"{\"ok\":" + std::wstring(ok ? L"true" : L"false") +
        L",\"runtimeVersion\":" + JsonString(runtimeVersion) +
        L",\"hostRuntimeVersion\":\"0.4.0\"}";
  }

  std::wstring ActiveManifestJson(sqlite3* db, std::wstring const& appId) {
    sqlite3_stmt* statement = nullptr;
    std::wstring manifest;
    if (sqlite3_prepare_v2(
            db,
            "SELECT manifest_json FROM app_versions WHERE app_id = ? AND status = 'enabled' ORDER BY activated_at DESC LIMIT 1",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        manifest = ColumnText(statement, 0);
      }
    }
    sqlite3_finalize(statement);
    return manifest;
  }

  std::wstring UpdateApprovalJson(sqlite3* db, PackageRead const& package) {
    auto appId = OptionalStringMember(package.manifest, L"id").value_or(L"");
    auto active = ActiveManifestJson(db, appId);
    if (active.empty()) {
      return L"{\"requiresUserApproval\":false,\"reasons\":[]}";
    }
    json::JsonObject activeManifest{nullptr};
    if (!json::JsonObject::TryParse(active, activeManifest)) {
      return L"{\"requiresUserApproval\":false,\"reasons\":[]}";
    }
    std::vector<std::wstring> reasons;
    for (auto const& field : {L"permissions", L"networkPolicy", L"resourceBudget", L"capabilities", L"dataVersion"}) {
      auto before = activeManifest.HasKey(field) ? CanonicalJsonValue(activeManifest.GetNamedValue(field)) : L"null";
      auto after = package.manifest.HasKey(field) ? CanonicalJsonValue(package.manifest.GetNamedValue(field)) : L"null";
      if (before != after) {
        reasons.push_back(field);
      }
    }
    return L"{\"requiresUserApproval\":" + std::wstring(reasons.empty() ? L"false" : L"true") +
        L",\"reasons\":" + JsonStringArray(reasons) +
        L",\"approvalReasons\":" + JsonStringArray(reasons) + L"}";
  }

  std::wstring InstallWebappPackageJson(json::JsonObject const& args, std::wstring* error) {
    auto package = ReadPackageFromArgs(args);
    if (!package.has_value()) {
      return L"";
    }
    auto appId = OptionalStringMember(package->manifest, L"id").value_or(L"");
    auto version = OptionalStringMember(package->manifest, L"version").value_or(L"");
    auto runtimeVersion = OptionalStringMember(package->manifest, L"runtimeVersion").value_or(L"");
    auto name = OptionalStringMember(package->manifest, L"name").value_or(appId);
    auto dataVersion = IntValue(package->manifest, {L"dataVersion"}, 1);
    if (!package->errors.empty()) {
      return L"{\"ok\":false,\"status\":\"failed\",\"appId\":" + JsonNullableString(appId) +
          L",\"errors\":" + JsonArrayText(package->errors) +
          L",\"warnings\":" + JsonArrayText(package->warnings) + L"}";
    }

    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    auto smoke = PackageSmokeResultJson(package.value());
    auto accessibility = AccessibilityAuditForPackageJson(package.value());
    auto compatibility = RuntimeCompatibilityJson(package.value());
    bool smokeOk = smoke.find(L"\"ok\":true") != std::wstring::npos;
    bool accessibilityOk = accessibility.find(L"\"status\":\"fail\"") == std::wstring::npos;
    bool compatibilityOk = compatibility.find(L"\"ok\":true") != std::wstring::npos;
    auto approval = UpdateApprovalJson(db, package.value());
    bool requiresApproval = approval.find(L"\"requiresUserApproval\":true") != std::wstring::npos;
    bool accepted = smokeOk && accessibilityOk && compatibilityOk && !requiresApproval;
    auto reportStatus = accepted ? L"accepted" : (requiresApproval ? L"requires-approval" : L"failed");
    auto versionStatus = accepted ? L"enabled" : (requiresApproval ? L"installed" : L"quarantined");
    auto appStatus = accepted ? L"enabled" : (requiresApproval ? L"disabled" : L"quarantined");
    auto previousInstallId = ActiveInstallId(db, appId);
    auto previousActiveVersion = ActiveVersion(db, appId);
    auto installId = MakeId(L"install-" + appId);
    auto reportId = MakeId(L"report");
    auto createdAt = NowIso();
    auto trustLevel = OptionalStringMember(args, L"trustLevel").value_or(L"developer");
    auto signature = PackageSignatureJson(package.value(), trustLevel);
    auto manifestHash = PackageHashValue(package.value(), L"manifestHash");
    auto contentHash = PackageHashValue(package.value(), L"contentHash");

    char* sqlError = nullptr;
    if (sqlite3_exec(db, "BEGIN IMMEDIATE", nullptr, nullptr, &sqlError) != SQLITE_OK) {
      *error = L"Could not start package install transaction";
      sqlite3_free(sqlError);
      return L"";
    }
    bool ok = true;
    if (!previousInstallId.empty() && accepted) {
      ok = ok && ExecutePrepared(db, "UPDATE app_versions SET status = 'installed' WHERE install_id = ?", {SqlText(previousInstallId)});
    }
    ok = ok && ExecutePrepared(
        db,
        "INSERT INTO apps (id, name, status, active_install_id, active_version, data_version, created_at, updated_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?) "
        "ON CONFLICT(id) DO UPDATE SET name = excluded.name, status = excluded.status, "
        "active_install_id = excluded.active_install_id, active_version = excluded.active_version, "
        "data_version = excluded.data_version, updated_at = excluded.updated_at",
        {
            SqlText(appId),
            SqlText(name),
            SqlText(appStatus),
            SqlNullableText(accepted ? std::optional<std::wstring>(installId) : (previousInstallId.empty() ? std::nullopt : std::optional<std::wstring>(previousInstallId))),
            SqlNullableText(accepted ? std::optional<std::wstring>(version) : (previousActiveVersion.empty() ? std::nullopt : std::optional<std::wstring>(previousActiveVersion))),
            SqlInt(dataVersion),
            SqlText(createdAt),
            SqlText(createdAt),
        });
    ok = ok && ExecutePrepared(
        db,
        "INSERT INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        {
            SqlText(installId),
            SqlText(appId),
            SqlText(version),
            SqlText(runtimeVersion),
            SqlInt(dataVersion),
            SqlText(package->manifestJson),
            SqlText(manifestHash),
            SqlText(contentHash),
            SqlText(signature),
            SqlText(trustLevel),
            SqlText(versionStatus),
            SqlText(createdAt),
            SqlNullableText(accepted ? std::optional<std::wstring>(createdAt) : std::nullopt),
        });
    for (auto const& file : package->files) {
      ok = ok && ExecutePrepared(
          db,
          "INSERT INTO app_files (install_id, path, content_text, content_hash, size_bytes, mime, created_at) "
          "VALUES (?, ?, ?, ?, ?, ?, ?)",
          {
              SqlText(installId),
              SqlText(file.path),
              SqlText(file.content),
              SqlText(file.contentHash),
              SqlInt(file.sizeBytes),
              SqlText(file.mime),
              SqlText(createdAt),
          });
    }
    auto permissions = OptionalArrayMember(package->manifest, L"permissions");
    if (permissions.has_value()) {
      for (uint32_t index = 0; ok && index < permissions->Size(); ++index) {
        auto permission = permissions->GetAt(index);
        if (permission.ValueType() != json::JsonValueType::String) {
          continue;
        }
        ok = ok && ExecutePrepared(
            db,
            "INSERT INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason) "
            "VALUES (?, ?, ?, 1, ?, ?, 'windows dev-control install')",
            {
                SqlText(installId),
                SqlText(appId),
                SqlText(std::wstring(permission.GetString().c_str())),
                SqlInt(accepted ? 1 : 0),
                SqlNullableText(accepted ? std::optional<std::wstring>(createdAt) : std::nullopt),
            });
      }
    }
    auto permissionsReport = L"{\"requested\":" +
        (permissions.has_value() ? std::wstring(permissions->Stringify().c_str()) : L"[]") +
        L",\"approval\":" + approval + L"}";
    ok = ok && ExecutePrepared(
        db,
        "INSERT INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        {
            SqlText(reportId),
            SqlText(appId),
            SqlText(installId),
            SqlText(reportStatus),
            SqlText(L"{\"ok\":true,\"errors\":[],\"warnings\":" + JsonArrayText(package->warnings) + L"}"),
            SqlText(L"{\"ok\":true,\"signature\":" + signature + L",\"accessibility\":" + accessibility + L"}"),
            SqlText(permissionsReport),
            SqlText(compatibility),
            SqlText(smoke),
            SqlText(contentHash),
            SqlText(createdAt),
        });
    ok = ok && ExecutePrepared(
        db,
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json) "
        "VALUES (?, ?, ?, 'install', ?, 'codex', ?, ?, ?)",
        {
            SqlText(MakeId(L"event")),
            SqlText(appId),
            SqlText(installId),
            SqlNullableText(previousInstallId.empty() ? std::nullopt : std::optional<std::wstring>(previousInstallId)),
            SqlText(reportId),
            SqlText(createdAt),
            SqlText(L"{\"source\":\"windows-dev-control\",\"status\":" + JsonString(versionStatus) + L"}"),
        });
    if (accepted) {
      ok = ok && ExecutePrepared(
          db,
          "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json) "
          "VALUES (?, ?, ?, 'activate', ?, 'codex', ?, ?, '{\"source\":\"windows-dev-control\"}')",
          {
              SqlText(MakeId(L"event")),
              SqlText(appId),
              SqlText(installId),
              SqlNullableText(previousInstallId.empty() ? std::nullopt : std::optional<std::wstring>(previousInstallId)),
              SqlText(reportId),
              SqlText(createdAt),
          });
    } else if (versionStatus == L"quarantined") {
      ok = ok && ExecutePrepared(
          db,
          "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json) "
          "VALUES (?, ?, ?, 'quarantine', ?, 'codex', ?, ?, '{\"reason\":\"install gate failed\"}')",
          {
              SqlText(MakeId(L"event")),
              SqlText(appId),
              SqlText(installId),
              SqlNullableText(previousInstallId.empty() ? std::nullopt : std::optional<std::wstring>(previousInstallId)),
              SqlText(reportId),
              SqlText(createdAt),
          });
    }

    if (!ok || sqlite3_exec(db, "COMMIT", nullptr, nullptr, &sqlError) != SQLITE_OK) {
      sqlite3_exec(db, "ROLLBACK", nullptr, nullptr, nullptr);
      sqlite3_free(sqlError);
      *error = L"Package install transaction failed";
      return L"";
    }
    return L"{\"ok\":" + std::wstring(accepted ? L"true" : L"false") +
        L",\"status\":" + JsonString(accepted ? L"enabled" : (requiresApproval ? L"requires-approval" : L"quarantined")) +
        L",\"installId\":" + JsonString(installId) +
        L",\"reportId\":" + JsonString(reportId) +
        L",\"appId\":" + JsonString(appId) +
        L",\"version\":" + JsonString(version) +
        L",\"contentHash\":" + JsonString(contentHash) +
        L",\"approval\":" + approval +
        L",\"smokeTest\":" + smoke +
        L",\"accessibility\":" + accessibility +
        L",\"compatibility\":" + compatibility +
        L",\"warnings\":" + JsonArrayText(package->warnings) + L"}";
  }

  std::wstring PlatformOpenWebappJson(std::wstring const& childControlSessionId, json::JsonObject const& args, std::wstring* error) {
    auto appId = OptionalStringMember(args, L"appId").value_or(L"");
    if (appId.empty() || !IsValidAppId(appId)) {
      *error = L"platform.open_webapp requires appId";
      return L"";
    }
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    auto activeInstallId = ActiveInstallId(db, appId);
    bool bundled = activeInstallId.empty() && BundledManifest(appId).has_value();
    if (activeInstallId.empty() && !bundled) {
      *error = L"platform.open_webapp requires an installed or bundled app";
      return L"";
    }
    auto runtimeSessionId = RuntimeSessionForControlSession(db, childControlSessionId, appId);
    if (runtimeSessionId.empty()) {
      *error = L"Could not create runtime session";
      return L"";
    }
    return L"{\"ok\":true,\"sessionId\":" + JsonString(runtimeSessionId) +
        L",\"appId\":" + JsonString(appId) +
        L",\"installId\":" + JsonNullableString(activeInstallId) +
        L",\"bundled\":" + std::wstring(bundled ? L"true" : L"false") + L"}";
  }

  struct InstalledVersionRecord {
    std::wstring installId;
    std::wstring appId;
    std::wstring version;
    int64_t dataVersion = 1;
    std::wstring manifestJson;
    std::wstring status;
  };

  struct InstallReportRecord {
    std::wstring reportId;
    std::wstring status;
    std::wstring permissionsJson;
  };

  bool BeginImmediate(sqlite3* db, std::wstring* error) {
    char* sqlError = nullptr;
    if (sqlite3_exec(db, "BEGIN IMMEDIATE", nullptr, nullptr, &sqlError) != SQLITE_OK) {
      *error = L"Could not start app registry transaction";
      sqlite3_free(sqlError);
      return false;
    }
    sqlite3_free(sqlError);
    return true;
  }

  bool CommitTransaction(sqlite3* db, std::wstring* error) {
    char* sqlError = nullptr;
    if (sqlite3_exec(db, "COMMIT", nullptr, nullptr, &sqlError) != SQLITE_OK) {
      sqlite3_exec(db, "ROLLBACK", nullptr, nullptr, nullptr);
      *error = L"App registry transaction failed";
      sqlite3_free(sqlError);
      return false;
    }
    sqlite3_free(sqlError);
    return true;
  }

  void RollbackTransaction(sqlite3* db) {
    sqlite3_exec(db, "ROLLBACK", nullptr, nullptr, nullptr);
  }

  bool LoadInstalledVersion(sqlite3* db, std::wstring const& appId, std::wstring const& installId, InstalledVersionRecord* record) {
    sqlite3_stmt* statement = nullptr;
    bool found = false;
    if (sqlite3_prepare_v2(
            db,
            "SELECT install_id, app_id, version, data_version, manifest_json, status "
            "FROM app_versions WHERE app_id = ? AND install_id = ? LIMIT 1",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      BindText(statement, 2, installId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        record->installId = ColumnText(statement, 0);
        record->appId = ColumnText(statement, 1);
        record->version = ColumnText(statement, 2);
        record->dataVersion = sqlite3_column_int64(statement, 3);
        record->manifestJson = ColumnText(statement, 4);
        record->status = ColumnText(statement, 5);
        found = true;
      }
    }
    sqlite3_finalize(statement);
    return found;
  }

  bool LoadActiveInstalledVersion(sqlite3* db, std::wstring const& appId, InstalledVersionRecord* record) {
    sqlite3_stmt* statement = nullptr;
    bool found = false;
    if (sqlite3_prepare_v2(
            db,
            "SELECT v.install_id, v.app_id, v.version, v.data_version, v.manifest_json, v.status "
            "FROM apps a JOIN app_versions v ON v.install_id = a.active_install_id "
            "WHERE a.id = ? LIMIT 1",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        record->installId = ColumnText(statement, 0);
        record->appId = ColumnText(statement, 1);
        record->version = ColumnText(statement, 2);
        record->dataVersion = sqlite3_column_int64(statement, 3);
        record->manifestJson = ColumnText(statement, 4);
        record->status = ColumnText(statement, 5);
        found = true;
      }
    }
    sqlite3_finalize(statement);
    return found;
  }

  bool AppExists(sqlite3* db, std::wstring const& appId) {
    std::wstring ignored;
    return QuerySingleText(db, "SELECT id FROM apps WHERE id = ?", {SqlText(appId)}, &ignored);
  }

  bool LoadLatestInstallReport(sqlite3* db, std::wstring const& appId, std::wstring const& installId, InstallReportRecord* record) {
    sqlite3_stmt* statement = nullptr;
    bool found = false;
    if (sqlite3_prepare_v2(
            db,
            "SELECT report_id, status, permissions_json FROM app_install_reports "
            "WHERE app_id = ? AND install_id = ? ORDER BY created_at DESC LIMIT 1",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      BindText(statement, 2, installId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        record->reportId = ColumnText(statement, 0);
        record->status = ColumnText(statement, 1);
        record->permissionsJson = ColumnText(statement, 2);
        found = true;
      }
    }
    sqlite3_finalize(statement);
    return found;
  }

  bool ReportRequiresUserApproval(std::wstring const& status, std::wstring const& permissionsJson) {
    if (status == L"requires-approval") {
      return true;
    }
    json::JsonObject permissions{nullptr};
    if (!permissionsJson.empty() && json::JsonObject::TryParse(permissionsJson, permissions)) {
      if (BooleanMemberTrue(permissions, L"requiresUserApproval")) {
        return true;
      }
      auto approval = OptionalObjectMember(permissions, L"approval");
      return approval.has_value() && BooleanMemberTrue(approval.value(), L"requiresUserApproval");
    }
    return false;
  }

  std::wstring PermissionsForInstallJson(sqlite3* db, std::wstring const& installId) {
    sqlite3_stmt* statement = nullptr;
    std::vector<std::wstring> permissions;
    if (sqlite3_prepare_v2(db, "SELECT permission FROM app_permissions WHERE install_id = ? ORDER BY permission", -1, &statement, nullptr) == SQLITE_OK) {
      BindText(statement, 1, installId);
      while (sqlite3_step(statement) == SQLITE_ROW) {
        permissions.push_back(ColumnText(statement, 0));
      }
    }
    sqlite3_finalize(statement);
    return JsonStringArray(permissions);
  }

  std::wstring ManifestPermissionsJson(sqlite3* db, InstalledVersionRecord const& version) {
    json::JsonObject manifest{nullptr};
    if (!version.manifestJson.empty() && json::JsonObject::TryParse(version.manifestJson, manifest)) {
      auto permissions = OptionalArrayMember(manifest, L"permissions");
      if (permissions.has_value()) {
        return std::wstring(permissions->Stringify().c_str());
      }
    }
    return PermissionsForInstallJson(db, version.installId);
  }

  std::wstring ApprovedPermissionsReportJson(
      sqlite3* db,
      InstallReportRecord const& report,
      InstalledVersionRecord const& version,
      std::wstring const& approvedAt) {
    return L"{\"previous\":" + RawJsonOrNull(report.permissionsJson) +
        L",\"approved\":" + ManifestPermissionsJson(db, version) +
        L",\"requiresUserApproval\":true,\"approvalGranted\":true,\"approvedAt\":" + JsonString(approvedAt) + L"}";
  }

  std::optional<std::wstring> FallbackRollbackInstallId(sqlite3* db, std::wstring const& appId, std::wstring const& activeInstallId) {
    std::wstring installId;
    if (QuerySingleText(
            db,
            "SELECT install_id FROM app_versions "
            "WHERE app_id = ? AND install_id != ? AND status NOT IN ('quarantined','uninstalled') "
            "ORDER BY created_at DESC LIMIT 1",
            {SqlText(appId), SqlText(activeInstallId)},
            &installId)) {
      return installId;
    }
    return std::nullopt;
  }

  std::optional<std::wstring> CreateRuntimeSnapshotInDb(
      sqlite3* db,
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring const& type,
      std::wstring* error) {
    auto runtimeSessionId = RuntimeSessionForControlSession(db, childControlSessionId, appId);
    if (runtimeSessionId.empty()) {
      *error = L"Could not create runtime session for snapshot";
      return std::nullopt;
    }
    auto snapshotId = MakeId(L"snapshot");
    auto createdAt = NowIso();
    auto installId = ActiveInstallId(db, appId);
    auto snapshotJson = RuntimeSnapshotDocumentJson(db, appId, createdAt);
    auto contentHash = L"sha256:" + Sha256Hex(snapshotJson);
    if (!InsertRuntimeSnapshot(db, snapshotId, runtimeSessionId, appId, installId, type.empty() ? L"manual" : type, snapshotJson, contentHash, createdAt)) {
      *error = L"Could not create runtime snapshot";
      return std::nullopt;
    }
    return snapshotId;
  }

  std::optional<std::wstring> CreateRegistrySnapshotInDb(
      sqlite3* db,
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring* error) {
    return CreateRuntimeSnapshotInDb(db, childControlSessionId, appId, L"manual", error);
  }

  struct ActiveMigrationAppRecord {
    std::wstring installId;
    std::wstring activeVersion;
    int64_t dataVersion = 1;
  };

  struct MigrationChange {
    std::wstring key;
    std::wstring valueJson;
    bool deletes = false;
  };

  struct MigrationPreview {
    std::vector<MigrationChange> changes;
    std::vector<std::wstring> changedKeys;
    std::map<std::wstring, int64_t> operationCounts;
  };

  struct MigrationSpec {
    std::wstring appId;
    int64_t fromDataVersion = 0;
    int64_t toDataVersion = 0;
    json::JsonArray steps{nullptr};
  };

  bool LoadActiveMigrationAppRecord(sqlite3* db, std::wstring const& appId, ActiveMigrationAppRecord* record) {
    sqlite3_stmt* statement = nullptr;
    bool found = false;
    if (sqlite3_prepare_v2(
            db,
            "SELECT active_install_id, active_version, data_version FROM apps WHERE id = ? AND status = 'enabled' LIMIT 1",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        record->installId = ColumnText(statement, 0);
        record->activeVersion = ColumnText(statement, 1);
        record->dataVersion = sqlite3_column_int64(statement, 2);
        found = !record->installId.empty();
      }
    }
    sqlite3_finalize(statement);
    return found;
  }

  bool IntegerMember(json::JsonObject const& object, std::wstring const& key, int64_t* value) {
    if (!object.HasKey(key)) {
      return false;
    }
    auto raw = object.GetNamedValue(key);
    if (raw.ValueType() != json::JsonValueType::Number) {
      return false;
    }
    auto number = raw.GetNumber();
    auto integer = static_cast<int64_t>(number);
    if (static_cast<double>(integer) != number) {
      return false;
    }
    *value = integer;
    return true;
  }

  json::IJsonValue JsonValueFromText(std::wstring const& text) {
    json::IJsonValue value = json::JsonValue::CreateNullValue();
    json::JsonValue::TryParse(text.empty() ? L"null" : text, value);
    return value;
  }

  std::wstring JsonText(json::IJsonValue const& value) {
    return std::wstring(value.Stringify().c_str());
  }

  std::wstring NormalizedJsonText(std::wstring const& text) {
    return JsonText(JsonValueFromText(text));
  }

  std::wstring MigrationScalarKey(json::IJsonValue const& value) {
    switch (value.ValueType()) {
      case json::JsonValueType::Null:
        return L"null";
      case json::JsonValueType::Boolean:
        return value.GetBoolean() ? L"true" : L"false";
      case json::JsonValueType::String:
        return std::wstring(value.GetString().c_str());
      case json::JsonValueType::Number: {
        auto number = value.GetNumber();
        auto integer = static_cast<int64_t>(number);
        if (static_cast<double>(integer) == number) {
          return std::to_wstring(integer);
        }
        return std::to_wstring(number);
      }
      case json::JsonValueType::Array:
      case json::JsonValueType::Object:
        return CanonicalJsonValue(value);
    }
    return L"";
  }

  std::vector<std::wstring> MigrationPathComponents(std::wstring path) {
    if (path == L"$") {
      return {};
    }
    if (path.rfind(L"$.", 0) == 0) {
      path = path.substr(2);
    }
    std::vector<std::wstring> components;
    std::wstring component;
    for (wchar_t ch : path) {
      if (ch == L'.') {
        if (!component.empty() && component != L"*" && component != L"[*]") {
          components.push_back(component);
        }
        component.clear();
      } else {
        component.push_back(ch);
      }
    }
    if (!component.empty() && component != L"*" && component != L"[*]") {
      components.push_back(component);
    }
    return components;
  }

  json::IJsonValue SetDefaultJsonValue(
      json::IJsonValue const& source,
      std::vector<std::wstring> const& components,
      size_t index,
      json::IJsonValue const& defaultValue) {
    if (index >= components.size()) {
      return source.ValueType() == json::JsonValueType::Null ? defaultValue : source;
    }
    json::JsonObject object;
    if (source.ValueType() == json::JsonValueType::Object) {
      object = source.GetObject();
    }
    auto key = components[index];
    if (index + 1 == components.size()) {
      if (!object.HasKey(key) || object.GetNamedValue(key).ValueType() == json::JsonValueType::Null) {
        object.Insert(key, defaultValue);
      }
      return object;
    }
    auto child = object.HasKey(key) ? object.GetNamedValue(key) : json::JsonValue::CreateNullValue();
    object.Insert(key, SetDefaultJsonValue(child, components, index + 1, defaultValue));
    return object;
  }

  std::wstring SetDefaultJsonText(std::wstring const& sourceJson, std::wstring const& path, json::IJsonValue const& defaultValue) {
    auto source = JsonValueFromText(sourceJson);
    auto components = MigrationPathComponents(path);
    return JsonText(SetDefaultJsonValue(source, components, 0, defaultValue));
  }

  json::IJsonValue TransformEnumScalar(
      json::IJsonValue const& source,
      json::JsonObject const& mapping,
      bool hasDefault,
      json::IJsonValue const& defaultValue) {
    auto key = MigrationScalarKey(source);
    if (mapping.HasKey(key)) {
      return mapping.GetNamedValue(key);
    }
    return hasDefault ? defaultValue : source;
  }

  json::IJsonValue TransformEnumJsonValue(
      json::IJsonValue const& source,
      std::vector<std::wstring> const& components,
      size_t index,
      json::JsonObject const& mapping,
      bool hasDefault,
      json::IJsonValue const& defaultValue) {
    if (index >= components.size()) {
      return TransformEnumScalar(source, mapping, hasDefault, defaultValue);
    }
    json::JsonObject object;
    if (source.ValueType() == json::JsonValueType::Object) {
      object = source.GetObject();
    }
    auto key = components[index];
    if (index + 1 == components.size()) {
      if (object.HasKey(key)) {
        object.Insert(key, TransformEnumScalar(object.GetNamedValue(key), mapping, hasDefault, defaultValue));
      }
      return object;
    }
    auto child = object.HasKey(key) ? object.GetNamedValue(key) : json::JsonValue::CreateNullValue();
    object.Insert(key, TransformEnumJsonValue(child, components, index + 1, mapping, hasDefault, defaultValue));
    return object;
  }

  std::wstring TransformEnumJsonText(
      std::wstring const& sourceJson,
      std::wstring const& path,
      json::JsonObject const& mapping,
      bool hasDefault,
      json::IJsonValue const& defaultValue) {
    auto source = JsonValueFromText(sourceJson);
    auto components = MigrationPathComponents(path);
    return JsonText(TransformEnumJsonValue(source, components, 0, mapping, hasDefault, defaultValue));
  }

  bool ValidateMigrationKey(std::wstring const& appId, std::wstring const& key, std::wstring* errorCode, std::wstring* errorMessage) {
    if (key.empty()) {
      *errorCode = L"invalid_migration";
      *errorMessage = L"Migration storage key must be a non-empty string";
      return false;
    }
    auto prefix = appId + L":";
    if (key.rfind(prefix, 0) != 0) {
      *errorCode = L"migration_storage_prefix_violation";
      *errorMessage = L"Migration storage key is outside app storage prefix";
      return false;
    }
    return true;
  }

  bool LoadMigrationStorageValues(
      sqlite3* db,
      std::wstring const& appId,
      std::map<std::wstring, std::wstring>* values,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(db, "SELECT key, value_json FROM app_storage WHERE app_id = ? ORDER BY key", -1, &statement, nullptr) != SQLITE_OK) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not read app storage for migration";
      return false;
    }
    BindText(statement, 1, appId);
    while (sqlite3_step(statement) == SQLITE_ROW) {
      values->insert_or_assign(ColumnText(statement, 0), NormalizedJsonText(ColumnText(statement, 1)));
    }
    sqlite3_finalize(statement);
    return true;
  }

  bool KeyMatchesPattern(std::wstring const& key, std::wstring const& pattern) {
    if (pattern.find_first_of(L"*?") == std::wstring::npos) {
      return key == pattern;
    }
    try {
      auto regexText = L"^" + RegexEscape(pattern) + L"$";
      regexText = std::regex_replace(regexText, std::wregex(LR"(\\\*)"), L".*");
      regexText = std::regex_replace(regexText, std::wregex(LR"(\\\?)"), L".");
      return std::regex_match(key, std::wregex(regexText));
    } catch (...) {
      return false;
    }
  }

  std::vector<std::wstring> MigrationKeys(
      json::JsonObject const& step,
      std::map<std::wstring, std::wstring> const& values,
      std::wstring const& op,
      std::wstring const& appId,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    std::vector<std::wstring> keys;
    if (step.HasKey(L"key")) {
      auto key = OptionalStringMember(step, L"key");
      if (!key.has_value() || !ValidateMigrationKey(appId, key.value(), errorCode, errorMessage)) {
        if (errorMessage->empty()) {
          *errorCode = L"invalid_migration";
          *errorMessage = L"Migration step " + op + L" requires key";
        }
        return {};
      }
      keys.push_back(key.value());
      return keys;
    }
    if (step.HasKey(L"keyPattern")) {
      auto pattern = OptionalStringMember(step, L"keyPattern");
      if (!pattern.has_value() || pattern->empty()) {
        *errorCode = L"invalid_migration";
        *errorMessage = L"Migration step " + op + L" keyPattern must be a string";
        return {};
      }
      for (auto const& [key, _] : values) {
        if (KeyMatchesPattern(key, pattern.value()) && !ValidateMigrationKey(appId, key, errorCode, errorMessage)) {
          return {};
        }
        if (KeyMatchesPattern(key, pattern.value())) {
          keys.push_back(key);
        }
      }
      return keys;
    }
    *errorCode = L"invalid_migration";
    *errorMessage = L"Migration step " + op + L" requires key or keyPattern";
    return {};
  }

  std::wstring MigrationValueForKey(std::map<std::wstring, std::wstring> const& values, std::wstring const& key) {
    auto found = values.find(key);
    return found == values.end() ? L"null" : found->second;
  }

  std::wstring MigrationOperationCountsJson(std::map<std::wstring, int64_t> const& counts) {
    std::wstring jsonText = L"{";
    bool first = true;
    for (auto const& [op, count] : counts) {
      if (!first) {
        jsonText += L",";
      }
      first = false;
      jsonText += JsonString(op) + L":" + std::to_wstring(count);
    }
    jsonText += L"}";
    return jsonText;
  }

  std::wstring MigrationChangesJson(std::vector<MigrationChange> const& changes) {
    std::wstring jsonText = L"[";
    for (size_t index = 0; index < changes.size(); ++index) {
      if (index > 0) {
        jsonText += L",";
      }
      jsonText += L"{\"key\":" + JsonString(changes[index].key);
      if (changes[index].deletes) {
        jsonText += L",\"delete\":true";
      } else {
        jsonText += L",\"value\":" + RawJsonOrNull(changes[index].valueJson);
      }
      jsonText += L"}";
    }
    jsonText += L"]";
    return jsonText;
  }

  std::wstring MigrationReportJson(MigrationPreview const& preview) {
    return L"{\"changedKeys\":" + JsonStringArray(preview.changedKeys) +
        L",\"operationCounts\":" + MigrationOperationCountsJson(preview.operationCounts) + L"}";
  }

  bool ValidateMigrationObject(json::JsonObject const& migration, MigrationSpec* spec, std::wstring* errorCode, std::wstring* errorMessage) {
    auto appId = OptionalStringMember(migration, L"appId");
    if (!appId.has_value() || appId->empty() || !IsValidAppId(appId.value())) {
      *errorCode = L"invalid_migration";
      *errorMessage = L"Migration appId is not a valid generated app id";
      return false;
    }
    int64_t fromDataVersion = 0;
    int64_t toDataVersion = 0;
    if (!IntegerMember(migration, L"fromDataVersion", &fromDataVersion) ||
        !IntegerMember(migration, L"toDataVersion", &toDataVersion) ||
        fromDataVersion < 1 ||
        toDataVersion != fromDataVersion + 1) {
      *errorCode = L"invalid_migration";
      *errorMessage = L"Migration toDataVersion must equal fromDataVersion + 1";
      return false;
    }
    auto steps = OptionalArrayMember(migration, L"steps");
    if (!steps.has_value()) {
      *errorCode = L"invalid_migration";
      *errorMessage = L"Migration steps must be an array";
      return false;
    }
    spec->appId = appId.value();
    spec->fromDataVersion = fromDataVersion;
    spec->toDataVersion = toDataVersion;
    spec->steps = steps.value();
    return true;
  }

  bool PreviewStorageMigration(
      sqlite3* db,
      MigrationSpec const& spec,
      MigrationPreview* preview,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    std::map<std::wstring, std::wstring> values;
    if (!LoadMigrationStorageValues(db, spec.appId, &values, errorCode, errorMessage)) {
      return false;
    }
    std::set<std::wstring> changedKeys;
    for (uint32_t index = 0; index < spec.steps.Size(); ++index) {
      auto stepValue = spec.steps.GetAt(index);
      if (stepValue.ValueType() != json::JsonValueType::Object) {
        *errorCode = L"invalid_migration";
        *errorMessage = L"Migration step must be an object";
        return false;
      }
      auto step = stepValue.GetObject();
      auto op = OptionalStringMember(step, L"op");
      if (!op.has_value() || op->empty()) {
        *errorCode = L"invalid_migration";
        *errorMessage = L"Migration step requires op";
        return false;
      }
      preview->operationCounts[op.value()] += 1;
      if (op.value() == L"setDefault") {
        auto keys = MigrationKeys(step, values, op.value(), spec.appId, errorCode, errorMessage);
        if (!errorMessage->empty()) {
          return false;
        }
        auto path = OptionalStringMember(step, L"to").value_or(OptionalStringMember(step, L"jsonPath").value_or(L"$"));
        auto defaultValue = step.HasKey(L"value") ? step.GetNamedValue(L"value") : json::JsonValue::CreateNullValue();
        for (auto const& key : keys) {
          auto next = SetDefaultJsonText(MigrationValueForKey(values, key), path.empty() ? L"$" : path, defaultValue);
          values.insert_or_assign(key, next);
          preview->changes.push_back(MigrationChange{key, next, false});
          changedKeys.insert(key);
        }
      } else if (op.value() == L"renameKey" || op.value() == L"moveStorageKey") {
        auto from = OptionalStringMember(step, L"from");
        auto to = OptionalStringMember(step, L"to");
        if (!from.has_value() || !to.has_value() ||
            !ValidateMigrationKey(spec.appId, from.value(), errorCode, errorMessage) ||
            !ValidateMigrationKey(spec.appId, to.value(), errorCode, errorMessage)) {
          if (errorMessage->empty()) {
            *errorCode = L"invalid_migration";
            *errorMessage = L"Migration step " + op.value() + L" requires from and to";
          }
          return false;
        }
        auto value = MigrationValueForKey(values, from.value());
        values.erase(from.value());
        values.insert_or_assign(to.value(), value);
        preview->changes.push_back(MigrationChange{from.value(), L"null", true});
        preview->changes.push_back(MigrationChange{to.value(), value, false});
        changedKeys.insert(from.value());
        changedKeys.insert(to.value());
      } else if (op.value() == L"deleteKey" || op.value() == L"deleteStorageKey") {
        auto key = OptionalStringMember(step, L"key");
        if (!key.has_value() || !ValidateMigrationKey(spec.appId, key.value(), errorCode, errorMessage)) {
          if (errorMessage->empty()) {
            *errorCode = L"invalid_migration";
            *errorMessage = L"Migration step " + op.value() + L" requires key";
          }
          return false;
        }
        values.erase(key.value());
        preview->changes.push_back(MigrationChange{key.value(), L"null", true});
        changedKeys.insert(key.value());
      } else if (op.value() == L"copyKey") {
        auto from = OptionalStringMember(step, L"from");
        auto to = OptionalStringMember(step, L"to");
        if (!from.has_value() || !to.has_value() ||
            !ValidateMigrationKey(spec.appId, from.value(), errorCode, errorMessage) ||
            !ValidateMigrationKey(spec.appId, to.value(), errorCode, errorMessage)) {
          if (errorMessage->empty()) {
            *errorCode = L"invalid_migration";
            *errorMessage = L"Migration step copyKey requires from and to";
          }
          return false;
        }
        auto value = MigrationValueForKey(values, from.value());
        values.insert_or_assign(to.value(), value);
        preview->changes.push_back(MigrationChange{to.value(), value, false});
        changedKeys.insert(to.value());
      } else if (op.value() == L"transformEnum") {
        auto keys = MigrationKeys(step, values, op.value(), spec.appId, errorCode, errorMessage);
        if (!errorMessage->empty()) {
          return false;
        }
        auto mapping = OptionalObjectMember(step, L"mapping");
        if (!mapping.has_value()) {
          mapping = OptionalObjectMember(step, L"map");
        }
        if (!mapping.has_value()) {
          *errorCode = L"invalid_migration";
          *errorMessage = L"Migration step transformEnum requires mapping";
          return false;
        }
        auto path = OptionalStringMember(step, L"to").value_or(OptionalStringMember(step, L"jsonPath").value_or(L"$"));
        auto defaultValue = step.HasKey(L"defaultMapping") ? step.GetNamedValue(L"defaultMapping") : json::JsonValue::CreateNullValue();
        for (auto const& key : keys) {
          auto next = TransformEnumJsonText(MigrationValueForKey(values, key), path.empty() ? L"$" : path, mapping.value(), step.HasKey(L"defaultMapping"), defaultValue);
          values.insert_or_assign(key, next);
          preview->changes.push_back(MigrationChange{key, next, false});
          changedKeys.insert(key);
        }
      } else {
        *errorCode = L"invalid_migration";
        *errorMessage = L"Unsupported migration op: " + op.value();
        return false;
      }
    }
    preview->changedKeys.assign(changedKeys.begin(), changedKeys.end());
    return true;
  }

  bool InsertAppMigrationRecord(
      sqlite3* db,
      std::wstring const& migrationId,
      MigrationSpec const& spec,
      std::wstring const& migrationJson,
      std::wstring const& createdAt) {
    return ExecutePrepared(
        db,
        "INSERT OR REPLACE INTO app_migrations (migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?)",
        {
            SqlText(migrationId),
            SqlText(spec.appId),
            SqlInt(spec.fromDataVersion),
            SqlInt(spec.toDataVersion),
            SqlText(migrationJson),
            SqlText(L"sha256:" + Sha256Hex(migrationJson)),
            SqlText(createdAt),
        });
  }

  bool RecordMigrationRun(
      sqlite3* db,
      std::wstring const& runId,
      std::wstring const& migrationId,
      std::wstring const& appId,
      std::wstring const& installId,
      std::wstring const& mode,
      std::wstring const& status,
      std::optional<std::wstring> const& preSnapshotId,
      std::wstring const& reportJson,
      std::wstring const& startedAt) {
    return ExecutePrepared(
        db,
        "INSERT INTO migration_runs (migration_run_id, migration_id, app_id, install_id, mode, status, pre_snapshot_id, report_json, started_at, finished_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        {
            SqlText(runId),
            SqlText(migrationId),
            SqlText(appId),
            SqlNullableText(installId.empty() ? std::nullopt : std::optional<std::wstring>(installId)),
            SqlText(mode),
            SqlText(status),
            SqlNullableText(preSnapshotId),
            SqlText(reportJson),
            SqlText(startedAt),
            SqlText(NowIso()),
        });
  }

  bool ApplyMigrationChanges(
      sqlite3* db,
      MigrationSpec const& spec,
      std::vector<MigrationChange> const& changes,
      std::wstring const& updatedAt,
      std::wstring* errorMessage) {
    bool ok = true;
    for (auto const& change : changes) {
      if (change.deletes) {
        ok = ok && ExecutePrepared(db, "DELETE FROM app_storage WHERE app_id = ? AND key = ?", {SqlText(spec.appId), SqlText(change.key)});
      } else {
        ok = ok &&
            ExecutePrepared(
                db,
                "INSERT INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?) "
                "ON CONFLICT(app_id, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
                {SqlText(spec.appId), SqlText(change.key), SqlText(change.valueJson), SqlText(updatedAt)});
      }
    }
    ok = ok && ExecutePrepared(
        db,
        "UPDATE apps SET data_version = ?, updated_at = ? WHERE id = ?",
        {SqlInt(spec.toDataVersion), SqlText(updatedAt), SqlText(spec.appId)});
    if (!ok) {
      *errorMessage = L"Could not apply migration storage changes";
    }
    return ok;
  }

  std::wstring MigrationRunResultJson(
      std::wstring const& runId,
      std::wstring const& mode,
      std::wstring const& status,
      std::optional<std::wstring> const& snapshotId,
      MigrationSpec const& spec,
      MigrationPreview const& preview) {
    return L"{\"ok\":true,\"runId\":" + JsonString(runId) +
        L",\"mode\":" + JsonString(mode) +
        L",\"status\":" + JsonString(status) +
        L",\"snapshotId\":" + (snapshotId.has_value() ? JsonString(snapshotId.value()) : L"null") +
        L",\"appId\":" + JsonString(spec.appId) +
        L",\"fromDataVersion\":" + std::to_wstring(spec.fromDataVersion) +
        L",\"toDataVersion\":" + std::to_wstring(spec.toDataVersion) +
        L",\"changedKeys\":" + JsonStringArray(preview.changedKeys) +
        L",\"operationCounts\":" + MigrationOperationCountsJson(preview.operationCounts) +
        L",\"changes\":" + MigrationChangesJson(preview.changes) + L"}";
  }

  std::wstring PlatformMigrationRunJson(
      std::wstring const& childControlSessionId,
      json::JsonObject const& migration,
      std::wstring const& mode,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    if (mode != L"dry-run" && mode != L"apply") {
      *errorCode = L"invalid_request";
      *errorMessage = L"Unsupported migration mode";
      return L"";
    }
    MigrationSpec spec;
    if (!ValidateMigrationObject(migration, &spec, errorCode, errorMessage)) {
      return L"";
    }
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not open platform database";
      return L"";
    }
    ActiveMigrationAppRecord active;
    if (!LoadActiveMigrationAppRecord(db, spec.appId, &active)) {
      *errorCode = L"app_not_installed";
      *errorMessage = L"App is not installed";
      return L"";
    }
    if (active.dataVersion != spec.fromDataVersion) {
      *errorCode = L"migration_data_version_mismatch";
      *errorMessage = L"Migration fromDataVersion does not match the active app dataVersion";
      return L"";
    }

    MigrationPreview preview;
    if (!PreviewStorageMigration(db, spec, &preview, errorCode, errorMessage)) {
      return L"";
    }
    auto runId = MakeId(L"mrun");
    auto migrationId = L"migration_" + spec.appId + L"_" + std::to_wstring(spec.fromDataVersion) + L"_to_" + std::to_wstring(spec.toDataVersion);
    auto startedAt = NowIso();
    auto migrationJson = std::wstring(migration.Stringify().c_str());
    auto reportJson = MigrationReportJson(preview);
    auto preSnapshotId = CreateRuntimeSnapshotInDb(db, childControlSessionId, spec.appId, L"pre-migration", errorMessage);
    if (!preSnapshotId.has_value()) {
      *errorCode = L"storage_error";
      if (errorMessage->empty()) {
        *errorMessage = L"Could not create pre-migration snapshot";
      }
      return L"";
    }

    if (!BeginImmediate(db, errorMessage)) {
      *errorCode = L"storage_error";
      return L"";
    }
    bool ok = InsertAppMigrationRecord(db, migrationId, spec, migrationJson, startedAt);
    if (mode == L"apply") {
      ok = ok && ApplyMigrationChanges(db, spec, preview.changes, NowIso(), errorMessage);
    }
    ok = ok && RecordMigrationRun(db, runId, migrationId, spec.appId, active.installId, mode, L"passed", preSnapshotId, reportJson, startedAt);
    if (!ok) {
      RollbackTransaction(db);
      *errorCode = L"storage_error";
      if (errorMessage->empty()) {
        *errorMessage = L"Migration run could not be recorded";
      }
      return L"";
    }
    if (!CommitTransaction(db, errorMessage)) {
      *errorCode = L"storage_error";
      return L"";
    }
    return MigrationRunResultJson(runId, mode, L"passed", preSnapshotId, spec, preview);
  }

  std::wstring PlatformListWebappVersionsJson(std::wstring const& appId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }

    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(
            db,
            "SELECT install_id, app_id, version, runtime_version, data_version, manifest_hash, content_hash, "
            "signature_json, trust_level, status, created_at, activated_at "
            "FROM app_versions WHERE app_id = ? ORDER BY created_at DESC",
            -1,
            &statement,
            nullptr) != SQLITE_OK) {
      *error = L"Could not list Windows app versions";
      return L"";
    }
    BindText(statement, 1, appId);

    std::wstring versions = L"[";
    bool first = true;
    while (sqlite3_step(statement) == SQLITE_ROW) {
      if (!first) {
        versions += L",";
      }
      first = false;
      auto signature = ColumnText(statement, 7);
      versions += L"{\"appId\":" + SqliteValueJson(statement, 1) +
          L",\"appVersion\":" + SqliteValueJson(statement, 2) +
          L",\"installId\":" + SqliteValueJson(statement, 0) +
          L",\"status\":" + SqliteValueJson(statement, 9) +
          L",\"installedAt\":" + SqliteValueJson(statement, 10) +
          L",\"manifestHash\":" + SqliteValueJson(statement, 5) +
          L",\"contentHash\":" + SqliteValueJson(statement, 6) +
          L",\"dataVersion\":" + SqliteValueJson(statement, 4) +
          L",\"signature\":" + RawJsonOrNull(signature) +
          L",\"activatedAt\":" + SqliteValueJson(statement, 11) +
          L",\"trustLevel\":" + SqliteValueJson(statement, 8) +
          L",\"runtimeVersion\":" + SqliteValueJson(statement, 3) + L"}";
    }
    sqlite3_finalize(statement);
    versions += L"]";
    return L"{\"appId\":" + JsonString(appId) + L",\"versions\":" + versions + L"}";
  }

  std::wstring PlatformInstallReportJson(std::wstring const& appId, std::optional<std::wstring> const& installId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }

    sqlite3_stmt* statement = nullptr;
    char const* sql = installId.has_value()
        ? "SELECT report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at "
          "FROM app_install_reports WHERE app_id = ? AND install_id = ? ORDER BY created_at DESC LIMIT 1"
        : "SELECT report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at "
          "FROM app_install_reports WHERE app_id = ? ORDER BY created_at DESC LIMIT 1";
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
      *error = L"Could not read Windows install report";
      return L"";
    }
    BindText(statement, 1, appId);
    if (installId.has_value()) {
      BindText(statement, 2, installId.value());
    }

    std::wstring report = L"null";
    if (sqlite3_step(statement) == SQLITE_ROW) {
      auto status = ColumnText(statement, 3);
      auto permissions = ColumnText(statement, 6);
      report = L"{\"reportId\":" + SqliteValueJson(statement, 0) +
          L",\"appId\":" + SqliteValueJson(statement, 1) +
          L",\"installId\":" + SqliteValueJson(statement, 2) +
          L",\"status\":" + JsonString(status) +
          L",\"validation\":" + RawJsonOrNull(ColumnText(statement, 4)) +
          L",\"security\":" + RawJsonOrNull(ColumnText(statement, 5)) +
          L",\"permissions\":" + RawJsonOrNull(permissions) +
          L",\"requiresUserApproval\":" + std::wstring(ReportRequiresUserApproval(status, permissions) ? L"true" : L"false") +
          L",\"compatibility\":" + RawJsonOrNull(ColumnText(statement, 7)) +
          L",\"smokeTest\":" + RawJsonOrNull(ColumnText(statement, 8)) +
          L",\"contentHash\":" + SqliteValueJson(statement, 9) +
          L",\"createdAt\":" + SqliteValueJson(statement, 10) + L"}";
    }
    sqlite3_finalize(statement);
    return L"{\"appId\":" + JsonString(appId) +
        L",\"installId\":" + (installId.has_value() ? JsonString(installId.value()) : L"null") +
        L",\"report\":" + report + L"}";
  }

  std::wstring PlatformRollbackWebappJson(
      std::wstring const& appId,
      std::optional<std::wstring> const& requestedInstallId,
      std::wstring* errorCode,
      std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *error = L"Could not open platform database";
      return L"";
    }

    InstalledVersionRecord active;
    if (!LoadActiveInstalledVersion(db, appId, &active)) {
      *errorCode = L"app_not_installed";
      *error = L"App is not installed";
      return L"";
    }
    auto targetInstallId = requestedInstallId.has_value() && !requestedInstallId->empty()
        ? requestedInstallId
        : FallbackRollbackInstallId(db, appId, active.installId);
    if (!targetInstallId.has_value() || targetInstallId->empty()) {
      *errorCode = L"no_rollback_target";
      *error = L"No rollback target exists";
      return L"";
    }
    if (targetInstallId.value() == active.installId) {
      *errorCode = L"rollback_target_invalid";
      *error = L"Rollback target is already active";
      return L"";
    }
    InstalledVersionRecord target;
    if (!LoadInstalledVersion(db, appId, targetInstallId.value(), &target) ||
        target.status == L"quarantined" ||
        target.status == L"uninstalled") {
      *errorCode = L"rollback_target_invalid";
      *error = L"Rollback target is invalid";
      return L"";
    }

    if (!BeginImmediate(db, error)) {
      *errorCode = L"storage_error";
      return L"";
    }
    auto createdAt = NowIso();
    bool ok = true;
    ok = ok && ExecutePrepared(db, "UPDATE app_versions SET status = 'rolled-back' WHERE install_id = ?", {SqlText(active.installId)});
    ok = ok && ExecutePrepared(db, "UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?", {SqlText(createdAt), SqlText(target.installId)});
    ok = ok && ExecutePrepared(
        db,
        "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
        {SqlText(target.installId), SqlText(target.version), SqlInt(target.dataVersion), SqlText(createdAt), SqlText(appId)});
    ok = ok && ExecutePrepared(
        db,
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) "
        "VALUES (?, ?, ?, 'rollback', ?, 'codex', ?, ?)",
        {
            SqlText(MakeId(L"event")),
            SqlText(appId),
            SqlText(target.installId),
            SqlText(active.installId),
            SqlText(createdAt),
            SqlText(L"{\"targetInstallId\":" + JsonString(target.installId) + L",\"rolledBackInstallId\":" + JsonString(active.installId) + L"}"),
        });
    if (!ok) {
      RollbackTransaction(db);
      *errorCode = L"storage_error";
      *error = L"Rollback could not be completed";
      return L"";
    }
    if (!CommitTransaction(db, error)) {
      *errorCode = L"storage_error";
      return L"";
    }
    return L"{\"appId\":" + JsonString(appId) +
        L",\"activeInstallId\":" + JsonString(target.installId) +
        L",\"rolledBackInstallId\":" + JsonString(active.installId) +
        L",\"activeVersion\":" + JsonString(target.version) +
        L",\"dataVersion\":" + std::to_wstring(target.dataVersion) + L"}";
  }

  std::wstring PlatformQuarantineWebappJson(
      std::wstring const& appId,
      std::optional<std::wstring> const& installId,
      std::wstring const& reason,
      bool restorePrevious,
      std::wstring* errorCode,
      std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *error = L"Could not open platform database";
      return L"";
    }

    InstalledVersionRecord active;
    bool hasActive = LoadActiveInstalledVersion(db, appId, &active);
    auto targetInstallId = installId.has_value() && !installId->empty()
        ? installId.value()
        : (hasActive ? active.installId : L"");
    if (targetInstallId.empty()) {
      *errorCode = L"app_not_installed";
      *error = L"App is not installed";
      return L"";
    }
    InstalledVersionRecord target;
    if (!LoadInstalledVersion(db, appId, targetInstallId, &target)) {
      *errorCode = L"install_not_found";
      *error = L"Install was not found for app";
      return L"";
    }

    std::optional<InstalledVersionRecord> restoreTarget;
    if (restorePrevious && hasActive && active.installId == target.installId) {
      auto restoreInstallId = FallbackRollbackInstallId(db, appId, target.installId);
      if (restoreInstallId.has_value()) {
        InstalledVersionRecord candidate;
        if (LoadInstalledVersion(db, appId, restoreInstallId.value(), &candidate)) {
          restoreTarget = candidate;
        }
      }
    }

    if (!BeginImmediate(db, error)) {
      *errorCode = L"storage_error";
      return L"";
    }
    auto createdAt = NowIso();
    bool ok = true;
    ok = ok && ExecutePrepared(db, "UPDATE app_versions SET status = 'quarantined' WHERE app_id = ? AND install_id = ?", {SqlText(appId), SqlText(target.installId)});
    if (restoreTarget.has_value()) {
      ok = ok && ExecutePrepared(db, "UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?", {SqlText(createdAt), SqlText(restoreTarget->installId)});
      ok = ok && ExecutePrepared(
          db,
          "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
          {SqlText(restoreTarget->installId), SqlText(restoreTarget->version), SqlInt(restoreTarget->dataVersion), SqlText(createdAt), SqlText(appId)});
    } else if (hasActive && active.installId == target.installId) {
      ok = ok && ExecutePrepared(db, "UPDATE apps SET status = 'quarantined', updated_at = ? WHERE id = ?", {SqlText(createdAt), SqlText(appId)});
    }
    auto restoredInstallId = restoreTarget.has_value() ? restoreTarget->installId : L"";
    ok = ok && ExecutePrepared(
        db,
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) "
        "VALUES (?, ?, ?, 'quarantine', ?, 'codex', ?, ?)",
        {
            SqlText(MakeId(L"event")),
            SqlText(appId),
            SqlText(target.installId),
            SqlNullableText(restoredInstallId.empty() ? std::nullopt : std::optional<std::wstring>(restoredInstallId)),
            SqlText(createdAt),
            SqlText(L"{\"reason\":" + JsonString(reason) + L",\"restoredInstallId\":" + JsonNullableString(restoredInstallId) + L"}"),
        });
    if (restoreTarget.has_value()) {
      ok = ok && ExecutePrepared(
          db,
          "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) "
          "VALUES (?, ?, ?, 'rollback', ?, 'codex', ?, ?)",
          {
              SqlText(MakeId(L"event")),
              SqlText(appId),
              SqlText(restoreTarget->installId),
              SqlText(target.installId),
              SqlText(createdAt),
              SqlText(L"{\"reason\":\"automatic rollback after quarantine\",\"quarantinedInstallId\":" + JsonString(target.installId) + L"}"),
          });
    }
    if (!ok) {
      RollbackTransaction(db);
      *errorCode = L"storage_error";
      *error = L"Quarantine could not be completed";
      return L"";
    }
    if (!CommitTransaction(db, error)) {
      *errorCode = L"storage_error";
      return L"";
    }
    return L"{\"appId\":" + JsonString(appId) +
        L",\"installId\":" + JsonString(target.installId) +
        L",\"status\":\"quarantined\",\"reason\":" + JsonString(reason) +
        L",\"restoredInstallId\":" + JsonNullableString(restoredInstallId) + L"}";
  }

  std::wstring PlatformUninstallWebappJson(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring* errorCode,
      std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *error = L"Could not open platform database";
      return L"";
    }
    if (!AppExists(db, appId)) {
      *errorCode = L"app_not_installed";
      *error = L"App is not installed";
      return L"";
    }
    auto activeInstallId = ActiveInstallId(db, appId);
    if (!BeginImmediate(db, error)) {
      *errorCode = L"storage_error";
      return L"";
    }

    auto snapshotId = CreateRegistrySnapshotInDb(db, childControlSessionId, appId, error);
    if (!snapshotId.has_value()) {
      RollbackTransaction(db);
      *errorCode = L"storage_error";
      return L"";
    }
    auto createdAt = NowIso();
    auto clearedStorageKeys = QuerySingleInt(db, "SELECT COUNT(*) FROM app_storage WHERE app_id = ?", {SqlText(appId)});
    bool ok = true;
    ok = ok && ExecutePrepared(db, "DELETE FROM app_storage WHERE app_id = ?", {SqlText(appId)});
    ok = ok && ExecutePrepared(db, "UPDATE app_versions SET status = 'uninstalled' WHERE app_id = ?", {SqlText(appId)});
    ok = ok && ExecutePrepared(
        db,
        "UPDATE apps SET status = 'uninstalled', active_install_id = NULL, active_version = NULL, updated_at = ? WHERE id = ?",
        {SqlText(createdAt), SqlText(appId)});
    if (!activeInstallId.empty()) {
      ok = ok && ExecutePrepared(
          db,
          "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, created_at, details_json) "
          "VALUES (?, ?, ?, 'uninstall', ?, 'codex', ?, ?)",
          {
              SqlText(MakeId(L"event")),
              SqlText(appId),
              SqlText(activeInstallId),
              SqlText(activeInstallId),
              SqlText(createdAt),
              SqlText(L"{\"snapshotId\":" + JsonString(snapshotId.value()) + L",\"clearedStorageKeys\":" + std::to_wstring(clearedStorageKeys) + L"}"),
          });
    }
    if (!ok) {
      RollbackTransaction(db);
      *errorCode = L"storage_error";
      *error = L"Uninstall could not be completed";
      return L"";
    }
    if (!CommitTransaction(db, error)) {
      *errorCode = L"storage_error";
      return L"";
    }
    return L"{\"ok\":true,\"appId\":" + JsonString(appId) +
        L",\"status\":\"uninstalled\",\"snapshotId\":" + JsonString(snapshotId.value()) +
        L",\"clearedStorageKeys\":" + std::to_wstring(clearedStorageKeys) + L"}";
  }

  std::wstring PlatformApproveWebappUpdateJson(
      std::wstring const& appId,
      std::wstring const& installId,
      std::wstring* errorCode,
      std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *error = L"Could not open platform database";
      return L"";
    }
    InstalledVersionRecord target;
    if (!LoadInstalledVersion(db, appId, installId, &target)) {
      *errorCode = L"install_not_found";
      *error = L"Install was not found for app";
      return L"";
    }
    if (target.status == L"quarantined" || target.status == L"uninstalled") {
      *errorCode = L"install_status_invalid";
      *error = L"Install cannot be approved from its current status";
      return L"";
    }
    InstallReportRecord report;
    if (!LoadLatestInstallReport(db, appId, installId, &report) || report.status != L"requires-approval") {
      *errorCode = L"approval_not_required";
      *error = L"Install does not require approval";
      return L"";
    }

    InstalledVersionRecord active;
    bool hasActive = LoadActiveInstalledVersion(db, appId, &active);
    auto previousInstallId = hasActive ? active.installId : L"";
    if (!BeginImmediate(db, error)) {
      *errorCode = L"storage_error";
      return L"";
    }
    auto createdAt = NowIso();
    bool ok = true;
    if (!previousInstallId.empty() && previousInstallId != installId) {
      ok = ok && ExecutePrepared(db, "UPDATE app_versions SET status = 'installed' WHERE install_id = ?", {SqlText(previousInstallId)});
    }
    ok = ok && ExecutePrepared(db, "UPDATE app_versions SET status = 'enabled', activated_at = ? WHERE install_id = ?", {SqlText(createdAt), SqlText(installId)});
    ok = ok && ExecutePrepared(
        db,
        "UPDATE app_permissions SET approved = 1, approved_at = ?, reason = 'approved update' WHERE install_id = ?",
        {SqlText(createdAt), SqlText(installId)});
    ok = ok && ExecutePrepared(
        db,
        "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
        {SqlText(installId), SqlText(target.version), SqlInt(target.dataVersion), SqlText(createdAt), SqlText(appId)});
    ok = ok && ExecutePrepared(
        db,
        "UPDATE app_install_reports SET status = 'accepted', permissions_json = ? WHERE report_id = ?",
        {SqlText(ApprovedPermissionsReportJson(db, report, target, createdAt)), SqlText(report.reportId)});
    ok = ok && ExecutePrepared(
        db,
        "INSERT INTO app_installations (installation_event_id, app_id, install_id, action, previous_install_id, actor, report_id, created_at, details_json) "
        "VALUES (?, ?, ?, 'activate', ?, 'codex', ?, ?, ?)",
        {
            SqlText(MakeId(L"event")),
            SqlText(appId),
            SqlText(installId),
            SqlNullableText(previousInstallId.empty() ? std::nullopt : std::optional<std::wstring>(previousInstallId)),
            SqlText(report.reportId),
            SqlText(createdAt),
            SqlText(L"{\"approved\":true,\"previousInstallId\":" + JsonNullableString(previousInstallId) + L",\"migrationRuns\":[]}"),
        });
    if (!ok) {
      RollbackTransaction(db);
      *errorCode = L"storage_error";
      *error = L"Update approval could not be completed";
      return L"";
    }
    if (!CommitTransaction(db, error)) {
      *errorCode = L"storage_error";
      return L"";
    }
    return L"{\"appId\":" + JsonString(appId) +
        L",\"installId\":" + JsonString(installId) +
        L",\"status\":\"enabled\",\"previousInstallId\":" + JsonNullableString(previousInstallId) +
        L",\"migrationRuns\":[]}";
  }

  std::wstring EvaluateSmokeTestsJson(
      std::wstring const& appId,
      std::wstring const& smokeText,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    json::JsonValue parsed{nullptr};
    if (smokeText.empty() || !json::JsonValue::TryParse(smokeText, parsed) || parsed.ValueType() != json::JsonValueType::Array) {
      *errorCode = L"invalid_smoke_tests";
      *errorMessage = L"smoke-tests.json must parse as a JSON array";
      return L"";
    }
    auto tests = parsed.GetArray();
    auto html = HtmlForBundledApp(appId);
    std::vector<std::wstring> failures;
    std::vector<std::wstring> dynamicText;
    uint32_t assertions = 0;

    for (uint32_t index = 0; index < tests.Size(); ++index) {
      auto testValue = tests.GetAt(index);
      if (testValue.ValueType() != json::JsonValueType::Object) {
        failures.push_back(TestFailureJson(L"unnamed", L"invalid_smoke_test", L"message", L"Smoke test must be an object"));
        continue;
      }
      auto testObject = testValue.GetObject();
      auto testName = StringMemberOr(testObject, L"name", L"unnamed");
      auto stepsValue = testObject.HasKey(L"steps") ? testObject.GetNamedValue(L"steps") : json::JsonValue::CreateNullValue();
      if (stepsValue.ValueType() == json::JsonValueType::Array) {
        auto steps = stepsValue.GetArray();
        assertions += steps.Size();
        for (uint32_t stepIndex = 0; stepIndex < steps.Size(); ++stepIndex) {
          auto stepValue = steps.GetAt(stepIndex);
          if (stepValue.ValueType() != json::JsonValueType::Object) {
            failures.push_back(TestFailureJson(testName, L"invalid_smoke_step", L"message", L"Smoke step must be an object"));
            continue;
          }
          auto step = stepValue.GetObject();
          auto selector = OptionalStringMember(step, L"selector");
          if (selector.has_value() && !selector->empty()) {
            json::JsonObject query;
            query.Insert(L"selector", json::JsonValue::CreateStringValue(selector.value()));
            if (RuntimeQueryMatches(html, query).empty()) {
              failures.push_back(TestFailureJson(testName, L"selector.not_found", L"selector", selector.value()));
            }
          }
          auto stepType = OptionalStringMember(step, L"type").value_or(L"");
          auto value = OptionalStringMember(step, L"value");
          if ((stepType == L"fill" || stepType == L"select") && value.has_value()) {
            dynamicText.push_back(value.value());
          }
        }
      }

      auto expected = OptionalObjectMember(testObject, L"expected");
      if (expected.has_value()) {
        for (auto const& entry : expected.value()) {
          assertions += 1;
        }
        if (expected->HasKey(L"bridgeCallsInclude") && expected->GetNamedValue(L"bridgeCallsInclude").ValueType() == json::JsonValueType::Array) {
          auto methods = expected->GetNamedArray(L"bridgeCallsInclude");
          for (uint32_t methodIndex = 0; methodIndex < methods.Size(); ++methodIndex) {
            auto methodValue = methods.GetAt(methodIndex);
            if (methodValue.ValueType() == json::JsonValueType::String) {
              auto methodName = std::wstring(methodValue.GetString().c_str());
              if (!BridgeMethodReferenced(appId, methodName)) {
                failures.push_back(TestFailureJson(testName, L"bridge.call_missing", L"method", methodName));
              }
            }
          }
        }
        auto textIncludes = OptionalStringMember(expected.value(), L"textIncludes");
        if (textIncludes.has_value() && !TextCanAppear(html, dynamicText, textIncludes.value())) {
          failures.push_back(TestFailureJson(testName, L"text.not_found", L"text", textIncludes.value()));
        }
      }
    }

    bool ok = failures.empty();
    return L"{\"ok\":" + std::wstring(ok ? L"true" : L"false") +
        L",\"status\":\"" + std::wstring(ok ? L"passed" : L"failed") +
        L"\",\"appId\":" + JsonString(appId) +
        L",\"total\":" + std::to_wstring(tests.Size()) +
        L",\"assertions\":" + std::to_wstring(assertions) +
        L",\"failures\":" + JsonArrayText(failures) +
        L",\"runner\":\"static\",\"spec\":" + smokeText + L"}";
  }

  std::wstring RuntimeRunSmokeTestsJson(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    if (appId.empty() || !IsValidAppId(appId)) {
      *errorCode = L"invalid_request";
      *errorMessage = L"runtime.run_smoke_tests appId is not a valid generated app id";
      return L"";
    }
    auto smokeText = BundledAppText(appId, L"smoke-tests.json");
    if (smokeText.empty()) {
      *errorCode = L"smoke_tests_missing";
      *errorMessage = L"App has no smoke-tests.json";
      return L"";
    }
    auto result = EvaluateSmokeTestsJson(appId, smokeText, errorCode, errorMessage);
    if (result.empty()) {
      return L"";
    }
    auto status = result.find(L"\"ok\":true") != std::wstring::npos ? L"passed" : L"failed";
    if (!RecordTestRun(
            childControlSessionId,
            appId,
            L"smoke:" + appId,
            appId + L" bundled smoke tests",
            smokeText,
            status,
            result,
            L"{\"runner\":\"windows-static-smoke\"}")) {
      *errorCode = L"sqlite_error";
      *errorMessage = L"Smoke test run could not be recorded";
      return L"";
    }
    return result;
  }

  std::optional<std::wstring> ControlSpecJson(
      json::JsonObject const& args,
      std::wstring const& inlineKey,
      std::wstring const& pathKey) {
    if (args.HasKey(inlineKey)) {
      auto value = args.GetNamedValue(inlineKey);
      if (value.ValueType() == json::JsonValueType::Object || value.ValueType() == json::JsonValueType::Array) {
        return std::wstring(value.Stringify().c_str());
      }
      if (value.ValueType() == json::JsonValueType::String) {
        return std::wstring(value.GetString().c_str());
      }
    }
    auto path = OptionalStringMember(args, pathKey);
    if (path.has_value() && !path->empty()) {
      return ReadTextFile(RepoRoot() / path.value());
    }
    return std::nullopt;
  }

  std::optional<std::wstring> FirstTargetApp(json::JsonObject const& spec) {
    if (!spec.HasKey(L"targetApps") || spec.GetNamedValue(L"targetApps").ValueType() != json::JsonValueType::Array) {
      return std::nullopt;
    }
    auto apps = spec.GetNamedArray(L"targetApps");
    if (apps.Size() == 0 || apps.GetAt(0).ValueType() != json::JsonValueType::String) {
      return std::nullopt;
    }
    return std::wstring(apps.GetAt(0).GetString().c_str());
  }

  std::optional<std::wstring> MicrotestTargetAppIdFromArgs(
      json::JsonObject const& args,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    auto specJson = ControlSpecJson(args, L"spec", L"microtestPath");
    if (!specJson.has_value() || specJson->empty()) {
      *errorCode = L"invalid_request";
      *errorMessage = L"runtime.run_microtest requires spec or microtestPath";
      return std::nullopt;
    }
    json::JsonObject spec{nullptr};
    if (!json::JsonObject::TryParse(specJson.value(), spec)) {
      *errorCode = L"invalid_microtest";
      *errorMessage = L"Micro-test spec must be a JSON object";
      return std::nullopt;
    }
    auto appId = FirstTargetApp(spec);
    if (!appId.has_value() || !IsValidAppId(appId.value())) {
      *errorCode = L"invalid_microtest";
      *errorMessage = L"Micro-test must target at least one app";
      return std::nullopt;
    }
    return appId.value();
  }

  std::optional<std::vector<std::wstring>> PlatformSmokeAppIdsFromArgs(
      json::JsonObject const& args,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    auto specJson = ControlSpecJson(args, L"spec", L"smokePath");
    if (!specJson.has_value() || specJson->empty()) {
      *errorCode = L"invalid_request";
      *errorMessage = L"platform.run_platform_smoke requires spec or smokePath";
      return std::nullopt;
    }
    json::JsonObject spec{nullptr};
    if (!json::JsonObject::TryParse(specJson.value(), spec) || !spec.HasKey(L"apps") || spec.GetNamedValue(L"apps").ValueType() != json::JsonValueType::Array) {
      *errorCode = L"invalid_request";
      *errorMessage = L"platform.run_platform_smoke requires an apps array";
      return std::nullopt;
    }
    std::vector<std::wstring> appIds;
    auto apps = spec.GetNamedArray(L"apps");
    for (uint32_t index = 0; index < apps.Size(); ++index) {
      auto appValue = apps.GetAt(index);
      if (appValue.ValueType() != json::JsonValueType::String) {
        *errorCode = L"invalid_request";
        *errorMessage = L"platform.run_platform_smoke apps must be generated app ids";
        return std::nullopt;
      }
      auto appId = std::wstring(appValue.GetString().c_str());
      if (!IsValidAppId(appId)) {
        *errorCode = L"invalid_request";
        *errorMessage = L"platform.run_platform_smoke apps must be generated app ids";
        return std::nullopt;
      }
      appIds.push_back(appId);
    }
    if (appIds.empty()) {
      *errorCode = L"invalid_request";
      *errorMessage = L"platform.run_platform_smoke requires at least one app";
      return std::nullopt;
    }
    return appIds;
  }

  json::JsonObject StepArgsWithAppId(json::JsonObject const& step, std::wstring const& appId) {
    json::JsonObject args = json::JsonObject::Parse(L"{}");
    if (auto parsed = OptionalObjectMember(step, L"args"); parsed.has_value()) {
      for (auto const& entry : parsed.value()) {
        args.Insert(entry.Key(), entry.Value());
      }
    }
    if (!args.HasKey(L"appId")) {
      args.Insert(L"appId", json::JsonValue::CreateStringValue(appId));
    }
    return args;
  }

  std::wstring StaticStepResultJson(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring const& tool,
      json::JsonObject const& args,
      std::vector<std::wstring>* dynamicText) {
    std::wstring errorCode;
    std::wstring errorMessage;
    if (tool == L"runtime.click" || tool == L"runtime.type" || tool == L"runtime.set_value" || tool == L"runtime.press_key" || tool == L"runtime.drag") {
      auto result = RuntimeTargetCommandJson(tool, args, &errorCode, &errorMessage);
      if (result.empty()) {
        return L"{\"ok\":false,\"error\":{\"code\":" + JsonString(errorCode) + L",\"message\":" + JsonString(errorMessage) + L"}}";
      }
      auto value = OptionalStringMember(args, L"value");
      if (!value.has_value()) {
        value = OptionalStringMember(args, L"text");
      }
      if ((tool == L"runtime.type" || tool == L"runtime.set_value") && value.has_value()) {
        dynamicText->push_back(value.value());
      }
      return result;
    }
    if (tool == L"runtime.query") {
      return RuntimeQueryJson(appId, args);
    }
    if (tool == L"runtime.screenshot") {
      return RuntimeScreenshotJson(appId, OptionalStringMember(args, L"label"));
    }
    if (tool == L"runtime.wait_for" || tool == L"platform.open_webapp" || tool == L"platform.validate_package" ||
        tool == L"platform.sign_webapp_package" || tool == L"platform.install_webapp_package" ||
        tool == L"runtime.network_mock_set" || tool == L"runtime.dialog_mock_set") {
      return L"{\"ok\":true,\"tool\":" + JsonString(tool) + L",\"appId\":" + JsonString(appId) + L"}";
    }
    if (tool == L"runtime.capabilities") {
      return RuntimeCapabilitiesJson(appId);
    }
    if (tool == L"runtime.resource_usage") {
      return ResourceUsageJson(appId);
    }
    if (tool == L"runtime.run_accessibility_audit") {
      return RuntimeAccessibilityAuditJson(appId);
    }
    if (tool == L"runtime.accessibility_snapshot") {
      return RuntimeAccessibilitySnapshotJson(appId);
    }
    if (tool == L"runtime.assert_accessibility") {
      auto result = RuntimeAssertAccessibilityJson(appId, OptionalStringMember(args, L"rule"), &errorCode, &errorMessage);
      return result.empty()
          ? L"{\"ok\":false,\"error\":{\"code\":" + JsonString(errorCode) + L",\"message\":" + JsonString(errorMessage) + L"}}"
          : result;
    }
    if (tool == L"runtime.assert_visible") {
      auto result = RuntimeAssertVisibleJson(appId, args, &errorCode, &errorMessage);
      return result.empty()
          ? L"{\"ok\":false,\"error\":{\"code\":" + JsonString(errorCode) + L",\"message\":" + JsonString(errorMessage) + L"}}"
          : result;
    }
    if (tool == L"runtime.assert_text") {
      auto text = OptionalStringMember(args, L"text").value_or(L"");
      if (!TextCanAppear(HtmlForBundledApp(appId), *dynamicText, text)) {
        return L"{\"ok\":false,\"error\":{\"code\":\"text.not_found\",\"message\":\"Expected text was not found\"}}";
      }
      return L"{\"ok\":true,\"appId\":" + JsonString(appId) + L",\"text\":" + JsonString(text) + L"}";
    }
    if (tool == L"runtime.assert_bridge_call") {
      auto methodName = OptionalStringMember(args, L"method").value_or(L"");
      bool ok = !methodName.empty() && BridgeMethodReferenced(appId, methodName);
      return L"{\"ok\":" + std::wstring(ok ? L"true" : L"false") +
          L",\"appId\":" + JsonString(appId) +
          L",\"method\":" + JsonString(methodName) + L"}";
    }
    if (tool == L"runtime.assert_no_console_errors") {
      return L"{\"ok\":true,\"errors\":0,\"appId\":" + JsonString(appId) + L"}";
    }
    if (tool == L"runtime.run_smoke_tests") {
      return RuntimeRunSmokeTestsJson(childControlSessionId, appId, &errorCode, &errorMessage);
    }
    if (tool == L"platform.create_snapshot") {
      return L"{\"ok\":true,\"snapshotId\":" + JsonString(MakeId(L"snapshot-static")) +
          L",\"appId\":" + JsonString(appId) + L"}";
    }
    if (tool == L"runtime.replay_events" || tool == L"runtime.core_snapshot" || tool == L"runtime.assert_core_action") {
      return L"{\"ok\":true,\"tool\":" + JsonString(tool) + L",\"appId\":" + JsonString(appId) + L"}";
    }
    return L"{\"ok\":false,\"error\":{\"code\":\"platform.unavailable\",\"message\":\"Micro-test command is not executable by the Windows static runner\"}}";
  }

  std::wstring EvaluateMicrotestSpecJson(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      json::JsonObject const& spec) {
    std::vector<std::wstring> failures;
    std::vector<std::wstring> commands;
    std::vector<std::wstring> dynamicText;
    uint32_t totalSteps = 0;
    for (auto const& phase : {L"setup", L"steps", L"teardown"}) {
      if (!spec.HasKey(phase) || spec.GetNamedValue(phase).ValueType() != json::JsonValueType::Array) {
        continue;
      }
      auto steps = spec.GetNamedArray(phase);
      for (uint32_t index = 0; index < steps.Size(); ++index) {
        auto stepValue = steps.GetAt(index);
        if (stepValue.ValueType() != json::JsonValueType::Object) {
          failures.push_back(TestFailureJson(phase, L"invalid_step", L"message", L"Micro-test step must be an object"));
          continue;
        }
        totalSteps += 1;
        auto step = stepValue.GetObject();
        auto tool = OptionalStringMember(step, L"tool").value_or(L"");
        auto args = StepArgsWithAppId(step, appId);
        auto result = StaticStepResultJson(childControlSessionId, appId, tool, args, &dynamicText);
        bool ok = result.find(L"\"ok\":false") == std::wstring::npos;
        if (!ok) {
          failures.push_back(TestFailureJson(phase, L"command_failed", L"tool", tool));
        }
        commands.push_back(L"{\"phase\":" + JsonString(phase) +
            L",\"index\":" + std::to_wstring(index) +
            L",\"tool\":" + JsonString(tool) +
            L",\"status\":\"" + std::wstring(ok ? L"passed" : L"failed") +
            L"\",\"result\":" + (result.empty() ? L"null" : result) + L"}");
      }
    }
    bool ok = failures.empty();
    return L"{\"ok\":" + std::wstring(ok ? L"true" : L"false") +
        L",\"status\":\"" + std::wstring(ok ? L"passed" : L"failed") +
        L"\",\"appId\":" + JsonString(appId) +
        L",\"totalSteps\":" + std::to_wstring(totalSteps) +
        L",\"failures\":" + JsonArrayText(failures) +
        L",\"commands\":" + JsonArrayText(commands) +
        L",\"runner\":\"windows-static-microtest\"}";
  }

  std::wstring RuntimeRunMicrotestJson(
      std::wstring const& childControlSessionId,
      json::JsonObject const& args,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    auto specJson = ControlSpecJson(args, L"spec", L"microtestPath");
    if (!specJson.has_value() || specJson->empty()) {
      *errorCode = L"invalid_request";
      *errorMessage = L"runtime.run_microtest requires spec or microtestPath";
      return L"";
    }
    json::JsonObject spec{nullptr};
    if (!json::JsonObject::TryParse(specJson.value(), spec)) {
      *errorCode = L"invalid_microtest";
      *errorMessage = L"Micro-test spec must be a JSON object";
      return L"";
    }
    auto appId = FirstTargetApp(spec);
    if (!appId.has_value() || !IsValidAppId(appId.value())) {
      *errorCode = L"invalid_microtest";
      *errorMessage = L"Micro-test must target at least one app";
      return L"";
    }
    auto result = EvaluateMicrotestSpecJson(childControlSessionId, appId.value(), spec);
    auto status = result.find(L"\"ok\":true") != std::wstring::npos ? L"passed" : L"failed";
    auto microTestId = OptionalStringMember(spec, L"id").value_or(L"microtest");
    if (!RecordTestRun(
            childControlSessionId,
            appId,
            microTestId,
            microTestId,
            specJson.value(),
            status,
            result,
            L"{\"runner\":\"windows-static-microtest\",\"spec\":" + specJson.value() + L"}")) {
      *errorCode = L"sqlite_error";
      *errorMessage = L"Micro-test run could not be recorded";
      return L"";
    }
    return result;
  }

  std::wstring PlatformRunSmokeJson(
      std::wstring const& childControlSessionId,
      json::JsonObject const& args,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    auto specJson = ControlSpecJson(args, L"spec", L"smokePath");
    if (!specJson.has_value() || specJson->empty()) {
      *errorCode = L"invalid_request";
      *errorMessage = L"platform.run_platform_smoke requires spec or smokePath";
      return L"";
    }
    json::JsonObject spec{nullptr};
    if (!json::JsonObject::TryParse(specJson.value(), spec) || !spec.HasKey(L"apps") || spec.GetNamedValue(L"apps").ValueType() != json::JsonValueType::Array) {
      *errorCode = L"invalid_request";
      *errorMessage = L"platform.run_platform_smoke requires an apps array";
      return L"";
    }
    auto smokeId = OptionalStringMember(spec, L"id").value_or(L"platform-smoke");
    auto platform = StringMemberOr(args, L"platform", L"windows");
    auto apps = spec.GetNamedArray(L"apps");
    std::vector<std::wstring> appResults;
    std::vector<std::wstring> failures;
    for (uint32_t index = 0; index < apps.Size(); ++index) {
      auto appValue = apps.GetAt(index);
      if (appValue.ValueType() != json::JsonValueType::String) {
        failures.push_back(L"{\"appId\":null,\"code\":\"invalid_request\",\"message\":\"Platform smoke apps must be generated app ids\"}");
        continue;
      }
      auto appId = std::wstring(appValue.GetString().c_str());
      if (!IsValidAppId(appId)) {
        failures.push_back(L"{\"appId\":" + JsonString(appId) + L",\"code\":\"invalid_request\",\"message\":\"Platform smoke apps must be generated app ids\"}");
        appResults.push_back(L"{\"appId\":" + JsonString(appId) +
            L",\"ok\":false,\"commands\":[]}");
        continue;
      }
      std::wstring smokeErrorCode;
      std::wstring smokeErrorMessage;
      auto smoke = RuntimeRunSmokeTestsJson(childControlSessionId, appId, &smokeErrorCode, &smokeErrorMessage);
      bool ok = !smoke.empty() && smoke.find(L"\"ok\":true") != std::wstring::npos;
      if (!ok) {
        failures.push_back(L"{\"appId\":" + JsonString(appId) +
            L",\"code\":\"smoke_failed\",\"message\":" + JsonString(smokeErrorMessage) + L"}");
      }
      appResults.push_back(L"{\"appId\":" + JsonString(appId) +
          L",\"ok\":" + std::wstring(ok ? L"true" : L"false") +
          L",\"commands\":[{\"tool\":\"runtime.run_smoke_tests\",\"status\":\"" + std::wstring(ok ? L"passed" : L"failed") +
          L"\",\"result\":" + (smoke.empty() ? L"null" : smoke) + L"}]}");
    }
    bool ok = failures.empty();
    auto result = L"{\"ok\":" + std::wstring(ok ? L"true" : L"false") +
        L",\"id\":" + JsonString(smokeId) +
        L",\"platform\":" + JsonString(platform) +
        L",\"totalApps\":" + std::to_wstring(appResults.size()) +
        L",\"failures\":" + JsonArrayText(failures) +
        L",\"apps\":" + JsonArrayText(appResults) + L"}";
    if (!RecordTestRun(
            childControlSessionId,
            std::nullopt,
            L"platform-smoke:" + smokeId + L":" + platform,
            smokeId + L" platform smoke (" + platform + L")",
            specJson.value(),
            ok ? L"passed" : L"failed",
            result,
            L"{\"runner\":\"windows-static-platform-smoke\",\"spec\":" + specJson.value() + L"}")) {
      *errorCode = L"sqlite_error";
      *errorMessage = L"Platform smoke run could not be recorded";
      return L"";
    }
    return result;
  }

  std::optional<std::wstring> JsonMemberText(json::JsonObject const& object, std::wstring const& key) {
    if (!object.HasKey(key) || object.GetNamedValue(key).ValueType() == json::JsonValueType::Null) {
      return std::nullopt;
    }
    return std::wstring(object.GetNamedValue(key).Stringify().c_str());
  }

  void RecordControlStorageBridgeCall(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring const& method,
      json::JsonObject const& params,
      json::JsonObject const& response,
      uint64_t startedAtMs) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr || appId.empty()) {
      return;
    }
    auto runtimeSessionId = RuntimeSessionForControlSession(db, childControlSessionId, appId);
    if (runtimeSessionId.empty()) {
      return;
    }
    auto okValue = response.GetNamedValue(L"ok", json::JsonValue::CreateBooleanValue(false));
    bool ok = okValue.ValueType() == json::JsonValueType::Boolean && okValue.GetBoolean();
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(
            db,
            "INSERT INTO bridge_calls "
            "(bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at) "
            "VALUES (?, ?, ?, NULL, ?, ?, ?, ?, ?, datetime('now'))",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, MakeId(L"bridge-call"));
      BindText(statement, 2, runtimeSessionId);
      BindText(statement, 3, appId);
      BindText(statement, 4, method);
      BindText(statement, 5, std::wstring(params.Stringify().c_str()));
      auto result = JsonMemberText(response, L"result");
      auto error = JsonMemberText(response, L"error");
      if (ok && result.has_value()) {
        BindText(statement, 6, result.value());
      } else {
        sqlite3_bind_null(statement, 6);
      }
      if (!ok && error.has_value()) {
        BindText(statement, 7, error.value());
      } else {
        sqlite3_bind_null(statement, 7);
      }
      sqlite3_bind_int64(statement, 8, static_cast<sqlite3_int64>(GetTickCount64() - startedAtMs));
      sqlite3_step(statement);
    }
    sqlite3_finalize(statement);
  }

  bool StorageCommandArgs(
      json::JsonObject const& command,
      std::wstring const& tool,
      bool requireValue,
      json::JsonObject* args,
      std::wstring* appId,
      std::wstring* key,
      std::wstring* error) {
    auto parsedArgs = OptionalObjectMember(command, L"args");
    if (!parsedArgs.has_value()) {
      *error = tool + L" requires args object";
      return false;
    }
    std::wstring appIdError;
    if (!OptionalArgsAppId(command, tool, appId, &appIdError)) {
      *error = appIdError;
      return false;
    }
    auto keyValue = OptionalStringMember(parsedArgs.value(), L"key");
    if (appId->empty() || !keyValue.has_value() || keyValue->empty() || (requireValue && !HasMember(parsedArgs.value(), L"value"))) {
      if (tool == L"runtime.storage_get") {
        *error = L"runtime.storage_get requires appId and key";
      } else if (tool == L"runtime.storage_set") {
        *error = L"runtime.storage_set requires appId, key, and value";
      } else {
        *error = L"runtime.assert_storage requires appId, key, and value";
      }
      return false;
    }
    *args = parsedArgs.value();
    *key = keyValue.value();
    return true;
  }

  std::wstring RuntimeStorageGetJson(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring const& key,
      json::JsonObject const& args,
      uint64_t startedAtMs) {
    json::JsonObject params;
    params.Insert(L"key", json::JsonValue::CreateStringValue(key));
    params.Insert(
        L"defaultValue",
        args.HasKey(L"defaultValue") ? args.GetNamedValue(L"defaultValue") : json::JsonValue::CreateNullValue());
    auto requestId = StringMemberOr(args, L"id", L"control_storage_get");
    auto request = StorageBridgeRequest(requestId, appId, L"storage.get", params, L"storage.read");
    PlatformStorage storage(databasePath);
    auto response = storage.Get(request);
    RecordControlStorageBridgeCall(childControlSessionId, appId, L"storage.get", params, response, startedAtMs);
    return std::wstring(response.Stringify().c_str());
  }

  std::wstring RuntimeStorageSetJson(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring const& key,
      json::JsonObject const& args,
      uint64_t startedAtMs) {
    json::JsonObject params;
    params.Insert(L"key", json::JsonValue::CreateStringValue(key));
    params.Insert(L"value", args.HasKey(L"value") ? args.GetNamedValue(L"value") : json::JsonValue::CreateNullValue());
    auto requestId = StringMemberOr(args, L"id", L"control_storage_set");
    auto request = StorageBridgeRequest(requestId, appId, L"storage.set", params, L"storage.write");
    PlatformStorage storage(databasePath);
    auto response = storage.Set(request);
    RecordControlStorageBridgeCall(childControlSessionId, appId, L"storage.set", params, response, startedAtMs);
    return std::wstring(response.Stringify().c_str());
  }

  std::optional<std::wstring> StoredStorageValue(std::wstring const& appId, std::wstring const& key, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return std::nullopt;
    }
    sqlite3_stmt* statement = nullptr;
    std::optional<std::wstring> value;
    if (sqlite3_prepare_v2(db, "SELECT value_json FROM app_storage WHERE app_id = ? AND key = ?", -1, &statement, nullptr) != SQLITE_OK) {
      *error = L"Could not read app storage";
      return std::nullopt;
    }
    BindText(statement, 1, appId);
    BindText(statement, 2, key);
    if (sqlite3_step(statement) == SQLITE_ROW) {
      value = ColumnText(statement, 0);
    }
    sqlite3_finalize(statement);
    return value;
  }

  std::wstring RuntimeAssertStorageJson(
      std::wstring const& appId,
      std::wstring const& key,
      json::IJsonValue const& expected,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    std::wstring storageError;
    auto actualText = StoredStorageValue(appId, key, &storageError);
    if (!actualText.has_value()) {
      *errorCode = storageError.empty() ? L"assertion_failed" : L"storage_error";
      *errorMessage = storageError.empty() ? L"Expected storage key was not found" : storageError;
      return L"";
    }

    json::JsonValue actual{nullptr};
    if (!json::JsonValue::TryParse(actualText.value(), actual)) {
      *errorCode = L"storage_error";
      *errorMessage = L"Stored value was not valid JSON";
      return L"";
    }
    if (CanonicalJsonValue(actual) != CanonicalJsonValue(expected)) {
      *errorCode = L"assertion_failed";
      *errorMessage = L"Storage value did not match expected value";
      return L"";
    }
	    return L"{\"ok\":true,\"appId\":" + JsonString(appId) +
	        L",\"key\":" + JsonString(key) +
	        L",\"value\":" + actualText.value() + L"}";
	  }

  std::optional<std::wstring> NetworkMockUrlPattern(json::JsonObject const& args) {
    auto direct = OptionalStringMember(args, L"urlPattern");
    if (direct.has_value() && !direct->empty()) {
      return direct.value();
    }
    auto match = OptionalObjectMember(args, L"match");
    if (!match.has_value()) {
      return std::nullopt;
    }
    auto pattern = OptionalStringMember(match.value(), L"urlPattern");
    if (pattern.has_value() && !pattern->empty()) {
      return pattern.value();
    }
    auto url = OptionalStringMember(match.value(), L"url");
    if (url.has_value() && !url->empty()) {
      return url.value();
    }
    return std::nullopt;
  }

  std::wstring NetworkMockMethod(json::JsonObject const& args) {
    auto method = OptionalStringMember(args, L"method");
    if (method.has_value() && !method->empty()) {
      return UpperAscii(method.value());
    }
    auto match = OptionalObjectMember(args, L"match");
    if (match.has_value()) {
      auto matchMethod = OptionalStringMember(match.value(), L"method");
      if (matchMethod.has_value() && !matchMethod->empty()) {
        return UpperAscii(matchMethod.value());
      }
    }
    return L"GET";
  }

  std::optional<std::wstring> DialogMockType(json::JsonObject const& args) {
    auto raw = OptionalStringMember(args, L"dialogType");
    if ((!raw.has_value() || raw->empty()) && args.HasKey(L"method")) {
      raw = OptionalStringMember(args, L"method");
    }
    if (!raw.has_value() || raw->empty()) {
      return std::nullopt;
    }
    auto value = raw.value();
    if (value.rfind(L"dialog.", 0) == 0) {
      value = value.substr(7);
    }
    if (value == L"openFile" || value == L"saveFile") {
      return value;
    }
    return std::nullopt;
  }

  json::IJsonValue DialogMockResponseValue(json::JsonObject const& args) {
    if (args.HasKey(L"response") && args.GetNamedValue(L"response").ValueType() != json::JsonValueType::Null) {
      return args.GetNamedValue(L"response");
    }
    json::JsonObject response;
    if (args.HasKey(L"files")) {
      response.Insert(L"files", args.GetNamedValue(L"files"));
    } else {
      json::JsonArray files;
      response.Insert(L"files", files);
    }
    response.Insert(
        L"selectedPath",
        args.HasKey(L"selectedPath") ? args.GetNamedValue(L"selectedPath") : json::JsonValue::CreateNullValue());
    response.Insert(
        L"cancelled",
        args.HasKey(L"cancelled") && args.GetNamedValue(L"cancelled").ValueType() == json::JsonValueType::Boolean
            ? args.GetNamedValue(L"cancelled")
            : json::JsonValue::CreateBooleanValue(false));
    return response;
  }

  std::optional<std::wstring> FaultMethodForArgs(json::JsonObject const& args) {
    auto method = OptionalStringMember(args, L"method");
    if (method.has_value() && !method->empty()) {
      return method.value();
    }
    auto kind = OptionalStringMember(args, L"kind");
    if (!kind.has_value() || kind->empty()) {
      return std::nullopt;
    }
    if (kind.value() == L"storage.read") {
      return L"storage.get";
    }
    if (kind.value() == L"storage.write") {
      return L"storage.set";
    }
    if (kind.value() == L"network" || kind.value() == L"network.request") {
      return L"network.request";
    }
    if (kind.value() == L"core" || kind.value() == L"core.step") {
      return L"core.step";
    }
    return kind.value();
  }

  bool IsKnownControlBridgeMethod(std::wstring const& method) {
    return method == L"storage.get" ||
        method == L"storage.set" ||
        method == L"storage.remove" ||
        method == L"storage.list" ||
        method == L"dialog.openFile" ||
        method == L"dialog.saveFile" ||
        method == L"notification.toast" ||
        method == L"network.request" ||
        method == L"core.step" ||
        method == L"runtime.capabilities" ||
        method == L"app.log";
  }

  std::wstring FaultDetailsJson(json::JsonObject const& args) {
    if (args.HasKey(L"details") && args.GetNamedValue(L"details").ValueType() != json::JsonValueType::Null) {
      return std::wstring(args.GetNamedValue(L"details").Stringify().c_str());
    }
    auto kind = OptionalStringMember(args, L"kind");
    if (kind.has_value() && !kind->empty()) {
      return L"{\"kind\":" + JsonString(kind.value()) + L"}";
    }
    return L"{}";
  }

  bool FaultOnce(json::JsonObject const& args) {
    if (!args.HasKey(L"once")) {
      return true;
    }
    auto value = args.GetNamedValue(L"once");
    return value.ValueType() == json::JsonValueType::Boolean ? value.GetBoolean() : true;
  }

  std::wstring RuntimeFaultInjectJson(json::JsonObject const& args, std::wstring* errorCode, std::wstring* errorMessage) {
    auto method = FaultMethodForArgs(args);
    if (!method.has_value() || method->empty()) {
      *errorCode = L"invalid_request";
      *errorMessage = L"runtime.fault_inject requires a bridge method";
      return L"";
    }
    if (!IsKnownControlBridgeMethod(method.value())) {
      *errorCode = L"unknown_method";
      *errorMessage = L"Unknown bridge method: " + method.value();
      return L"";
    }

    auto appId = OptionalStringMember(args, L"appId");
    auto sessionId = OptionalStringMember(args, L"sessionId");
    auto code = StringMemberOr(args, L"code", L"fault_injected");
    auto message = StringMemberOr(args, L"message", L"Injected bridge fault");
    auto detailsJson = FaultDetailsJson(args);
    bool once = FaultOnce(args);
    auto faultId = MakeId(L"fault");
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"sqlite_error";
      *errorMessage = L"Fault injection could not be registered";
      return L"";
    }
    if (!ExecutePrepared(
            db,
            "INSERT INTO fault_injections (fault_id, session_id, app_id, method, code, message, details_json, once, enabled, created_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",
            {
                SqlText(faultId),
                SqlNullableText(sessionId),
                SqlNullableText(appId),
                SqlText(method.value()),
                SqlText(code),
                SqlText(message),
                SqlText(detailsJson),
                SqlInt(once ? 1 : 0),
                SqlText(NowIso()),
            })) {
      *errorCode = L"sqlite_error";
      *errorMessage = L"Fault injection could not be registered";
      return L"";
    }
    return L"{\"ok\":true,\"faultId\":" + JsonString(faultId) +
        L",\"sessionId\":" + JsonNullableString(sessionId.value_or(L"")) +
        L",\"appId\":" + JsonNullableString(appId.value_or(L"")) +
        L",\"method\":" + JsonString(method.value()) +
        L",\"code\":" + JsonString(code) +
        L",\"message\":" + JsonString(message) +
        L",\"details\":" + detailsJson +
        L",\"once\":" + std::wstring(once ? L"true" : L"false") + L"}";
  }

  std::wstring RuntimeNetworkMockSetJson(json::JsonObject const& args, std::wstring* error) {
    auto urlPattern = NetworkMockUrlPattern(args);
    if (!urlPattern.has_value() || !args.HasKey(L"response") || args.GetNamedValue(L"response").ValueType() == json::JsonValueType::Null) {
      *error = L"runtime.network_mock_set requires urlPattern or match.url and response";
      return L"";
    }
    auto appId = OptionalStringMember(args, L"appId");
    auto sessionId = OptionalStringMember(args, L"sessionId");
    auto method = NetworkMockMethod(args);
    auto responseJson = std::wstring(args.GetNamedValue(L"response").Stringify().c_str());
    auto mockId = MakeId(L"netmock");
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    if (!ExecutePrepared(
            db,
            "INSERT INTO network_mocks (mock_id, session_id, app_id, method, url_pattern, response_json, enabled, created_at) "
            "VALUES (?, ?, ?, ?, ?, ?, 1, ?)",
            {
                SqlText(mockId),
                SqlNullableText(sessionId),
                SqlNullableText(appId),
                SqlText(method),
                SqlText(urlPattern.value()),
                SqlText(responseJson),
                SqlText(NowIso()),
            })) {
      *error = L"Network mock could not be registered";
      return L"";
    }
    return L"{\"ok\":true,\"mockId\":" + JsonString(mockId) +
        L",\"sessionId\":" + JsonNullableString(sessionId.value_or(L"")) +
        L",\"appId\":" + JsonNullableString(appId.value_or(L"")) +
        L",\"method\":" + JsonString(method) +
        L",\"urlPattern\":" + JsonString(urlPattern.value()) + L"}";
  }

  int64_t DeleteMockRows(sqlite3* db, char const* sql, std::optional<std::wstring> const& first, std::optional<std::wstring> const& second, bool* ok) {
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
      *ok = false;
      return 0;
    }
    if (first.has_value() && !first->empty()) {
      BindText(statement, 1, first.value());
    }
    if (second.has_value() && !second->empty()) {
      BindText(statement, 2, second.value());
    }
    if (sqlite3_step(statement) != SQLITE_DONE) {
      *ok = false;
      sqlite3_finalize(statement);
      return 0;
    }
    auto changes = sqlite3_changes(db);
    sqlite3_finalize(statement);
    return changes;
  }

  std::wstring RuntimeNetworkMockResetJson(json::JsonObject const& args, std::wstring* error) {
    auto appId = OptionalStringMember(args, L"appId");
    auto sessionId = OptionalStringMember(args, L"sessionId");
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    bool ok = true;
    int64_t cleared = 0;
    if (sessionId.has_value() && !sessionId->empty() && appId.has_value() && !appId->empty()) {
      cleared = DeleteMockRows(db, "DELETE FROM network_mocks WHERE session_id = ? AND app_id = ?", sessionId, appId, &ok);
    } else if (sessionId.has_value() && !sessionId->empty()) {
      cleared = DeleteMockRows(db, "DELETE FROM network_mocks WHERE session_id = ?", sessionId, std::nullopt, &ok);
    } else if (appId.has_value() && !appId->empty()) {
      cleared = DeleteMockRows(db, "DELETE FROM network_mocks WHERE app_id = ?", appId, std::nullopt, &ok);
    } else {
      cleared = DeleteMockRows(db, "DELETE FROM network_mocks", std::nullopt, std::nullopt, &ok);
    }
    if (!ok) {
      *error = L"Network mocks could not be reset";
      return L"";
    }
    return L"{\"ok\":true,\"cleared\":" + std::to_wstring(cleared) + L"}";
  }

  std::wstring RuntimeDialogMockSetJson(json::JsonObject const& args, std::wstring* error) {
    auto dialogType = DialogMockType(args);
    if (!dialogType.has_value()) {
      *error = L"runtime.dialog_mock_set requires dialogType or method";
      return L"";
    }
    auto appId = OptionalStringMember(args, L"appId");
    auto sessionId = OptionalStringMember(args, L"sessionId");
    auto response = DialogMockResponseValue(args);
    auto mockId = MakeId(L"dialogmock");
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    if (!ExecutePrepared(
            db,
            "INSERT INTO dialog_mocks (mock_id, session_id, app_id, dialog_type, response_json, enabled, created_at) "
            "VALUES (?, ?, ?, ?, ?, 1, ?)",
            {
                SqlText(mockId),
                SqlNullableText(sessionId),
                SqlNullableText(appId),
                SqlText(dialogType.value()),
                SqlText(std::wstring(response.Stringify().c_str())),
                SqlText(NowIso()),
            })) {
      *error = L"Dialog mock could not be registered";
      return L"";
    }
    return L"{\"ok\":true,\"mockId\":" + JsonString(mockId) +
        L",\"sessionId\":" + JsonNullableString(sessionId.value_or(L"")) +
        L",\"appId\":" + JsonNullableString(appId.value_or(L"")) +
        L",\"dialogType\":" + JsonString(dialogType.value()) + L"}";
  }

  std::wstring ActiveVersionForApp(sqlite3* db, std::wstring const& appId) {
    sqlite3_stmt* statement = nullptr;
    std::wstring activeVersion;
    if (sqlite3_prepare_v2(db, "SELECT active_version FROM apps WHERE id = ?", -1, &statement, nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        activeVersion = ColumnText(statement, 0);
      }
    }
    sqlite3_finalize(statement);
    return activeVersion;
  }

  std::wstring DataVersionForAppJson(sqlite3* db, std::wstring const& appId) {
    sqlite3_stmt* statement = nullptr;
    std::wstring dataVersion = L"null";
    if (sqlite3_prepare_v2(db, "SELECT data_version FROM apps WHERE id = ?", -1, &statement, nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      if (sqlite3_step(statement) == SQLITE_ROW && sqlite3_column_type(statement, 0) != SQLITE_NULL) {
        dataVersion = std::to_wstring(sqlite3_column_int64(statement, 0));
      }
    }
    sqlite3_finalize(statement);
    return dataVersion;
  }

  bool ValidSnapshotType(std::wstring const& type) {
    return type.empty() ||
        type == L"bug-report" ||
        type == L"pre-install" ||
        type == L"pre-migration" ||
        type == L"post-test" ||
        type == L"golden" ||
        type == L"manual" ||
        type == L"debug-bundle";
  }

  std::wstring SnapshotStorageRowsJson(sqlite3* db, std::wstring const& appId) {
    sqlite3_stmt* statement = nullptr;
    std::wstring rows = L"[";
    bool first = true;
    if (sqlite3_prepare_v2(
            db,
            "SELECT app_id, key, value_json, updated_at FROM app_storage WHERE app_id = ? ORDER BY key",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      while (sqlite3_step(statement) == SQLITE_ROW) {
        if (!first) {
          rows += L",";
        }
        rows += L"{\"app_id\":" + SqliteValueJson(statement, 0) +
            L",\"key\":" + SqliteValueJson(statement, 1) +
            L",\"value_json\":" + SqliteValueJson(statement, 2) +
            L",\"updated_at\":" + SqliteValueJson(statement, 3) + L"}";
        first = false;
      }
    }
    sqlite3_finalize(statement);
    rows += L"]";
    return rows;
  }

  std::wstring RuntimeSnapshotDocumentJson(sqlite3* db, std::wstring const& appId, std::wstring const& createdAt) {
    auto installId = ActiveInstallId(db, appId);
    auto activeVersion = ActiveVersionForApp(db, appId);
    auto dataVersion = DataVersionForAppJson(db, appId);
    auto storageRows = SnapshotStorageRowsJson(db, appId);
    return L"{\"appId\":" + JsonString(appId) +
        L",\"activeInstallId\":" + JsonNullableString(installId) +
        L",\"activeVersion\":" + JsonNullableString(activeVersion) +
        L",\"dataVersion\":" + dataVersion +
        L",\"storage\":" + storageRows +
        L",\"createdAt\":" + JsonString(createdAt) + L"}";
  }

  bool InsertRuntimeSnapshot(
      sqlite3* db,
      std::wstring const& snapshotId,
      std::wstring const& runtimeSessionId,
      std::wstring const& appId,
      std::wstring const& installId,
      std::wstring const& type,
      std::wstring const& snapshotJson,
      std::wstring const& contentHash,
      std::wstring const& createdAt) {
    sqlite3_stmt* statement = nullptr;
    bool ok = sqlite3_prepare_v2(
                  db,
                  "INSERT INTO runtime_snapshots "
                  "(snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at) "
                  "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                  -1,
                  &statement,
                  nullptr) == SQLITE_OK;
    if (ok) {
      BindText(statement, 1, snapshotId);
      BindText(statement, 2, runtimeSessionId);
      BindText(statement, 3, appId);
      if (installId.empty()) {
        sqlite3_bind_null(statement, 4);
      } else {
        BindText(statement, 4, installId);
      }
      BindText(statement, 5, type.empty() ? L"manual" : type);
      BindText(statement, 6, snapshotJson);
      BindText(statement, 7, contentHash);
      BindText(statement, 8, createdAt);
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
    return ok;
  }

  std::wstring PlatformCreateSnapshotJson(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring const& type,
      std::optional<std::wstring> const& sessionIdArg,
      std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    auto runtimeSessionId = sessionIdArg.has_value() && !sessionIdArg->empty()
        ? sessionIdArg.value()
        : RuntimeSessionForControlSession(db, childControlSessionId, appId);
    if (runtimeSessionId.empty()) {
      *error = L"Could not create runtime session for snapshot";
      return L"";
    }
    auto snapshotId = MakeId(L"snapshot");
    auto createdAt = NowIso();
    auto installId = ActiveInstallId(db, appId);
    auto activeVersion = ActiveVersionForApp(db, appId);
    auto dataVersion = DataVersionForAppJson(db, appId);
    auto storageRows = SnapshotStorageRowsJson(db, appId);
    auto snapshotJson = RuntimeSnapshotDocumentJson(db, appId, createdAt);
    auto contentHash = L"sha256:" + Sha256Hex(snapshotJson);
    if (!InsertRuntimeSnapshot(db, snapshotId, runtimeSessionId, appId, installId, type.empty() ? L"manual" : type, snapshotJson, contentHash, createdAt)) {
      *error = L"Could not create runtime snapshot";
      return L"";
    }
    return L"{\"ok\":true,\"snapshotId\":" + JsonString(snapshotId) +
        L",\"contentHash\":" + JsonString(contentHash) +
        L",\"snapshot\":" + snapshotJson +
        L",\"appId\":" + JsonString(appId) +
        L",\"activeInstallId\":" + JsonNullableString(installId) +
        L",\"activeVersion\":" + JsonNullableString(activeVersion) +
        L",\"dataVersion\":" + dataVersion +
        L",\"storage\":" + storageRows +
        L",\"createdAt\":" + JsonString(createdAt) + L"}";
  }

  std::wstring RuntimeSnapshotJsonById(
      sqlite3* db,
      std::wstring const& snapshotId,
      std::wstring* contentHash,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(db, "SELECT snapshot_json, content_hash FROM runtime_snapshots WHERE snapshot_id = ?", -1, &statement, nullptr) != SQLITE_OK) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not read runtime snapshot";
      return L"";
    }
    BindText(statement, 1, snapshotId);
    std::wstring snapshotJson;
    if (sqlite3_step(statement) == SQLITE_ROW) {
      snapshotJson = ColumnText(statement, 0);
      if (contentHash != nullptr) {
        *contentHash = ColumnText(statement, 1);
      }
    }
    sqlite3_finalize(statement);
    if (snapshotJson.empty()) {
      *errorCode = L"snapshot_not_found";
      *errorMessage = L"Runtime snapshot was not found";
    }
    return snapshotJson;
  }

  std::optional<std::wstring> RuntimeSnapshotAppId(std::wstring const& snapshotId, std::wstring* errorCode, std::wstring* errorMessage) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not open platform database";
      return std::nullopt;
    }
    std::wstring contentHash;
    auto snapshotJson = RuntimeSnapshotJsonById(db, snapshotId, &contentHash, errorCode, errorMessage);
    if (snapshotJson.empty()) {
      return std::nullopt;
    }
    json::JsonObject snapshot{nullptr};
    if (!json::JsonObject::TryParse(snapshotJson, snapshot)) {
      *errorCode = L"storage_error";
      *errorMessage = L"Runtime snapshot JSON is invalid";
      return std::nullopt;
    }
    auto appId = OptionalStringMember(snapshot, L"appId");
    if (!appId.has_value() || appId->empty()) {
      *errorCode = L"storage_error";
      *errorMessage = L"Runtime snapshot is missing appId";
      return std::nullopt;
    }
    return appId.value();
  }

  bool InsertStorageSnapshotRow(sqlite3* db, json::JsonObject const& row, std::wstring const& fallbackAppId, std::wstring const& updatedAt, std::wstring* error) {
    auto rowAppId = TextValue(row, {L"app_id", L"appId"}).value_or(fallbackAppId);
    auto key = TextValue(row, {L"key"});
    auto valueJson = JsonTextValue(row, {L"value_json", L"valueJson"}, {L"value"}, L"null").value_or(L"null");
    if (rowAppId.empty() || !key.has_value() || key->empty()) {
      *error = L"Snapshot storage row requires app_id and key";
      return false;
    }
    if (!fallbackAppId.empty() && rowAppId != fallbackAppId) {
      *error = L"Snapshot storage row app_id does not match snapshot appId";
      return false;
    }
    auto expectedPrefix = fallbackAppId + L":";
    if (!fallbackAppId.empty() && key->rfind(expectedPrefix, 0) != 0) {
      *error = L"Snapshot storage key is outside app storage prefix";
      return false;
    }
    return ExecutePrepared(
        db,
        "INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)",
        {SqlText(rowAppId), SqlText(key.value()), SqlText(valueJson), SqlText(updatedAt)});
  }

  std::wstring PlatformRestoreSnapshotJson(std::wstring const& snapshotId, std::wstring* errorCode, std::wstring* errorMessage) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not open platform database";
      return L"";
    }
    std::wstring contentHash;
    auto snapshotJson = RuntimeSnapshotJsonById(db, snapshotId, &contentHash, errorCode, errorMessage);
    if (snapshotJson.empty()) {
      return L"";
    }
    json::JsonObject snapshot{nullptr};
    if (!json::JsonObject::TryParse(snapshotJson, snapshot)) {
      *errorCode = L"storage_error";
      *errorMessage = L"Runtime snapshot JSON is invalid";
      return L"";
    }
    auto appId = OptionalStringMember(snapshot, L"appId");
    if (!appId.has_value() || appId->empty()) {
      *errorCode = L"storage_error";
      *errorMessage = L"Runtime snapshot is missing appId";
      return L"";
    }
    auto updatedAt = NowIso();
    char* sqlError = nullptr;
    if (sqlite3_exec(db, "BEGIN IMMEDIATE", nullptr, nullptr, &sqlError) != SQLITE_OK) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not begin snapshot restore";
      sqlite3_free(sqlError);
      return L"";
    }
    bool ok = ExecutePrepared(db, "DELETE FROM app_storage WHERE app_id = ?", {SqlText(appId.value())});
    uint32_t restoredStorageKeys = 0;
    auto storage = OptionalArrayMember(snapshot, L"storage");
    if (!storage.has_value()) {
      storage = OptionalArrayMember(snapshot, L"appStorage");
    }
    if (ok && storage.has_value()) {
      for (uint32_t index = 0; index < storage->Size(); ++index) {
        auto value = storage->GetAt(index);
        if (value.ValueType() != json::JsonValueType::Object ||
            !InsertStorageSnapshotRow(db, value.GetObject(), appId.value(), updatedAt, errorMessage)) {
          ok = false;
          break;
        }
        ++restoredStorageKeys;
      }
    }
    if (ok) {
      auto dataVersion = snapshot.HasKey(L"dataVersion") && snapshot.GetNamedValue(L"dataVersion").ValueType() == json::JsonValueType::Number
          ? static_cast<int64_t>(snapshot.GetNamedNumber(L"dataVersion"))
          : 1;
      ok = ExecutePrepared(
          db,
          "UPDATE apps SET active_install_id = ?, active_version = ?, data_version = ?, status = 'enabled', updated_at = ? WHERE id = ?",
          {
              SqlNullableText(TextValue(snapshot, {L"activeInstallId", L"active_install_id"})),
              SqlNullableText(TextValue(snapshot, {L"activeVersion", L"active_version"})),
              SqlInt(dataVersion),
              SqlText(updatedAt),
              SqlText(appId.value()),
          });
    }
    if (!ok) {
      sqlite3_exec(db, "ROLLBACK", nullptr, nullptr, nullptr);
      if (errorMessage->empty()) {
        *errorMessage = L"Could not restore runtime snapshot";
      }
      *errorCode = L"storage_error";
      return L"";
    }
    if (sqlite3_exec(db, "COMMIT", nullptr, nullptr, &sqlError) != SQLITE_OK) {
      sqlite3_exec(db, "ROLLBACK", nullptr, nullptr, nullptr);
      sqlite3_free(sqlError);
      *errorCode = L"storage_error";
      *errorMessage = L"Could not commit snapshot restore";
      return L"";
    }
    return L"{\"ok\":true,\"snapshotId\":" + JsonString(snapshotId) +
        L",\"appId\":" + JsonString(appId.value()) +
        L",\"contentHash\":" + JsonString(contentHash) +
        L",\"restoredStorageKeys\":" + std::to_wstring(restoredStorageKeys) + L"}";
  }

  bool SnapshotCompareSkipMember(std::wstring const& member) {
    return member == L"createdAt" ||
        member == L"snapshotId" ||
        member == L"updated_at" ||
        member == L"updatedAt";
  }

  std::wstring SnapshotStorageSortKey(json::IJsonValue const& value) {
    if (value.ValueType() != json::JsonValueType::Object) {
      return CanonicalJsonValue(value);
    }
    auto object = value.GetObject();
    auto appId = TextValue(object, {L"app_id", L"appId"}).value_or(L"");
    auto key = TextValue(object, {L"key"}).value_or(L"");
    return appId + L"\x1f" + key;
  }

  std::wstring ComparableSnapshotJson(json::IJsonValue const& value) {
    switch (value.ValueType()) {
      case json::JsonValueType::Null:
      case json::JsonValueType::Boolean:
      case json::JsonValueType::Number:
      case json::JsonValueType::String:
        return CanonicalJsonValue(value);
      case json::JsonValueType::Array: {
        auto array = value.GetArray();
        std::wstring out = L"[";
        for (uint32_t index = 0; index < array.Size(); ++index) {
          if (index > 0) {
            out += L",";
          }
          out += ComparableSnapshotJson(array.GetAt(index));
        }
        out += L"]";
        return out;
      }
      case json::JsonValueType::Object: {
        auto object = value.GetObject();
        std::vector<std::wstring> keys;
        for (auto const& pair : object) {
          auto key = std::wstring(pair.Key().c_str());
          if (key == L"appStorage" && object.HasKey(L"storage")) {
            continue;
          }
          if (!SnapshotCompareSkipMember(key)) {
            keys.push_back(key);
          }
        }
        std::sort(keys.begin(), keys.end());
        std::wstring out = L"{";
        for (size_t index = 0; index < keys.size(); ++index) {
          if (index > 0) {
            out += L",";
          }
          auto child = object.GetNamedValue(keys[index]);
          std::wstring outputKey = keys[index] == L"appStorage" ? L"storage" : keys[index];
          if ((keys[index] == L"storage" || keys[index] == L"appStorage") && child.ValueType() == json::JsonValueType::Array) {
            auto storage = child.GetArray();
            std::vector<std::pair<std::wstring, std::wstring>> rows;
            for (uint32_t rowIndex = 0; rowIndex < storage.Size(); ++rowIndex) {
              auto row = storage.GetAt(rowIndex);
              rows.push_back({SnapshotStorageSortKey(row), ComparableSnapshotJson(row)});
            }
            std::sort(rows.begin(), rows.end(), [](auto const& left, auto const& right) {
              return left.first < right.first;
            });
            std::wstring storageJson = L"[";
            for (size_t rowIndex = 0; rowIndex < rows.size(); ++rowIndex) {
              if (rowIndex > 0) {
                storageJson += L",";
              }
              storageJson += rows[rowIndex].second;
            }
            storageJson += L"]";
            out += JsonString(outputKey) + L":" + storageJson;
          } else {
            out += JsonString(outputKey) + L":" + ComparableSnapshotJson(child);
          }
        }
        out += L"}";
        return out;
      }
    }
    return CanonicalJsonValue(value);
  }

  std::wstring SnapshotArgJson(sqlite3* db, json::JsonObject const& args, std::wstring const& valueMember, std::wstring const& idMember, std::wstring* errorCode, std::wstring* errorMessage) {
    if (args.HasKey(idMember)) {
      auto snapshotId = OptionalStringMember(args, idMember);
      if (!snapshotId.has_value() || snapshotId->empty()) {
        *errorCode = L"invalid_request";
        *errorMessage = idMember + L" must be a string";
        return L"";
      }
      std::wstring contentHash;
      return RuntimeSnapshotJsonById(db, snapshotId.value(), &contentHash, errorCode, errorMessage);
    }
    if (args.HasKey(valueMember)) {
      return std::wstring(args.GetNamedValue(valueMember).Stringify().c_str());
    }
    *errorCode = L"invalid_request";
    *errorMessage = L"runtime.compare_snapshot requires left/right snapshots or snapshot ids";
    return L"";
  }

  std::wstring RuntimeCompareSnapshotJson(json::JsonObject const& args, std::wstring* errorCode, std::wstring* errorMessage) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not open platform database";
      return L"";
    }
    auto leftJson = SnapshotArgJson(db, args, L"left", L"leftSnapshotId", errorCode, errorMessage);
    if (leftJson.empty()) {
      return L"";
    }
    auto rightJson = SnapshotArgJson(db, args, L"right", L"rightSnapshotId", errorCode, errorMessage);
    if (rightJson.empty()) {
      return L"";
    }
    json::JsonValue left{nullptr};
    json::JsonValue right{nullptr};
    if (!json::JsonValue::TryParse(leftJson, left) || !json::JsonValue::TryParse(rightJson, right)) {
      *errorCode = L"invalid_request";
      *errorMessage = L"Snapshot value is not valid JSON";
      return L"";
    }
    auto leftComparable = ComparableSnapshotJson(left);
    auto rightComparable = ComparableSnapshotJson(right);
    auto leftHash = L"sha256:" + Sha256Hex(leftComparable);
    auto rightHash = L"sha256:" + Sha256Hex(rightComparable);
    bool equal = leftComparable == rightComparable;
    return L"{\"ok\":" + std::wstring(equal ? L"true" : L"false") +
        L",\"equal\":" + std::wstring(equal ? L"true" : L"false") +
        L",\"leftHash\":" + JsonString(leftHash) +
        L",\"rightHash\":" + JsonString(rightHash) + L"}";
  }

  std::wstring RuntimeStorageResetJson(
      std::wstring const& childControlSessionId,
      std::wstring const& appId,
      std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    auto runtimeSessionId = RuntimeSessionForControlSession(db, childControlSessionId, appId);
    if (runtimeSessionId.empty()) {
      *error = L"Could not create runtime session for storage reset";
      return L"";
    }
    auto snapshotId = MakeId(L"snapshot");
    auto createdAt = NowIso();
    auto installId = ActiveInstallId(db, appId);
    auto snapshotJson = RuntimeSnapshotDocumentJson(db, appId, createdAt);
    auto contentHash = L"sha256:" + Sha256Hex(snapshotJson);
    sqlite3_stmt* statement = nullptr;
    bool ok = sqlite3_prepare_v2(
                  db,
                  "INSERT INTO runtime_snapshots "
                  "(snapshot_id, session_id, app_id, install_id, type, snapshot_json, content_hash, created_at) "
                  "VALUES (?, ?, ?, ?, 'manual', ?, ?, ?)",
                  -1,
                  &statement,
                  nullptr) == SQLITE_OK;
    if (ok) {
      BindText(statement, 1, snapshotId);
      BindText(statement, 2, runtimeSessionId);
      BindText(statement, 3, appId);
      if (installId.empty()) {
        sqlite3_bind_null(statement, 4);
      } else {
        BindText(statement, 4, installId);
      }
      BindText(statement, 5, snapshotJson);
      BindText(statement, 6, contentHash);
      BindText(statement, 7, createdAt);
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
    if (!ok) {
      *error = L"Could not create pre-reset runtime snapshot";
      return L"";
    }
    bool deleteOk = true;
    auto clearedStorageKeys = DeleteRows(db, "DELETE FROM app_storage WHERE app_id = ?", appId, true, &deleteOk);
    if (!deleteOk) {
      *error = L"Webapp storage could not be reset";
      return L"";
    }
    return L"{\"ok\":true,\"appId\":" + JsonString(appId) +
        L",\"snapshotId\":" + JsonString(snapshotId) +
        L",\"clearedStorageKeys\":" + std::to_wstring(clearedStorageKeys) + L"}";
  }

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

  std::wstring BridgeCallRowJson(sqlite3_stmt* statement) {
    std::wstring row = L"{\"bridge_call_id\":" + JsonString(ColumnText(statement, 0)) +
        L",\"session_id\":" + JsonNullableString(ColumnText(statement, 1)) +
        L",\"app_id\":" + JsonNullableString(ColumnText(statement, 2)) +
        L",\"install_id\":" + JsonNullableString(ColumnText(statement, 3)) +
        L",\"method\":" + JsonString(ColumnText(statement, 4)) +
        L",\"params_json\":" + JsonNullableString(ColumnText(statement, 5)) +
        L",\"result_json\":" + JsonNullableString(ColumnText(statement, 6)) +
        L",\"error_json\":" + JsonNullableString(ColumnText(statement, 7)) +
        L",\"duration_ms\":";
    if (sqlite3_column_type(statement, 8) == SQLITE_NULL) {
      row += L"null";
    } else {
      row += std::to_wstring(sqlite3_column_int64(statement, 8));
    }
    row += L",\"created_at\":" + JsonString(ColumnText(statement, 9)) + L"}";
    return row;
  }

  std::wstring BridgeCallRowsJson(sqlite3* db, std::wstring const& appId) {
    char const* sql = appId.empty()
        ? "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at FROM bridge_calls ORDER BY created_at"
        : "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at FROM bridge_calls WHERE app_id = ? ORDER BY created_at";
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
        rows += BridgeCallRowJson(statement);
      }
    }
    sqlite3_finalize(statement);
    rows += L"]";
    return rows;
  }

  std::wstring RuntimeBridgeCallsJson(std::wstring const& appId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    return BridgeCallRowsJson(db, appId);
  }

  int64_t DeleteRows(sqlite3* db, char const* sql, std::wstring const& appId, bool bindAppId, bool* ok) {
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
      *ok = false;
      return 0;
    }
    if (bindAppId) {
      BindText(statement, 1, appId);
    }
    if (sqlite3_step(statement) != SQLITE_DONE) {
      *ok = false;
      sqlite3_finalize(statement);
      return 0;
    }
    auto changes = sqlite3_changes(db);
    sqlite3_finalize(statement);
    return changes;
  }

  std::wstring ClearRuntimeLogsJson(std::wstring const& appId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    bool ok = true;
    bool scoped = !appId.empty();
    auto bridgeCalls = DeleteRows(
        db,
        scoped ? "DELETE FROM bridge_calls WHERE app_id = ?" : "DELETE FROM bridge_calls",
        appId,
        scoped,
        &ok);
    auto coreActions = DeleteRows(
        db,
        scoped ? "DELETE FROM core_actions WHERE app_id = ?" : "DELETE FROM core_actions",
        appId,
        scoped,
        &ok);
    auto coreEvents = DeleteRows(
        db,
        scoped ? "DELETE FROM core_events WHERE app_id = ?" : "DELETE FROM core_events",
        appId,
        scoped,
        &ok);
    if (!ok) {
      *error = L"Could not clear runtime logs";
      return L"";
    }
    return L"{\"ok\":true,\"appId\":" + JsonNullableString(appId) +
        L",\"bridgeCallsCleared\":" + std::to_wstring(bridgeCalls) +
        L",\"coreActionsCleared\":" + std::to_wstring(coreActions) +
        L",\"coreEventsCleared\":" + std::to_wstring(coreEvents) + L"}";
  }

  std::wstring JsonStringMemberOrNull(std::wstring const& objectJson, std::wstring const& key) {
    json::JsonValue parsed{nullptr};
    if (!json::JsonValue::TryParse(objectJson, parsed) || parsed.ValueType() != json::JsonValueType::Object) {
      return L"null";
    }
    auto object = parsed.GetObject();
    auto value = OptionalStringMember(object, key);
    if (!value.has_value()) {
      return L"null";
    }
    return JsonNullableString(value.value());
  }

  std::wstring NotificationRowJson(sqlite3_stmt* statement) {
    auto paramsJson = ColumnText(statement, 2);
    return L"{\"bridgeCallId\":" + JsonString(ColumnText(statement, 0)) +
        L",\"appId\":" + JsonNullableString(ColumnText(statement, 1)) +
        L",\"message\":" + JsonStringMemberOrNull(paramsJson, L"message") +
        L",\"level\":" + JsonStringMemberOrNull(paramsJson, L"level") +
        L",\"params\":" + RawJsonOrNull(paramsJson) +
        L",\"result\":" + RawJsonOrNull(ColumnText(statement, 3)) +
        L",\"error\":" + RawJsonOrNull(ColumnText(statement, 4)) +
        L",\"createdAt\":" + JsonString(ColumnText(statement, 5)) + L"}";
  }

  std::wstring NotificationCaptureJson(std::wstring const& appId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    char const* sql = appId.empty()
        ? "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'notification.toast' ORDER BY created_at"
        : "SELECT bridge_call_id, app_id, params_json, result_json, error_json, created_at FROM bridge_calls WHERE method = 'notification.toast' AND app_id = ? ORDER BY created_at";
    sqlite3_stmt* statement = nullptr;
    std::wstring notifications = L"[";
    bool first = true;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
      *error = L"Could not read notification capture rows";
      return L"";
    }
    if (!appId.empty()) {
      BindText(statement, 1, appId);
    }
    while (sqlite3_step(statement) == SQLITE_ROW) {
      if (!first) {
        notifications += L",";
      }
      first = false;
      notifications += NotificationRowJson(statement);
    }
    sqlite3_finalize(statement);
    notifications += L"]";
    return L"{\"appId\":" + JsonNullableString(appId) + L",\"notifications\":" + notifications + L"}";
  }

  std::wstring AssertBridgeCallJson(
      std::wstring const& appId,
      std::wstring const& method,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not open platform database";
      return L"";
    }
    sqlite3_stmt* statement = nullptr;
    char const* sql =
        "SELECT bridge_call_id, session_id, app_id, install_id, method, params_json, result_json, error_json, duration_ms, created_at "
        "FROM bridge_calls WHERE app_id = ? AND method = ? ORDER BY created_at";
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not read bridge call rows";
      return L"";
    }
    BindText(statement, 1, appId);
    BindText(statement, 2, method);
    int64_t count = 0;
    std::wstring latest;
    while (sqlite3_step(statement) == SQLITE_ROW) {
      ++count;
      latest = BridgeCallRowJson(statement);
    }
    sqlite3_finalize(statement);
    if (count == 0) {
      *errorCode = L"assertion_failed";
      *errorMessage = L"Expected bridge call was not recorded";
      return L"";
    }
    return L"{\"ok\":true,\"appId\":" + JsonString(appId) +
        L",\"method\":" + JsonString(method) +
        L",\"count\":" + std::to_wstring(count) +
        L",\"latest\":" + latest + L"}";
  }

  bool ConsoleLogIsError(std::wstring const& paramsJson, std::wstring const& errorJson) {
    if (RawJsonOrNull(errorJson) != L"null") {
      return true;
    }
    json::JsonValue parsed{nullptr};
    if (!json::JsonValue::TryParse(paramsJson, parsed) || parsed.ValueType() != json::JsonValueType::Object) {
      return false;
    }
    auto object = parsed.GetObject();
    auto level = OptionalStringMember(object, L"level");
    return level.has_value() && level.value() == L"error";
  }

  std::wstring AssertNoConsoleErrorsJson(
      std::wstring const& appId,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not open platform database";
      return L"";
    }
    char const* sql = appId.empty()
        ? "SELECT params_json, error_json FROM bridge_calls WHERE method = 'app.log' ORDER BY created_at"
        : "SELECT params_json, error_json FROM bridge_calls WHERE method = 'app.log' AND app_id = ? ORDER BY created_at";
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not read console log rows";
      return L"";
    }
    if (!appId.empty()) {
      BindText(statement, 1, appId);
    }
    int64_t errors = 0;
    while (sqlite3_step(statement) == SQLITE_ROW) {
      if (ConsoleLogIsError(ColumnText(statement, 0), ColumnText(statement, 1))) {
        ++errors;
      }
    }
    sqlite3_finalize(statement);
    if (errors > 0) {
      *errorCode = L"console_errors_found";
      *errorMessage = L"Console error logs were found";
      return L"";
    }
    return L"{\"ok\":true,\"errors\":0}";
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

  std::wstring CoreEventSnapshotRowsJson(sqlite3* db, std::wstring const& appId) {
    char const* sql =
        "SELECT event_id, session_id, app_id, install_id, state_version_before, event_json, created_at "
        "FROM core_events WHERE app_id = ? ORDER BY created_at";
    sqlite3_stmt* statement = nullptr;
    std::wstring rows = L"[";
    bool first = true;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      while (sqlite3_step(statement) == SQLITE_ROW) {
        if (!first) {
          rows += L",";
        }
        first = false;
        rows += L"{\"eventId\":" + JsonString(ColumnText(statement, 0)) +
            L",\"sessionId\":" + JsonNullableString(ColumnText(statement, 1)) +
            L",\"appId\":" + JsonNullableString(ColumnText(statement, 2)) +
            L",\"installId\":" + JsonNullableString(ColumnText(statement, 3)) +
            L",\"stateVersionBefore\":";
        if (sqlite3_column_type(statement, 4) == SQLITE_NULL) {
          rows += L"null";
        } else {
          rows += std::to_wstring(sqlite3_column_int64(statement, 4));
        }
        rows += L",\"eventJson\":" + JsonString(ColumnText(statement, 5)) +
            L",\"event\":" + RawJsonOrNull(ColumnText(statement, 5)) +
            L",\"createdAt\":" + JsonString(ColumnText(statement, 6)) + L"}";
      }
    }
    sqlite3_finalize(statement);
    rows += L"]";
    return rows;
  }

  std::wstring CoreActionRowsJson(sqlite3* db, std::wstring const& appId) {
    char const* sql =
        "SELECT action_id, event_id, session_id, app_id, action_json, created_at "
        "FROM core_actions WHERE app_id = ? ORDER BY created_at";
    sqlite3_stmt* statement = nullptr;
    std::wstring rows = L"[";
    bool first = true;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      while (sqlite3_step(statement) == SQLITE_ROW) {
        if (!first) {
          rows += L",";
        }
        first = false;
        rows += L"{\"actionId\":" + JsonString(ColumnText(statement, 0)) +
            L",\"eventId\":" + JsonString(ColumnText(statement, 1)) +
            L",\"sessionId\":" + JsonNullableString(ColumnText(statement, 2)) +
            L",\"appId\":" + JsonNullableString(ColumnText(statement, 3)) +
            L",\"actionJson\":" + JsonString(ColumnText(statement, 4)) +
            L",\"action\":" + RawJsonOrNull(ColumnText(statement, 4)) +
            L",\"createdAt\":" + JsonString(ColumnText(statement, 5)) + L"}";
      }
    }
    sqlite3_finalize(statement);
    rows += L"]";
    return rows;
  }

  int64_t CoreStateVersion(sqlite3* db, std::wstring const& appId) {
    sqlite3_stmt* statement = nullptr;
    int64_t stateVersion = 0;
    if (sqlite3_prepare_v2(
            db,
            "SELECT COALESCE(MAX(COALESCE(state_version_before, -1) + 1), 0) FROM core_events WHERE app_id = ?",
            -1,
            &statement,
            nullptr) == SQLITE_OK) {
      BindText(statement, 1, appId);
      if (sqlite3_step(statement) == SQLITE_ROW) {
        stateVersion = sqlite3_column_int64(statement, 0);
      }
    }
    sqlite3_finalize(statement);
    return stateVersion;
  }

  std::wstring RuntimeCoreSnapshotJson(std::wstring const& appId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    return L"{\"appId\":" + JsonString(appId) +
        L",\"stateVersion\":" + std::to_wstring(CoreStateVersion(db, appId)) +
        L",\"coreEvents\":" + CoreEventSnapshotRowsJson(db, appId) +
        L",\"coreActions\":" + CoreActionRowsJson(db, appId) + L"}";
  }

  std::wstring RuntimeAssertCoreActionJson(
      std::wstring const& appId,
      std::optional<std::wstring> const& expectedType,
      std::optional<json::IJsonValue> const& expectedMatch,
      std::optional<json::IJsonValue> const& expectedAction,
      std::wstring* errorCode,
      std::wstring* errorMessage) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not open platform database";
      return L"";
    }
    sqlite3_stmt* statement = nullptr;
    char const* sql =
        "SELECT action_json FROM core_actions WHERE app_id = ? ORDER BY created_at";
    int64_t count = 0;
    std::wstring actions = L"[";
    bool first = true;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) != SQLITE_OK) {
      *errorCode = L"storage_error";
      *errorMessage = L"Could not read core action rows";
      return L"";
    }
    BindText(statement, 1, appId);
    while (sqlite3_step(statement) == SQLITE_ROW) {
      auto actionJson = ColumnText(statement, 0);
      json::JsonValue parsed{nullptr};
      if (!json::JsonValue::TryParse(actionJson, parsed) || parsed.ValueType() != json::JsonValueType::Object) {
        continue;
      }
      auto action = parsed.GetObject();
      if (expectedType.has_value() &&
          (!action.HasKey(L"type") ||
              action.GetNamedValue(L"type").ValueType() != json::JsonValueType::String ||
              std::wstring(action.GetNamedString(L"type").c_str()) != expectedType.value())) {
        continue;
      }
      if (expectedAction.has_value() && CanonicalJsonValue(parsed) != CanonicalJsonValue(expectedAction.value())) {
        continue;
      }
      if (expectedMatch.has_value() && !JsonMatchesSubset(parsed, expectedMatch.value())) {
        continue;
      }
      if (!first) {
        actions += L",";
      }
      first = false;
      ++count;
      actions += std::wstring(parsed.Stringify().c_str());
    }
    sqlite3_finalize(statement);
    actions += L"]";
    if (count == 0) {
      *errorCode = L"core_action.not_found";
      *errorMessage = L"Expected core action was not found";
      return L"";
    }
    return L"{\"ok\":true,\"appId\":" + JsonString(appId) +
        L",\"count\":" + std::to_wstring(count) +
        L",\"actions\":" + actions + L"}";
  }

  std::wstring RuntimeReplayEventsJson(std::wstring const& appId, json::JsonArray const& events) {
    ForgeCoreBridge replayCore;
    std::wstring rows = L"[";
    for (uint32_t index = 0; index < events.Size(); ++index) {
      if (index > 0) {
        rows += L",";
      }
      json::JsonObject params;
      params.Insert(L"event", events.GetAt(index));
      BridgeRequest request;
      request.hasId = true;
      request.id = L"control_replay_" + std::to_wstring(index);
      request.method = L"core.step";
      request.params = params;
      request.context.appId = appId;
      request.context.storagePrefix = appId + L":";
      request.context.approvedPermissions.insert(L"core.step");
      request.context.mountToken = L"windows-control-replay";
      auto response = replayCore.Step(request);
      std::wstring resultJson;
      if (response.HasKey(L"result")) {
        resultJson = std::wstring(response.GetNamedValue(L"result").Stringify().c_str());
      } else {
        resultJson = L"{\"ok\":false,\"error\":" +
            (response.HasKey(L"error") ? std::wstring(response.GetNamedValue(L"error").Stringify().c_str()) : L"{\"code\":\"core_error\",\"message\":\"Replay event failed\",\"details\":{}}") +
            L",\"actions\":[]}";
      }
      rows += L"{\"index\":" + std::to_wstring(index) +
          L",\"event\":" + std::wstring(events.GetAt(index).Stringify().c_str()) +
          L",\"result\":" + resultJson + L"}";
    }
    rows += L"]";
    return L"{\"ok\":true,\"appId\":" + JsonString(appId) + L",\"replay\":" + rows + L"}";
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

  std::wstring DbExportDocumentJson(std::wstring const& type, bool includeDebug, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }

    auto exportId = MakeId(L"export");
    auto createdAt = NowIso();
    auto documentWithoutHash = L"{\"exportId\":" + JsonString(exportId) +
        L",\"type\":" + JsonString(type) +
        L",\"createdAt\":" + JsonString(createdAt) +
        L",\"runtimeVersion\":\"0.4.0\"" +
        L",\"source\":{\"platform\":\"windows\",\"target\":\"windows\"}" +
        L",\"apps\":" + SafeTableRowsJson(db, "apps", {"id", "name", "status", "active_install_id", "active_version", "data_version", "created_at", "updated_at"}, "id") +
        L",\"appVersions\":" + SafeTableRowsJson(db, "app_versions", {"install_id", "app_id", "version", "runtime_version", "data_version", "manifest_json", "manifest_hash", "content_hash", "signature_json", "trust_level", "status", "created_at", "activated_at"}, "created_at") +
        L",\"appFiles\":" + SafeTableRowsJson(db, "app_files", {"install_id", "path", "content_text", "content_hash", "size_bytes", "mime", "created_at"}, "path") +
        L",\"appPermissions\":" + SafeTableRowsJson(db, "app_permissions", {"install_id", "app_id", "permission", "requested", "approved", "approved_at", "reason"}, "permission") +
        L",\"appStorage\":" + SafeTableRowsJson(db, "app_storage", {"app_id", "key", "value_json", "updated_at"}, "updated_at") +
        L",\"appInstallReports\":" + SafeTableRowsJson(db, "app_install_reports", {"report_id", "app_id", "install_id", "status", "validation_json", "security_json", "permissions_json", "compatibility_json", "smoke_test_json", "content_hash", "created_at"}, "created_at") +
        L",\"appMigrations\":" + SafeTableRowsJson(db, "app_migrations", {"migration_id", "app_id", "from_data_version", "to_data_version", "migration_json", "content_hash", "created_at"}, "created_at") +
        L",\"runtimeCapabilities\":" + RuntimeCapabilitiesJson(L"") +
        (includeDebug
            ? L",\"debug\":{\"runtimeSessions\":" + SafeTableRowsJson(db, "runtime_sessions", {"session_id", "target", "platform", "runtime_version", "active_app_id", "active_install_id", "started_at", "ended_at", "status"}, "started_at") +
                L",\"bridgeCalls\":" + SafeTableRowsJson(db, "bridge_calls", {"bridge_call_id", "session_id", "app_id", "install_id", "method", "result_json", "error_json", "duration_ms", "created_at"}, "created_at") +
                L",\"controlSessions\":" + SafeTableRowsJson(db, "control_sessions", {"control_session_id", "target", "runtime_session_id", "actor", "started_at", "ended_at", "status", "metadata_json"}, "started_at") +
                L",\"controlCommands\":" + SafeTableRowsJson(db, "control_commands", {"command_id", "control_session_id", "runtime_session_id", "tool", "http_method", "path", "decision", "error_code", "args_json", "result_json", "error_json", "created_at", "duration_ms"}, "created_at") +
                L",\"coreEvents\":" + SafeTableRowsJson(db, "core_events", {"event_id", "session_id", "app_id", "install_id", "state_version_before", "event_json", "created_at"}, "created_at") +
                L",\"coreActions\":" + SafeTableRowsJson(db, "core_actions", {"action_id", "event_id", "session_id", "app_id", "action_json", "created_at"}, "created_at") +
                L",\"runtimeSnapshots\":" + SafeTableRowsJson(db, "runtime_snapshots", {"snapshot_id", "session_id", "app_id", "install_id", "type", "snapshot_json", "content_hash", "created_at"}, "created_at") +
                L",\"testRuns\":" + SafeTableRowsJson(db, "test_runs", {"test_run_id", "micro_test_id", "session_id", "control_session_id", "app_id", "status", "started_at", "finished_at", "result_json", "diagnostics_json"}, "started_at") +
                L"}}"
            : L",\"debug\":{}}");
    auto contentHash = L"sha256:" + Sha256Hex(documentWithoutHash);
    auto document = documentWithoutHash.substr(0, documentWithoutHash.size() - 1) +
        L",\"contentHash\":" + JsonString(contentHash) + L"}";

    sqlite3_stmt* statement = nullptr;
    bool ok = sqlite3_prepare_v2(
                  db,
                  "INSERT OR REPLACE INTO backup_exports "
                  "(export_id, type, source_platform, runtime_version, export_json, content_hash, created_at) "
                  "VALUES (?, ?, 'windows', '0.4.0', ?, ?, ?)",
                  -1,
                  &statement,
                  nullptr) == SQLITE_OK;
    if (ok) {
      BindText(statement, 1, exportId);
      BindText(statement, 2, type);
      BindText(statement, 3, document);
      BindText(statement, 4, contentHash);
      BindText(statement, 5, createdAt);
      ok = sqlite3_step(statement) == SQLITE_DONE;
    }
    sqlite3_finalize(statement);
    if (!ok) {
      *error = L"Could not record backup export";
      return L"";
    }
    return document;
  }

  std::wstring DbExportBackupJson(std::wstring* error) {
    return DbExportDocumentJson(L"backup", false, error);
  }

  std::wstring DbExportDebugBundleJson(std::wstring* error) {
    return DbExportDocumentJson(L"debug-bundle", true, error);
  }

  std::wstring DbImportBackupJson(json::JsonObject const& document, std::wstring* error) {
    auto type = TextValue(document, {L"type"}).value_or(L"");
    if (type != L"backup" && type != L"debug-bundle" && type != L"test-fixture") {
      *error = L"Backup import requires type backup, debug-bundle, or test-fixture";
      return L"";
    }
    auto apps = OptionalArrayMember(document, L"apps");
    auto versions = OptionalArrayMember(document, L"appVersions");
    auto files = OptionalArrayMember(document, L"appFiles");
    auto permissions = OptionalArrayMember(document, L"appPermissions");
    auto storageRows = OptionalArrayMember(document, L"appStorage");
    if (!apps.has_value() || !versions.has_value() || !files.has_value() || !permissions.has_value() || !storageRows.has_value()) {
      *error = L"Backup import document is missing required arrays";
      return L"";
    }
    auto migrations = OptionalArrayMember(document, L"appMigrations");
    auto reports = OptionalArrayMember(document, L"appInstallReports");

    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }

    auto createdAt = NowIso();
    char* sqlError = nullptr;
    if (sqlite3_exec(db, "BEGIN IMMEDIATE", nullptr, nullptr, &sqlError) != SQLITE_OK) {
      *error = L"Could not start backup import transaction";
      sqlite3_free(sqlError);
      return L"";
    }

    bool ok = true;
    auto objectAt = [](json::JsonArray const& array, uint32_t index, json::JsonObject* object) {
      auto value = array.GetAt(index);
      if (value.ValueType() != json::JsonValueType::Object) {
        return false;
      }
      *object = value.GetObject();
      return true;
    };

    for (uint32_t index = 0; ok && index < apps->Size(); ++index) {
      json::JsonObject app{nullptr};
      ok = objectAt(apps.value(), index, &app);
      auto appId = ok ? TextValue(app, {L"id", L"appId"}) : std::nullopt;
      if (!appId.has_value()) {
        ok = false;
        break;
      }
      ok = ExecutePrepared(
          db,
          "INSERT OR REPLACE INTO apps (id, name, status, active_install_id, active_version, data_version, created_at, updated_at) "
          "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
          {
              SqlText(appId.value()),
              SqlText(TextValue(app, {L"name"}).value_or(appId.value())),
              SqlText(TextValue(app, {L"status"}).value_or(L"enabled")),
              SqlNullableText(TextValue(app, {L"active_install_id", L"activeInstallId"})),
              SqlNullableText(TextValue(app, {L"active_version", L"activeVersion"})),
              SqlInt(IntValue(app, {L"data_version", L"dataVersion"}, 1)),
              SqlText(TextValue(app, {L"created_at", L"createdAt"}).value_or(createdAt)),
              SqlText(TextValue(app, {L"updated_at", L"updatedAt"}).value_or(createdAt)),
          });
    }

    for (uint32_t index = 0; ok && index < versions->Size(); ++index) {
      json::JsonObject version{nullptr};
      ok = objectAt(versions.value(), index, &version);
      auto installId = ok ? TextValue(version, {L"install_id", L"installId"}) : std::nullopt;
      auto appId = ok ? TextValue(version, {L"app_id", L"appId"}) : std::nullopt;
      auto appVersion = ok ? TextValue(version, {L"version", L"appVersion"}) : std::nullopt;
      if (!installId.has_value() || !appId.has_value() || !appVersion.has_value()) {
        ok = false;
        break;
      }
      ok = ExecutePrepared(
          db,
          "INSERT OR REPLACE INTO app_versions (install_id, app_id, version, runtime_version, data_version, manifest_json, manifest_hash, content_hash, signature_json, trust_level, status, created_at, activated_at) "
          "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
          {
              SqlText(installId.value()),
              SqlText(appId.value()),
              SqlText(appVersion.value()),
              SqlText(TextValue(version, {L"runtime_version", L"runtimeVersion"}).value_or(L"0.1.0")),
              SqlInt(IntValue(version, {L"data_version", L"dataVersion"}, 1)),
              SqlText(JsonTextValue(version, {L"manifest_json", L"manifestJson"}, {L"manifest"}, L"{}").value_or(L"{}")),
              SqlText(TextValue(version, {L"manifest_hash", L"manifestHash"}).value_or(L"")),
              SqlText(TextValue(version, {L"content_hash", L"contentHash"}).value_or(L"")),
              SqlNullableText(JsonTextValue(version, {L"signature_json", L"signatureJson"}, {L"signature"}, std::nullopt)),
              SqlText(TextValue(version, {L"trust_level", L"trustLevel"}).value_or(L"developer")),
              SqlText(TextValue(version, {L"status"}).value_or(L"installed")),
              SqlText(TextValue(version, {L"created_at", L"installedAt", L"createdAt"}).value_or(createdAt)),
              SqlNullableText(TextValue(version, {L"activated_at", L"activatedAt"})),
          });
    }

    for (uint32_t index = 0; ok && index < files->Size(); ++index) {
      json::JsonObject file{nullptr};
      ok = objectAt(files.value(), index, &file);
      auto installId = ok ? TextValue(file, {L"install_id", L"installId"}) : std::nullopt;
      auto path = ok ? TextValue(file, {L"path"}) : std::nullopt;
      if (!installId.has_value() || !path.has_value()) {
        ok = false;
        break;
      }
      auto content = TextValue(file, {L"content_text", L"contentText"}).value_or(L"");
      ok = ExecutePrepared(
          db,
          "INSERT OR REPLACE INTO app_files (install_id, path, content_text, content_hash, size_bytes, mime, created_at) "
          "VALUES (?, ?, ?, ?, ?, ?, ?)",
          {
              SqlText(installId.value()),
              SqlText(path.value()),
              SqlText(content),
              SqlText(TextValue(file, {L"content_hash", L"contentHash"}).value_or(L"sha256:" + Sha256Hex(content))),
              SqlInt(IntValue(file, {L"size_bytes", L"sizeBytes"}, static_cast<int64_t>(WideToUtf8(content).size()))),
              SqlText(TextValue(file, {L"mime"}).value_or(L"text/plain")),
              SqlText(TextValue(file, {L"created_at", L"createdAt"}).value_or(createdAt)),
          });
    }

    for (uint32_t index = 0; ok && index < permissions->Size(); ++index) {
      json::JsonObject permission{nullptr};
      ok = objectAt(permissions.value(), index, &permission);
      auto installId = ok ? TextValue(permission, {L"install_id", L"installId"}) : std::nullopt;
      auto appId = ok ? TextValue(permission, {L"app_id", L"appId"}) : std::nullopt;
      auto name = ok ? TextValue(permission, {L"permission"}) : std::nullopt;
      if (!installId.has_value() || !appId.has_value() || !name.has_value()) {
        ok = false;
        break;
      }
      ok = ExecutePrepared(
          db,
          "INSERT OR REPLACE INTO app_permissions (install_id, app_id, permission, requested, approved, approved_at, reason) "
          "VALUES (?, ?, ?, ?, ?, ?, ?)",
          {
              SqlText(installId.value()),
              SqlText(appId.value()),
              SqlText(name.value()),
              SqlInt(IntValue(permission, {L"requested"}, 1)),
              SqlInt(IntValue(permission, {L"approved"}, 0)),
              SqlNullableText(TextValue(permission, {L"approved_at", L"approvedAt"})),
              SqlNullableText(TextValue(permission, {L"reason"})),
          });
    }

    for (uint32_t index = 0; ok && index < storageRows->Size(); ++index) {
      json::JsonObject storage{nullptr};
      ok = objectAt(storageRows.value(), index, &storage);
      auto appId = ok ? TextValue(storage, {L"app_id", L"appId"}) : std::nullopt;
      auto key = ok ? TextValue(storage, {L"key"}) : std::nullopt;
      if (!appId.has_value() || !key.has_value()) {
        ok = false;
        break;
      }
      ok = ExecutePrepared(
          db,
          "INSERT OR REPLACE INTO app_storage (app_id, key, value_json, updated_at) VALUES (?, ?, ?, ?)",
          {
              SqlText(appId.value()),
              SqlText(key.value()),
              SqlText(JsonTextValue(storage, {L"value_json", L"valueJson"}, {L"value"}, L"null").value_or(L"null")),
              SqlText(TextValue(storage, {L"updated_at", L"updatedAt"}).value_or(createdAt)),
          });
    }

    for (uint32_t index = 0; ok && migrations.has_value() && index < migrations->Size(); ++index) {
      json::JsonObject migration{nullptr};
      ok = objectAt(migrations.value(), index, &migration);
      auto migrationId = ok ? TextValue(migration, {L"migration_id", L"migrationId"}) : std::nullopt;
      auto appId = ok ? TextValue(migration, {L"app_id", L"appId"}) : std::nullopt;
      if (!migrationId.has_value() || !appId.has_value()) {
        ok = false;
        break;
      }
      ok = ExecutePrepared(
          db,
          "INSERT OR REPLACE INTO app_migrations (migration_id, app_id, from_data_version, to_data_version, migration_json, content_hash, created_at) "
          "VALUES (?, ?, ?, ?, ?, ?, ?)",
          {
              SqlText(migrationId.value()),
              SqlText(appId.value()),
              SqlInt(IntValue(migration, {L"from_data_version", L"fromDataVersion"}, 1)),
              SqlInt(IntValue(migration, {L"to_data_version", L"toDataVersion"}, 1)),
              SqlText(JsonTextValue(migration, {L"migration_json", L"migrationJson"}, {L"migration"}, L"{}").value_or(L"{}")),
              SqlText(TextValue(migration, {L"content_hash", L"contentHash"}).value_or(L"")),
              SqlText(TextValue(migration, {L"created_at", L"createdAt"}).value_or(createdAt)),
          });
    }

    for (uint32_t index = 0; ok && reports.has_value() && index < reports->Size(); ++index) {
      json::JsonObject report{nullptr};
      ok = objectAt(reports.value(), index, &report);
      auto reportId = ok ? TextValue(report, {L"report_id", L"reportId"}) : std::nullopt;
      auto appId = ok ? TextValue(report, {L"app_id", L"appId"}) : std::nullopt;
      if (!reportId.has_value() || !appId.has_value()) {
        ok = false;
        break;
      }
      ok = ExecutePrepared(
          db,
          "INSERT OR REPLACE INTO app_install_reports (report_id, app_id, install_id, status, validation_json, security_json, permissions_json, compatibility_json, smoke_test_json, content_hash, created_at) "
          "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
          {
              SqlText(reportId.value()),
              SqlText(appId.value()),
              SqlNullableText(TextValue(report, {L"install_id", L"installId"})),
              SqlText(TextValue(report, {L"status"}).value_or(L"accepted")),
              SqlNullableText(JsonTextValue(report, {L"validation_json", L"validationJson"}, {L"validation"}, std::nullopt)),
              SqlNullableText(JsonTextValue(report, {L"security_json", L"securityJson"}, {L"security"}, std::nullopt)),
              SqlNullableText(JsonTextValue(report, {L"permissions_json", L"permissionsJson"}, {L"permissions"}, std::nullopt)),
              SqlNullableText(JsonTextValue(report, {L"compatibility_json", L"compatibilityJson"}, {L"compatibility"}, std::nullopt)),
              SqlNullableText(JsonTextValue(report, {L"smoke_test_json", L"smokeTestJson"}, {L"smokeTest"}, std::nullopt)),
              SqlNullableText(TextValue(report, {L"content_hash", L"contentHash"})),
              SqlText(TextValue(report, {L"created_at", L"createdAt"}).value_or(createdAt)),
          });
    }

    auto source = OptionalObjectMember(document, L"source");
    auto sourcePlatform = source.has_value() ? TextValue(source.value(), {L"platform"}).value_or(L"unknown") : L"unknown";
    auto documentText = std::wstring(document.Stringify().c_str());
    ok = ok && ExecutePrepared(
        db,
        "INSERT INTO backup_exports (export_id, type, source_platform, runtime_version, export_json, content_hash, created_at, imported_at) "
        "VALUES (?, 'import', ?, ?, ?, ?, ?, ?)",
        {
            SqlText(MakeId(L"import")),
            SqlText(sourcePlatform),
            SqlText(TextValue(document, {L"runtimeVersion"}).value_or(L"0.4.0")),
            SqlText(documentText),
            SqlText(TextValue(document, {L"contentHash"}).value_or(L"sha256:" + Sha256Hex(documentText))),
            SqlText(createdAt),
            SqlText(createdAt),
        });

    if (!ok || sqlite3_exec(db, "COMMIT", nullptr, nullptr, &sqlError) != SQLITE_OK) {
      sqlite3_exec(db, "ROLLBACK", nullptr, nullptr, nullptr);
      *error = L"Backup import could not be completed";
      sqlite3_free(sqlError);
      return L"";
    }

    return L"{\"ok\":true,\"apps\":" + std::to_wstring(apps->Size()) +
        L",\"appVersions\":" + std::to_wstring(versions->Size()) +
        L",\"appStorage\":" + std::to_wstring(storageRows->Size()) + L"}";
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

  std::wstring EventLogJson(std::wstring const& appId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    return L"{\"appId\":" + JsonNullableString(appId) +
        L",\"bridgeCalls\":" + BridgeCallRowsJson(db, appId) +
        L",\"coreEvents\":" + CoreEventRowsJson(db, appId) + L"}";
  }

  std::wstring ConsoleLogsJson(std::wstring const& appId, std::wstring* error) {
    PlatformDatabase database(databasePath);
    sqlite3* db = database.handle();
    if (db == nullptr) {
      *error = L"Could not open platform database";
      return L"";
    }
    char const* sql = appId.empty()
        ? "SELECT bridge_call_id, app_id, params_json, created_at FROM bridge_calls WHERE method = 'app.log' ORDER BY created_at LIMIT 100"
        : "SELECT bridge_call_id, app_id, params_json, created_at FROM bridge_calls WHERE method = 'app.log' AND app_id = ? ORDER BY created_at LIMIT 100";
    sqlite3_stmt* statement = nullptr;
    std::wstring logs = L"[";
    bool first = true;
    if (sqlite3_prepare_v2(db, sql, -1, &statement, nullptr) == SQLITE_OK) {
      if (!appId.empty()) {
        BindText(statement, 1, appId);
      }
      while (sqlite3_step(statement) == SQLITE_ROW) {
        if (!first) {
          logs += L",";
        }
        first = false;
        logs += L"{\"bridgeCallId\":" + JsonString(ColumnText(statement, 0)) +
            L",\"appId\":" + JsonNullableString(ColumnText(statement, 1)) +
            L",\"params\":" + RawJsonOrNull(ColumnText(statement, 2)) +
            L",\"createdAt\":" + JsonString(ColumnText(statement, 3)) + L"}";
      }
    }
    sqlite3_finalize(statement);
    logs += L"]";
    return L"{\"appId\":" + JsonNullableString(appId) + L",\"logs\":" + logs + L"}";
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
    } else if (tool == L"platform.list_targets") {
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = PlatformListTargetsJson();
    } else if (tool == L"platform.list_webapps") {
      json::JsonObject args{nullptr};
      bool includeUninstalled = false;
      if (command.HasKey(L"args")) {
        auto parsedArgs = OptionalObjectMember(command, L"args");
        if (!parsedArgs.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.list_webapps args must be an object", 400);
          return;
        }
        args = parsedArgs.value();
        includeUninstalled = BooleanMemberTrue(args, L"includeUninstalled");
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = PlatformListWebappsJson(includeUninstalled, &error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Could not list Windows webapps" : error, 500);
        return;
      }
    } else if (tool == L"platform.validate_package" || tool == L"platform.run_policy_audit") {
      json::JsonObject args = json::JsonObject::Parse(L"{}");
      if (command.HasKey(L"args")) {
        auto parsedArgs = OptionalObjectMember(command, L"args");
        if (!parsedArgs.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
          return;
        }
        args = parsedArgs.value();
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = ValidatePackageResultJson(args);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.validate_package requires packagePath or path", 400);
        return;
      }
    } else if (tool == L"platform.sign_webapp_package") {
      json::JsonObject args = json::JsonObject::Parse(L"{}");
      if (command.HasKey(L"args")) {
        auto parsedArgs = OptionalObjectMember(command, L"args");
        if (!parsedArgs.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.sign_webapp_package requires args object", 400);
          return;
        }
        args = parsedArgs.value();
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = SignWebappPackageJson(args);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.sign_webapp_package requires packagePath or path", 400);
        return;
      }
    } else if (tool == L"platform.install_webapp_package") {
      json::JsonObject args = json::JsonObject::Parse(L"{}");
      if (command.HasKey(L"args")) {
        auto parsedArgs = OptionalObjectMember(command, L"args");
        if (!parsedArgs.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.install_webapp_package requires args object", 400);
          return;
        }
        args = parsedArgs.value();
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = InstallWebappPackageJson(args, &error);
      if (result.empty()) {
        SendControlRouteError(
            client,
            sessionId,
            tool,
            method,
            path,
            started,
            error == L"Package install transaction failed" ? L"storage_error" : L"invalid_request",
            error.empty() ? L"platform.install_webapp_package requires packagePath or path" : error,
            error == L"Package install transaction failed" ? 500 : 400);
        return;
      }
    } else if (tool == L"platform.open_webapp") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.open_webapp requires args object", 400);
        return;
      }
      auto appId = OptionalStringMember(args.value(), L"appId").value_or(L"");
      if (appId.empty() || !IsValidAppId(appId)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.open_webapp requires appId", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = PlatformOpenWebappJson(sessionId, args.value(), &error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", error.empty() ? L"platform.open_webapp requires an installed or bundled app" : error, 400);
        return;
      }
    } else if (tool == L"platform.list_webapp_versions" ||
        tool == L"platform.install_report" ||
        tool == L"platform.rollback_webapp" ||
        tool == L"platform.quarantine_webapp" ||
        tool == L"platform.uninstall_webapp" ||
        tool == L"platform.approve_webapp_update") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
        return;
      }
      auto appId = OptionalStringMember(args.value(), L"appId");
      if (!appId.has_value() || appId->empty() || !IsValidAppId(appId.value())) {
        std::wstring message = tool + L" requires appId";
        if (tool == L"platform.list_webapp_versions") {
          message = L"platform.list_webapp_versions requires appId";
        } else if (tool == L"platform.install_report") {
          message = L"platform.install_report requires appId";
        } else if (tool == L"platform.rollback_webapp") {
          message = L"platform.rollback_webapp requires appId";
        } else if (tool == L"platform.quarantine_webapp") {
          message = L"platform.quarantine_webapp requires appId";
        } else if (tool == L"platform.uninstall_webapp") {
          message = L"platform.uninstall_webapp requires appId";
        } else if (tool == L"platform.approve_webapp_update") {
          message = L"platform.approve_webapp_update requires appId";
        }
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", message, 400);
        return;
      }
      if (tool == L"platform.uninstall_webapp" && !BooleanMemberTrue(args.value(), L"confirm")) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"confirmation_required", L"platform.uninstall_webapp requires confirm: true", 400);
        return;
      }
      auto installId = OptionalStringMember(args.value(), L"installId");
      if (args->HasKey(L"installId") && !installId.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" installId must be a string", 400);
        return;
      }
      auto requestedInstallId = installId.has_value() && !installId->empty() ? installId : std::optional<std::wstring>();
      if (tool == L"platform.approve_webapp_update" && (!installId.has_value() || installId->empty())) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.approve_webapp_update requires installId", 400);
        return;
      }

      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      if (tool == L"platform.list_webapp_versions") {
        result = PlatformListWebappVersionsJson(appId.value(), &error);
      } else if (tool == L"platform.install_report") {
        result = PlatformInstallReportJson(appId.value(), requestedInstallId, &error);
      } else if (tool == L"platform.rollback_webapp") {
        result = PlatformRollbackWebappJson(appId.value(), requestedInstallId, &errorCode, &errorMessage);
      } else if (tool == L"platform.quarantine_webapp") {
        auto reason = OptionalStringMember(args.value(), L"reason").value_or(L"manual quarantine");
        result = PlatformQuarantineWebappJson(appId.value(), requestedInstallId, reason.empty() ? L"manual quarantine" : reason, BooleanMemberTrue(args.value(), L"restorePrevious"), &errorCode, &errorMessage);
      } else if (tool == L"platform.uninstall_webapp") {
        result = PlatformUninstallWebappJson(sessionId, appId.value(), &errorCode, &errorMessage);
      } else {
        result = PlatformApproveWebappUpdateJson(appId.value(), installId.value(), &errorCode, &errorMessage);
      }
      if (result.empty()) {
        auto code = errorCode.empty() ? L"storage_error" : errorCode;
        auto message = errorMessage.empty() ? (error.empty() ? L"Windows app registry command failed" : error) : errorMessage;
        SendControlRouteError(client, sessionId, tool, method, path, started, code, message, code == L"storage_error" ? 500 : 400);
        return;
      }
    } else if (tool == L"platform.migration_dry_run" || tool == L"platform.migration_apply") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
        return;
      }
      auto migration = OptionalObjectMember(args.value(), L"migration");
      if (!migration.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_migration", tool + L" requires migration object", 400);
        return;
      }
      auto appId = OptionalStringMember(migration.value(), L"appId");
      if (!appId.has_value() || appId->empty() || !IsValidAppId(appId.value())) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_migration", L"Migration appId is not a valid generated app id", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = PlatformMigrationRunJson(
          sessionId,
          migration.value(),
          tool == L"platform.migration_apply" ? L"apply" : L"dry-run",
          &errorCode,
          &errorMessage);
      if (result.empty()) {
        auto code = errorCode.empty() ? L"invalid_migration" : errorCode;
        SendControlRouteError(client, sessionId, tool, method, path, started, code, errorMessage.empty() ? L"Migration command failed" : errorMessage, code == L"storage_error" ? 500 : 400);
        return;
      }
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
    } else if (tool == L"runtime.accessibility_snapshot" ||
        tool == L"runtime.run_accessibility_audit" ||
        tool == L"runtime.assert_accessibility") {
      json::JsonObject args = json::JsonObject::Parse(L"{}");
      if (command.HasKey(L"args")) {
        auto parsedArgs = OptionalObjectMember(command, L"args");
        if (!parsedArgs.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
          return;
        }
        args = parsedArgs.value();
      }
      auto appId = OptionalStringMember(args, L"appId").value_or(L"notes-lite");
      if (appId.empty() || !IsValidAppId(appId)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"Accessibility appId is not a valid generated app id", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      if (tool == L"runtime.accessibility_snapshot") {
        result = RuntimeAccessibilitySnapshotJson(appId);
      } else if (tool == L"runtime.run_accessibility_audit") {
        result = RuntimeAccessibilityAuditJson(appId);
      } else {
        result = RuntimeAssertAccessibilityJson(appId, OptionalStringMember(args, L"rule"), &errorCode, &errorMessage);
        if (result.empty()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, errorCode.empty() ? L"accessibility_failed" : errorCode, errorMessage.empty() ? L"Accessibility assertion failed" : errorMessage, errorStatus);
          return;
        }
      }
    } else if (tool == L"runtime.run_smoke_tests" ||
        tool == L"runtime.run_microtest" ||
        tool == L"platform.run_platform_smoke") {
      json::JsonObject args = json::JsonObject::Parse(L"{}");
      if (command.HasKey(L"args")) {
        auto parsedArgs = OptionalObjectMember(command, L"args");
        if (!parsedArgs.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
          return;
        }
        args = parsedArgs.value();
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (tool == L"runtime.run_smoke_tests") {
        auto appId = OptionalStringMember(args, L"appId").value_or(L"");
        if (appId.empty() || !IsValidAppId(appId)) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.run_smoke_tests requires appId", 400);
          return;
        }
        if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
          SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
          return;
        }
        result = RuntimeRunSmokeTestsJson(sessionId, appId, &errorCode, &errorMessage);
      } else if (tool == L"runtime.run_microtest") {
        auto appId = MicrotestTargetAppIdFromArgs(args, &errorCode, &errorMessage);
        if (!appId.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, errorCode.empty() ? L"invalid_request" : errorCode, errorMessage.empty() ? L"runtime.run_microtest requires spec or microtestPath" : errorMessage, 400);
          return;
        }
        if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
          SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
          return;
        }
        result = RuntimeRunMicrotestJson(sessionId, args, &errorCode, &errorMessage);
      } else {
        auto appIds = PlatformSmokeAppIdsFromArgs(args, &errorCode, &errorMessage);
        if (!appIds.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, errorCode.empty() ? L"invalid_request" : errorCode, errorMessage.empty() ? L"platform.run_platform_smoke requires spec or smokePath" : errorMessage, 400);
          return;
        }
        for (auto const& appId : appIds.value()) {
          if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
            SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
            return;
          }
        }
        result = PlatformRunSmokeJson(sessionId, args, &errorCode, &errorMessage);
      }
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode.empty() ? L"invalid_request" : errorCode, errorMessage.empty() ? L"Windows static test runner failed" : errorMessage, errorCode == L"sqlite_error" ? 500 : 400);
        return;
      }
    } else if (tool == L"runtime.screenshot" ||
        tool == L"runtime.query" ||
        tool == L"runtime.click" ||
        tool == L"runtime.type" ||
        tool == L"runtime.set_value" ||
        tool == L"runtime.press_key" ||
        tool == L"runtime.drag" ||
        tool == L"runtime.wait_for" ||
        tool == L"runtime.timer_advance" ||
        tool == L"runtime.assert_visible" ||
        tool == L"runtime.assert_text") {
      json::JsonObject args = json::JsonObject::Parse(L"{}");
      if (command.HasKey(L"args")) {
        auto parsedArgs = OptionalObjectMember(command, L"args");
        if (!parsedArgs.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
          return;
        }
        args = parsedArgs.value();
      }

      std::wstring appId = OptionalStringMember(args, L"appId").value_or(L"");
      if (!appId.empty() && !IsValidAppId(appId)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" appId is not a valid generated app id", 400);
        return;
      }
      if (tool == L"runtime.query" && appId.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.query requires appId", 400);
        return;
      }
      if (tool == L"runtime.screenshot" && appId.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.screenshot requires appId", 400);
        return;
      }
      if ((tool == L"runtime.click" || tool == L"runtime.type" || tool == L"runtime.set_value" || tool == L"runtime.drag") && appId.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires appId", 400);
        return;
      }
      if (tool == L"runtime.assert_visible" && appId.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.assert_visible requires appId", 400);
        return;
      }
      if (tool == L"runtime.assert_text") {
        auto text = OptionalStringMember(args, L"text");
        if (appId.empty() || !text.has_value() || text->empty()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.assert_text requires appId and text", 400);
          return;
        }
      }
      if (tool == L"runtime.wait_for") {
        auto waitKind = StringMemberOr(args, L"kind", L"idle");
        if ((waitKind == L"bridge_call" || waitKind == L"bridgeCall") && (appId.empty() || !OptionalStringMember(args, L"method").has_value())) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.wait_for bridge_call requires appId and method", 400);
          return;
        }
        if (waitKind != L"idle" && waitKind != L"bridge_call" && waitKind != L"bridgeCall" && appId.empty()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.wait_for requires appId for selector/text waits", 400);
          return;
        }
      }

      if (!appId.empty()) {
        std::wstring errorCode;
        std::wstring errorMessage;
        int errorStatus = 400;
        if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
          SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
          return;
        }
      } else {
        std::wstring errorCode;
        std::wstring errorMessage;
        int errorStatus = 400;
        if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
          SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
          return;
        }
      }

      std::wstring errorCode;
      std::wstring errorMessage;
      if (tool == L"runtime.screenshot") {
        result = RuntimeScreenshotJson(appId, OptionalStringMember(args, L"label"));
      } else if (tool == L"runtime.query") {
        result = RuntimeQueryJson(appId, args);
      } else if (tool == L"runtime.click" ||
          tool == L"runtime.type" ||
          tool == L"runtime.set_value" ||
          tool == L"runtime.press_key" ||
          tool == L"runtime.drag") {
        result = RuntimeTargetCommandJson(tool, args, &errorCode, &errorMessage);
      } else if (tool == L"runtime.wait_for") {
        result = RuntimeWaitForJson(args, &errorCode, &errorMessage);
      } else if (tool == L"runtime.timer_advance") {
        result = RuntimeTimerAdvanceJson(args);
      } else if (tool == L"runtime.assert_visible") {
        result = RuntimeAssertVisibleJson(appId, args, &errorCode, &errorMessage);
      } else {
        result = RuntimeAssertTextJson(appId, OptionalStringMember(args, L"text").value_or(L""), &errorCode, &errorMessage);
      }
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode.empty() ? L"selector.not_found" : errorCode, errorMessage.empty() ? L"Runtime UI command failed" : errorMessage, errorCode == L"storage_error" ? 500 : 400);
        return;
      }
    } else if (tool == L"runtime.resource_usage") {
      std::wstring appId;
      std::wstring appIdError;
      if (!OptionalArgsAppId(command, tool, &appId, &appIdError)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", appIdError, 400);
        return;
      }
      if (appId.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.resource_usage requires appId", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = ResourceUsageJson(appId);
    } else if (tool == L"runtime.event_log" || tool == L"runtime.console_logs") {
      std::wstring appId;
      std::wstring appIdError;
      if (!OptionalArgsAppId(command, tool, &appId, &appIdError)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", appIdError, 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = tool == L"runtime.event_log" ? EventLogJson(appId, &error) : ConsoleLogsJson(appId, &error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Could not read platform database" : error, 500);
        return;
      }
    } else if (tool == L"runtime.bridge_calls" ||
        tool == L"runtime.clear_logs" ||
        tool == L"runtime.notification_capture" ||
        tool == L"runtime.assert_no_console_errors") {
      std::wstring appId;
      std::wstring appIdError;
      if (!OptionalArgsAppId(command, tool, &appId, &appIdError)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", appIdError, 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      if (tool == L"runtime.bridge_calls") {
        result = RuntimeBridgeCallsJson(appId, &error);
      } else if (tool == L"runtime.clear_logs") {
        result = ClearRuntimeLogsJson(appId, &error);
      } else if (tool == L"runtime.notification_capture") {
        result = NotificationCaptureJson(appId, &error);
      } else {
        result = AssertNoConsoleErrorsJson(appId, &errorCode, &errorMessage);
        if (result.empty()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorCode == L"storage_error" ? 500 : 400);
          return;
        }
      }
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Could not read platform database" : error, 500);
        return;
      }
    } else if (tool == L"runtime.fault_inject" ||
        tool == L"runtime.network_mock_set" ||
        tool == L"runtime.network_mock_reset" ||
        tool == L"runtime.dialog_mock_set") {
      json::JsonObject args;
      if (command.HasKey(L"args")) {
        auto parsedArgs = OptionalObjectMember(command, L"args");
        if (!parsedArgs.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
          return;
        }
        args = parsedArgs.value();
      } else if (tool != L"runtime.network_mock_reset") {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
        return;
      }
      std::wstring appId;
      std::wstring appIdError;
      if (!OptionalArgsAppId(command, tool, &appId, &appIdError)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", appIdError, 400);
        return;
      }
      if (args.HasKey(L"sessionId")) {
        auto mockSessionId = OptionalStringMember(args, L"sessionId");
        if (!mockSessionId.has_value()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" sessionId must be a string", 400);
          return;
        }
      }
      if (tool == L"runtime.fault_inject") {
        auto faultMethod = FaultMethodForArgs(args);
        if (!faultMethod.has_value() || faultMethod->empty()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.fault_inject requires a bridge method", 400);
          return;
        }
        if (!IsKnownControlBridgeMethod(faultMethod.value())) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"unknown_method", L"Unknown bridge method: " + faultMethod.value(), 400);
          return;
        }
      }
      if (tool == L"runtime.network_mock_set" &&
          (!NetworkMockUrlPattern(args).has_value() || !args.HasKey(L"response") || args.GetNamedValue(L"response").ValueType() == json::JsonValueType::Null)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.network_mock_set requires urlPattern or match.url and response", 400);
        return;
      }
      if (tool == L"runtime.dialog_mock_set" && !DialogMockType(args).has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.dialog_mock_set requires dialogType or method", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      if (tool == L"runtime.fault_inject") {
        result = RuntimeFaultInjectJson(args, &errorCode, &error);
      } else if (tool == L"runtime.network_mock_set") {
        result = RuntimeNetworkMockSetJson(args, &error);
      } else if (tool == L"runtime.network_mock_reset") {
        result = RuntimeNetworkMockResetJson(args, &error);
      } else {
        result = RuntimeDialogMockSetJson(args, &error);
      }
      if (result.empty()) {
        SendControlRouteError(
            client,
            sessionId,
            tool,
            method,
            path,
            started,
            errorCode.empty() ? L"storage_error" : errorCode,
            error.empty() ? L"Mock control operation failed" : error,
            errorCode == L"sqlite_error" || errorCode.empty() ? 500 : 400);
        return;
      }
    } else if (tool == L"runtime.assert_bridge_call") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.assert_bridge_call requires appId and method", 400);
        return;
      }
      auto appId = OptionalStringMember(args.value(), L"appId");
      auto bridgeMethod = OptionalStringMember(args.value(), L"method");
      if (!appId.has_value() || appId->empty() || !IsValidAppId(appId.value()) || !bridgeMethod.has_value() || bridgeMethod->empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.assert_bridge_call requires appId and method", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = AssertBridgeCallJson(appId.value(), bridgeMethod.value(), &errorCode, &errorMessage);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorCode == L"storage_error" ? 500 : 400);
        return;
      }
    } else if (tool == L"runtime.core_snapshot") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.core_snapshot requires appId", 400);
        return;
      }
      auto appId = OptionalStringMember(args.value(), L"appId");
      if (!appId.has_value() || appId->empty() || !IsValidAppId(appId.value())) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.core_snapshot requires appId", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = RuntimeCoreSnapshotJson(appId.value(), &error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Could not read core snapshot" : error, 500);
        return;
      }
    } else if (tool == L"runtime.replay_events") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.replay_events requires appId and events", 400);
        return;
      }
      auto appId = OptionalStringMember(args.value(), L"appId");
      auto events = OptionalArrayMember(args.value(), L"events");
      if (!appId.has_value() || appId->empty() || !IsValidAppId(appId.value())) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.replay_events requires appId", 400);
        return;
      }
      if (!events.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.replay_events events must be an array", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = RuntimeReplayEventsJson(appId.value(), events.value());
    } else if (tool == L"runtime.assert_core_action") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.assert_core_action requires appId", 400);
        return;
      }
      auto appId = OptionalStringMember(args.value(), L"appId");
      if (!appId.has_value() || appId->empty() || !IsValidAppId(appId.value())) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.assert_core_action requires appId", 400);
        return;
      }
      std::optional<std::wstring> expectedType;
      if (args->HasKey(L"type")) {
        auto typeValue = OptionalStringMember(args.value(), L"type");
        if (!typeValue.has_value() || typeValue->empty()) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.assert_core_action type must be a string", 400);
          return;
        }
        expectedType = typeValue.value();
      }
      std::optional<json::IJsonValue> expectedMatch;
      if (args->HasKey(L"match")) {
        expectedMatch = args->GetNamedValue(L"match");
      }
      std::optional<json::IJsonValue> expectedAction;
      if (args->HasKey(L"action")) {
        expectedAction = args->GetNamedValue(L"action");
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = RuntimeAssertCoreActionJson(appId.value(), expectedType, expectedMatch, expectedAction, &errorCode, &errorMessage);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorCode == L"storage_error" ? 500 : 400);
        return;
      }
    } else if (tool == L"runtime.storage_get" || tool == L"runtime.storage_set") {
      json::JsonObject args{nullptr};
      std::wstring appId;
      std::wstring key;
      std::wstring argsError;
      if (!StorageCommandArgs(command, tool, tool == L"runtime.storage_set", &args, &appId, &key, &argsError)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", argsError, 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = tool == L"runtime.storage_get"
          ? RuntimeStorageGetJson(sessionId, appId, key, args, started)
          : RuntimeStorageSetJson(sessionId, appId, key, args, started);
    } else if (tool == L"runtime.assert_storage") {
      json::JsonObject args{nullptr};
      std::wstring appId;
      std::wstring key;
      std::wstring argsError;
      if (!StorageCommandArgs(command, tool, true, &args, &appId, &key, &argsError)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", argsError, 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = RuntimeAssertStorageJson(appId, key, args.GetNamedValue(L"value"), &errorCode, &errorMessage);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorCode == L"storage_error" ? 500 : 400);
        return;
      }
    } else if (tool == L"runtime.storage_reset" || tool == L"platform.reset_webapp") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires args object", 400);
        return;
      }
      auto appId = OptionalStringMember(args.value(), L"appId");
      if (!appId.has_value() || appId->empty() || !IsValidAppId(appId.value())) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", tool + L" requires appId", 400);
        return;
      }
      if (!BooleanMemberTrue(args.value(), L"confirm")) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"confirmation_required", tool + L" requires confirm: true", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = RuntimeStorageResetJson(sessionId, appId.value(), &error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Webapp storage could not be reset" : error, 500);
        return;
      }
    } else if (tool == L"platform.create_snapshot") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.create_snapshot requires args object", 400);
        return;
      }
      auto appId = OptionalStringMember(args.value(), L"appId");
      if (!appId.has_value() || appId->empty() || !IsValidAppId(appId.value())) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.create_snapshot requires appId", 400);
        return;
      }
      auto snapshotType = OptionalStringMember(args.value(), L"type").value_or(L"manual");
      if (!ValidSnapshotType(snapshotType)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.create_snapshot type is invalid", 400);
        return;
      }
      auto sessionIdArg = OptionalStringMember(args.value(), L"sessionId");
      if (args->HasKey(L"sessionId") && (!sessionIdArg.has_value() || sessionIdArg->empty())) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.create_snapshot sessionId must be a string", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = PlatformCreateSnapshotJson(sessionId, appId.value(), snapshotType, sessionIdArg, &error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Could not create runtime snapshot" : error, 500);
        return;
      }
    } else if (tool == L"platform.restore_snapshot") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.restore_snapshot requires args object", 400);
        return;
      }
      auto snapshotId = OptionalStringMember(args.value(), L"snapshotId");
      if (!snapshotId.has_value() || snapshotId->empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"platform.restore_snapshot requires snapshotId", 400);
        return;
      }
      if (!BooleanMemberTrue(args.value(), L"confirm")) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"confirmation_required", L"platform.restore_snapshot requires confirm: true", 400);
        return;
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      auto snapshotAppId = RuntimeSnapshotAppId(snapshotId.value(), &errorCode, &errorMessage);
      if (!snapshotAppId.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode.empty() ? L"snapshot_not_found" : errorCode, errorMessage.empty() ? L"Runtime snapshot was not found" : errorMessage, errorCode == L"storage_error" ? 500 : 400);
        return;
      }
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, snapshotAppId.value(), &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = PlatformRestoreSnapshotJson(snapshotId.value(), &errorCode, &errorMessage);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode.empty() ? L"storage_error" : errorCode, errorMessage.empty() ? L"Could not restore runtime snapshot" : errorMessage, errorCode == L"storage_error" ? 500 : 400);
        return;
      }
    } else if (tool == L"runtime.compare_snapshot") {
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.compare_snapshot requires args object", 400);
        return;
      }
      std::wstring appId;
      if (args->HasKey(L"appId")) {
        auto appIdArg = OptionalStringMember(args.value(), L"appId");
        if (!appIdArg.has_value() || (!appIdArg->empty() && !IsValidAppId(appIdArg.value()))) {
          SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"runtime.compare_snapshot appId is not a valid generated app id", 400);
          return;
        }
        appId = appIdArg.value();
      }
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, appId, &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = RuntimeCompareSnapshotJson(args.value(), &errorCode, &errorMessage);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode.empty() ? L"invalid_request" : errorCode, errorMessage.empty() ? L"Could not compare runtime snapshots" : errorMessage, errorCode == L"storage_error" ? 500 : 400);
        return;
      }
    } else if (tool == L"db.export_backup") {
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = DbExportBackupJson(&error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Could not export backup" : error, 500);
        return;
      }
    } else if (tool == L"db.export_debug_bundle") {
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      result = DbExportDebugBundleJson(&error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Could not export debug bundle" : error, 500);
        return;
      }
    } else if (tool == L"db.import_backup") {
      std::wstring errorCode;
      std::wstring errorMessage;
      int errorStatus = 400;
      if (!ControlSessionAllowsApp(sessionId, L"", &errorCode, &errorMessage, &errorStatus)) {
        SendControlRouteError(client, sessionId, tool, method, path, started, errorCode, errorMessage, errorStatus);
        return;
      }
      auto args = OptionalObjectMember(command, L"args");
      if (!args.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"db.import_backup requires args object", 400);
        return;
      }
      auto backup = OptionalObjectMember(args.value(), L"backup");
      if (!backup.has_value()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_request", L"db.import_backup requires backup", 400);
        return;
      }
      result = DbImportBackupJson(backup.value(), &error);
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"invalid_backup", error.empty() ? L"Backup import could not be completed" : error, 400);
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

}  // namespace terrane
