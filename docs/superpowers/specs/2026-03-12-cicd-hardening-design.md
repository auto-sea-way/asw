# CI/CD Hardening — Design Spec

**Date:** 2026-03-12
**Status:** Draft

## Goal

Make CI/CD pipeline production-ready: fix all blockers, add CI gates, cross-platform releases (4 targets), basic unit tests, security scanning, dependency automation.

## Context

Current state:
- 3 workflows: `ci.yml` (push/PR), `docker.yml` (tags), `release.yml` (tags)
- Multi-stage Dockerfile (rust:bookworm → distroless)
- Zero tags ever pushed — Docker and Release workflows never tested
- Zero tests in codebase — `cargo test` passes vacuously
- 2 clippy warnings fail `cargo clippy -- -D warnings` (CI's own check)
- No Rust version pinning, no security scanning, no dependency automation
- Release builds linux-amd64 only

## Changes

### 1. Fix Clippy Warnings

Two warnings in `crates/asw-cli/src/main.rs`:

**Line 503** — `manual implementation of .is_multiple_of()`:
```rust
// Before:
if hex_count % 1_000_000 == 0 {
// After:
if hex_count.is_multiple_of(1_000_000) {
```

**Line 580** — `needless_range_loop`:
```rust
// Before:
for idx in 0..=15 {
    hex_features.append(&mut layers[idx]);
}
// After:
for layer in layers.iter_mut().take(16) {
    hex_features.append(layer);
}
```

### 2. Basic Unit Tests

Add `#[cfg(test)]` modules to three files. Tests use only in-memory data — no shapefiles, no fixtures on disk.

**`crates/asw-core/src/h3.rs`** — test `haversine_km`:
- Zero distance (same point)
- Known distance: London → Paris ≈ 344 km (within 1%)
- Antipodal points ≈ 20,015 km (half circumference, within 1%)
- Symmetry: `haversine_km(a, b) == haversine_km(b, a)`

**`crates/asw-core/src/graph.rs`** — test GraphBuilder + RoutingGraph:
- Build a 4-node graph (square), verify `num_nodes`, `num_edges`
- `edges()` returns correct neighbors
- `edges_with_weights()` returns correct (target, weight) pairs
- `node_pos()` round-trips lat/lng (within f32 precision)
- `connected_components()` on connected graph returns `[4]`
- `connected_components()` on graph with isolated node returns `[4, 1]`
- `save()` + `load()` round-trip produces identical graph

**`crates/asw-core/src/routing.rs`** — test A* on small graph:
- Build a small graph where one path is strictly shorter (e.g., A→B 5km, A→C 10km, B→D 5km, C→D 10km → A→B→D = 10km is unique shortest)
- Assert on total cost and path length, not specific intermediate nodes (avoids tie-breaking flakiness)
- A* from A→A should return `Some(([A], 0.0))`
- A* to unreachable node returns `None`

**`crates/asw-serve/src/api.rs`** — test `parse_latlng`:
- Valid: `"36.85,28.27"` → `Some((36.85, 28.27))`
- Whitespace: `" 36.85 , 28.27 "` → `Some((36.85, 28.27))`
- Negative: `"-33.9,18.4"` → `Some((-33.9, 18.4))`
- Invalid: `"abc,def"` → `None`
- Too many commas: `"1,2,3"` → `None`
- Empty: `""` → `None`

### 3. `rust-toolchain.toml`

New file at workspace root:
```toml
[toolchain]
channel = "1.94.0"
components = ["rustfmt", "clippy"]
```

All CI workflows and local builds use this automatically. Replaces the explicit `rustup toolchain install` steps in workflows.

**Note:** The Dockerfile hard-codes `rust:1.94.0-bookworm`. When bumping the Rust version in `rust-toolchain.toml`, the Dockerfile `FROM` line must be updated to match.

### 4. Reusable CI Workflow (`.github/workflows/ci-check.yml`)

Extracted as a reusable workflow (`workflow_call`). Sets `CARGO_TERM_COLOR: always` at workflow level. Steps:

1. Checkout
2. Cargo cache (key uses `hashFiles('rust-toolchain.toml')` + `hashFiles('Cargo.lock')`)
3. `cargo fmt --all -- --check`
4. `cargo clippy --workspace -- -D warnings`
5. `cargo build --workspace`
6. `cargo test --workspace`
7. `EmbarkStudios/cargo-deny-action@v2` — license + advisory checks (pre-built, no compile)
8. `rustsec/audit-check@v2` — vulnerability scan (pre-built, no compile)

**Note:** Using official GitHub Actions for cargo-deny and cargo-audit avoids 5+ minutes of compilation per CI run. Run `cargo deny check` locally before merging to validate the license allowlist against actual dependencies.

### 5. CI Workflow (`.github/workflows/ci.yml`)

Simplified to call `ci-check.yml`:

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  check:
    uses: ./.github/workflows/ci-check.yml
```

### 6. Docker Workflow (`.github/workflows/docker.yml`)

Add CI gate before build:

```yaml
jobs:
  check:
    uses: ./.github/workflows/ci-check.yml

  build-and-push:
    needs: [check]
    # ... existing build-and-push steps unchanged
```

### 7. Release Workflow (`.github/workflows/release.yml`)

Major rework — matrix build for 4 targets with CI gate.

```yaml
jobs:
  check:
    uses: ./.github/workflows/ci-check.yml

  build:
    needs: [check]
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
            binary: asw-linux-amd64
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
            binary: asw-linux-arm64
            cross: true
          - target: x86_64-apple-darwin
            os: macos-13
            binary: asw-darwin-amd64
          - target: aarch64-apple-darwin
            os: macos-latest
            binary: asw-darwin-arm64
    runs-on: ${{ matrix.os }}
    permissions:
      contents: read
    steps:
      - checkout
      - add target: `rustup target add ${{ matrix.target }}`
      - install cross (if matrix.cross): `taiki-e/install-action@v2` with `tool: cross` (pre-built binary)
      - build: always use `--target ${{ matrix.target }}` flag explicitly
        - if matrix.cross: `cross build --release --target ... -p asw-cli`
        - else: `cargo build --release --target ... -p asw-cli`
      - rename binary to matrix.binary
      - upload artifact

  release:
    needs: [build]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - download all artifacts
      - generate SHA256 checksums file
      - create GitHub release with all binaries + checksums
```

### 8. `deny.toml`

New file at workspace root:

```toml
[advisories]
db-path = "~/.cargo/advisory-db"
db-urls = ["https://github.com/rustsec/advisory-db"]
vulnerability = "deny"
unmaintained = "warn"
yanked = "warn"

[licenses]
unlicensed = "deny"
allow = [
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Zlib",
    "OpenSSL",
    "MPL-2.0",
]

[bans]
multiple-versions = "warn"
wildcards = "allow"

[sources]
unknown-registry = "warn"
unknown-git = "warn"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
allow-git = []
```

### 9. `.github/dependabot.yml`

```yaml
version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    schedule:
      interval: "weekly"
    labels: ["dependencies"]

  - package-ecosystem: "github-actions"
    directory: "/"
    schedule:
      interval: "weekly"
    labels: ["ci"]
```

### 10. Dockerfile — No Changes

The `|| true` pattern on line 17 is intentional for dep caching with dummy sources. The real build on line 21 will fail properly. No changes needed.

## Files Changed

| File | Action | Description |
|------|--------|-------------|
| `crates/asw-cli/src/main.rs` | Edit | Fix 2 clippy warnings |
| `crates/asw-core/src/h3.rs` | Edit | Add `#[cfg(test)]` module |
| `crates/asw-core/src/graph.rs` | Edit | Add `#[cfg(test)]` module |
| `crates/asw-core/src/routing.rs` | Edit | Add `#[cfg(test)]` module |
| `crates/asw-serve/src/api.rs` | Edit | Add `#[cfg(test)]` module (test module can access private `parse_latlng` directly) |
| `rust-toolchain.toml` | Create | Pin Rust 1.94.0 |
| `deny.toml` | Create | License + advisory config |
| `.github/workflows/ci-check.yml` | Create | Reusable CI workflow |
| `.github/workflows/ci.yml` | Rewrite | Call ci-check, add concurrency |
| `.github/workflows/docker.yml` | Edit | Add ci-check gate |
| `.github/workflows/release.yml` | Rewrite | Matrix build, 4 targets, ci-check gate, checksums |
| `.github/dependabot.yml` | Create | Weekly cargo + actions updates |

## Out of Scope

- Full test suite (integration tests, property-based tests, fixture graphs) — separate effort
- Docker multi-arch images — can be added later
- Branch protection rules — requires manual GitHub settings configuration
- Changelog automation — nice-to-have for later
