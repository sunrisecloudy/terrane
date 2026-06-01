#pragma once

#include <cstdint>
#include <map>
#include <set>
#include <string>
#include <vector>
#include <winrt/Windows.Data.Json.h>

namespace terrane {

struct NetworkPolicyRule {
  std::wstring origin;
  std::set<std::wstring> methods;
  std::wstring pathPrefix;
  std::set<std::wstring> allowedHeaders;
  uint32_t maxRequestBytes = 0;
  uint32_t maxResponseBytes = 0;
  uint32_t timeoutMs = 10000;
};

struct AppSandboxContext {
  std::wstring appId;
  std::wstring storagePrefix;
  std::set<std::wstring> approvedPermissions;
  std::vector<NetworkPolicyRule> networkPolicy;
  std::map<std::wstring, uint32_t> resourceBudget;
  bool denyPrivateNetwork = true;
  std::wstring mountToken;
};

struct BridgeRequest {
  bool hasId = false;
  std::wstring id;
  std::wstring method;
  winrt::Windows::Data::Json::JsonObject params{nullptr};
  AppSandboxContext context;
};

struct BridgeResponse {
  static winrt::Windows::Data::Json::JsonObject Success(
      std::wstring const& id,
      bool hasId,
      winrt::Windows::Data::Json::IJsonValue const& result);

  static winrt::Windows::Data::Json::JsonObject Failure(
      std::wstring const& id,
      bool hasId,
      std::wstring const& code,
      std::wstring const& message,
      winrt::Windows::Data::Json::JsonObject const& details = winrt::Windows::Data::Json::JsonObject());
};

std::string WideToUtf8(std::wstring const& value);
std::wstring Utf8ToWide(std::string const& value);

}  // namespace terrane
