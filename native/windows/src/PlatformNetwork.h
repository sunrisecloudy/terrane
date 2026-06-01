#pragma once

#include "BridgeTypes.h"

#include <winsqlite/winsqlite3.h>

namespace terrane {

class PlatformNetwork {
 public:
  winrt::Windows::Data::Json::JsonObject Request(BridgeRequest const& request, sqlite3* db = nullptr);
};

}  // namespace terrane
