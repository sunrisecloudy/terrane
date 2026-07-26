# The macOS app does not use llama.cpp's HTTP download helpers. Keep the
# `common` library for grammar sampling, but avoid introducing an OpenSSL
# dependency into Terrane's statically linked application bundle.
set(LLAMA_OPENSSL OFF CACHE BOOL "Disable unused llama.cpp HTTPS helpers" FORCE)

# Cargo's cmake wrapper observes MACOSX_DEPLOYMENT_TARGET for Rust but does not
# populate this CMake cache entry. Pin it explicitly so every bundled C/C++
# object remains loadable on Terrane's declared macOS 13 minimum.
set(
  CMAKE_OSX_DEPLOYMENT_TARGET
  "13.0"
  CACHE STRING "Terrane minimum supported macOS version"
  FORCE
)
