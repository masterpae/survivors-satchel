# Cross-compiles Survivor's Satchel for 64-bit Windows (x86_64-pc-windows-msvc) on Linux.
# cargo-xwin fetches the Microsoft CRT and Windows SDK on first use (cached under /cache);
# using it means accepting https://go.microsoft.com/fwlink/?LinkId=2086102
# The base image by its digest, so every build starts from the same image; Dependabot proposes
# newer ones (.github/dependabot.yml).
FROM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7

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

# clippy and rustfmt, for the two lint checks (`build.sh clippy`, `build.sh fmt`): they run with the
# toolchain that builds the exe.
RUN rustup component add clippy rustfmt

# Everything that grows (crates, the Windows SDK, build output) lives in the /cache mount.
ENV XWIN_ARCH=x86_64 \
    XWIN_CACHE_DIR=/cache/xwin \
    CARGO_HOME=/cache/cargo \
    CARGO_TARGET_DIR=/cache/target \
    HOME=/tmp

WORKDIR /src
