FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends qemu-system-arm qemu-utils socat \
    && rm -rf /var/lib/apt/lists/*

COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh

# Guest SSH and management-interface web UI (forwarded by QEMU user networking).
EXPOSE 2222 8080
VOLUME ["/images", "/state"]
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
