# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
ARG CARGO_PROFILE_RELEASE_LTO=true
ARG CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin arc \
    && cp target/release/arc /usr/local/bin/arc

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd -g 1000 arc \
    && useradd -u 1000 -g 1000 -d /home/arc -s /usr/sbin/nologin arc \
    && install -d -o 1000 -g 1000 /home/arc/.config/arc /home/arc/.cache/arc /notes
COPY --from=build /usr/local/bin/arc /usr/local/bin/arc
ENV HOME=/home/arc \
    XDG_CONFIG_HOME=/home/arc/.config \
    XDG_CACHE_HOME=/home/arc/.cache
USER 1000:1000
WORKDIR /home/arc
ENTRYPOINT ["arc"]
