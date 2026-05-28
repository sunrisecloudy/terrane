#include "PlatformNetwork.h"

namespace nativeai {

winrt::Windows::Data::Json::JsonObject PlatformNetwork::Request(BridgeRequest const& request) {
  return BridgeResponse::Failure(
      request.id,
      request.hasId,
      L"platform_unsupported",
      L"network.request will be wired through WinHTTP after manifest networkPolicy enforcement lands");
}

}  // namespace nativeai
