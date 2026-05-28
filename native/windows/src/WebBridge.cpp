#include "WebBridge.h"

namespace nativeai {
namespace json = winrt::Windows::Data::Json;

WebBridge::WebBridge(std::filesystem::path databasePath) : storage_(std::move(databasePath)) {}

std::wstring WebBridge::HandleJson(std::wstring const& body, AppSandboxContext const& context) {
  BridgeRequest request;
  request.context = context;

  json::JsonObject parsed{nullptr};
  if (!json::JsonObject::TryParse(body, parsed)) {
    return BridgeResponse::Failure(L"", false, L"invalid_request", L"Bridge message body must be JSON").Stringify().c_str();
  }

  if (parsed.HasKey(L"id")) {
    request.hasId = true;
    request.id = parsed.GetNamedString(L"id", L"").c_str();
  }
  request.method = parsed.GetNamedString(L"method", L"").c_str();
  request.params = parsed.GetNamedObject(L"params", json::JsonObject());

  if (auto permission = permissionForBridgeMethod(request.method);
      permission.has_value() && !request.context.approvedPermissions.contains(permission.value())) {
    json::JsonObject details;
    details.Insert(L"appId", json::JsonValue::CreateStringValue(request.context.appId));
    details.Insert(L"method", json::JsonValue::CreateStringValue(request.method));
    details.Insert(L"requiredPermission", json::JsonValue::CreateStringValue(permission.value()));
    return BridgeResponse::Failure(
               request.id,
               request.hasId,
               L"permission_denied",
               L"App " + request.context.appId + L" cannot call " + request.method,
               details)
        .Stringify()
        .c_str();
  }

  return Dispatch(request).Stringify().c_str();
}

std::optional<std::wstring> WebBridge::permissionForBridgeMethod(std::wstring const& method) const {
  if (method == L"storage.get" || method == L"storage.list") {
    return L"storage.read";
  }
  if (method == L"storage.set" || method == L"storage.remove") {
    return L"storage.write";
  }
  if (method == L"dialog.openFile" || method == L"dialog.saveFile" || method == L"notification.toast" ||
      method == L"network.request" || method == L"core.step") {
    return method;
  }
  return std::nullopt;
}

json::JsonObject WebBridge::Dispatch(BridgeRequest const& request) {
  if (request.method == L"storage.get") {
    return storage_.Get(request);
  }
  if (request.method == L"storage.set") {
    return storage_.Set(request);
  }
  if (request.method == L"storage.remove") {
    return storage_.Remove(request);
  }
  if (request.method == L"storage.list") {
    return storage_.List(request);
  }
  if (request.method == L"dialog.openFile") {
    return dialogs_.OpenFile(request);
  }
  if (request.method == L"dialog.saveFile") {
    return dialogs_.SaveFile(request);
  }
  if (request.method == L"notification.toast") {
    return notifications_.Toast(request);
  }
  if (request.method == L"network.request") {
    return network_.Request(request);
  }
  if (request.method == L"core.step") {
    return core_.Step(request);
  }
  if (request.method == L"runtime.capabilities") {
    return Capabilities(request);
  }
  if (request.method == L"app.log") {
    json::JsonObject result;
    result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
    return BridgeResponse::Success(request.id, request.hasId, result);
  }
  return BridgeResponse::Failure(request.id, request.hasId, L"unknown_method", L"Unknown bridge method: " + request.method);
}

json::JsonObject WebBridge::Capabilities(BridgeRequest const& request) const {
  json::JsonObject features;
  features.Insert(L"storage.get", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"storage.set", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"storage.remove", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"storage.list", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"dialog.openFile", json::JsonValue::CreateBooleanValue(false));
  features.Insert(L"dialog.saveFile", json::JsonValue::CreateBooleanValue(false));
  features.Insert(L"notification.toast", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"network.request", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"core.step", json::JsonValue::CreateBooleanValue(core_.IsAvailable()));
  features.Insert(L"runtime.capabilities", json::JsonValue::CreateBooleanValue(true));
  features.Insert(L"app.log", json::JsonValue::CreateBooleanValue(true));

  json::JsonObject limits;
  limits.Insert(L"maxPackageBytes", json::JsonValue::CreateNumberValue(1048576));
  limits.Insert(L"maxFileBytes", json::JsonValue::CreateNumberValue(524288));

  json::JsonObject result;
  result.Insert(L"platform", json::JsonValue::CreateStringValue(L"windows"));
  result.Insert(L"target", json::JsonValue::CreateStringValue(L"windows"));
  result.Insert(L"runtimeVersion", json::JsonValue::CreateStringValue(L"0.1.0"));
  result.Insert(L"devMode", json::JsonValue::CreateBooleanValue(true));
  result.Insert(L"features", features);
  result.Insert(L"limits", limits);
  return BridgeResponse::Success(request.id, request.hasId, result);
}

}  // namespace nativeai
