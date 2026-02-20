# Multi-stage build for MicroDNS
# Produces a minimal scratch image with a static binary
# Supports amd64 and arm64 architectures

# Stage 1: Build
FROM rust:1.88-bookworm AS builder

# Install protobuf compiler and musl tools
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    musl-tools \
    && rm -rf /var/lib/apt/lists/*

# Detect architecture and add correct musl target
ARG TARGETARCH
RUN case "${TARGETARCH}" in \
      amd64) echo "x86_64-unknown-linux-musl" > /tmp/rust-target ;; \
      arm64) echo "aarch64-unknown-linux-musl" > /tmp/rust-target ;; \
      *) echo "Unsupported arch: ${TARGETARCH}" && exit 1 ;; \
    esac && \
    rustup target add $(cat /tmp/rust-target)

WORKDIR /build

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates/microdns-core/Cargo.toml crates/microdns-core/
COPY crates/microdns-auth/Cargo.toml crates/microdns-auth/
COPY crates/microdns-api/Cargo.toml crates/microdns-api/
COPY crates/microdns-recursor/Cargo.toml crates/microdns-recursor/
COPY crates/microdns-lb/Cargo.toml crates/microdns-lb/
COPY crates/microdns-dhcp/Cargo.toml crates/microdns-dhcp/
COPY crates/microdns-msg/Cargo.toml crates/microdns-msg/
COPY crates/microdns-federation/Cargo.toml crates/microdns-federation/

# Create dummy source files for dependency compilation
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs && \
    for crate in microdns-core microdns-auth microdns-api microdns-recursor microdns-lb microdns-dhcp microdns-msg microdns-federation; do \
        mkdir -p crates/$crate/src && echo '' > crates/$crate/src/lib.rs; \
    done

# Copy build scripts and proto file needed for API and federation crates
COPY crates/microdns-api/build.rs crates/microdns-api/
COPY crates/microdns-federation/build.rs crates/microdns-federation/
COPY proto/ proto/

# Build dependencies only (cached layer)
RUN cargo build --release --target $(cat /tmp/rust-target) 2>/dev/null || true

# Copy actual source code
COPY . .

# Touch source files to invalidate cache for actual sources only
RUN find . -name "*.rs" -exec touch {} +

# Build release binary
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN cargo build --release --target $(cat /tmp/rust-target) && \
    cp /build/target/$(cat /tmp/rust-target)/release/microdns /microdns-bin

# Stage 2: Runtime (scratch)
FROM scratch

# Copy the static binary
COPY --from=builder /microdns-bin /microdns

# Copy default config
COPY config/microdns.toml /etc/microdns/microdns.toml

# Create data directory
VOLUME ["/data"]

# Expose ports
# DNS (auth)
EXPOSE 53/udp
EXPOSE 53/tcp
# DNS (recursor)
EXPOSE 5353/udp
EXPOSE 5353/tcp
# DHCP
EXPOSE 67/udp
EXPOSE 547/udp
# REST API
EXPOSE 8080/tcp
# gRPC
EXPOSE 50051/tcp

ENTRYPOINT ["/microdns"]
CMD ["--config", "/etc/microdns/microdns.toml"]
