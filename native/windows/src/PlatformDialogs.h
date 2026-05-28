#pragma once

#include "BridgeTypes.h"

namespace nativeai {

class PlatformDialogs {
 public:
  winrt::Windows::Data::Json::JsonObject OpenFile(BridgeRequest const& request);
  winrt::Windows::Data::Json::JsonObject SaveFile(BridgeRequest const& request);
};

}  // namespace nativeai
