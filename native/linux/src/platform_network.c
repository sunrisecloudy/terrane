#include "platform_network.h"

JsonNode *platform_network_request(PlatformNetwork *network, const BridgeRequest *request) {
  (void)network;
  return bridge_failure(request, "platform_unsupported", "network.request will be wired through libsoup after manifest networkPolicy enforcement lands", NULL);
}
