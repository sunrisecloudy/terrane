#include "zig_core_bridge.h"

JsonNode *zig_core_bridge_step(ZigCoreBridge *core, const BridgeRequest *request) {
  (void)core;
  return bridge_failure(request, "platform_unsupported", "core.step requires loading libzig_core.so into the Linux host", NULL);
}
