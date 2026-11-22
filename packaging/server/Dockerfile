# syntax=docker/dockerfile:1
FROM h3nc4/yt-dlp-slim:latest@sha256:34a07c5f2c4ccd7a11b21e8a934aad5444deae88a5b437e48debd3435ac9d21a AS yt-dlp

FROM ghcr.io/jim60105/bgutil-pot:v0.8.1@sha256:289d82576a24bb68bac6f19f499facc74b494f04eb499ae6fa51b27c77c64d31 AS bgutil-pot

FROM debian:trixie-slim AS ffmpeg
RUN apt-get update \
    && apt-get install -y --no-install-recommends xz-utils \
    && rm -rf /var/lib/apt/lists/*
ARG FFMPEG_BUILD=autobuild-2026-11-16-23-35
ARG FFMPEG_ARCHIVE=ffmpeg-N-128970-g11337618b2-linux64-gpl
ARG FFMPEG_SHA256=49d74477584a7a5d786003d7ef67e1e42304c022239ee1186ade87eefb480687
ADD --checksum=sha256:${FFMPEG_SHA256} \
    https://github.com/yt-dlp/FFmpeg-Builds/releases/download/${FFMPEG_BUILD}/${FFMPEG_ARCHIVE}.tar.xz \
    /tmp/ffmpeg.tar.xz
RUN tar -xJf /tmp/ffmpeg.tar.xz -C /usr/local/bin --strip-components=2 \
    ${FFMPEG_ARCHIVE}/bin/ffmpeg ${FFMPEG_ARCHIVE}/bin/ffprobe

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
COPY --from=bgutil-pot /bgutil-pot /usr/local/bin/bgutil-pot
COPY --from=bgutil-pot /client/yt_dlp_plugins /etc/yt-dlp/plugins/bgutil-ytdlp-pot-provider/yt_dlp_plugins
COPY --from=ffmpeg /usr/local/bin/ffmpeg /usr/local/bin/ffprobe /usr/local/bin/
COPY packaging/server/yt-dlp.conf /etc/yt-dlp.conf
COPY --from=build /out/muzik-server /usr/local/bin/muzik-server
ENV MUZIK_HOME=/data \
    MUZIK_SERVER_BIND=0.0.0.0:8080
USER 10001
VOLUME ["/data", "/music"]
EXPOSE 8080
ENTRYPOINT ["muzik-server"]
