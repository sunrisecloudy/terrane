#include "PlatformNetwork.h"

#include <winhttp.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <cwctype>
#include <limits>
#include <map>
#include <optional>
#include <sstream>
#include <vector>

namespace nativeai {
namespace json = winrt::Windows::Data::Json;

namespace {

struct ParsedUrl {
  std::wstring origin;
  std::wstring host;
  std::wstring path;
  std::wstring policyPath;
  INTERNET_PORT port = 0;
  bool secure = false;
};

struct RequestBody {
  bool valid = true;
  std::string bytes;
  bool hasBody = false;
};

struct HttpHandle {
  HINTERNET value = nullptr;
  ~HttpHandle() {
    if (value != nullptr) {
      WinHttpCloseHandle(value);
    }
  }
};

std::wstring ToUpper(std::wstring value) {
  std::transform(value.begin(), value.end(), value.begin(), [](wchar_t ch) { return static_cast<wchar_t>(std::towupper(ch)); });
  return value;
}

std::wstring ToLower(std::wstring value) {
  std::transform(value.begin(), value.end(), value.begin(), [](wchar_t ch) { return static_cast<wchar_t>(std::towlower(ch)); });
  return value;
}

std::optional<ParsedUrl> ParseUrl(std::wstring const& text) {
  URL_COMPONENTS components{};
  components.dwStructSize = sizeof(components);
  std::vector<wchar_t> host(512);
  std::vector<wchar_t> path(4096);
  std::vector<wchar_t> extra(4096);
  components.lpszHostName = host.data();
  components.dwHostNameLength = static_cast<DWORD>(host.size());
  components.lpszUrlPath = path.data();
  components.dwUrlPathLength = static_cast<DWORD>(path.size());
  components.lpszExtraInfo = extra.data();
  components.dwExtraInfoLength = static_cast<DWORD>(extra.size());

  if (!WinHttpCrackUrl(text.c_str(), 0, 0, &components)) {
    return std::nullopt;
  }

  ParsedUrl parsed;
  if (components.nScheme == INTERNET_SCHEME_HTTP) {
    parsed.origin = L"http://";
    parsed.secure = false;
  } else if (components.nScheme == INTERNET_SCHEME_HTTPS) {
    parsed.origin = L"https://";
    parsed.secure = true;
  } else {
    return std::nullopt;
  }

  parsed.host.assign(components.lpszHostName, components.dwHostNameLength);
  parsed.host = ToLower(parsed.host);
  parsed.port = components.nPort;
  parsed.origin += parsed.host;
  if ((components.nScheme == INTERNET_SCHEME_HTTP && parsed.port != INTERNET_DEFAULT_HTTP_PORT) ||
      (components.nScheme == INTERNET_SCHEME_HTTPS && parsed.port != INTERNET_DEFAULT_HTTPS_PORT)) {
    parsed.origin += L":" + std::to_wstring(parsed.port);
  }

  parsed.path.assign(components.lpszUrlPath, components.dwUrlPathLength);
  if (parsed.path.empty()) {
    parsed.path = L"/";
  }
  parsed.policyPath = parsed.path;
  parsed.path.append(components.lpszExtraInfo, components.dwExtraInfoLength);
  return parsed;
}

std::optional<std::map<std::wstring, std::wstring>> RequestHeaders(json::JsonObject const& params) {
  std::map<std::wstring, std::wstring> headers;
  if (!params.HasKey(L"headers")) {
    return headers;
  }
  auto value = params.GetNamedValue(L"headers");
  if (value.ValueType() == json::JsonValueType::Null) {
    return headers;
  }
  if (value.ValueType() != json::JsonValueType::Object) {
    return std::nullopt;
  }

  for (auto const& entry : value.GetObject()) {
    if (entry.Value().ValueType() != json::JsonValueType::String) {
      return std::nullopt;
    }
    headers[ToLower(std::wstring(entry.Key().c_str()))] = std::wstring(entry.Value().GetString().c_str());
  }
  return headers;
}

RequestBody RequestBodyFromParams(json::JsonObject const& params) {
  if (!params.HasKey(L"body")) {
    return {};
  }
  auto value = params.GetNamedValue(L"body");
  if (value.ValueType() == json::JsonValueType::Null) {
    return {};
  }
  if (value.ValueType() != json::JsonValueType::String) {
    return RequestBody{.valid = false};
  }
  auto text = std::wstring(value.GetString().c_str());
  return RequestBody{.valid = true, .bytes = WideToUtf8(text), .hasBody = true};
}

bool RuleAllows(
    NetworkPolicyRule const& rule,
    std::wstring const& origin,
    std::wstring const& method,
    std::wstring const& path,
    std::map<std::wstring, std::wstring> const& headers) {
  if (rule.origin != origin || !rule.methods.contains(method)) {
    return false;
  }
  if (!rule.pathPrefix.empty() && path.rfind(rule.pathPrefix, 0) != 0) {
    return false;
  }
  for (auto const& [name, _] : headers) {
    auto normalized = ToLower(name);
    if (normalized == L"cookie" || normalized == L"set-cookie" || !rule.allowedHeaders.contains(normalized)) {
      return false;
    }
  }
  return true;
}

NetworkPolicyRule const* FindRule(
    std::vector<NetworkPolicyRule> const& rules,
    std::wstring const& origin,
    std::wstring const& method,
    std::wstring const& path,
    std::map<std::wstring, std::wstring> const& headers) {
  for (auto const& rule : rules) {
    if (RuleAllows(rule, origin, method, path, headers)) {
      return &rule;
    }
  }
  return nullptr;
}

bool EndsWith(std::wstring const& value, std::wstring const& suffix) {
  return value.size() >= suffix.size() && value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}

std::optional<std::array<uint8_t, 4>> ParseIpv4Host(std::wstring const& host) {
  std::array<uint8_t, 4> octets{};
  size_t start = 0;
  for (size_t index = 0; index < 4; ++index) {
    auto end = index == 3 ? host.size() : host.find(L".", start);
    if (end == std::wstring::npos || end == start || end - start > 3) {
      return std::nullopt;
    }
    uint32_t value = 0;
    for (size_t pos = start; pos < end; ++pos) {
      if (host[pos] < L'0' || host[pos] > L'9') {
        return std::nullopt;
      }
      value = value * 10 + static_cast<uint32_t>(host[pos] - L'0');
      if (value > 255) {
        return std::nullopt;
      }
    }
    octets[index] = static_cast<uint8_t>(value);
    start = end + 1;
  }
  if (start != host.size() + 1) {
    return std::nullopt;
  }
  return octets;
}

bool IsPrivateIpv4(std::array<uint8_t, 4> const& octets) {
  auto first = octets[0];
  auto second = octets[1];
  return first == 0 ||
      first == 10 ||
      first == 127 ||
      (first == 100 && second >= 64 && second <= 127) ||
      (first == 169 && second == 254) ||
      (first == 172 && second >= 16 && second <= 31) ||
      (first == 192 && second == 168);
}

std::optional<uint16_t> ParseHex16(std::wstring const& text) {
  if (text.empty() || text.size() > 4) {
    return std::nullopt;
  }
  uint16_t value = 0;
  for (wchar_t ch : text) {
    uint16_t digit = 0;
    if (ch >= L'0' && ch <= L'9') {
      digit = static_cast<uint16_t>(ch - L'0');
    } else if (ch >= L'a' && ch <= L'f') {
      digit = static_cast<uint16_t>(ch - L'a' + 10);
    } else if (ch >= L'A' && ch <= L'F') {
      digit = static_cast<uint16_t>(ch - L'A' + 10);
    } else {
      return std::nullopt;
    }
    value = static_cast<uint16_t>(value * 16 + digit);
  }
  return value;
}

bool IsPrivateIpv4MappedHost(std::wstring const& tail) {
  if (auto dotted = ParseIpv4Host(tail)) {
    return IsPrivateIpv4(dotted.value());
  }
  auto separator = tail.find(L":");
  if (separator == std::wstring::npos || tail.find(L":", separator + 1) != std::wstring::npos) {
    return false;
  }
  auto high = ParseHex16(tail.substr(0, separator));
  auto low = ParseHex16(tail.substr(separator + 1));
  if (!high.has_value() || !low.has_value()) {
    return false;
  }
  std::array<uint8_t, 4> octets{
      static_cast<uint8_t>(high.value() >> 8),
      static_cast<uint8_t>(high.value() & 0x00ff),
      static_cast<uint8_t>(low.value() >> 8),
      static_cast<uint8_t>(low.value() & 0x00ff),
  };
  return IsPrivateIpv4(octets);
}

bool IsPrivateNetworkHost(std::wstring host) {
  host = ToLower(host);
  if (host.size() >= 2 && host.front() == L'[' && host.back() == L']') {
    host = host.substr(1, host.size() - 2);
  }
  if (auto zone = host.find(L"%"); zone != std::wstring::npos) {
    host = host.substr(0, zone);
  }
  if (host == L"localhost" || EndsWith(host, L".localhost")) {
    return true;
  }
  if (auto ipv4 = ParseIpv4Host(host)) {
    return IsPrivateIpv4(ipv4.value());
  }
  if (host == L"::1") {
    return true;
  }
  if (host.starts_with(L"fc") || host.starts_with(L"fd")) {
    return true;
  }
  if (host.starts_with(L"fe8") || host.starts_with(L"fe9") || host.starts_with(L"fea") || host.starts_with(L"feb")) {
    return true;
  }
  if (host.starts_with(L"::ffff:")) {
    return IsPrivateIpv4MappedHost(host.substr(7));
  }
  return false;
}

json::JsonObject Failure(BridgeRequest const& request, std::wstring const& code, std::wstring const& message) {
  return BridgeResponse::Failure(request.id, request.hasId, code, message);
}

json::JsonObject InvalidTimeoutFailure(BridgeRequest const& request) {
  json::JsonObject details;
  details.Insert(L"timeoutMs", request.params.GetNamedValue(L"timeoutMs"));
  return BridgeResponse::Failure(
      request.id,
      request.hasId,
      L"invalid_request",
      L"network.request timeoutMs must be a positive integer",
      details);
}

json::JsonObject TimeoutFailure(BridgeRequest const& request, uint32_t timeoutMs) {
  json::JsonObject details;
  details.Insert(L"timeoutMs", json::JsonValue::CreateNumberValue(timeoutMs));
  return BridgeResponse::Failure(request.id, request.hasId, L"timeout", L"network.request timed out", details);
}

std::optional<uint32_t> RequestedTimeoutMs(json::JsonObject const& params, bool& invalid) {
  invalid = false;
  if (!params.HasKey(L"timeoutMs")) {
    return std::nullopt;
  }
  auto value = params.GetNamedValue(L"timeoutMs");
  if (value.ValueType() != json::JsonValueType::Number) {
    invalid = true;
    return std::nullopt;
  }
  auto timeout = value.GetNumber();
  if (!std::isfinite(timeout) ||
      std::floor(timeout) != timeout ||
      timeout <= 0 ||
      timeout > static_cast<double>(std::numeric_limits<int>::max())) {
    invalid = true;
    return std::nullopt;
  }
  return static_cast<uint32_t>(timeout);
}

uint32_t EffectiveTimeoutMs(NetworkPolicyRule const& rule, std::optional<uint32_t> requestedTimeout) {
  return requestedTimeout.has_value() ? std::min(rule.timeoutMs, requestedTimeout.value()) : rule.timeoutMs;
}

json::JsonObject NetworkTransportFailure(BridgeRequest const& request, DWORD error, uint32_t timeoutMs) {
  if (error == ERROR_WINHTTP_TIMEOUT) {
    return TimeoutFailure(request, timeoutMs);
  }
  return Failure(request, L"network_error", L"network.request failed");
}

std::optional<std::wstring> Header(HINTERNET request, DWORD header) {
  DWORD bytes = 0;
  WinHttpQueryHeaders(request, header, WINHTTP_HEADER_NAME_BY_INDEX, nullptr, &bytes, WINHTTP_NO_HEADER_INDEX);
  if (GetLastError() != ERROR_INSUFFICIENT_BUFFER) {
    return std::nullopt;
  }
  std::wstring value(bytes / sizeof(wchar_t), L'\0');
  if (!WinHttpQueryHeaders(request, header, WINHTTP_HEADER_NAME_BY_INDEX, value.data(), &bytes, WINHTTP_NO_HEADER_INDEX)) {
    return std::nullopt;
  }
  while (!value.empty() && value.back() == L'\0') {
    value.pop_back();
  }
  return value;
}

DWORD StatusCode(HINTERNET request) {
  DWORD status = 0;
  DWORD bytes = sizeof(status);
  WinHttpQueryHeaders(
      request,
      WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
      WINHTTP_HEADER_NAME_BY_INDEX,
      &status,
      &bytes,
      WINHTTP_NO_HEADER_INDEX);
  return status;
}

json::JsonObject ResponseHeaders(HINTERNET request) {
  json::JsonObject headers;
  auto raw = Header(request, WINHTTP_QUERY_RAW_HEADERS_CRLF);
  if (!raw.has_value()) {
    return headers;
  }

  std::wstringstream lines(raw.value());
  std::wstring line;
  bool first = true;
  while (std::getline(lines, line)) {
    if (!line.empty() && line.back() == L'\r') {
      line.pop_back();
    }
    if (first) {
      first = false;
      continue;
    }
    auto colon = line.find(L":");
    if (colon == std::wstring::npos) {
      continue;
    }
    auto name = ToLower(line.substr(0, colon));
    auto value = line.substr(colon + 1);
    while (!value.empty() && value.front() == L' ') {
      value.erase(value.begin());
    }
    headers.Insert(name, json::JsonValue::CreateStringValue(value));
  }
  return headers;
}

std::optional<std::wstring> ReadBody(HINTERNET request, uint32_t maxBytes, DWORD* lastError) {
  if (lastError != nullptr) {
    *lastError = ERROR_SUCCESS;
  }
  std::string body;
  while (true) {
    DWORD available = 0;
    if (!WinHttpQueryDataAvailable(request, &available)) {
      if (lastError != nullptr) {
        *lastError = GetLastError();
      }
      return std::nullopt;
    }
    if (available == 0) {
      break;
    }
    if (body.size() + available > maxBytes) {
      return std::nullopt;
    }
    std::string chunk(available, '\0');
    DWORD read = 0;
    if (!WinHttpReadData(request, chunk.data(), available, &read)) {
      if (lastError != nullptr) {
        *lastError = GetLastError();
      }
      return std::nullopt;
    }
    chunk.resize(read);
    body += chunk;
  }
  return Utf8ToWide(body);
}

std::wstring NormalizeRedirectPath(std::wstring const& path) {
  std::vector<std::wstring> segments;
  size_t start = 0;
  while (start <= path.size()) {
    size_t slash = path.find(L'/', start);
    auto segment = path.substr(start, slash == std::wstring::npos ? std::wstring::npos : slash - start);
    if (segment.empty() || segment == L".") {
      // Skip empty and current-directory segments.
    } else if (segment == L"..") {
      if (!segments.empty()) {
        segments.pop_back();
      }
    } else {
      segments.push_back(segment);
    }
    if (slash == std::wstring::npos) {
      break;
    }
    start = slash + 1;
  }

  std::wstring normalized = L"/";
  for (size_t index = 0; index < segments.size(); ++index) {
    if (index > 0) {
      normalized += L"/";
    }
    normalized += segments[index];
  }
  if (!path.empty() && path.back() == L'/' && normalized.back() != L'/') {
    normalized += L"/";
  }
  return normalized;
}

std::wstring BaseDirectoryPath(std::wstring const& path) {
  if (path.empty() || path == L"/") {
    return L"/";
  }
  if (path.back() == L'/') {
    return path;
  }
  auto slash = path.find_last_of(L'/');
  if (slash == std::wstring::npos || slash == 0) {
    return L"/";
  }
  return path.substr(0, slash + 1);
}

std::wstring ResolveRedirectUrl(ParsedUrl const& current, std::wstring const& location) {
  if (location.starts_with(L"http://") || location.starts_with(L"https://")) {
    return location;
  }
  if (location.starts_with(L"//")) {
    return std::wstring(current.secure ? L"https:" : L"http:") + location;
  }
  if (location.starts_with(L"/")) {
    return current.origin + NormalizeRedirectPath(location);
  }
  if (location.starts_with(L"?") || location.starts_with(L"#")) {
    return current.origin + current.policyPath + location;
  }

  auto suffixStart = location.find_first_of(L"?#");
  auto relativePath = suffixStart == std::wstring::npos ? location : location.substr(0, suffixStart);
  auto suffix = suffixStart == std::wstring::npos ? std::wstring() : location.substr(suffixStart);
  return current.origin + NormalizeRedirectPath(BaseDirectoryPath(current.policyPath) + relativePath) + suffix;
}

std::optional<std::wstring> RedirectUrl(ParsedUrl const& current, HINTERNET request) {
  auto location = Header(request, WINHTTP_QUERY_LOCATION);
  if (!location.has_value() || location->empty()) {
    return std::nullopt;
  }
  return ResolveRedirectUrl(current, location.value());
}

}  // namespace

json::JsonObject PlatformNetwork::Request(BridgeRequest const& request) {
  auto urlText = std::wstring(request.params.GetNamedString(L"url", L"").c_str());
  auto parsed = ParseUrl(urlText);
  if (!parsed.has_value()) {
    return Failure(request, L"invalid_request", L"network.request requires an absolute http or https url");
  }

  auto method = ToUpper(std::wstring(request.params.GetNamedString(L"method", L"GET").c_str()));
  auto headers = RequestHeaders(request.params);
  if (!headers.has_value()) {
    return Failure(request, L"invalid_request", L"network.request headers must be strings");
  }
  auto body = RequestBodyFromParams(request.params);
  if (!body.valid) {
    return Failure(request, L"invalid_request", L"network.request body must be a string or null");
  }
  if (request.params.HasKey(L"credentials") && request.params.GetNamedValue(L"credentials").ValueType() != json::JsonValueType::Null) {
    return Failure(request, L"network_policy_denied", L"network.request credentials are not allowed");
  }
  if (request.context.denyPrivateNetwork && IsPrivateNetworkHost(parsed->host)) {
    return Failure(request, L"network_policy_denied", L"network.request private network targets are denied");
  }

  auto rule = FindRule(request.context.networkPolicy, parsed->origin, method, parsed->policyPath, headers.value());
  if (rule == nullptr) {
    return Failure(request, L"network_policy_denied", L"network.request is not allowed by manifest.networkPolicy");
  }
  if (body.hasBody && body.bytes.size() > rule->maxRequestBytes) {
    return Failure(request, L"network_policy_denied", L"network.request body exceeds manifest.networkPolicy maxRequestBytes");
  }
  bool invalidTimeout = false;
  auto requestedTimeout = RequestedTimeoutMs(request.params, invalidTimeout);
  if (invalidTimeout) {
    return InvalidTimeoutFailure(request);
  }

  HttpHandle session{WinHttpOpen(L"NativeAIWebappPlatform/0.1", WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, nullptr, nullptr, 0)};
  if (session.value == nullptr) {
    return Failure(request, L"network_error", L"WinHTTP session creation failed");
  }

  ParsedUrl current = parsed.value();
  for (int redirects = 0; redirects < 6; ++redirects) {
    HttpHandle connect{WinHttpConnect(session.value, current.host.c_str(), current.port, 0)};
    if (connect.value == nullptr) {
      return Failure(request, L"network_error", L"WinHTTP connection failed");
    }

    HttpHandle httpRequest{
        WinHttpOpenRequest(connect.value, method.c_str(), current.path.c_str(), nullptr, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, current.secure ? WINHTTP_FLAG_SECURE : 0)};
    if (httpRequest.value == nullptr) {
      return Failure(request, L"network_error", L"WinHTTP request creation failed");
    }

    auto effectiveTimeoutMs = EffectiveTimeoutMs(*rule, requestedTimeout);
    auto timeout = static_cast<int>(effectiveTimeoutMs);
    WinHttpSetTimeouts(httpRequest.value, timeout, timeout, timeout, timeout);
    DWORD disabled = WINHTTP_DISABLE_REDIRECTS;
    WinHttpSetOption(httpRequest.value, WINHTTP_OPTION_DISABLE_FEATURE, &disabled, sizeof(disabled));
    for (auto const& [name, value] : headers.value()) {
      auto headerLine = name + L": " + value;
      WinHttpAddRequestHeaders(httpRequest.value, headerLine.c_str(), static_cast<DWORD>(-1), WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE);
    }

    void* sendBody = body.hasBody ? static_cast<void*>(body.bytes.data()) : nullptr;
    auto sendBodySize = body.hasBody ? static_cast<DWORD>(body.bytes.size()) : 0;
    if (!WinHttpSendRequest(httpRequest.value, WINHTTP_NO_ADDITIONAL_HEADERS, 0, sendBody, sendBodySize, sendBodySize, 0)) {
      return NetworkTransportFailure(request, GetLastError(), effectiveTimeoutMs);
    }
    if (!WinHttpReceiveResponse(httpRequest.value, nullptr)) {
      return NetworkTransportFailure(request, GetLastError(), effectiveTimeoutMs);
    }

    auto status = StatusCode(httpRequest.value);
    if (status >= 300 && status < 400) {
      auto next = RedirectUrl(current, httpRequest.value);
      auto nextParsed = next.has_value() ? ParseUrl(next.value()) : std::nullopt;
      auto nextRule = nextParsed.has_value() ? FindRule(request.context.networkPolicy, nextParsed->origin, method, nextParsed->policyPath, headers.value()) : nullptr;
      if (!nextParsed.has_value() || (request.context.denyPrivateNetwork && IsPrivateNetworkHost(nextParsed->host)) || nextRule == nullptr) {
        return Failure(request, L"network_policy_denied", L"network.request redirect is not allowed by manifest.networkPolicy");
      }
      rule = nextRule;
      current = nextParsed.value();
      continue;
    }

    DWORD readError = ERROR_SUCCESS;
    auto bodyText = ReadBody(httpRequest.value, rule->maxResponseBytes, &readError);
    if (!bodyText.has_value()) {
      if (readError == ERROR_WINHTTP_TIMEOUT) {
        return TimeoutFailure(request, effectiveTimeoutMs);
      }
      return Failure(request, L"network_policy_denied", L"network.response exceeds manifest.networkPolicy maxResponseBytes");
    }

    json::JsonObject result;
    result.Insert(L"status", json::JsonValue::CreateNumberValue(status));
    result.Insert(L"headers", ResponseHeaders(httpRequest.value));
    result.Insert(L"bodyText", json::JsonValue::CreateStringValue(bodyText.value()));
    return BridgeResponse::Success(request.id, request.hasId, result);
  }

  return Failure(request, L"network_error", L"network.request exceeded redirect limit");
}

}  // namespace nativeai
