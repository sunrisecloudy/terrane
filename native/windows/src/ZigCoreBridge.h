#pragma once

#include "BridgeTypes.h"

namespace nativeai {

class ZigCoreBridge {
 public:
  winrt::Windows::Data::Json::JsonObject Step(BridgeRequest const& request);
};

}  // namespace nativeai
