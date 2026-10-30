# Cross-compiles Survivor's Satchel for 64-bit Windows (x86_64-pc-windows-msvc) on Linux.
# cargo-xwin fetches the Microsoft CRT and Windows SDK on first use (cached under /cache);
# using it means accepting https://go.microsoft.com/fwlink/?LinkId=2086102
FROM rust:1.99.1-slim-trixie

RUN apt-get update \
 && apt-get install --no-install-recommends -y clang llvm \
 && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-pc-windows-msvc \
 && cargo install --locked cargo-xwin@0.23.1 \
 && rm -rf "$CARGO_HOME/registry" "$CARGO_HOME/git"

# Fonts (and fontconfig, which Slint uses on Linux) for tools/ui-preview, which renders the screens without Windows.
RUN apt-get update \
 && apt-get install --no-install-recommends -y fonts-dejavu-core libfontconfig-dev pkg-config \
 && rm -rf /var/lib/apt/lists/*

# Everything that grows (crates, the Windows SDK, build output) lives in the /cache mount.
ENV XWIN_ARCH=x86_64 \
    XWIN_CACHE_DIR=/cache/xwin \
    CARGO_HOME=/cache/cargo \
    CARGO_TARGET_DIR=/cache/target \
    HOME=/tmp

WORKDIR /src
