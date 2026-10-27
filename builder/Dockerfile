# Image preparation environment. Must be run with --privileged -v /dev:/dev
# (loop devices, plus an arm64 binfmt handler for chrooting into the Pi rootfs
# on non-arm64 hosts).
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        qemu-user-static qemu-utils e2fsprogs zerofree fdisk util-linux mount ca-certificates \
    && rm -rf /var/lib/apt/lists/*
