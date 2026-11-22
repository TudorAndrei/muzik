# syntax=docker/dockerfile:1
FROM h3nc4/yt-dlp-slim@sha256:34a07c5f2c4ccd7a11b21e8a934aad5444deae88a5b437e48debd3435ac9d21a AS yt-dlp

FROM mwader/static-ffmpeg:9.1@sha256:dd09509d8c2152bfa601c8c20e3dd166bde906d37951b9f7e8bbd922267f5ac9 AS ffmpeg

FROM rust:1.99.1-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release -p muzik-server \
    && install -D target/release/muzik-server /out/muzik-server

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends quickjs ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 --create-home --home-dir /home/muzik muzik \
    && mkdir -p /data /music \
    && chown muzik:muzik /data /music
COPY --from=yt-dlp /bin/yt-dlp /usr/local/bin/yt-dlp
COPY --from=ffmpeg /ffmpeg /ffprobe /usr/local/bin/
COPY packaging/server/yt-dlp.conf /etc/yt-dlp.conf
COPY --from=build /out/muzik-server /usr/local/bin/muzik-server
ENV MUZIK_HOME=/data \
    MUZIK_SERVER_BIND=0.0.0.0:8080
USER 10001
VOLUME ["/data", "/music"]
EXPOSE 8080
ENTRYPOINT ["muzik-server"]
