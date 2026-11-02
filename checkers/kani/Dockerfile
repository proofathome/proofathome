# Kani checker image for Proof@home. See checkers/README.md.
#
# The Proof@home CLI passes the full command; there is no entrypoint.
# Runs as any uid: everything is readable by all, nothing under the install
# directories is written at check time, and build output goes to /tmp.

# rust:1.99.1-slim-trixie. Kani installs and uses its own nightly toolchain.
FROM rust:1.99.1-slim-trixie@sha256:b8e062b1c3a80183df83d9ba187cadd2b5ab84bc78d453dbfeb2fccd5f982247

ARG KANI_VERSION=0.68.0
# From https://github.com/model-checking/kani/releases/download/kani-0.68.0/kani-0.68.0-x86_64-unknown-linux-gnu.tar.gz
ARG KANI_BUNDLE_SHA256=32e2b484d73ede0bbf64a2cf0879c4259422497d8aec4ee67d448de9ae7843d3

ENV KANI_HOME=/opt/kani

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL -o /tmp/kani.tar.gz \
        "https://github.com/model-checking/kani/releases/download/kani-${KANI_VERSION}/kani-${KANI_VERSION}-x86_64-unknown-linux-gnu.tar.gz" \
    && echo "${KANI_BUNDLE_SHA256}  /tmp/kani.tar.gz" | sha256sum -c - \
    && cargo install --locked kani-verifier --version "${KANI_VERSION}" \
    && cargo kani setup --use-local-bundle /tmp/kani.tar.gz \
    && rm -rf /tmp/kani.tar.gz "${CARGO_HOME}/registry" "${CARGO_HOME}/git" \
    && chmod -R a+rX,go-w "${KANI_HOME}" "${CARGO_HOME}" "${RUSTUP_HOME}"

ENV HOME=/tmp \
    CARGO_TARGET_DIR=/tmp/target

RUN cargo kani --version

LABEL org.opencontainers.image.title="proofathome-kani" \
      org.opencontainers.image.description="Kani ${KANI_VERSION}, for Proof@home checks" \
      org.opencontainers.image.source="https://github.com/proofathome/proofathome" \
      org.opencontainers.image.version="${KANI_VERSION}" \
      org.opencontainers.image.licenses="MIT"
