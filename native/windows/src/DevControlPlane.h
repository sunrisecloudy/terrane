#pragma once

#include <cstdint>
#include <filesystem>
#include <memory>
#include <string>

namespace terrane {

class WebViewHost;

struct DevControlPlaneConfig {
  uint16_t requestedPort = 0;
  std::filesystem::path databasePath;
};

class DevControlPlane {
 public:
  DevControlPlane();
  ~DevControlPlane();

  DevControlPlane(DevControlPlane const&) = delete;
  DevControlPlane& operator=(DevControlPlane const&) = delete;

  bool Start(DevControlPlaneConfig const& config, std::wstring* error);
  void SetHost(WebViewHost* host);
  void Stop();

  uint16_t Port() const;
  std::filesystem::path TokenPath() const;

 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

}  // namespace terrane
