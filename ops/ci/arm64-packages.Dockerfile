# Compilation is native X64; BuildKit's private QEMU runs the ARM64 checks.
# Both tool environments preserve the existing Ubuntu 22.04/glibc 2.35 baseline.
FROM --platform=$BUILDPLATFORM node:22-bookworm-slim@sha256:43ac6c60b8f89723f746e8a92ce91abd5017e627ce1ddfe4238355d3a30b772c AS node-build
FROM node:22-bookworm-slim@sha256:43ac6c60b8f89723f746e8a92ce91abd5017e627ce1ddfe4238355d3a30b772c AS node-runtime
FROM --platform=$BUILDPLATFORM rust:1.95.0-slim-bookworm@sha256:d7482085ff5b415f84dba5647ae71606650bdef00db7aeb69f4b3d170c3e4082 AS rust-build
FROM rust:1.95.0-slim-bookworm@sha256:d7482085ff5b415f84dba5647ae71606650bdef00db7aeb69f4b3d170c3e4082 AS rust-runtime
FROM --platform=$BUILDPLATFORM ubuntu:22.04@sha256:5ec03bb3441e8b0bf3b4f9cd4629a1ae763010dc3035bb8da3ae6cf026486401 AS build-tools
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config git jq curl ca-certificates libssl3 libtinfo6 gcc-aarch64-linux-gnu libc6-dev-arm64-cross && rm -rf /var/lib/apt/lists/*
COPY --from=rust-build /usr/local/cargo /usr/local/cargo
COPY --from=rust-build /usr/local/rustup /usr/local/rustup
COPY --from=node-build /usr/local/bin/node /usr/local/bin/node
COPY --from=node-build /usr/local/lib/node_modules /usr/local/lib/node_modules
ENV PATH=/usr/local/cargo/bin:$PATH RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx && rustup target add aarch64-unknown-linux-gnu
WORKDIR /workspace
COPY . .
RUN git config --global --add safe.directory /workspace
FROM build-tools AS source-bound
ARG SOURCE_SHA
ARG SOURCE_TREE
RUN bash ops/ci/package-build-custody.sh checkout "$SOURCE_SHA" "$SOURCE_TREE" && mkdir /source-verified && printf '%s %s\n' "$SOURCE_SHA" "$SOURCE_TREE" > /source-verified/source-identity
FROM scratch AS source-verified
COPY --from=source-bound /source-verified/ /

FROM source-bound AS build
ARG TAG
ARG SOURCE_SHA
ARG SOURCE_TREE
ENV OUTPUT_DIR=/packages/${SOURCE_SHA} REDLINE_PACKAGE_TARGET=aarch64-unknown-linux-gnu
# Hold the target cache until staging finishes, not just while Cargo runs:
# another checkout otherwise replaces release binaries between build and cp.
RUN --mount=type=cache,id=redline-arm64-cargo,target=/usr/local/cargo/registry --mount=type=cache,id=redline-arm64-git,target=/usr/local/cargo/git --mount=type=cache,id=redline-arm64-target,target=/workspace/target,sharing=locked bash ops/ci/package-build-custody.sh checkout "$SOURCE_SHA" "$SOURCE_TREE" && bash ops/ci/packages.sh build && bash ops/ci/package-build-custody.sh archives "$OUTPUT_DIR" "$SOURCE_SHA" "$SOURCE_TREE"
FROM scratch AS archives
ARG SOURCE_SHA
COPY --from=build /packages/${SOURCE_SHA}/ /

# A fresh target-platform stage reads downloaded archives, never build output.
FROM ubuntu:22.04@sha256:5ec03bb3441e8b0bf3b4f9cd4629a1ae763010dc3035bb8da3ae6cf026486401 AS runtime-tools
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config curl git jq ca-certificates libssl3 libtinfo6 && rm -rf /var/lib/apt/lists/*
COPY --from=rust-runtime /usr/local/cargo /usr/local/cargo
COPY --from=rust-runtime /usr/local/rustup /usr/local/rustup
COPY --from=node-runtime /usr/local/bin/node /usr/local/bin/node
COPY --from=node-runtime /usr/local/lib/node_modules /usr/local/lib/node_modules
ENV PATH=/usr/local/cargo/bin:$PATH RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm
WORKDIR /workspace
COPY . .
RUN git config --global --add safe.directory /workspace
FROM runtime-tools AS runtime
ARG SOURCE_SHA
ARG SOURCE_TREE
COPY --from=packages / /workspace/target/packages/
ENV OUTPUT_DIR=/workspace/target/packages
RUN test "$(uname -m)" = aarch64 && bash ops/ci/package-build-custody.sh checkout "$SOURCE_SHA" "$SOURCE_TREE" && bash ops/ci/package-build-custody.sh archives /workspace/target/packages "$SOURCE_SHA" "$SOURCE_TREE" && bash ops/ci/packages.sh runtime && bash scripts/test-package-licenses.sh && bash scripts/test-package-ffi.sh && bash ops/ci/packages.sh installer && bash ops/ci/packages.sh native-install && bash ops/ci/packages.sh quickstart && bash ops/ci/packages.sh published-check && mkdir /verified && touch /verified/arm64-runtime-passed
FROM scratch AS verified
COPY --from=runtime /verified/ /
