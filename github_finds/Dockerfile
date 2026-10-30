# The binary, built natively for the architecture being built: the Supervisor builds on the
# machine that runs the add-on, and the Rust image is multi-arch. Tag and index digest pinned.
FROM rust:1.98.1-alpine3.24@sha256:7cc1c22d77d9432f7fe012a70e6d3e555af54c2a6832700ed7d553f1769ae89f AS finds

SHELL ["/bin/ash", "-o", "pipefail", "-c"]

ARG BUILD_ARCH=amd64
WORKDIR /src
COPY finds/Cargo.toml finds/Cargo.lock ./
COPY finds/src ./src
RUN case "${BUILD_ARCH}" in \
      aarch64) TARGET=aarch64-unknown-linux-musl ;; \
      amd64)   TARGET=x86_64-unknown-linux-musl ;; \
      *)       echo "unsupported BUILD_ARCH ${BUILD_ARCH}" >&2; exit 1 ;; \
    esac \
 && cargo build --release --locked --target "${TARGET}" \
 && install -m 755 "target/${TARGET}/release/github-finds" /github-finds

# Home Assistant's base image is published for amd64 and arm64 under one tag; the build's
# platform picks the matching one.
FROM ghcr.io/home-assistant/base:3.24

SHELL ["/bin/ash", "-o", "pipefail", "-c"]

ARG BUILD_ARCH=amd64
COPY --from=finds /github-finds /usr/bin/github-finds
RUN { /usr/bin/github-finds 2>&1 || true; } | grep -q "github-finds serve"

# The links file is the add-on's config folder, /config inside the container; discovery keeps
# its state in /data.
CMD ["/usr/bin/github-finds", "serve", "--links", "/config/links.json", "--port", "8091", "--options", "/data/options.json", "--state", "/data/discover.json"]

HEALTHCHECK --interval=60s --timeout=10s --start-period=30s --retries=3 \
    CMD ["/bin/sh", "-c", "curl -fsS -o /dev/null http://127.0.0.1:8091/healthz || exit 1"]

ARG BUILD_VERSION
LABEL \
    io.hass.name="GitHub Finds" \
    io.hass.description="New GitHub projects for your Home Assistant setup, found daily and shown on one page" \
    io.hass.arch="${BUILD_ARCH}" \
    io.hass.type="app" \
    io.hass.version="${BUILD_VERSION}" \
    maintainer="Lewis Heidrick" \
    org.opencontainers.image.title="GitHub Finds" \
    org.opencontainers.image.description="New GitHub projects for your Home Assistant setup, found daily and shown on one page" \
    org.opencontainers.image.authors="Lewis Heidrick" \
    org.opencontainers.image.licenses="MIT" \
    org.opencontainers.image.url="https://github.com/heidrickla/ha-github-finds" \
    org.opencontainers.image.source="https://github.com/heidrickla/ha-github-finds" \
    org.opencontainers.image.documentation="https://github.com/heidrickla/ha-github-finds/blob/main/github_finds/DOCS.md" \
    org.opencontainers.image.version="${BUILD_VERSION}"
