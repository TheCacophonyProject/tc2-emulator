# Image preparation environment. Must be run with --privileged -v /dev:/dev
# (loop devices + arm64 binfmt for chrooting into the Pi rootfs).
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        qemu-user-static qemu-utils e2fsprogs zerofree fdisk util-linux mount ca-certificates \
    && rm -rf /var/lib/apt/lists/*
