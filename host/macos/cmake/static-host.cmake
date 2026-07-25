# The macOS app does not use llama.cpp's HTTP download helpers. Keep the
# `common` library for grammar sampling, but avoid introducing an OpenSSL
# dependency into Terrane's statically linked application bundle.
set(LLAMA_OPENSSL OFF CACHE BOOL "Disable unused llama.cpp HTTPS helpers" FORCE)
