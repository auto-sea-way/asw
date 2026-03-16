FROM rust:1.94.0-bookworm AS builder
WORKDIR /src

# Copy manifests only
COPY Cargo.toml Cargo.lock ./
COPY crates/asw-core/Cargo.toml crates/asw-core/Cargo.toml
COPY crates/asw-build/Cargo.toml crates/asw-build/Cargo.toml
COPY crates/asw-serve/Cargo.toml crates/asw-serve/Cargo.toml
COPY crates/asw-cloud/Cargo.toml crates/asw-cloud/Cargo.toml
COPY crates/asw-cli/Cargo.toml crates/asw-cli/Cargo.toml

# Create dummy source so cargo can resolve workspace and cache deps
RUN for crate in asw-core asw-build asw-serve asw-cloud; do \
      mkdir -p crates/$crate/src && echo "" > crates/$crate/src/lib.rs; \
    done && \
    mkdir -p crates/asw-cli/src && echo "fn main() {}" > crates/asw-cli/src/main.rs
RUN cargo build --release -p asw-cli || true

# Copy real source and rebuild (touch to invalidate cargo fingerprints)
COPY crates/ crates/
RUN touch crates/*/src/*.rs && cargo build --release -p asw-cli

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /src/target/release/asw /usr/local/bin/asw
ENV ASW_GRAPH=/data/asw.graph
ENV ASW_HOST=0.0.0.0
ENV ASW_PORT=3000
EXPOSE 3000
HEALTHCHECK --interval=10s --timeout=5s --retries=3 --start-period=120s \
  CMD ["/usr/local/bin/asw", "healthcheck"]
VOLUME /data
ENTRYPOINT ["asw"]
CMD ["serve"]
