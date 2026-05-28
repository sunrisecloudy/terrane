#pragma once

#include "BridgeTypes.h"

#include <Windows.h>

namespace nativeai {

class PlatformDialogs {
 public:
  explicit PlatformDialogs(HWND ownerWindow = nullptr);

  winrt::Windows::Data::Json::JsonObject OpenFile(BridgeRequest const& request);
  winrt::Windows::Data::Json::JsonObject SaveFile(BridgeRequest const& request);

 private:
  HWND ownerWindow_ = nullptr;
};

}  // namespace nativeai
