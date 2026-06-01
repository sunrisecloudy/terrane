#include "PlatformNotifications.h"

namespace terrane {
namespace json = winrt::Windows::Data::Json;

namespace {

bool TryGetStringMember(json::JsonObject const& object, std::wstring const& member, std::wstring& out) {
  if (!object.HasKey(member)) {
    return false;
  }
  auto value = object.GetNamedValue(member);
  if (value.ValueType() != json::JsonValueType::String) {
    return false;
  }
  out = value.GetString().c_str();
  return true;
}

bool ValidNotificationLevel(std::wstring const& level) {
  return level == L"info" || level == L"success" || level == L"warning" || level == L"error";
}

}  // namespace

json::JsonObject PlatformNotifications::Toast(BridgeRequest const& request) {
  std::wstring message;
  if (!TryGetStringMember(request.params, L"message", message)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"notification.toast requires message");
  }

  if (request.params.HasKey(L"level")) {
    auto levelValue = request.params.GetNamedValue(L"level");
    if (levelValue.ValueType() != json::JsonValueType::Null) {
      std::wstring level;
      if (!TryGetStringMember(request.params, L"level", level)) {
        return BridgeResponse::Failure(
            request.id,
            request.hasId,
            L"invalid_request",
            L"notification.toast level must be a string");
      }
      if (!ValidNotificationLevel(level)) {
        json::JsonObject details;
        details.Insert(L"level", json::JsonValue::CreateStringValue(level));
        return BridgeResponse::Failure(
            request.id,
            request.hasId,
            L"invalid_request",
            L"notification.toast level must be info, success, warning, or error",
            details);
      }
    }
  }

  json::JsonObject result;
  result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
  return BridgeResponse::Success(request.id, request.hasId, result);
}

}  // namespace terrane
