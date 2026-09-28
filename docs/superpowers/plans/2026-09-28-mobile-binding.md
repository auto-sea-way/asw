# asw-mobile Binding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `asw-mobile`, a UniFFI-generated Swift and Kotlin binding over `asw-core` with four calls (`open`, `version`, `is_water`, `route`), packaged as an iOS xcframework and an Android AAR attached to every release.

**Architecture:** A new crate wraps `RoutingGraph` in a reference-counted `Graph` object with one lazily created A* buffer set behind a mutex; every exported call runs under `catch_unwind` so panics become errors or `Unknown`. `uniffi-bindgen` (a bin target in the crate) generates the Swift and Kotlin sources from the compiled host library. Two shell scripts build the xcframework and the AAR; the release workflow runs them on macOS and Ubuntu runners and attaches the results.

**Tech Stack:** Rust 1.94 (workspace toolchain), `uniffi` 0.32 with proc-macros, `memmap2` via `asw-core`, Xcode `xcodebuild -create-xcframework`, `cargo-ndk` 4, Android Gradle Plugin, JNA.

**Spec:** `docs/superpowers/specs/2026-09-28-mobile-binding-design.md`

## Global Constraints

- Branch `feat/mobile-binding`, based on `main` after v0.7.0.
- The crate, its docs, comments, scripts, the Gradle project and the README never name any consuming app or company. Grep the tree for the consuming app's name (case-insensitive; the name is in the project memory, not in this repo) before every commit.
- Bindings by UniFFI proc-macros only: `uniffi::setup_scaffolding!()`, `#[uniffi::export]`, `#[derive(uniffi::Object|Record|Enum|Error)]`. No UDL file.
- Module names: Swift `AswMobile` (FFI module `AswMobileFFI`), Kotlin package `org.autoseaway.mobile`, library name `asw_mobile`.
- iOS: targets `aarch64-apple-ios` and `aarch64-apple-ios-sim`, deployment target 17.0. Android: target `aarch64-linux-android` only (ABI `arm64-v8a`), minSdk 21, `-Wl,-z,max-page-size=16384`.
- JNA `net.java.dev.jna:jna:5.14.0@aar` (UniFFI requires 5.12 or newer).
- `route` validates `shore_buffer_nm` to `0.0..=5.0` and rejects non-finite coordinates with `InvalidArgument`.
- Panics never cross the boundary: `route` and `open` panics become `AswError::Internal`, `is_water` panics become `Water::Unknown`.
- Distances are nautical miles. `cargo fmt --all` before every commit; `cargo clippy --workspace --all-targets -- -D warnings` clean at the end of every task; plain conventional commit messages, no attribution trailers.
- This Mac has no Android NDK, no cargo-ndk and no `uniffi-bindgen` on PATH; the Android and iOS packaging steps are verified in CI (Task 7). Locally only the Rust crate, its tests, the host binding generation and the Swift type-check run.

## Review Focus

1. A `Graph` shared across threads with two routes at once: the second must wait on the mutex and both must return correct results, never a poisoned-mutex panic. Pinned by `concurrent_routes_serialise_on_one_buffer_set` in Task 3.
2. A route whose endpoints snap to the same node or lie in clear line of sight: `route` must return the two requested points and a positive distance, not `NoRoute`. Pinned by `route_clear_line_of_sight_returns_two_points` in Task 3.
3. `open` on a directory, a zero-byte file, or a file with the v3 magic: `BadFormat` with a readable message, never a panic or an `io` error string leak. Pinned by `open_rejects_directory_empty_and_v3` in Task 1.
4. `is_water` far from any node (an inland point 400 nm from the sea in a regional graph): `Land`, in bounded time. Pinned by `is_water_far_inland_is_land` in Task 2.
5. A poisoned buffer mutex after a panic inside `route`: the next `route` must still work. Pinned by `route_recovers_after_a_panic` in Task 3.

---

## File Structure

| Path | Responsibility |
| --- | --- |
| `Cargo.toml` (workspace) | add member `crates/asw-mobile`, workspace dep `uniffi = "0.32"` |
| `crates/asw-mobile/Cargo.toml` | crate types `lib`, `staticlib`, `cdylib`; deps `asw-core`, `uniffi`; bin `uniffi-bindgen` |
| `crates/asw-mobile/uniffi.toml` | Swift and Kotlin module and package names |
| `crates/asw-mobile/src/lib.rs` | `AswError`, `Water`, `Coordinate`, `Route`, `Graph`, `open`, panic wrapping, unit tests |
| `crates/asw-mobile/src/bin/uniffi-bindgen.rs` | the bindgen CLI entry point |
| `crates/asw-mobile/scripts/build-ios.sh` | staticlibs for both slices, Swift bindings, xcframework, zip |
| `crates/asw-mobile/scripts/build-android.sh` | cargo-ndk shared library, Kotlin bindings, Gradle AAR |
| `crates/asw-mobile/android/{settings.gradle.kts,build.gradle.kts,gradle.properties,src/main/AndroidManifest.xml}` | minimal Android library project wrapping the .so and generated Kotlin |
| `crates/asw-mobile/README.md` | how to consume both artefacts, measured numbers |
| `.github/workflows/ci-check.yml`, `ci.yml`, `release.yml` | binding checks on PRs, `mobile` job on release, assets in the release |
| `README.md`, `CHANGELOG.md` | mention the binding under Packages; Unreleased entry |

---

### Task 1: Crate skeleton, `open` and `version`

**Files:**
- Modify: `Cargo.toml` (workspace members and dependencies)
- Create: `crates/asw-mobile/Cargo.toml`, `crates/asw-mobile/uniffi.toml`, `crates/asw-mobile/src/lib.rs`, `crates/asw-mobile/src/bin/uniffi-bindgen.rs`

**Interfaces:**
- Consumes: `asw_core::graph::{RoutingGraph, GraphBuilder}` (`RoutingGraph::open(&Path, populate: bool) -> anyhow::Result<RoutingGraph>`, `version() -> &str`, `save(&Path)`; `GraphBuilder::with_version`, `add_node`, `add_edge`, `coastline_runs`, `build()`).
- Produces: `pub enum AswError { NotFound, BadFormat { message }, InvalidArgument { message }, NoRoute, Internal { message } }`, `pub struct Graph`, `pub fn open(path: String) -> Result<Arc<Graph>, AswError>`, `Graph::version(&self) -> String`, and a test helper `fn fixture_graph_path() -> (tempdir, PathBuf)`.

- [ ] **Step 1: Workspace and crate manifests**

Append to `[workspace.dependencies]` in the root `Cargo.toml`:

```toml
uniffi = "0.32"
```

and add `"crates/asw-mobile",` to `members`.

Create `crates/asw-mobile/Cargo.toml`:

```toml
[package]
name = "asw-mobile"
version = "0.7.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Mobile bindings (Swift, Kotlin) for the auto-sea-way routing graph"

[lib]
name = "asw_mobile"
crate-type = ["lib", "staticlib", "cdylib"]

[[bin]]
name = "uniffi-bindgen"
path = "src/bin/uniffi-bindgen.rs"

[dependencies]
asw-core = { path = "../asw-core" }
uniffi = { workspace = true, features = ["cli"] }
```

Create `crates/asw-mobile/uniffi.toml`:

```toml
[bindings.swift]
module_name = "AswMobile"
ffi_module_name = "AswMobileFFI"
cdylib_name = "asw_mobile"

[bindings.kotlin]
package_name = "org.autoseaway.mobile"
cdylib_name = "asw_mobile"
```

Create `crates/asw-mobile/src/bin/uniffi-bindgen.rs`:

```rust
fn main() {
    uniffi::uniffi_bindgen_main()
}
```

- [ ] **Step 2: Write the failing tests**

Create `crates/asw-mobile/src/lib.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use asw_core::graph::GraphBuilder;
    use std::path::PathBuf;

    /// One res-5 water node at (36.5, 28.3), a thin mole ring between it and
    /// a berth at (36.5, 28.0), and an island ring around (36.5, 27.6).
    /// Written to a temporary directory as a v4 file.
    pub(crate) fn fixture_graph_path() -> (PathBuf, PathBuf) {
        let cell = h3o::LatLng::new(36.5, 28.3)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let mut b = GraphBuilder::with_version("test 2026-09-28");
        b.add_node(u64::from(cell), 255);
        b.coastline_runs = vec![
            vec![(28.10, 36.40), (28.11, 36.40), (28.11, 36.60), (28.10, 36.60), (28.10, 36.40)],
            vec![(27.5, 36.4), (27.7, 36.4), (27.7, 36.6), (27.5, 36.6), (27.5, 36.4)],
        ];
        let dir = std::env::temp_dir().join(format!(
            "asw-mobile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fixture.graph");
        b.build().save(&path).unwrap();
        (dir, path)
    }

    #[test]
    fn open_reports_the_header_version() {
        let (dir, path) = fixture_graph_path();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.version(), "test 2026-09-28");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_missing_file_is_not_found() {
        let err = open("/nonexistent/asw.graph".into()).unwrap_err();
        assert_eq!(err, AswError::NotFound);
    }

    #[test]
    fn open_rejects_directory_empty_and_v3() {
        let (dir, _) = fixture_graph_path();
        let empty = dir.join("empty.graph");
        std::fs::write(&empty, b"").unwrap();
        let v3 = dir.join("v3.graph");
        std::fs::write(&v3, b"ASW\x03whatever").unwrap();
        for p in [dir.clone(), empty, v3] {
            match open(p.to_string_lossy().into_owned()) {
                Err(AswError::BadFormat { message }) => assert!(!message.is_empty()),
                other => panic!("{p:?}: expected BadFormat, got {other:?}"),
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn error_display_is_readable() {
        assert_eq!(AswError::NotFound.to_string(), "graph file not found");
        assert_eq!(
            AswError::BadFormat {
                message: "x".into()
            }
            .to_string(),
            "not a usable graph file: x"
        );
    }
}
```

Add `h3o.workspace = true` under `[dev-dependencies]` in the crate manifest (tests build cells).

- [ ] **Step 3: Run to see them fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p asw-mobile 2>&1 | grep -E "^error" | sort | uniq -c`
Expected: errors that `open`, `AswError` and `Graph` are not found.

- [ ] **Step 4: Implement**

Above the test module in `crates/asw-mobile/src/lib.rs`:

```rust
//! Mobile bindings for the auto-sea-way routing graph. Four calls over a
//! memory-mapped v4 file: `open`, `version`, `is_water`, `route`. Generated
//! into Swift and Kotlin by UniFFI; panics never cross the boundary.

use asw_core::astar_pool::AstarBuffers;
use asw_core::graph::RoutingGraph;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Arc, Mutex};

uniffi::setup_scaffolding!();

#[derive(Debug, Clone, PartialEq, uniffi::Error)]
pub enum AswError {
    NotFound,
    BadFormat { message: String },
    InvalidArgument { message: String },
    NoRoute,
    Internal { message: String },
}

impl std::fmt::Display for AswError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AswError::NotFound => write!(f, "graph file not found"),
            AswError::BadFormat { message } => write!(f, "not a usable graph file: {message}"),
            AswError::InvalidArgument { message } => write!(f, "invalid argument: {message}"),
            AswError::NoRoute => write!(f, "no route between the given points"),
            AswError::Internal { message } => write!(f, "internal error: {message}"),
        }
    }
}

impl std::error::Error for AswError {}

/// An opened graph file. Reference counted across the FFI; safe to share
/// between threads. Routes serialise on the single A* buffer set.
#[derive(uniffi::Object)]
pub struct Graph {
    inner: RoutingGraph,
    buffers: Mutex<Option<AstarBuffers>>,
}

/// Turn a panic payload into a message.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

/// Memory-map a v4 graph file and validate its header. Milliseconds; no
/// per-node work.
#[uniffi::export]
pub fn open(path: String) -> Result<Arc<Graph>, AswError> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(AswError::NotFound);
    }
    let opened = catch_unwind(AssertUnwindSafe(|| RoutingGraph::open(p, false)))
        .map_err(|e| AswError::Internal {
            message: panic_message(e),
        })?;
    let inner = opened.map_err(|e| AswError::BadFormat {
        message: format!("{e:#}"),
    })?;
    Ok(Arc::new(Graph {
        inner,
        buffers: Mutex::new(None),
    }))
}

#[uniffi::export]
impl Graph {
    /// The version string stored in the file header.
    pub fn version(&self) -> String {
        self.inner.version().to_string()
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p asw-mobile 2>&1 | grep -E "^test |test result"`
Expected: 4 passed. If `open` on the directory returns an `Internal` error instead of `BadFormat`, `RoutingGraph::open` panicked on a directory read; wrap the `File::open` failure inside the `BadFormat` mapping instead (it should already be an `Err`, since `memmap2` on a directory fails with an io error).

- [ ] **Step 6: Clippy, fmt, commit**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all`

```bash
git add Cargo.toml Cargo.lock crates/asw-mobile
git commit -m "feat(mobile): asw-mobile crate with open and version"
```

---

### Task 2: `is_water` with panic containment

**Files:**
- Modify: `crates/asw-mobile/src/lib.rs`

**Interfaces:**
- Consumes: `asw_core::routing::is_water(&RoutingGraph, lat, lon) -> bool`.
- Produces: `#[derive(uniffi::Enum)] pub enum Water { Water, Land, Unknown }`, `Graph::is_water(&self, lat: f64, lon: f64) -> Water`, and a test-only panic switch `PANIC_NEXT: AtomicBool` consulted by the inner functions.

- [ ] **Step 1: Write the failing tests**

Add to the test module:

```rust
    #[test]
    fn is_water_marina_behind_mole_is_water() {
        let (dir, path) = fixture_graph_path();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.is_water(36.5, 28.0), Water::Water, "berth behind the mole");
        assert_eq!(g.is_water(36.5, 28.3), Water::Water, "next to the node");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn is_water_inside_island_is_land() {
        let (dir, path) = fixture_graph_path();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.is_water(36.5, 27.6), Water::Land);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn is_water_far_inland_is_land() {
        // A graph with no node at all: the snapping ladder finds nothing, so
        // the answer is Land, in bounded time (the res-3 fallback disk).
        let empty = GraphBuilder::default().build();
        let dir = std::env::temp_dir().join(format!("asw-mobile-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.graph");
        empty.save(&path).unwrap();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.is_water(44.8, 20.5), Water::Land);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn is_water_panic_becomes_unknown() {
        let (dir, path) = fixture_graph_path();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        PANIC_NEXT.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(g.is_water(36.5, 28.3), Water::Unknown);
        assert_eq!(g.is_water(36.5, 28.3), Water::Water, "the switch is one-shot");
        std::fs::remove_dir_all(dir).unwrap();
    }
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p asw-mobile is_water 2>&1 | grep -E "^error" | sort | uniq -c`
Expected: `Water`, `is_water` and `PANIC_NEXT` not found.

- [ ] **Step 3: Implement**

Add after `AswError`'s impls:

```rust
/// Answer of `Graph::is_water`. `Unknown` only when the call could not be
/// evaluated (a panic inside the graph code or a vanished mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Water {
    Water,
    Land,
    Unknown,
}

/// Test-only one-shot switch that makes the next `is_water` or `route` panic,
/// proving the panic never crosses the boundary.
#[cfg(test)]
static PANIC_NEXT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
fn maybe_panic() {
    if PANIC_NEXT.swap(false, std::sync::atomic::Ordering::SeqCst) {
        panic!("forced panic for the boundary test");
    }
}

#[cfg(not(test))]
fn maybe_panic() {}
```

and inside the `#[uniffi::export] impl Graph` block:

```rust
    /// Is the point on navigable water? Snap to the nearest water node and
    /// count coastline crossings on the way; even means water, no node means
    /// land. Never panics.
    pub fn is_water(&self, lat: f64, lon: f64) -> Water {
        match catch_unwind(AssertUnwindSafe(|| {
            maybe_panic();
            asw_core::routing::is_water(&self.inner, lat, lon)
        })) {
            Ok(true) => Water::Water,
            Ok(false) => Water::Land,
            Err(_) => Water::Unknown,
        }
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p asw-mobile 2>&1 | grep -E "^test |test result"`
Expected: 8 passed.

- [ ] **Step 5: Clippy, fmt, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all
git add crates/asw-mobile
git commit -m "feat(mobile): is_water with panic containment"
```

---

### Task 3: `route` with shore buffer, validation and buffer recovery

**Files:**
- Modify: `crates/asw-mobile/src/lib.rs`

**Interfaces:**
- Consumes: `asw_core::routing::compute_route(graph, from_lat, from_lon, to_lat, to_lon, &coastline, &knn, &mut buffers, shore_buffer_nm) -> Option<RouteResult { distance_nm: f64, raw_hops, smooth_hops, coordinates: Vec<[f64; 2]> /* [lon, lat] */, land_legs: Vec<usize> }>`, `RoutingGraph::nearest_node`, `RoutingGraph::coastline()`, `RoutingGraph::num_nodes()`, `AstarBuffers::new(n)`, `AstarBuffers::reset()`.
- Produces: `#[derive(uniffi::Record)] Coordinate { lat: f64, lon: f64 }`, `#[derive(uniffi::Record)] Route { coordinates: Vec<Coordinate>, distance_nm: f64, land_legs: Vec<u32> }`, `Graph::route(&self, from_lat, from_lon, to_lat, to_lon, shore_buffer_nm: f64) -> Result<Route, AswError>`.

- [ ] **Step 1: Write the failing tests**

Add a second fixture and the tests:

```rust
    /// Three res-5 nodes in a chain across the Aegean, no coastline, so a
    /// route must go through the graph when the direct line is blocked by a
    /// wall we add per test.
    pub(crate) fn chain_graph_path(coast: Vec<Vec<(f64, f64)>>) -> (PathBuf, PathBuf) {
        let coords = [(36.0, 26.0), (36.5, 27.0), (37.0, 28.0)];
        let mut h3s: Vec<u64> = coords
            .iter()
            .map(|&(lat, lon)| {
                u64::from(
                    h3o::LatLng::new(lat, lon)
                        .unwrap()
                        .to_cell(h3o::Resolution::Five),
                )
            })
            .collect();
        h3s.sort_unstable();
        h3s.dedup();
        let mut b = GraphBuilder::with_version("chain");
        let ids: Vec<u32> = h3s.iter().map(|&h| b.add_node(h, 255)).collect();
        for w in ids.windows(2) {
            b.add_edge(w[0], w[1]);
        }
        b.coastline_runs = coast;
        let dir = std::env::temp_dir().join(format!(
            "asw-mobile-chain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chain.graph");
        b.build().save(&path).unwrap();
        (dir, path)
    }

    #[test]
    fn route_clear_line_of_sight_returns_two_points() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        let r = g.route(36.0, 26.0, 37.0, 28.0, 0.0).unwrap();
        assert_eq!(r.coordinates.len(), 2);
        assert_eq!(r.coordinates[0], Coordinate { lat: 36.0, lon: 26.0 });
        assert_eq!(r.coordinates[1], Coordinate { lat: 37.0, lon: 28.0 });
        assert!(r.distance_nm > 90.0 && r.distance_nm < 130.0, "{}", r.distance_nm);
        assert!(r.land_legs.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_through_the_graph_when_the_direct_line_is_blocked() {
        // A wall at lon 27.5 blocks the direct line, so the route goes through
        // the graph. The chain's own hop 27.0 -> 28.0 crosses the wall too and
        // is reported as a land leg; what this test pins is that a route comes
        // back and that a shore buffer is accepted.
        let wall = vec![vec![(27.5, 36.2), (27.5, 37.5)]];
        let (dir, path) = chain_graph_path(wall);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        let r = g.route(36.0, 26.0, 37.0, 28.0, 0.5).unwrap();
        assert!(r.coordinates.len() >= 2);
        assert!(r.distance_nm > 0.0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_rejects_bad_arguments() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, 6.0),
            Err(AswError::InvalidArgument { .. })
        ));
        assert!(matches!(
            g.route(f64::NAN, 26.0, 37.0, 28.0, 0.0),
            Err(AswError::InvalidArgument { .. })
        ));
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, -0.1),
            Err(AswError::InvalidArgument { .. })
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_with_no_nodes_and_a_wall_is_no_route() {
        let empty = {
            let mut b = GraphBuilder::default();
            b.coastline_runs = vec![vec![(27.5, 36.2), (27.5, 37.5)]];
            b.build()
        };
        let dir = std::env::temp_dir().join(format!("asw-mobile-noroute-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("noroute.graph");
        empty.save(&path).unwrap();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.route(36.0, 26.0, 37.0, 28.0, 0.0).unwrap_err(), AswError::NoRoute);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_recovers_after_a_panic() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        PANIC_NEXT.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, 0.0),
            Err(AswError::Internal { .. })
        ));
        assert!(g.route(36.0, 26.0, 37.0, 28.0, 0.0).is_ok(), "mutex must not stay poisoned");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_routes_serialise_on_one_buffer_set() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let g = Arc::clone(&g);
                std::thread::spawn(move || g.route(36.0, 26.0, 37.0, 28.0, 0.0).unwrap().distance_nm)
            })
            .collect();
        let d: Vec<f64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(d.iter().all(|&x| (x - d[0]).abs() < 1e-9));
        std::fs::remove_dir_all(dir).unwrap();
    }
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p asw-mobile route 2>&1 | grep -E "^error" | sort | uniq -c`
Expected: `route`, `Route`, `Coordinate` not found.

- [ ] **Step 3: Implement**

Add the records after `Water`:

```rust
/// A point as the app thinks of it: latitude first, degrees.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Coordinate {
    pub lat: f64,
    pub lon: f64,
}

/// Result of `Graph::route`. Same semantics as the HTTP `/route` response:
/// the polyline starts and ends at the requested points, `distance_nm`
/// counts water segments only, `land_legs` are indices of segments
/// (`coordinates[i] -> coordinates[i+1]`) that cross land.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Route {
    pub coordinates: Vec<Coordinate>,
    pub distance_nm: f64,
    pub land_legs: Vec<u32>,
}
```

and inside the `#[uniffi::export] impl Graph` block:

```rust
    /// Route between two points with an optional shore clearance in
    /// nautical miles (0 to 5). Blocking; routes serialise on one buffer set.
    pub fn route(
        &self,
        from_lat: f64,
        from_lon: f64,
        to_lat: f64,
        to_lon: f64,
        shore_buffer_nm: f64,
    ) -> Result<Route, AswError> {
        for (name, v) in [
            ("from_lat", from_lat),
            ("from_lon", from_lon),
            ("to_lat", to_lat),
            ("to_lon", to_lon),
        ] {
            if !v.is_finite() {
                return Err(AswError::InvalidArgument {
                    message: format!("{name} is not a finite number"),
                });
            }
        }
        if !shore_buffer_nm.is_finite() || !(0.0..=5.0).contains(&shore_buffer_nm) {
            return Err(AswError::InvalidArgument {
                message: "shore_buffer_nm must be between 0 and 5 nautical miles".into(),
            });
        }
        // A panic inside must not poison the mutex for the next call, so
        // the lock is taken inside the unwind boundary and the guard is
        // dropped before the boundary is left; `unwrap_or_else` below
        // recovers a poisoned lock for good measure.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            maybe_panic();
            let mut slot = self
                .buffers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let buffers =
                slot.get_or_insert_with(|| AstarBuffers::new(self.inner.num_nodes() as usize));
            let knn = |lat: f64, lon: f64| self.inner.nearest_node(lat, lon);
            let result = asw_core::routing::compute_route(
                &self.inner,
                from_lat,
                from_lon,
                to_lat,
                to_lon,
                &self.inner.coastline(),
                &knn,
                buffers,
                shore_buffer_nm,
            );
            buffers.reset();
            result
        }));
        match outcome {
            Err(payload) => Err(AswError::Internal {
                message: panic_message(payload),
            }),
            Ok(None) => Err(AswError::NoRoute),
            Ok(Some(r)) => Ok(Route {
                coordinates: r
                    .coordinates
                    .iter()
                    .map(|c| Coordinate { lat: c[1], lon: c[0] })
                    .collect(),
                distance_nm: r.distance_nm,
                land_legs: r.land_legs.iter().map(|&i| i as u32).collect(),
            }),
        }
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p asw-mobile 2>&1 | grep -E "^test |test result"`
Expected: 14 passed. If `route_with_no_nodes_and_a_wall_is_no_route` returns `Ok`, the wall does not cross the direct line; move it to lon 27.0 between lat 35.0 and 38.0.

- [ ] **Step 5: Clippy, fmt, commit**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all
git add crates/asw-mobile
git commit -m "feat(mobile): route with shore buffer, argument validation and panic recovery"
```

---

### Task 4: Binding generation on the host and the PR check

**Files:**
- Modify: `.github/workflows/ci-check.yml` (Kotlin generation and compile on Ubuntu)
- Modify: `.github/workflows/ci.yml` (a macOS job for the Swift type-check)
- Create: `crates/asw-mobile/scripts/gen-bindings.sh`

**Interfaces:**
- Consumes: the `uniffi-bindgen` bin from Task 1, the host `cdylib` at `target/<profile>/libasw_mobile.{dylib,so}`.
- Produces: `crates/asw-mobile/scripts/gen-bindings.sh <profile> <language> <out-dir>` writing `AswMobile.swift`, `AswMobileFFI.h`, `AswMobileFFI.modulemap` for swift and `org/autoseaway/mobile/asw_mobile.kt` for kotlin.

- [ ] **Step 1: The generation script**

Create `crates/asw-mobile/scripts/gen-bindings.sh` (executable):

```bash
#!/usr/bin/env bash
# Generate UniFFI bindings from the host build of asw-mobile.
# Usage: gen-bindings.sh <debug|release> <swift|kotlin> <out-dir>
set -euo pipefail
profile="$1"; lang="$2"; out="$3"
case "$(uname -s)" in
  Darwin) lib="target/$profile/libasw_mobile.dylib" ;;
  *)      lib="target/$profile/libasw_mobile.so" ;;
esac
flag=""; [ "$profile" = "release" ] && flag="--release"
cargo build $flag -p asw-mobile
cargo run $flag -p asw-mobile --bin uniffi-bindgen -- generate \
  --library "$lib" --language "$lang" --out-dir "$out" \
  --config crates/asw-mobile/uniffi.toml
```

Run it once locally for both languages and check the outputs exist:

Run: `chmod +x crates/asw-mobile/scripts/gen-bindings.sh && crates/asw-mobile/scripts/gen-bindings.sh debug swift target/uniffi/swift && crates/asw-mobile/scripts/gen-bindings.sh debug kotlin target/uniffi/kotlin && ls target/uniffi/swift target/uniffi/kotlin/org/autoseaway/mobile`
Expected: `AswMobile.swift AswMobileFFI.h AswMobileFFI.modulemap` and `asw_mobile.kt`.

- [ ] **Step 2: Swift type-check locally**

Run:
```bash
mkdir -p target/uniffi/swift-mod && cp target/uniffi/swift/AswMobileFFI.h target/uniffi/swift-mod/ && cp target/uniffi/swift/AswMobileFFI.modulemap target/uniffi/swift-mod/module.modulemap
swiftc -typecheck -I target/uniffi/swift-mod target/uniffi/swift/AswMobile.swift && echo "swift ok"
```
Expected: `swift ok`. Confirm `grep -n "func route\|func isWater\|func version\|func open" target/uniffi/swift/AswMobile.swift` shows the four calls.

- [ ] **Step 3: CI, Ubuntu side**

In `.github/workflows/ci-check.yml`, after the `Test` step add:

```yaml
      - name: Generate Kotlin bindings
        run: crates/asw-mobile/scripts/gen-bindings.sh debug kotlin target/uniffi/kotlin

      - name: Compile Kotlin bindings
        run: |
          curl -sSL -o /tmp/jna.jar https://repo1.maven.org/maven2/net/java/dev/jna/jna/5.14.0/jna-5.14.0.jar
          kotlinc -cp /tmp/jna.jar -d /tmp/kt-out $(find target/uniffi/kotlin -name '*.kt')
```

`kotlinc` is preinstalled on `ubuntu-latest`; if the runner image ever drops it, install with `sudo snap install --classic kotlin`.

- [ ] **Step 4: CI, macOS side**

In `.github/workflows/ci.yml` add a second job:

```yaml
  mobile-swift:
    runs-on: macos-latest
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@v7
      - name: Cache cargo
        uses: actions/cache@v6
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-mobile-${{ hashFiles('rust-toolchain.toml') }}-${{ hashFiles('Cargo.lock') }}
      - name: Generate Swift bindings
        run: crates/asw-mobile/scripts/gen-bindings.sh debug swift target/uniffi/swift
      - name: Type-check Swift bindings
        run: |
          mkdir -p target/uniffi/swift-mod
          cp target/uniffi/swift/AswMobileFFI.h target/uniffi/swift-mod/
          cp target/uniffi/swift/AswMobileFFI.modulemap target/uniffi/swift-mod/module.modulemap
          swiftc -typecheck -I target/uniffi/swift-mod target/uniffi/swift/AswMobile.swift
```

- [ ] **Step 5: Commit**

```bash
git add crates/asw-mobile/scripts/gen-bindings.sh .github/workflows/ci-check.yml .github/workflows/ci.yml
git commit -m "ci(mobile): generate and type-check the Swift and Kotlin bindings on every PR"
```

---

### Task 5: Android AAR project and build script

**Files:**
- Create: `crates/asw-mobile/android/settings.gradle.kts`, `build.gradle.kts`, `gradle.properties`, `src/main/AndroidManifest.xml`
- Create: `crates/asw-mobile/scripts/build-android.sh`

**Interfaces:**
- Consumes: `gen-bindings.sh` from Task 4.
- Produces: `crates/asw-mobile/scripts/build-android.sh <version>` writing `target/mobile/asw-mobile-<version>.aar`.

- [ ] **Step 1: Gradle project**

`crates/asw-mobile/android/settings.gradle.kts`:

```kotlin
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "asw-mobile"
```

`crates/asw-mobile/android/build.gradle.kts`:

```kotlin
plugins {
    id("com.android.library") version "8.13.0"
    id("org.jetbrains.kotlin.android") version "2.2.20"
}

android {
    namespace = "org.autoseaway.mobile"
    compileSdk = 36
    defaultConfig {
        minSdk = 21
    }
    sourceSets["main"].kotlin.srcDir("../../../target/uniffi/kotlin")
    sourceSets["main"].jniLibs.srcDir("../../../target/jniLibs")
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation("net.java.dev.jna:jna:5.14.0@aar")
}
```

`crates/asw-mobile/android/gradle.properties`:

```
android.useAndroidX=true
org.gradle.jvmargs=-Xmx2g
```

`crates/asw-mobile/android/src/main/AndroidManifest.xml`:

```xml
<manifest xmlns:android="http://schemas.android.com/apk/res/android" />
```

- [ ] **Step 2: Build script**

`crates/asw-mobile/scripts/build-android.sh` (executable):

```bash
#!/usr/bin/env bash
# Build the Android AAR: arm64-v8a shared library + generated Kotlin.
# Usage: build-android.sh <version>   (needs ANDROID_NDK_HOME, cargo-ndk, gradle)
set -euo pipefail
version="$1"
rustup target add aarch64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked 2>/dev/null || true
rm -rf target/jniLibs
# 16 KB page alignment for Android 15 devices.
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
  cargo ndk --platform 21 -t arm64-v8a -o target/jniLibs build --release -p asw-mobile
crates/asw-mobile/scripts/gen-bindings.sh release kotlin target/uniffi/kotlin
(cd crates/asw-mobile/android && gradle --no-daemon assembleRelease)
mkdir -p target/mobile
cp crates/asw-mobile/android/build/outputs/aar/asw-mobile-release.aar "target/mobile/asw-mobile-$version.aar"
echo "wrote target/mobile/asw-mobile-$version.aar"
```

Add `crates/asw-mobile/android/build/` and `crates/asw-mobile/android/.gradle/` to `.gitignore`.

- [ ] **Step 3: Verify what can be verified locally**

The NDK is not on this Mac, so only the Gradle project's syntax is checked here: `cd crates/asw-mobile/android && gradle --no-daemon help -q` must print the help text (the plugins resolve; Gradle is available through the Android project on this machine, otherwise skip and rely on Task 7's CI run).

- [ ] **Step 4: Commit**

```bash
git add crates/asw-mobile/android crates/asw-mobile/scripts/build-android.sh .gitignore
git commit -m "feat(mobile): Android AAR project and build script"
```

---

### Task 6: iOS xcframework build script

**Files:**
- Create: `crates/asw-mobile/scripts/build-ios.sh`

**Interfaces:**
- Consumes: `gen-bindings.sh`.
- Produces: `crates/asw-mobile/scripts/build-ios.sh <version>` writing `target/mobile/AswMobile-<version>.zip` containing `AswMobile.xcframework/` and `AswMobile.swift`.

- [ ] **Step 1: Script**

`crates/asw-mobile/scripts/build-ios.sh` (executable):

```bash
#!/usr/bin/env bash
# Build the iOS xcframework (device + simulator, arm64) and zip it with the
# generated Swift source. Usage: build-ios.sh <version>   (macOS with Xcode)
set -euo pipefail
version="$1"
export IPHONEOS_DEPLOYMENT_TARGET=17.0
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build --release -p asw-mobile --target aarch64-apple-ios
cargo build --release -p asw-mobile --target aarch64-apple-ios-sim
crates/asw-mobile/scripts/gen-bindings.sh release swift target/uniffi/swift
rm -rf target/xc && mkdir -p target/xc/Headers target/mobile
cp target/uniffi/swift/AswMobileFFI.h target/xc/Headers/
cp target/uniffi/swift/AswMobileFFI.modulemap target/xc/Headers/module.modulemap
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libasw_mobile.a -headers target/xc/Headers \
  -library target/aarch64-apple-ios-sim/release/libasw_mobile.a -headers target/xc/Headers \
  -output target/xc/AswMobile.xcframework
cp target/uniffi/swift/AswMobile.swift target/xc/
(cd target/xc && rm -f "../mobile/AswMobile-$version.zip" && zip -qr "../mobile/AswMobile-$version.zip" AswMobile.xcframework AswMobile.swift)
echo "wrote target/mobile/AswMobile-$version.zip"
```

- [ ] **Step 2: Run it locally**

This Mac has Xcode 27, so the full iOS build runs here.

Run: `chmod +x crates/asw-mobile/scripts/build-ios.sh && crates/asw-mobile/scripts/build-ios.sh 0.7.0 && unzip -l target/mobile/AswMobile-0.7.0.zip | tail -3 && lipo -info target/aarch64-apple-ios/release/libasw_mobile.a`
Expected: the zip lists `AswMobile.xcframework/ios-arm64/...`, `ios-arm64-simulator/...` and `AswMobile.swift`; `lipo` reports `arm64`. If `rustup target add` fails because the Homebrew rustc is first on PATH, run with `export PATH="$HOME/.cargo/bin:$PATH"`.

- [ ] **Step 3: Smoke-check the Swift package compiles against the xcframework**

Run:
```bash
mkdir -p target/xc/smoke && cat > target/xc/smoke/main.swift <<'EOF'
import AswMobile
do { _ = try open(path: "/nonexistent") } catch let e as AswError { print("expected error: \(e)") }
EOF
swiftc -target arm64-apple-ios17.0-simulator -sdk "$(xcrun --sdk iphonesimulator --show-sdk-path)" \
  -I target/xc/AswMobile.xcframework/ios-arm64-simulator/Headers \
  target/xc/AswMobile.swift target/xc/smoke/main.swift \
  target/xc/AswMobile.xcframework/ios-arm64-simulator/libasw_mobile.a \
  -o target/xc/smoke/smoke && echo "links ok"
```
Expected: `links ok`. (The module map inside the xcframework exposes `AswMobileFFI`; `AswMobile.swift` imports it.) Running the binary needs a simulator and is not required.

- [ ] **Step 4: Commit**

```bash
git add crates/asw-mobile/scripts/build-ios.sh
git commit -m "feat(mobile): iOS xcframework build script"
```

---

### Task 7: Release workflow `mobile` job and assets

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `build-ios.sh`, `build-android.sh`.
- Produces: release assets `AswMobile-<version>.zip` and `asw-mobile-<version>.aar`, listed in `SHA256SUMS`.

- [ ] **Step 1: Add the job**

After the `build` job in `.github/workflows/release.yml`:

```yaml
  mobile:
    needs: [check]
    strategy:
      matrix:
        include:
          - os: macos-latest
            script: crates/asw-mobile/scripts/build-ios.sh
            artifact: AswMobile-${{ inputs.version }}.zip
          - os: ubuntu-latest
            script: crates/asw-mobile/scripts/build-android.sh
            artifact: asw-mobile-${{ inputs.version }}.aar
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v7

      - name: Cache cargo
        uses: actions/cache@v6
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-mobile-${{ hashFiles('rust-toolchain.toml') }}-${{ hashFiles('Cargo.lock') }}

      - name: Set up Gradle
        if: runner.os == 'Linux'
        uses: gradle/actions/setup-gradle@v5
        with:
          gradle-version: "8.14"

      - name: Set up JDK 17
        if: runner.os == 'Linux'
        uses: actions/setup-java@v5
        with:
          distribution: temurin
          java-version: "17"

      - name: Build
        run: ${{ matrix.script }} ${{ inputs.version }}
        env:
          ANDROID_NDK_HOME: ${{ env.ANDROID_NDK_LATEST_HOME }}

      - name: Upload artifact
        uses: actions/upload-artifact@v7
        with:
          name: ${{ matrix.artifact }}
          path: target/mobile/${{ matrix.artifact }}
```

- [ ] **Step 2: Collect the assets**

Change the `release` job: `needs: [build, mobile]`, and in `Collect binaries and generate checksums`:

```yaml
      - name: Collect binaries and generate checksums
        run: |
          mkdir -p release
          find artifacts -type f \( -name 'asw-*' -o -name 'AswMobile-*.zip' \) -exec cp {} release/ \;
          cp asw.graph release/
          cd release
          sha256sum asw-* AswMobile-*.zip asw.graph > SHA256SUMS
```

(The AAR is named `asw-mobile-<version>.aar`, so `asw-*` already matches it.) In `Create release` add `release/AswMobile-*.zip` to `files:`.

- [ ] **Step 3: Validate the YAML**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release.yml')); yaml.safe_load(open('.github/workflows/ci.yml')); yaml.safe_load(open('.github/workflows/ci-check.yml')); print('yaml ok')"`
Expected: `yaml ok`. (If PyYAML is missing: `python3 -m pip install --user pyyaml`.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): build and attach the iOS xcframework and Android AAR"
```

---

### Task 8: README, changelog, measured numbers

**Files:**
- Create: `crates/asw-mobile/README.md`
- Modify: `README.md` (Packages section), `CHANGELOG.md` (Unreleased)

- [ ] **Step 1: Measure on the planet file**

The release graph is at `export/asw-v4d.graph` (1,437,518,024 bytes, sha256 `0575cb5b…`). Write a throwaway probe in the scratchpad (not in the repo) that opens it through `asw_mobile::open`, then reads resident memory from `ps -o rss=` for its own pid after each step:

```rust
// scratchpad probe, depends on asw-mobile by path
use std::time::Instant;
fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap() / 1024.0
}
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let base = rss_mb();
    let t = Instant::now();
    let g = asw_mobile::open(path).unwrap();
    println!("open: {:?}, rss +{:.1} MB", t.elapsed(), rss_mb() - base);
    let t = Instant::now();
    let w = g.is_water(36.5, 28.0);
    println!("is_water {:?}: {:?}, rss +{:.1} MB", w, t.elapsed(), rss_mb() - base);
    let t = Instant::now();
    let r = g.route(36.85, 28.28, 36.44, 28.23, 0.0).unwrap();
    println!("short route {:.1} nm: {:?}, rss +{:.1} MB", r.distance_nm, t.elapsed(), rss_mb() - base);
    let t = Instant::now();
    let r = g.route(40.65, -74.03, 50.89, -1.39, 0.0).unwrap();
    println!("transatlantic {:.1} nm: {:?}, rss +{:.1} MB", r.distance_nm, t.elapsed(), rss_mb() - base);
}
```

Run it in release mode against `export/asw-v4d.graph` and record the four lines. The measurement is on macOS with the file in the page cache; label it so in the README.

- [ ] **Step 2: Crate README**

`crates/asw-mobile/README.md`:

```markdown
# asw-mobile

Swift and Kotlin bindings for the auto-sea-way routing graph. Four calls over a
memory-mapped v4 graph file:

    open(path) -> Graph
    Graph.version() -> String
    Graph.isWater(lat, lon) -> Water (.water | .land | .unknown)
    Graph.route(fromLat, fromLon, toLat, toLon, shoreBufferNm) -> Route

`Route` has `coordinates` (latitude, longitude), `distanceNm` (water only) and
`landLegs` (indices of segments that cross land). Errors are `AswError`:
`NotFound`, `BadFormat`, `InvalidArgument`, `NoRoute`, `Internal`. A panic inside
the graph code surfaces as `Internal` (or `.unknown` from `isWater`); the library
never aborts the host app.

## iOS

Every release attaches `AswMobile-<version>.zip` containing `AswMobile.xcframework`
(arm64 device and simulator, iOS 17+) and `AswMobile.swift`. Declare a binary
target with the asset URL and its checksum from `SHA256SUMS`, and add
`AswMobile.swift` to your app target.

## Android

Every release attaches `asw-mobile-<version>.aar` (arm64-v8a, minSdk 21, Android
15 page alignment). Fetch it for your pinned version into `libs/` and add
`net.java.dev.jna:jna:5.14.0@aar` alongside it; the generated Kotlin lives in
package `org.autoseaway.mobile`.

## Memory and timing

Measured on the 1.44 GB planet file (macOS, file in page cache):

| Step | Time | Resident memory added |
| --- | --- | --- |
| open | <from step 1> | <from step 1> |
| isWater | <from step 1> | <from step 1> |
| short route (25 nm) | <from step 1> | <from step 1> |
| transatlantic route (3,040 nm) | <from step 1> | <from step 1> |

Mapped file pages are clean and evictable. The A* buffers are zero-filled and
allocated on the first route, so resident memory follows the search, not the
graph. All calls block and are safe from any thread; routes serialise on one
buffer set.

## Building locally

`scripts/build-ios.sh <version>` needs Xcode; `scripts/build-android.sh <version>`
needs the Android NDK, `cargo-ndk` and Gradle. `scripts/gen-bindings.sh` regenerates
the Swift or Kotlin sources from the host build.
```

Replace the `<from step 1>` cells with the measured values before committing.

- [ ] **Step 3: Root README and changelog**

In `README.md` under `## Packages` add after the Docker images subsection:

```markdown
### Mobile bindings

Each release also attaches `AswMobile-<version>.zip` (iOS xcframework and Swift
source) and `asw-mobile-<version>.aar` (Android, arm64-v8a): the same graph file,
opened on the phone through `open`, `version`, `isWater` and `route`. See
[crates/asw-mobile/README.md](crates/asw-mobile/README.md).
```

In `CHANGELOG.md` under `## [Unreleased]`:

```markdown
### Added

- `asw-mobile`: Swift and Kotlin bindings (UniFFI) over the v4 graph file with `open`, `version`, `isWater` and `route`, shipped as an iOS xcframework and an Android AAR on every release.
```

- [ ] **Step 4: Final checks and commit**

Run: `grep -rni "$APP_NAME" crates/asw-mobile README.md CHANGELOG.md .github/workflows | wc -l` with `APP_NAME` set from the project memory → must print `0`.
Run: `cargo test --workspace 2>&1 | grep -E "test result|FAILED"` and `cargo clippy --workspace --all-targets -- -D warnings`.

```bash
cargo fmt --all
git add crates/asw-mobile/README.md README.md CHANGELOG.md
git commit -m "docs(mobile): usage, measured numbers, changelog"
```

Push the branch and open the PR; the `mobile-swift` job and the Kotlin compile step in CI are the first real run of the packaging on both platforms.

---

## Self-review notes

- Spec coverage: API and object (Tasks 1-3), errors and panics (Tasks 1-3), memory and threading (Task 3 tests, Task 8 numbers), packaging (Tasks 5-6), CI and release (Tasks 4, 7), testing (Tasks 1-3 unit tests, Task 4 binding compiles), README (Task 8), no app names (global constraint, Task 8 grep).
- Type consistency: `AswError` variants with named fields throughout; `Water` enum; `Coordinate { lat, lon }`; `Route { coordinates, distance_nm, land_legs }`; `open(path: String) -> Result<Arc<Graph>, AswError>`; `gen-bindings.sh <profile> <language> <out-dir>`; scripts write to `target/mobile/`.
- Review Focus items are pinned by named tests in Tasks 1-3.
