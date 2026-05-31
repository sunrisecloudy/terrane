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
#include "ZigCoreBridge.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <cctype>
#include <cwctype>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <optional>
#include <regex>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
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
    *error = tool + L" appId must be a string";
    return false;
  }
  *appId = value.value();
  if (!appId->empty() && !IsValidAppId(*appId)) {
    *error = tool + L" appId is not a valid generated app id";
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
    auto activeVersion = ActiveVersionForApp(db, appId);
    auto dataVersion = DataVersionForAppJson(db, appId);
    auto storageRows = SafeTableRowsJson(db, "app_storage", {"app_id", "key", "value_json", "updated_at"}, "key", "app_id", appId);
    auto snapshotJson = L"{\"appId\":" + JsonString(appId) +
        L",\"activeInstallId\":" + JsonNullableString(installId) +
        L",\"activeVersion\":" + JsonNullableString(activeVersion) +
        L",\"dataVersion\":" + dataVersion +
        L",\"storage\":" + storageRows +
        L",\"createdAt\":" + JsonString(createdAt) + L"}";
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
    ZigCoreBridge replayCore;
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
    } else if (tool == L"runtime.network_mock_set" ||
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
      if (tool == L"runtime.network_mock_set") {
        result = RuntimeNetworkMockSetJson(args, &error);
      } else if (tool == L"runtime.network_mock_reset") {
        result = RuntimeNetworkMockResetJson(args, &error);
      } else {
        result = RuntimeDialogMockSetJson(args, &error);
      }
      if (result.empty()) {
        SendControlRouteError(client, sessionId, tool, method, path, started, L"storage_error", error.empty() ? L"Mock control operation failed" : error, 500);
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

}  // namespace nativeai
