FROM ubuntu:24.04

ARG TARGETARCH
ARG ZIG_VERSION=0.15.2

ENV DEBIAN_FRONTEND=noninteractive

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
  case "${TARGETARCH:-amd64}" in \
    amd64) zig_arch="x86_64" ;; \
    arm64) zig_arch="aarch64" ;; \
    *) echo "Unsupported Docker target architecture: ${TARGETARCH}" >&2; exit 1 ;; \
  esac; \
  curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-${zig_arch}-linux-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz; \
  mkdir -p /opt/zig; \
  tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1; \
  ln -s /opt/zig/zig /usr/local/bin/zig; \
  rm /tmp/zig.tar.xz; \
  zig version

WORKDIR /workspace

ENV GSETTINGS_BACKEND=memory \
    GTK_A11Y=none \
    TERRANE_LINUX_SMOKE_LAUNCH=1 \
    NO_AT_BRIDGE=1 \
    WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 \
    WEBKIT_DISABLE_COMPOSITING_MODE=1

CMD ["node", "--test", "--no-warnings", "tools/reference-host/test/linux-native-build.test.js"]
