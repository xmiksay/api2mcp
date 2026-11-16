# syntax=docker/dockerfile:1
#
# Release image: the SPA is built in a Node stage and handed to the Rust stage as web/dist,
# where rust-embed bakes it into the binary at compile time. SKIP_UI_BUILD=1 stops build.rs
# from running npm a second time; the runtime stage carries only the binary.
#
# Build through `make image` (local) or .github/workflows/release.yml (tags) — both pass
# NODE_VERSION from .nvmrc, RUST_VERSION from rust-toolchain.toml, and GIT_COMMIT, since .git is
# not in the build context. The defaults below only matter for a bare `docker build`.

ARG NODE_VERSION=22.23.1
ARG RUST_VERSION=1.99.0

FROM node:${NODE_VERSION}-bookworm-slim AS ui
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:${RUST_VERSION}-bookworm AS build
ARG GIT_COMMIT=unknown
ENV SKIP_UI_BUILD=1 \
    CARGO_BUILD_JOBS=4 \
    A2M_COMMIT=${GIT_COMMIT}
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ src/
COPY --from=ui /src/web/dist/ web/dist/
# The cache mounts keep the registry and target dir across local rebuilds; the binary is copied
# out inside the same RUN because a cache mount is not part of the resulting layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin api2mcp \
 && cp target/release/api2mcp /usr/local/bin/api2mcp

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/api2mcp /usr/local/bin/api2mcp
# Numeric UID so a runAsNonRoot pod policy can verify it without an /etc/passwd lookup.
USER 10001:10001
ENV A2M_HOST=0.0.0.0 \
    A2M_PORT=8080
EXPOSE 8080
ENTRYPOINT ["api2mcp"]
CMD ["serve"]
