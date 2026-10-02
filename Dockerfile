# syntax=docker/dockerfile:1
#
# Pumpkin server image, built from this repository's source (not from an
# upstream release binary, so the image carries this fork's code).
#
#   docker compose up -d        # see docker-compose.yml and docs/hosting.md
#
# Stage 1 builds a static musl binary; stage 2 is a small Alpine runtime that
# runs it as the non-root user `pumpkin` (UID/GID 2613) with /data as its
# working directory.

ARG RUST_IMAGE=rust:1-alpine
ARG RUNTIME_IMAGE=alpine:3.24

FROM ${RUST_IMAGE} AS builder

# ring and zstd-sys compile C.
RUN apk add --no-cache build-base musl-dev perl

# rustc overflows its default stack compiling the generated pumpkin-data crate.
ENV RUST_MIN_STACK=268435456 \
    CARGO_TERM_COLOR=always

# Copy only what the Rust build reads. .git, docker/ (scripts, defaults,
# tests) and the docs stay out of this stage, so a commit that does not touch
# the Rust sources reuses the cached release build instead of recompiling it
# (a cold LTO build takes well over an hour on a CI runner). The cost: without
# .git, build.rs reports the commit hash as "unknown" in-game; the image
# carries the revision as an OCI label instead (VCS_REF below).
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY tools ./tools
COPY assets ./assets

# Uses rust-toolchain.toml (latest stable) and the workspace's release profile.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked --bin pumpkin && \
    install -Dm755 target/release/pumpkin /out/pumpkin

FROM ${RUNTIME_IMAGE}

# tzdata: honour TZ for log timestamps and backup names.
RUN apk add --no-cache ca-certificates tzdata && \
    addgroup -g 2613 pumpkin && \
    adduser -u 2613 -G pumpkin -D -H -h /data pumpkin && \
    mkdir -p /data /backups /import /etc/pumpkin && \
    chown pumpkin:pumpkin /data /backups

COPY --from=builder /out/pumpkin /usr/local/bin/pumpkin
COPY docker/pumpkin.toml /etc/pumpkin/pumpkin.toml
COPY --chmod=755 docker/entrypoint.sh /usr/local/bin/pumpkin-entrypoint
COPY --chmod=755 docker/backup.sh /usr/local/bin/pumpkin-backup

# Set by CI (--build-arg VCS_REF=<sha>); only labels the final stage.
ARG VCS_REF=unknown
LABEL org.opencontainers.image.revision=${VCS_REF} \
      org.opencontainers.image.source=https://github.com/MrFruitDude/Pumpkin

USER pumpkin:pumpkin
WORKDIR /data
VOLUME ["/data", "/backups"]

ENV RUST_BACKTRACE=1 \
    PUMPKIN_TELEMETRY=false \
    PUMPKIN_BEDROCK=false

# Java Edition only. Bedrock (19132/udp) is off by default and not exposed.
EXPOSE 25565/tcp

# Healthy once something listens on TCP 25565 (0x63DD, state 0A = LISTEN).
HEALTHCHECK --interval=15s --timeout=3s --start-period=60s --retries=3 \
    CMD grep -Eq ':63DD [0-9A-F]+:0000 0A' /proc/net/tcp /proc/net/tcp6 || exit 1

STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/pumpkin-entrypoint"]
