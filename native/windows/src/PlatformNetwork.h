#pragma once

#include "BridgeTypes.h"

namespace nativeai {

class PlatformNetwork {
 public:
  winrt::Windows::Data::Json::JsonObject Request(BridgeRequest const& request);
};

}  // namespace nativeai
