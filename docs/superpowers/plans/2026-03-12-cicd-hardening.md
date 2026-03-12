# CI/CD Hardening Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make CI/CD pipeline production-ready with all blockers fixed, CI gates, cross-platform releases, basic tests, and security scanning.

**Architecture:** Fix clippy blockers first, add unit tests for core pure functions, then layer in toolchain pinning, reusable CI workflow, gated Docker/Release workflows, cargo-deny, and dependabot.

**Tech Stack:** Rust 1.94.0, GitHub Actions, cargo-deny, cargo-audit, cross-rs, softprops/action-gh-release

**Spec:** `docs/superpowers/specs/2026-03-12-cicd-hardening-design.md`

---

## Chunk 1: Fix Blockers and Add Unit Tests

### Task 1: Fix Clippy Warnings

**Files:**
- Modify: `crates/asw-cli/src/main.rs:503,580`

- [ ] **Step 1: Verify clippy fails with -D warnings**

Run: `cargo clippy --workspace -- -D warnings 2>&1 | tail -5`
Expected: `error: could not compile` with 2 warnings-as-errors

- [ ] **Step 2: Fix `is_multiple_of` warning (line 503)**

In `crates/asw-cli/src/main.rs`, change:
```rust
// line 503 — before:
        if hex_count % 1_000_000 == 0 {
// after:
        if hex_count.is_multiple_of(1_000_000) {
```

- [ ] **Step 3: Fix `needless_range_loop` warning (line 580)**

In `crates/asw-cli/src/main.rs`, change:
```rust
// lines 580-582 — before:
    for idx in 0..=15 {
        hex_features.append(&mut layers[idx]);
    }
// after:
    for layer in layers.iter_mut().take(16) {
        hex_features.append(layer);
    }
```

- [ ] **Step 4: Verify clippy passes**

Run: `cargo clippy --workspace -- -D warnings 2>&1 | tail -3`
Expected: `Finished` with no errors

- [ ] **Step 5: Commit**

```bash
git add crates/asw-cli/src/main.rs
git commit -m "fix: resolve clippy warnings in asw-cli"
```

---

### Task 2: Add haversine_km Tests

**Files:**
- Modify: `crates/asw-core/src/h3.rs` (append test module)

- [ ] **Step 1: Add test module to h3.rs**

Append to `crates/asw-core/src/h3.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_zero_distance() {
        let d = haversine_km(51.5074, -0.1278, 51.5074, -0.1278);
        assert!((d - 0.0).abs() < 1e-10);
    }

    #[test]
    fn haversine_london_paris() {
        // London (51.5074, -0.1278) to Paris (48.8566, 2.3522) ≈ 344 km
        let d = haversine_km(51.5074, -0.1278, 48.8566, 2.3522);
        assert!((d - 344.0).abs() < 5.0, "London-Paris was {d} km, expected ~344");
    }

    #[test]
    fn haversine_antipodal() {
        // North pole to south pole ≈ 20015 km
        let d = haversine_km(90.0, 0.0, -90.0, 0.0);
        assert!((d - 20015.0).abs() < 100.0, "Antipodal was {d} km, expected ~20015");
    }

    #[test]
    fn haversine_symmetry() {
        let d1 = haversine_km(51.5074, -0.1278, 48.8566, 2.3522);
        let d2 = haversine_km(48.8566, 2.3522, 51.5074, -0.1278);
        assert!((d1 - d2).abs() < 1e-10);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p asw-core -- h3::tests -v`
Expected: 4 tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/asw-core/src/h3.rs
git commit -m "test: add haversine_km unit tests"
```

---

### Task 3: Add Graph Builder/RoutingGraph Tests

**Files:**
- Modify: `crates/asw-core/src/graph.rs` (append test module)

- [ ] **Step 1: Add test module to graph.rs**

Append to `crates/asw-core/src/graph.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 4-node square graph: 0-1-2-3-0, all edges 10km, bidirectional.
    fn square_graph() -> RoutingGraph {
        let mut b = GraphBuilder::new();
        b.add_node(0.0, 0.0); // node 0
        b.add_node(0.0, 1.0); // node 1
        b.add_node(1.0, 1.0); // node 2
        b.add_node(1.0, 0.0); // node 3
        b.add_edge(0, 1, 10.0);
        b.add_edge(1, 2, 10.0);
        b.add_edge(2, 3, 10.0);
        b.add_edge(3, 0, 10.0);
        b.build()
    }

    #[test]
    fn graph_builder_counts() {
        let g = square_graph();
        assert_eq!(g.num_nodes, 4);
        // 4 bidirectional edges = 8 directed edges
        assert_eq!(g.num_edges, 8);
    }

    #[test]
    fn graph_edges() {
        let g = square_graph();
        let neighbors: Vec<u32> = g.edges(0).to_vec();
        // Node 0 connects to 1 and 3 (bidirectional)
        assert_eq!(neighbors.len(), 2);
        assert!(neighbors.contains(&1));
        assert!(neighbors.contains(&3));
    }

    #[test]
    fn graph_edges_with_weights() {
        let g = square_graph();
        let edges: Vec<(u32, f32)> = g.edges_with_weights(0).collect();
        assert_eq!(edges.len(), 2);
        for (_, w) in &edges {
            assert!((*w - 10.0).abs() < 1e-6);
        }
    }

    #[test]
    fn graph_node_pos_roundtrip() {
        let g = square_graph();
        let (lat, lon) = g.node_pos(2);
        assert!((lat - 1.0).abs() < 1e-6);
        assert!((lon - 1.0).abs() < 1e-6);
    }

    #[test]
    fn graph_connected_components_single() {
        let g = square_graph();
        let comps = g.connected_components();
        assert_eq!(comps, vec![4]);
    }

    #[test]
    fn graph_connected_components_isolated() {
        let mut b = GraphBuilder::new();
        b.add_node(0.0, 0.0);
        b.add_node(0.0, 1.0);
        b.add_node(1.0, 1.0);
        b.add_node(1.0, 0.0);
        b.add_node(5.0, 5.0); // isolated node
        b.add_edge(0, 1, 10.0);
        b.add_edge(1, 2, 10.0);
        b.add_edge(2, 3, 10.0);
        b.add_edge(3, 0, 10.0);
        let g = b.build();
        let comps = g.connected_components();
        assert_eq!(comps, vec![4, 1]);
    }

    #[test]
    fn graph_save_load_roundtrip() {
        let g = square_graph();
        let mut buf = Vec::new();
        g.save(&mut buf).unwrap();

        let g2 = RoutingGraph::load(std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(g2.num_nodes, g.num_nodes);
        assert_eq!(g2.num_edges, g.num_edges);
        assert_eq!(g2.node_lats, g.node_lats);
        assert_eq!(g2.node_lngs, g.node_lngs);
        assert_eq!(g2.adjacency, g.adjacency);
        assert_eq!(g2.weights, g.weights);
        assert_eq!(g2.offsets, g.offsets);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p asw-core -- graph::tests -v`
Expected: 7 tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/asw-core/src/graph.rs
git commit -m "test: add graph builder and routing graph unit tests"
```

---

### Task 4: Add A* Routing Tests

**Files:**
- Modify: `crates/asw-core/src/routing.rs` (append test module)

- [ ] **Step 1: Add test module to routing.rs**

Append to `crates/asw-core/src/routing.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphBuilder;

    /// Diamond graph: A(0)->B(1) 5km, A(0)->C(2) 10km, B(1)->D(3) 5km, C(2)->D(3) 10km.
    /// Shortest A->D is A->B->D = 10km (unique).
    /// Nodes placed at known lat/lng so haversine heuristic works.
    fn diamond_graph() -> RoutingGraph {
        let mut b = GraphBuilder::new();
        // Place nodes along a line so heuristic doesn't distort
        b.add_node(0.0, 0.0);  // A = node 0
        b.add_node(0.05, 0.0); // B = node 1
        b.add_node(0.0, 0.05); // C = node 2
        b.add_node(0.05, 0.05); // D = node 3
        b.add_edge(0, 1, 5.0);
        b.add_edge(0, 2, 10.0);
        b.add_edge(1, 3, 5.0);
        b.add_edge(2, 3, 10.0);
        b.build()
    }

    #[test]
    fn astar_shortest_path() {
        let g = diamond_graph();
        let result = astar(&g, 0, 3);
        assert!(result.is_some());
        let (path, cost) = result.unwrap();
        assert!((cost - 10.0).abs() < 1e-6, "cost was {cost}, expected 10.0");
        assert_eq!(path.len(), 3); // A -> B -> D
        assert_eq!(path[0], 0); // starts at A
        assert_eq!(*path.last().unwrap(), 3); // ends at D
    }

    #[test]
    fn astar_same_node() {
        let g = diamond_graph();
        let result = astar(&g, 0, 0);
        assert!(result.is_some());
        let (path, cost) = result.unwrap();
        assert_eq!(path, vec![0]);
        assert!((cost - 0.0).abs() < 1e-6);
    }

    #[test]
    fn astar_unreachable() {
        let mut b = GraphBuilder::new();
        b.add_node(0.0, 0.0);
        b.add_node(1.0, 1.0);
        // No edges — nodes are disconnected
        let g = b.build();
        let result = astar(&g, 0, 1);
        assert!(result.is_none());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p asw-core -- routing::tests -v`
Expected: 3 tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/asw-core/src/routing.rs
git commit -m "test: add A* routing unit tests"
```

---

### Task 5: Add parse_latlng Tests

**Files:**
- Modify: `crates/asw-serve/src/api.rs` (append test module)

- [ ] **Step 1: Add test module to api.rs**

Append to `crates/asw-serve/src/api.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid() {
        let (lat, lon) = parse_latlng("36.85,28.27").unwrap();
        assert!((lat - 36.85).abs() < 1e-10);
        assert!((lon - 28.27).abs() < 1e-10);
    }

    #[test]
    fn parse_whitespace() {
        let (lat, lon) = parse_latlng(" 36.85 , 28.27 ").unwrap();
        assert!((lat - 36.85).abs() < 1e-10);
        assert!((lon - 28.27).abs() < 1e-10);
    }

    #[test]
    fn parse_negative() {
        let (lat, lon) = parse_latlng("-33.9,18.4").unwrap();
        assert!((lat - -33.9).abs() < 1e-10);
        assert!((lon - 18.4).abs() < 1e-10);
    }

    #[test]
    fn parse_invalid() {
        assert!(parse_latlng("abc,def").is_none());
    }

    #[test]
    fn parse_too_many_commas() {
        assert!(parse_latlng("1,2,3").is_none());
    }

    #[test]
    fn parse_empty() {
        assert!(parse_latlng("").is_none());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p asw-serve -- api::tests -v`
Expected: 6 tests pass

- [ ] **Step 3: Run full test suite**

Run: `cargo test --workspace`
Expected: 20 tests pass, 0 failures

- [ ] **Step 4: Commit**

```bash
git add crates/asw-serve/src/api.rs
git commit -m "test: add parse_latlng unit tests"
```

---

## Chunk 2: Toolchain Pinning and Config Files

### Task 6: Add rust-toolchain.toml

**Files:**
- Create: `rust-toolchain.toml`

- [ ] **Step 1: Create rust-toolchain.toml**

Write `rust-toolchain.toml` at workspace root:
```toml
[toolchain]
channel = "1.94.0"
components = ["rustfmt", "clippy"]
```

- [ ] **Step 2: Verify it works**

Run: `rustc --version`
Expected: `rustc 1.94.0`

- [ ] **Step 3: Verify build still works**

Run: `cargo clippy --workspace -- -D warnings 2>&1 | tail -3`
Expected: `Finished` with no errors

- [ ] **Step 4: Commit**

```bash
git add rust-toolchain.toml
git commit -m "build: pin Rust 1.94.0 via rust-toolchain.toml"
```

---

### Task 7: Add deny.toml

**Files:**
- Create: `deny.toml`

- [ ] **Step 1: Create deny.toml**

Write `deny.toml` at workspace root:
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

- [ ] **Step 2: Install cargo-deny locally and validate**

Run: `cargo install cargo-deny --locked && cargo deny check 2>&1 | tail -10`
Expected: passes (possibly with warnings for duplicate crates). If licenses are rejected, add them to the allowlist and update spec.

- [ ] **Step 3: Commit**

```bash
git add deny.toml
git commit -m "build: add cargo-deny config for license and advisory checks"
```

---

### Task 8: Add Dependabot Config

**Files:**
- Create: `.github/dependabot.yml`

- [ ] **Step 1: Create dependabot.yml**

Write `.github/dependabot.yml`:
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

- [ ] **Step 2: Commit**

```bash
git add .github/dependabot.yml
git commit -m "ci: add dependabot for cargo and actions updates"
```

---

## Chunk 3: GitHub Actions Workflows

### Task 9: Create Reusable CI Workflow

**Files:**
- Create: `.github/workflows/ci-check.yml`

- [ ] **Step 1: Create ci-check.yml**

Write `.github/workflows/ci-check.yml`:
```yaml
name: CI Check

on:
  workflow_call:

env:
  CARGO_TERM_COLOR: always

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('rust-toolchain.toml') }}-${{ hashFiles('Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-cargo-${{ hashFiles('rust-toolchain.toml') }}-
            ${{ runner.os }}-cargo-

      - name: Check formatting
        run: cargo fmt --all -- --check

      - name: Clippy
        run: cargo clippy --workspace -- -D warnings

      - name: Build
        run: cargo build --workspace

      - name: Test
        run: cargo test --workspace

      - name: License and advisory check
        uses: EmbarkStudios/cargo-deny-action@v2

      - name: Security audit
        uses: rustsec/audit-check@v2
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/ci-check.yml
git commit -m "ci: add reusable CI check workflow"
```

---

### Task 10: Rewrite CI Workflow

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Rewrite ci.yml**

Replace entire `.github/workflows/ci.yml` with:
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

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: simplify CI to use reusable workflow, add concurrency"
```

---

### Task 11: Add CI Gate to Docker Workflow

**Files:**
- Modify: `.github/workflows/docker.yml`

- [ ] **Step 1: Add concurrency and CI gate to docker.yml**

Replace entire `.github/workflows/docker.yml` with:
```yaml
name: Docker

on:
  push:
    tags: ["v*"]

concurrency:
  group: docker-${{ github.ref }}
  cancel-in-progress: true

env:
  REGISTRY: ghcr.io
  IMAGE_NAME: ${{ github.repository }}

jobs:
  check:
    uses: ./.github/workflows/ci-check.yml

  build-and-push:
    needs: [check]
    runs-on: ubuntu-latest
    permissions:
      contents: read
      packages: write

    steps:
      - uses: actions/checkout@v6

      - name: Log in to Container registry
        uses: docker/login-action@v4
        with:
          registry: ${{ env.REGISTRY }}
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Extract metadata
        id: meta
        uses: docker/metadata-action@v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}
          tags: |
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}
            type=raw,value=latest

      - name: Build and push
        uses: docker/build-push-action@v6
        with:
          context: .
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          labels: ${{ steps.meta.outputs.labels }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/docker.yml
git commit -m "ci: add CI gate to Docker workflow"
```

---

### Task 12: Rewrite Release Workflow

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Rewrite release.yml with matrix build**

Replace entire `.github/workflows/release.yml` with:
```yaml
name: Release

on:
  push:
    tags: ["v*"]

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: true

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
      - uses: actions/checkout@v6

      - name: Add target
        run: rustup target add ${{ matrix.target }}

      - name: Install cross
        if: matrix.cross
        uses: taiki-e/install-action@v2
        with:
          tool: cross

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-release-${{ matrix.target }}-${{ hashFiles('rust-toolchain.toml') }}-${{ hashFiles('Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-release-${{ matrix.target }}-

      - name: Build (cross)
        if: matrix.cross
        run: cross build --release --target ${{ matrix.target }} -p asw-cli

      - name: Build (native)
        if: ${{ !matrix.cross }}
        run: cargo build --release --target ${{ matrix.target }} -p asw-cli

      - name: Rename binary
        run: cp target/${{ matrix.target }}/release/asw ${{ matrix.binary }}

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.binary }}
          path: ${{ matrix.binary }}

  release:
    needs: [build]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v6

      - name: Download all artifacts
        uses: actions/download-artifact@v4
        with:
          path: artifacts

      - name: Collect binaries and generate checksums
        run: |
          mkdir -p release
          find artifacts -type f -name 'asw-*' -exec cp {} release/ \;
          cd release
          sha256sum asw-* > SHA256SUMS

      - name: Create release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            release/asw-*
            release/SHA256SUMS
          generate_release_notes: true
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: cross-platform release with 4 targets and SHA256 checksums"
```

---

### Task 13: Final Verification

- [ ] **Step 1: Run full local checks**

Run: `cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings && cargo test --workspace`
Expected: All pass, 20 tests

- [ ] **Step 2: Verify cargo-deny passes**

Run: `cargo deny check 2>&1 | tail -5`
Expected: Passes (warnings OK, no errors)

- [ ] **Step 3: Review all changes**

Run: `git log --oneline HEAD~10..HEAD`
Verify commits look correct and are in logical order.
