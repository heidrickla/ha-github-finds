ARG BUILD_FROM=ghcr.io/home-assistant/amd64-base:3.21
# hadolint ignore=DL3006
FROM ${BUILD_FROM}

SHELL ["/bin/ash", "-o", "pipefail", "-c"]

ARG BUILD_ARCH=amd64

# A static musl binary built by tools/deploy.sh; nothing is installed.
COPY github-finds /usr/bin/github-finds
RUN chmod 755 /usr/bin/github-finds \
 && { /usr/bin/github-finds 2>&1 || true; } | grep -q "github-finds serve"

COPY run.sh /run.sh
RUN chmod +x /run.sh

CMD ["/run.sh"]

ARG BUILD_DATE
ARG BUILD_DESCRIPTION
ARG BUILD_NAME
ARG BUILD_REF
ARG BUILD_VERSION
LABEL \
    io.hass.name="${BUILD_NAME}" \
    io.hass.description="${BUILD_DESCRIPTION}" \
    io.hass.arch="${BUILD_ARCH}" \
    io.hass.type="addon" \
    io.hass.version="${BUILD_VERSION}" \
    maintainer="Lewis Heidrick" \
    org.opencontainers.image.title="${BUILD_NAME}" \
    org.opencontainers.image.licenses="MIT" \
    org.opencontainers.image.created="${BUILD_DATE}" \
    org.opencontainers.image.revision="${BUILD_REF}" \
    org.opencontainers.image.version="${BUILD_VERSION}"
