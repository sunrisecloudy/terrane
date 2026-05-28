#include "BridgeTypes.h"

#include <Windows.h>

namespace nativeai {
namespace json = winrt::Windows::Data::Json;

json::JsonObject BridgeResponse::Success(
    std::wstring const& id,
    bool hasId,
    json::IJsonValue const& result) {
  json::JsonObject body;
  body.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
  body.Insert(L"result", result);
  if (hasId) {
    body.Insert(L"id", json::JsonValue::CreateStringValue(id));
  }
  return body;
}

json::JsonObject BridgeResponse::Failure(
    std::wstring const& id,
    bool hasId,
    std::wstring const& code,
    std::wstring const& message,
    json::JsonObject const& details) {
  json::JsonObject error;
  error.Insert(L"code", json::JsonValue::CreateStringValue(code));
  error.Insert(L"message", json::JsonValue::CreateStringValue(message));
  error.Insert(L"details", details);

  json::JsonObject body;
  body.Insert(L"ok", json::JsonValue::CreateBooleanValue(false));
  body.Insert(L"error", error);
  if (hasId) {
    body.Insert(L"id", json::JsonValue::CreateStringValue(id));
  }
  return body;
}

std::string WideToUtf8(std::wstring const& value) {
  if (value.empty()) {
    return {};
  }
  int bytes = WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
  std::string out(bytes, '\0');
  WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), out.data(), bytes, nullptr, nullptr);
  return out;
}

std::wstring Utf8ToWide(std::string const& value) {
  if (value.empty()) {
    return {};
  }
  int chars = MultiByteToWideChar(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0);
  std::wstring out(chars, L'\0');
  MultiByteToWideChar(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), out.data(), chars);
  return out;
}

}  // namespace nativeai
