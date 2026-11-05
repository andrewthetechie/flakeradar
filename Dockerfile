# The FlakeRadar image: one static binary and nothing else.
#
# The dashboard is built once by Node and embedded by `crates/flakeradar/build.rs`,
# so the running image needs neither Node nor a `dist` directory: the bytes are
# in the binary. See docs/adr/0010-rust-rewrite.md.

# Build the dashboard. Cargo never runs npm; this stage is how a build gets one.
FROM node:22-slim AS frontend
WORKDIR /build
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# Build the binary for the architecture the image is built for. Alpine is musl
# natively, so the result is a fully static binary with no cross toolchain and no
# QEMU in the way. `mimalloc` is the global allocator on this target.
FROM rust:1.99-alpine AS builder
# build-base carries the C compiler `ring` and `mimalloc` need, and musl-dev the
# headers they build against.
RUN apk add --no-cache build-base perl
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml clippy.toml ./
COPY crates ./crates
COPY --from=frontend /build/dist ./frontend/dist
RUN cargo build --release --locked -p flakeradar

FROM gcr.io/distroless/static-debian13:nonroot@sha256:25ac8f0c3b2334a9eb05452266bd4697864a8c8ef0ef71e5e6480d7f92868329
COPY --from=builder /src/target/release/flakeradar /flakeradar
# The image is the one deployment that must answer from outside its container;
# everything else binds loopback until its operator says otherwise.
ENV FLAKERADAR_HOST=0.0.0.0
EXPOSE 8000
# The probe is the binary itself, which the image has and the reference's slim
# image did not have a client for. The start period covers the migration.
HEALTHCHECK --interval=30s --timeout=3s --start-period=15s --retries=3 \
  CMD ["/flakeradar", "healthcheck"]
ENTRYPOINT ["/flakeradar"]
