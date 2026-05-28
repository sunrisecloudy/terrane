#pragma once

#include "BridgeTypes.h"

namespace nativeai {

class PlatformNotifications {
 public:
  winrt::Windows::Data::Json::JsonObject Toast(BridgeRequest const& request);
};

}  // namespace nativeai
