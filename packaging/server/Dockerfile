# syntax=docker/dockerfile:1
FROM rust:1.99.1-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release -p muzik-server \
    && install -D target/release/muzik-server /out/muzik-server

FROM debian:trixie-slim
ARG TARGETARCH
ARG YT_DLP_VERSION=2026.10.26
RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg ca-certificates curl \
    && case "$TARGETARCH" in \
        amd64) asset=yt-dlp_linux ;; \
        arm64) asset=yt-dlp_linux_aarch64 ;; \
        *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && curl -fsSL -o /usr/local/bin/yt-dlp \
        "https://github.com/yt-dlp/yt-dlp/releases/download/${YT_DLP_VERSION}/${asset}" \
    && chmod 0755 /usr/local/bin/yt-dlp \
    && apt-get purge -y --auto-remove curl \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 --create-home --home-dir /home/muzik muzik \
    && mkdir -p /data /music \
    && chown muzik:muzik /data /music
COPY --from=build /out/muzik-server /usr/local/bin/muzik-server
ENV MUZIK_HOME=/data \
    MUZIK_SERVER_BIND=0.0.0.0:8080
USER 10001
VOLUME ["/data", "/music"]
EXPOSE 8080
ENTRYPOINT ["muzik-server"]
