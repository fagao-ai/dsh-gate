# Multi-stage: compile in a full Rust image, ship a minimal alpine runtime.
FROM rust:1.97-alpine AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

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
