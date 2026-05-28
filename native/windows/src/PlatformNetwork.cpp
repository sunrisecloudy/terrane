#include "PlatformNetwork.h"

#include <winhttp.h>

#include <algorithm>
#include <cwctype>
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
  parsed.path.append(components.lpszExtraInfo, components.dwExtraInfoLength);
  if (parsed.path.empty()) {
    parsed.path = L"/";
  }
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
    std::map<std::wstring, std::wstring> const& headers) {
  if (rule.origin != origin || !rule.methods.contains(method)) {
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
    std::map<std::wstring, std::wstring> const& headers) {
  for (auto const& rule : rules) {
    if (RuleAllows(rule, origin, method, headers)) {
      return &rule;
    }
  }
  return nullptr;
}

json::JsonObject Failure(BridgeRequest const& request, std::wstring const& code, std::wstring const& message) {
  return BridgeResponse::Failure(request.id, request.hasId, code, message);
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

std::optional<std::wstring> ReadBody(HINTERNET request, uint32_t maxBytes) {
  std::string body;
  while (true) {
    DWORD available = 0;
    if (!WinHttpQueryDataAvailable(request, &available)) {
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
      return std::nullopt;
    }
    chunk.resize(read);
    body += chunk;
  }
  return Utf8ToWide(body);
}

std::optional<std::wstring> RedirectUrl(ParsedUrl const& current, HINTERNET request) {
  auto location = Header(request, WINHTTP_QUERY_LOCATION);
  if (!location.has_value() || location->empty()) {
    return std::nullopt;
  }
  if (location->starts_with(L"http://") || location->starts_with(L"https://")) {
    return location;
  }
  if (location->starts_with(L"/")) {
    return current.origin + location.value();
  }
  return std::nullopt;
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

  auto rule = FindRule(request.context.networkPolicy, parsed->origin, method, headers.value());
  if (rule == nullptr) {
    return Failure(request, L"network_policy_denied", L"network.request is not allowed by manifest.networkPolicy");
  }
  if (body.hasBody && body.bytes.size() > rule->maxRequestBytes) {
    return Failure(request, L"network_policy_denied", L"network.request body exceeds manifest.networkPolicy maxRequestBytes");
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

    auto timeout = static_cast<int>(rule->timeoutMs);
    WinHttpSetTimeouts(httpRequest.value, timeout, timeout, timeout, timeout);
    DWORD disabled = WINHTTP_DISABLE_REDIRECTS;
    WinHttpSetOption(httpRequest.value, WINHTTP_OPTION_DISABLE_FEATURE, &disabled, sizeof(disabled));
    for (auto const& [name, value] : headers.value()) {
      auto headerLine = name + L": " + value;
      WinHttpAddRequestHeaders(httpRequest.value, headerLine.c_str(), static_cast<DWORD>(-1), WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE);
    }

    void* sendBody = body.hasBody ? static_cast<void*>(body.bytes.data()) : nullptr;
    auto sendBodySize = body.hasBody ? static_cast<DWORD>(body.bytes.size()) : 0;
    if (!WinHttpSendRequest(httpRequest.value, WINHTTP_NO_ADDITIONAL_HEADERS, 0, sendBody, sendBodySize, sendBodySize, 0) ||
        !WinHttpReceiveResponse(httpRequest.value, nullptr)) {
      return Failure(request, L"network_error", L"network.request failed");
    }

    auto status = StatusCode(httpRequest.value);
    if (status >= 300 && status < 400) {
      auto next = RedirectUrl(current, httpRequest.value);
      auto nextParsed = next.has_value() ? ParseUrl(next.value()) : std::nullopt;
      auto nextRule = nextParsed.has_value() ? FindRule(request.context.networkPolicy, nextParsed->origin, method, headers.value()) : nullptr;
      if (!nextParsed.has_value() || nextRule == nullptr) {
        return Failure(request, L"network_policy_denied", L"network.request redirect is not allowed by manifest.networkPolicy");
      }
      rule = nextRule;
      current = nextParsed.value();
      continue;
    }

    auto bodyText = ReadBody(httpRequest.value, rule->maxResponseBytes);
    if (!bodyText.has_value()) {
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
