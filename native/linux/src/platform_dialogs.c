#include "platform_dialogs.h"

JsonNode *platform_dialogs_open_file(PlatformDialogs *dialogs, const BridgeRequest *request) {
  (void)dialogs;
  return bridge_failure(request, "platform_unsupported", "dialog.openFile will be wired through GtkFileDialog after window ownership lands", NULL);
}

JsonNode *platform_dialogs_save_file(PlatformDialogs *dialogs, const BridgeRequest *request) {
  (void)dialogs;
  return bridge_failure(request, "platform_unsupported", "dialog.saveFile will be wired through GtkFileDialog after window ownership lands", NULL);
}
