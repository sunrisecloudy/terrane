#include "ZigCoreBridge.h"

namespace nativeai {

winrt::Windows::Data::Json::JsonObject ZigCoreBridge::Step(BridgeRequest const& request) {
  return BridgeResponse::Failure(
      request.id,
      request.hasId,
      L"platform_unsupported",
      L"core.step requires loading zig_core.dll into the Windows host");
}

}  // namespace nativeai
