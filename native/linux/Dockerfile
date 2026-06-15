FROM ubuntu:24.04

ARG TARGETARCH
ARG RUST_VERSION=1.96.0

ENV DEBIAN_FRONTEND=noninteractive
ENV PATH="/root/.cargo/bin:${PATH}"

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    dbus-x11 \
    gcc \
    libc6-dev \
    libgtk-4-dev \
    libjson-glib-dev \
    libsoup-3.0-dev \
    libsqlite3-dev \
    libwebkitgtk-6.0-dev \
    meson \
    ninja-build \
    nodejs \
    npm \
    pkg-config \
    sqlite3 \
    xz-utils \
    xvfb \
  && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
  curl -fsSL https://sh.rustup.rs -o /tmp/rustup.sh; \
  sh /tmp/rustup.sh -y --profile minimal --default-toolchain "${RUST_VERSION}"; \
  rm /tmp/rustup.sh; \
  rustc --version; \
  cargo --version

WORKDIR /workspace

ENV GSETTINGS_BACKEND=memory \
    GTK_A11Y=none \
    TERRANE_LINUX_SMOKE_LAUNCH=1 \
    NO_AT_BRIDGE=1 \
    WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 \
    WEBKIT_DISABLE_COMPOSITING_MODE=1

CMD ["node", "--test", "--no-warnings", "tools/reference-host/test/linux-native-build.test.js"]
