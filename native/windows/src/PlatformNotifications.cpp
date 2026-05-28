#include "PlatformNotifications.h"

namespace nativeai {
namespace json = winrt::Windows::Data::Json;

json::JsonObject PlatformNotifications::Toast(BridgeRequest const& request) {
  json::JsonObject result;
  result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
  return BridgeResponse::Success(request.id, request.hasId, result);
}

}  // namespace nativeai
