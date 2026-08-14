# Multi-stage: compile in a full Rust image, ship a minimal alpine runtime.
FROM rust:1.97-alpine AS build
WORKDIR /build
# China-friendly crates mirror for the build stage.
RUN mkdir -p /root/.cargo && printf '%s\n' \
    '[source.crates-io]' \
    'replace-with = "rsproxy-sparse"' \
    '' \
    '[source.rsproxy-sparse]' \
    'registry = "sparse+https://rsproxy.cn/index/"' > /root/.cargo/config.toml
# Fetch dependencies once; this layer only rebuilds when Cargo.toml/lock change.
COPY Cargo.toml Cargo.lock ./
RUN cargo fetch
COPY src/ src/
RUN cargo build --release --offline

FROM alpine:3.20
RUN apk add --no-cache ca-certificates
COPY --from=build /build/target/release/dsh-rs-gateway /usr/local/bin/dsh-rs-gateway
# The gateway must reach the host-resident dsh web. On Docker Desktop
# (macOS/Windows) use http://host.docker.internal:3080; on Linux prefer
# --network host so 127.0.0.1:3080 works directly.
ENV LISTEN=0.0.0.0:8080 \
    BACKEND=http://host.docker.internal:3080
EXPOSE 8080
CMD ["dsh-rs-gateway"]
