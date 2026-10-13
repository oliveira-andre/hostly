# The Hostly sync server. Build from the repository root:
#   docker build -f crates/hostly-server/Dockerfile -t hostly-server .
# Configure with DATABASE_URL (required) and RUST_LOG; see .env.example.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# Cache mounts keep dependency downloads and builds between image builds; the
# binary is copied out because the target directory is a cache, not a layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p hostly-server \
    && cp target/release/hostly-server /usr/local/bin/hostly-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home hostly
COPY --from=build /usr/local/bin/hostly-server /usr/local/bin/hostly-server
USER hostly
ENV HOSTLY_BIND=0.0.0.0:8080
EXPOSE 8080
CMD ["hostly-server"]
