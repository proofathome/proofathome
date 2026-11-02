# Verus checker image for Proof@home. See checkers/README.md.
#
# The Proof@home CLI passes the full command; there is no entrypoint.
# Runs as any uid: everything is readable by all, and build output goes to
# /tmp.

# rust:1.99.1-slim-trixie. Verus needs exactly this Rust toolchain.
FROM rust:1.99.1-slim-trixie@sha256:b8e062b1c3a80183df83d9ba187cadd2b5ab84bc78d453dbfeb2fccd5f982247 AS download

ARG VERUS_VERSION=0.2026.10.26.d17fbd0
# From https://github.com/verus-lang/verus/releases/tag/release/0.2026.10.26.d17fbd0
ARG VERUS_SHA256=361674d05f0c1b2c9c3f4d5bc56a8fb2ab9241ed0cceca27780668c292bb4940

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL -o /tmp/verus.zip \
        "https://github.com/verus-lang/verus/releases/download/release/${VERUS_VERSION}/verus-${VERUS_VERSION}-x86-linux.zip" \
    && echo "${VERUS_SHA256}  /tmp/verus.zip" | sha256sum -c - \
    && unzip -q /tmp/verus.zip -d /tmp/verus \
    && mv /tmp/verus/verus-x86-linux /opt/verus \
    && chmod -R a+rX,go-w /opt/verus

FROM rust:1.99.1-slim-trixie@sha256:b8e062b1c3a80183df83d9ba187cadd2b5ab84bc78d453dbfeb2fccd5f982247

ARG VERUS_VERSION=0.2026.10.26.d17fbd0

COPY --from=download /opt/verus /opt/verus

ENV PATH=/opt/verus:$PATH \
    HOME=/tmp \
    CARGO_TARGET_DIR=/tmp/target

RUN verus --version && cargo verus --help > /dev/null

LABEL org.opencontainers.image.title="proofathome-verus" \
      org.opencontainers.image.description="Verus ${VERUS_VERSION} with Rust 1.99.1, for Proof@home checks" \
      org.opencontainers.image.source="https://github.com/proofathome/proofathome" \
      org.opencontainers.image.version="${VERUS_VERSION}" \
      org.opencontainers.image.licenses="MIT"
