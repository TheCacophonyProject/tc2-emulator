# Builds the tc2-emu-* binaries: aarch64 for the guest, and the Docker host's
# architecture (x86_64, or aarch64 on Apple silicon) for the container. Both are
# static musl binaries. On an arm64 builder the native gcc is already
# aarch64-linux-gnu-gcc, so the cross toolchain is only needed elsewhere.
FROM rust:1.90-bookworm

# Everything rust-toolchain.toml asks for, so rustup doesn't download what's
# missing in every (throwaway) container.
RUN rustup component add rustfmt \
    && rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl \
    && if [ "$(dpkg --print-architecture)" != arm64 ]; then \
        apt-get update \
        && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
        && rm -rf /var/lib/apt/lists/*; \
    fi

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc
