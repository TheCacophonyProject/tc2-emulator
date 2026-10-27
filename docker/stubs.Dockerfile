# Builds the tc2-emu-* binaries: aarch64 for the guest, x86_64 for the container.
# Both are static musl binaries.
FROM rust:1.90-bookworm

RUN rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl \
    && apt-get update \
    && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
    && rm -rf /var/lib/apt/lists/*

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc
