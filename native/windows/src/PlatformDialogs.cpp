#include "PlatformDialogs.h"

namespace nativeai {

winrt::Windows::Data::Json::JsonObject PlatformDialogs::OpenFile(BridgeRequest const& request) {
  return BridgeResponse::Failure(
      request.id,
      request.hasId,
      L"platform_unsupported",
      L"dialog.openFile will be wired through Win32 common dialogs after the window owner plumbing lands");
}

winrt::Windows::Data::Json::JsonObject PlatformDialogs::SaveFile(BridgeRequest const& request) {
  return BridgeResponse::Failure(
      request.id,
      request.hasId,
      L"platform_unsupported",
      L"dialog.saveFile will be wired through Win32 common dialogs after the window owner plumbing lands");
}

}  // namespace nativeai
