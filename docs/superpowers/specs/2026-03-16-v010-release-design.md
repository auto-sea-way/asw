# v0.1.0 Release Design

**Date:** 2026-03-16
**Status:** Draft

## Goal

Ship auto-sea-way v0.1.0 as a ready-to-run maritime routing service. The primary audience is end users who want a working routing API with a pre-built global graph — not developers building their own graphs.

## Decisions

- Ship with Panama Canal and Kiel Canal routing bugs as known limitations (18/20 benchmark routes work correctly)
- Graph file (843 MB, already zstd-19 compressed internally) hosted on GitHub Releases alongside binaries
- Two Docker image variants: slim (download-on-start) and full (graph baked in)
- Multi-arch Docker images: linux/amd64 + linux/arm64
- Single `workflow_dispatch` release workflow replaces tag-push trigger

## 1. Release Workflow

### Current State

Two tag-triggered workflows:
- `release.yml` — builds 4-platform binaries, creates GitHub Release
- `docker.yml` — builds and pushes Docker image to ghcr.io

### New State

One `workflow_dispatch` workflow (`release.yml`) with two inputs:
- **version** (string, required): Semantic version, e.g., `0.1.0`
- **graph_url** (string, required): Public URL to the pre-built `asw.graph` file

### Graph Upload Flow

The graph file (843 MB) cannot be passed as a workflow_dispatch input directly. Instead:

1. Before triggering the workflow, create a **draft GitHub Release** (any temporary tag) and upload `asw.graph` to it:
   ```bash
   gh release create draft-graph --draft --title "Graph staging" --notes ""
   gh release upload draft-graph export/asw.graph
   ```
2. Copy the asset download URL and pass it as the `graph_url` workflow input.
3. The release job downloads the graph from that URL and attaches it to the real release.
4. After the workflow completes, delete the draft release:
   ```bash
   gh release delete draft-graph --yes --cleanup-tag
   ```

This avoids Actions artifact size limits and keeps the flow within GitHub.

### Job Sequence

All jobs are sequential — each depends on the previous:

1. **ci-check** — fmt, clippy, tests, cargo-deny, audit (existing reusable workflow)
2. **build-binaries** — 4-target matrix (existing):
   - `x86_64-unknown-linux-gnu` (ubuntu-latest, native cargo)
   - `aarch64-unknown-linux-gnu` (ubuntu-latest, cross)
   - `x86_64-apple-darwin` (macos-13, native cargo)
   - `aarch64-apple-darwin` (macos-latest, native cargo)
3. **release** — download graph from `graph_url`, create annotated git tag `v{version}` on the workflow's ref, create GitHub Release, upload:
   - 4 platform binaries
   - SHA256SUMS
   - `asw.graph`
   - Guard: workflow must be running on `main` branch, otherwise fail immediately
4. **docker** — build and push both image variants (see Section 2)

The graph download URL is deterministic once the release exists:
```
https://github.com/{owner}/{repo}/releases/download/v{version}/asw.graph
```

### Removed

- `docker.yml` — merged into `release.yml` as the docker job
- Tag-push trigger on `release.yml` — replaced entirely by `workflow_dispatch`

## 2. Docker Images

### Slim Image

The existing `Dockerfile`, unchanged. Users provide the graph via:
- `ASW_GRAPH_URL` env var (auto-downloads on first start)
- Volume mount: `-v /path/to/asw.graph:/data/asw.graph`

Size: ~25 MB (distroless base + asw binary)

### Full Image

New standalone `Dockerfile.full` — duplicates the builder stage from the slim Dockerfile, then adds the graph via `ADD`:

```dockerfile
FROM rust:1.94.0-bookworm AS builder
WORKDIR /src

# Copy manifests only (layer caching)
COPY Cargo.toml Cargo.lock ./
COPY crates/asw-core/Cargo.toml crates/asw-core/Cargo.toml
COPY crates/asw-build/Cargo.toml crates/asw-build/Cargo.toml
COPY crates/asw-serve/Cargo.toml crates/asw-serve/Cargo.toml
COPY crates/asw-cloud/Cargo.toml crates/asw-cloud/Cargo.toml
COPY crates/asw-cli/Cargo.toml crates/asw-cli/Cargo.toml

# Dummy source for dep caching
RUN for crate in asw-core asw-build asw-serve asw-cloud; do \
      mkdir -p crates/$crate/src && echo "" > crates/$crate/src/lib.rs; \
    done && \
    mkdir -p crates/asw-cli/src && echo "fn main() {}" > crates/asw-cli/src/main.rs
RUN cargo build --release -p asw-cli || true

# Real source
COPY crates/ crates/
RUN touch crates/*/src/*.rs && cargo build --release -p asw-cli

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /src/target/release/asw /usr/local/bin/asw
ARG ASW_GRAPH_URL
ADD ${ASW_GRAPH_URL} /data/asw.graph
ENV ASW_GRAPH=/data/asw.graph
ENV ASW_HOST=0.0.0.0
ENV ASW_PORT=3000
EXPOSE 3000
ENTRYPOINT ["asw"]
CMD ["serve"]
```

Note: `VOLUME /data` is omitted from the full image since the graph is baked in and no persistent storage is needed.

Precondition: `ASW_GRAPH_URL` must be a publicly accessible URL (GitHub Releases assets for public repos satisfy this).

Size: ~870 MB (25 MB base + 843 MB graph)

### Tags

| Variant | Tags |
|---------|------|
| Slim | `ghcr.io/{owner}/auto-sea-way:0.1.0`, `:latest` |
| Full | `ghcr.io/{owner}/auto-sea-way:0.1.0-full`, `:latest-full` |

### Multi-arch

Both variants built for `linux/amd64` and `linux/arm64` using:
- `docker/setup-qemu-action` — QEMU emulation for cross-platform builds
- `docker/build-push-action` with `platforms: linux/amd64,linux/arm64`

Note: QEMU-emulated Rust builds for arm64 are slow (potentially 30+ minutes). This is acceptable for release builds since they run infrequently. If build times become problematic, switch to native arm64 runners or the `cross` tool (already used for the binary release job).

### Docker Job in Release Workflow

The docker job runs after the release job. It:
1. Sets up QEMU + Docker Buildx
2. Logs into ghcr.io
3. Builds and pushes slim image (both platforms)
4. Builds and pushes full image with `ASW_GRAPH_URL` pointing to the just-created release asset

## 3. Deployment Guide

New file: `docs/deployment.md`

### Docker Compose

Two examples:

**Full image (zero-config):**
```yaml
services:
  asw:
    image: ghcr.io/{owner}/auto-sea-way:0.1.0-full
    ports:
      - "3000:3000"
    healthcheck:
      test: ["CMD", "/usr/local/bin/asw", "healthcheck"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 30s
```

Note: The distroless image has no `wget` or `curl`. The `asw healthcheck` subcommand (to be added) hits `http://localhost:$ASW_PORT/ready` and exits 0/1. This is a minimal addition to the CLI.

**Slim image (with download):**
```yaml
services:
  asw:
    image: ghcr.io/{owner}/auto-sea-way:0.1.0
    ports:
      - "3000:3000"
    environment:
      - ASW_GRAPH_URL=https://github.com/{owner}/{repo}/releases/download/v0.1.0/asw.graph
    volumes:
      - asw-data:/data
    healthcheck:
      test: ["CMD", "/usr/local/bin/asw", "healthcheck"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 120s  # longer: graph download on first start; adjust for network speed
volumes:
  asw-data:
```

### Kubernetes

- Deployment with readiness probe (`/ready`, periodSeconds: 5) and liveness probe (`/health`, periodSeconds: 10)
- Full image variant: straightforward Deployment + Service
- Slim image variant: `ASW_GRAPH_URL` env var + PersistentVolumeClaim so the graph persists across pod restarts
- Service (ClusterIP)
- Optional Ingress example

### Bare-metal

- Download binary and graph from GitHub Releases page
- Run: `asw serve --graph ./asw.graph --port 3000`
- Systemd unit file example for running as a service

## 4. README Updates

- Graph stats: 40,397,636 nodes, 305,031,722 edges, 843 MB (was 11.5M / 85.6M / 1.4 GB)
- Build server: ccx33 — 8 dedicated AMD CPUs, 32 GB RAM (was cpx62)
- Benchmark table: replace with current 20-route results
- Known Limitations: add Panama Canal and Kiel Canal passage routing bugs
- Docker examples: show both slim and full image usage
- Link to `docs/deployment.md`

## 5. Code Fixes

Before merging `feat/compact-graph` to `main`:

- Fix all `cargo fmt` warnings in `crates/asw-core/src/graph.rs`
- Fix all `cargo clippy` warnings in `crates/asw-core/src/graph.rs` (`.div_ceil()` usage)

## 6. Release Checklist

1. Fix fmt + clippy on `feat/compact-graph`
2. Merge `feat/compact-graph` to `main`
3. Add `asw healthcheck` subcommand (hits `/ready`, exits 0/1)
4. Create `Dockerfile.full`
5. Rewrite `release.yml` (workflow_dispatch with `version` + `graph_url` inputs, sequential jobs, docker build)
6. Remove `docker.yml`
7. Write `docs/deployment.md`
8. Update README stats, benchmarks, known limitations, Docker examples
9. Create draft release, upload `asw.graph`, trigger release workflow with version `0.1.0` and graph URL
10. Delete draft release after workflow completes
