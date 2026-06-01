#pragma once

#include "BridgeTypes.h"

namespace terrane {

class PlatformNotifications {
 public:
  winrt::Windows::Data::Json::JsonObject Toast(BridgeRequest const& request);
};

}  // namespace terrane
