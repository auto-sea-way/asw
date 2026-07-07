# Shore Buffer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-node distance-to-shore stored in the graph at build time; a `shore_buffer` (nautical miles) query parameter on `/route` applied as a graded A* cost penalty plus buffer-aware path smoothing (issue #26).

**Architecture:** Graph format bumps v2 → v3 with a `shore_dist: Vec<u8>` per-node field (0.02 nm quantization, saturating at 5.1 nm). Build pipeline computes exact straight-line distance to the coastline R-tree in parallel. At query time, A* multiplies edge weights into sub-buffer nodes by `1 + k·(1 − d/buffer)` (k=15), and smoothing rejects shortcuts that come closer to the coast than `min(buffer, raw-path minimum clearance)`.

**Tech Stack:** Rust workspace (asw-core, asw-build, asw-serve, asw-cli), rstar R-tree, h3o, rayon, axum, bitcode+zstd serialization.

**Spec:** `docs/superpowers/specs/2026-07-06-shore-buffer-design.md`

## Global Constraints

- All distances user-facing and internal are **nautical miles** (never km).
- `shore_dist` quantization: **0.02 nm per unit**, `255` = saturated (≥ 5.1 nm), stored value **rounds DOWN** (never overstate clearance).
- `ShorePenalty::from_nm` quantizes the buffer **UP** (never understate the requested clearance). Penalty strength `k = 15.0`.
- API validation: `0 ≤ shore_buffer ≤ 5.0`, finite; default `0` = exactly current behavior.
- Graph magic becomes `ASW\x03`; loader accepts **only** version 3 (error message follows the existing "Rebuild required" pattern).
- `cargo` is not on default PATH: run `export PATH="$HOME/.cargo/bin:$PATH"` once per shell.
- Run `cargo fmt --all` before every commit (CI checks formatting).
- Every task must leave the whole workspace compiling and `cargo test --workspace` green.

## File Structure

| File | Change |
|---|---|
| `crates/asw-core/src/graph.rs` | v3 field `shore_dist`, magic bump, quantization consts/fn, `add_node` 4th arg, `prune_to_main_component` |
| `crates/asw-core/src/geo_index.rs` | `min_distance_nm`, `segment_min_distance_nm`, `nm_frame`/`cos_lat_clamped` helpers |
| `crates/asw-core/src/routing.rs` | `ShorePenalty`, penalty in `astar`, buffer-aware `smooth`, `compute_route` param |
| `crates/asw-build/src/shore.rs` | **new** — parallel per-cell shore distance computation |
| `crates/asw-build/src/pipeline.rs` | wire shore distances into node creation; replace inline pruning with core method |
| `crates/asw-build/src/lib.rs` | `pub mod shore;` |
| `crates/asw-serve/src/api.rs` | `shore_buffer` query param, validation, response echo |
| `crates/asw-serve/src/state.rs` | test call-site updates only |
| `crates/asw-cli/src/main.rs`, `crates/asw-cli/src/bench.rs` | `--shore-buffer` flag threading |
| `README.md`, `CHANGELOG.md`, `CLAUDE.md` | docs |

---

### Task 1: Graph format v3 — `shore_dist` field, quantization, magic bump

**Files:**
- Modify: `crates/asw-core/src/graph.rs`
- Modify (mechanical call-site updates): `crates/asw-core/src/routing.rs`, `crates/asw-serve/src/state.rs`, `crates/asw-build/src/pipeline.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub const SHORE_DIST_UNIT_NM: f64 = 0.02` and `pub const SHORE_DIST_MAX_NM: f64` in `asw_core::graph`
  - `pub fn quantize_shore_dist(nm: f64) -> u8` in `asw_core::graph`
  - `RoutingGraph.shore_dist: Vec<u8>` (len == num_nodes)
  - `GraphBuilder::add_node(&mut self, h3_index: u64, lat: f64, lng: f64, shore_dist_q: u8) -> u32`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `crates/asw-core/src/graph.rs`:

```rust
#[test]
fn quantize_rounds_down_and_saturates() {
    assert_eq!(quantize_shore_dist(0.0), 0);
    assert_eq!(quantize_shore_dist(0.019), 0); // rounds down, not nearest
    assert_eq!(quantize_shore_dist(0.02), 1);
    assert_eq!(quantize_shore_dist(0.199), 9); // 9.95 -> 9
    assert_eq!(quantize_shore_dist(5.1), 255);
    assert_eq!(quantize_shore_dist(99.0), 255); // saturates
    assert_eq!(quantize_shore_dist(-1.0), 0); // clamps
}

#[test]
fn load_rejects_v2_files() {
    let bytes = b"ASW\x02whatever".to_vec();
    let err = RoutingGraph::load(&bytes[..]).unwrap_err();
    assert!(
        err.to_string().contains("Unsupported ASW graph version 2"),
        "got: {err}"
    );
}

#[test]
fn shore_dist_survives_save_load_roundtrip() {
    // Reuse the same construction pattern as the existing roundtrip test,
    // but with distinct shore_dist values per node.
    let c0 = h3o::LatLng::new(0.0, 0.0)
        .unwrap()
        .to_cell(h3o::Resolution::Five);
    let c1 = h3o::LatLng::new(5.0, 5.0)
        .unwrap()
        .to_cell(h3o::Resolution::Five);
    let mut cells = vec![(u64::from(c0), 0.0, 0.0, 7u8), (u64::from(c1), 5.0, 5.0, 200u8)];
    cells.sort_by_key(|(h3, _, _, _)| *h3);

    let mut b = GraphBuilder::new();
    let mut ids = Vec::new();
    for &(h3, lat, lng, q) in &cells {
        ids.push(b.add_node(h3, lat, lng, q));
    }
    b.add_edge(ids[0], ids[1], 1.0);
    let g = b.build();

    let mut buf = Vec::new();
    g.save(&mut buf).unwrap();
    let loaded = RoutingGraph::load(&buf[..]).unwrap();
    assert_eq!(loaded.shore_dist, g.shore_dist);
    assert_eq!(loaded.shore_dist.len(), loaded.num_nodes as usize);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p asw-core quantize_rounds -- --nocapture 2>&1 | tail -5`
Expected: compile error — `quantize_shore_dist` not found (this is the failure; the signature change hasn't happened yet either).

- [ ] **Step 3: Implement the format change**

In `crates/asw-core/src/graph.rs`:

3a. Constants and quantization above `RoutingGraph`:

```rust
/// Quantization unit for `shore_dist`: 0.02 nm (~37 m) per step.
pub const SHORE_DIST_UNIT_NM: f64 = 0.02;
/// Saturation ceiling: 255 units = 5.1 nm. Distances beyond this store 255.
pub const SHORE_DIST_MAX_NM: f64 = SHORE_DIST_UNIT_NM * 255.0;

/// Quantize a shore distance (nm) to `shore_dist` units, rounding DOWN so
/// the stored clearance never overstates the real one.
pub fn quantize_shore_dist(nm: f64) -> u8 {
    (nm / SHORE_DIST_UNIT_NM).floor().clamp(0.0, 255.0) as u8
}
```

3b. Struct field — add after `edge_data`, with doc comment:

```rust
    /// Quantized straight-line distance from node center to nearest coastline.
    /// Unit: SHORE_DIST_UNIT_NM (0.02 nm). 255 = saturated (>= 5.1 nm).
    /// Rounded down at build time. Length = num_nodes.
    pub shore_dist: Vec<u8>,
```

3c. Magic and version check:

```rust
    const MAGIC: &'static [u8; 4] = b"ASW\x03";
```

and in `load()` change the version arm to:

```rust
        if magic[3] != 3 {
            anyhow::bail!(
                "Unsupported ASW graph version {} (expected 3). Rebuild required.",
                magic[3]
            );
        }
```

3d. Load-time validation — add alongside the `node_h3.len()` check:

```rust
        anyhow::ensure!(
            graph.shore_dist.len() == n,
            "shore_dist length {} != num_nodes {}",
            graph.shore_dist.len(),
            n
        );
```

3e. `GraphBuilder`: change `nodes: Vec<(u64, f64, f64)>` to `Vec<(u64, f64, f64, u8)>`, and:

```rust
    /// Add a node with its quantized shore distance. Returns node ID.
    pub fn add_node(&mut self, h3_index: u64, lat: f64, lng: f64, shore_dist_q: u8) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push((h3_index, lat, lng, shore_dist_q));
        id
    }
```

In `build()`:

```rust
        let node_h3: Vec<u64> = self.nodes.iter().map(|(h3, _, _, _)| *h3).collect();
        let shore_dist: Vec<u8> = self.nodes.iter().map(|(_, _, _, q)| *q).collect();
```

and add `shore_dist,` to the `RoutingGraph { ... }` literal.

- [ ] **Step 4: Update all call sites (mechanical)**

Append `, 255` (saturated = "far from shore", penalty-neutral) to every existing 3-arg `add_node` call:

- `crates/asw-core/src/graph.rs` — test helpers (~5 places: the roundtrip helper, component tests, node_pos tests). Also the two tests that build `RoutingGraph { ... }` struct literals directly (single-node graphs around lines 496/517): add `shore_dist: vec![255],` to each literal.
- `crates/asw-core/src/routing.rs` — `diamond_graph()` and `astar_unreachable` (2 places).
- `crates/asw-serve/src/state.rs` — test helpers (5 places).
- `crates/asw-build/src/pipeline.rs` line ~78: `builder.add_node(u64::from(*cell), lat, lng, 255)` — **temporary**; Task 3 replaces `255` with computed values.
- `crates/asw-build/src/pipeline.rs` line ~129 (pruning rebuild): use the real value immediately:

```rust
                    let new_id =
                        new_builder.add_node(h3, lat, lon, graph.shore_dist[old_id as usize]);
```

Verify no site was missed: `grep -rn "add_node(" crates --include="*.rs" | grep -v "fn add_node"` — every hit must have 4 arguments.

- [ ] **Step 5: Run the full test suite**

Run: `cargo test --workspace 2>&1 | tail -15`
Expected: all tests PASS, including the three new ones.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add -A crates
git commit -m "feat(core): graph format v3 with per-node shore_dist (issue #26)"
```

---

### Task 2: `CoastlineIndex` distance queries in nautical miles

**Files:**
- Modify: `crates/asw-core/src/geo_index.rs`

**Interfaces:**
- Consumes: existing `CoastlineIndex`, private `point_to_segment_dist(p, a, b)` (planar, unit-agnostic).
- Produces:
  - `pub fn min_distance_nm(&self, lon: f64, lat: f64, max_nm: f64) -> f64` — point to nearest coastline, capped; returns `max_nm` when nothing is within range.
  - `pub fn segment_min_distance_nm(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64, max_nm: f64) -> f64` — same for a segment; `0.0` if it crosses a coastline segment.

- [ ] **Step 1: Write the failing tests**

Add a `#[cfg(test)] mod distance_tests` at the bottom of `geo_index.rs`:

```rust
#[cfg(test)]
mod distance_tests {
    use super::*;

    /// Vertical coastline at lon=28.0 from lat 36.0 to 37.0.
    fn coast() -> CoastlineIndex {
        let line = LineString::from(vec![(28.0, 36.0), (28.0, 37.0)]);
        CoastlineIndex::new(vec![CoastlineSegment::new(line)])
    }

    #[test]
    fn point_distance_mid_latitude() {
        // 0.1 deg of longitude at lat 36.5 = 0.1 * 60 * cos(36.5 deg) nm
        let expected = 0.1 * 60.0 * (36.5f64).to_radians().cos();
        let d = coast().min_distance_nm(28.1, 36.5, 5.1);
        assert!((d - expected).abs() < 0.05, "got {d}, expected {expected}");
    }

    #[test]
    fn point_distance_high_latitude_cos_correction() {
        let line = LineString::from(vec![(10.0, 59.5), (10.0, 60.5)]);
        let idx = CoastlineIndex::new(vec![CoastlineSegment::new(line)]);
        // 0.1 deg lon at lat 60 = 0.1 * 60 * 0.5 = 3.0 nm
        let d = idx.min_distance_nm(10.1, 60.0, 5.1);
        assert!((d - 3.0).abs() < 0.05, "got {d}, expected 3.0");
    }

    #[test]
    fn point_beyond_cap_returns_max() {
        // ~48 nm away, cap 5.1 -> envelope finds nothing
        let d = coast().min_distance_nm(29.0, 36.5, 5.1);
        assert_eq!(d, 5.1);
    }

    #[test]
    fn empty_index_returns_max() {
        let idx = CoastlineIndex::new(vec![]);
        assert_eq!(idx.min_distance_nm(28.0, 36.5, 5.1), 5.1);
    }

    #[test]
    fn segment_distance_parallel() {
        // Query segment at lon 28.05, parallel to the coast
        let expected = 0.05 * 60.0 * (36.5f64).to_radians().cos();
        let d = coast().segment_min_distance_nm(28.05, 36.4, 28.05, 36.6, 5.1);
        assert!((d - expected).abs() < 0.05, "got {d}, expected {expected}");
    }

    #[test]
    fn segment_crossing_coast_returns_zero() {
        let d = coast().segment_min_distance_nm(27.9, 36.5, 28.1, 36.5, 5.1);
        assert_eq!(d, 0.0);
    }

    #[test]
    fn point_distance_across_antimeridian() {
        // Coastline just west of the seam; query point just east of it.
        let line = LineString::from(vec![(179.98, -0.5), (179.98, 0.5)]);
        let idx = CoastlineIndex::new(vec![CoastlineSegment::new(line)]);
        // 0.03 deg of longitude at the equator = 1.8 nm, across the seam
        let d = idx.min_distance_nm(-179.99, 0.0, 5.1);
        assert!((d - 1.8).abs() < 0.05, "got {d}, expected 1.8");
    }

    #[test]
    fn segment_distance_across_antimeridian() {
        // Coastline north of a seam-crossing query segment.
        let line = LineString::from(vec![(179.98, 0.05), (179.98, 0.2)]);
        let idx = CoastlineIndex::new(vec![CoastlineSegment::new(line)]);
        // Horizontal query at the equator crossing the seam: closest approach
        // is the coastline's south endpoint, 0.05 deg lat = 3.0 nm.
        let d = idx.segment_min_distance_nm(179.9, 0.0, -179.9, 0.0, 5.1);
        assert!((d - 3.0).abs() < 0.05, "got {d}, expected 3.0");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p asw-core distance_tests 2>&1 | tail -5`
Expected: compile error — `min_distance_nm` not found.

- [ ] **Step 3: Implement**

Add to `geo_index.rs` (helpers as free functions next to `point_to_segment_dist`):

```rust
/// cos(lat) clamped away from zero for degree->nm longitude scaling near poles.
fn cos_lat_clamped(lat: f64) -> f64 {
    lat.to_radians().cos().max(0.01)
}

/// Project a lon/lat coordinate into a local equirectangular nm frame
/// centered on (ref_lon, ref_lat). Exact enough at <= ~5 nm scale.
fn nm_frame(c: Coord<f64>, ref_lon: f64, ref_lat: f64, coslat: f64) -> Coord<f64> {
    Coord {
        x: (c.x - ref_lon) * 60.0 * coslat,
        y: (c.y - ref_lat) * 60.0,
    }
}
```

Add to `impl CoastlineIndex`. **Antimeridian handling is required** — follow the patterns commit `747c757` established in this file: stored segments always live within [-180, 180], `split_at_antimeridian(lon1, lat1, lon2, lat2)` (existing free function) splits a seam-crossing query segment, and shifted-by-±360 retries mirror `transmeridian_variants`. Iterate candidate segments via `seg.line.lines()` (allocation-free), not by collecting coords.

```rust
    /// Minimum distance in nautical miles from (lon, lat) to any coastline
    /// segment, capped at `max_nm`. Returns `max_nm` when no segment lies
    /// within the search envelope.
    ///
    /// Antimeridian-aware: when the buffer-expanded envelope overflows
    /// lon +/-180, the query also runs shifted by -/+360 (stored segments
    /// always live within [-180, 180]) and the minimum is taken.
    pub fn min_distance_nm(&self, lon: f64, lat: f64, max_nm: f64) -> f64 {
        let coslat = cos_lat_clamped(lat);
        let dlon = max_nm / (60.0 * coslat);
        let mut best = self.min_distance_nm_planar(lon, lat, max_nm, coslat);
        if lon + dlon > 180.0 {
            best = best.min(self.min_distance_nm_planar(lon - 360.0, lat, max_nm, coslat));
        } else if lon - dlon < -180.0 {
            best = best.min(self.min_distance_nm_planar(lon + 360.0, lat, max_nm, coslat));
        }
        best
    }

    fn min_distance_nm_planar(&self, lon: f64, lat: f64, max_nm: f64, coslat: f64) -> f64 {
        let dlat = max_nm / 60.0;
        let dlon = max_nm / (60.0 * coslat);
        let envelope =
            AABB::from_corners([lon - dlon, lat - dlat], [lon + dlon, lat + dlat]);

        let origin = Coord { x: 0.0, y: 0.0 };
        let mut best = max_nm;
        for seg in self.tree.locate_in_envelope_intersecting(&envelope) {
            for line in seg.line.lines() {
                let a = nm_frame(line.start, lon, lat, coslat);
                let b = nm_frame(line.end, lon, lat, coslat);
                let d = point_to_segment_dist(origin, a, b);
                if d < best {
                    best = d;
                }
            }
        }
        best
    }

    /// Minimum distance in nm from the segment (lon1,lat1)-(lon2,lat2) to any
    /// coastline segment, capped at `max_nm`. Returns 0.0 on intersection.
    /// For non-intersecting segments the minimum is attained at an endpoint,
    /// so the four endpoint-to-segment distances are exact.
    ///
    /// Seam-crossing query segments are split at lon +/-180 first (same
    /// predicate as `crosses_land`); each half also retries shifted by
    /// -/+360 when its buffer-expanded envelope overflows the seam.
    pub fn segment_min_distance_nm(
        &self,
        lon1: f64,
        lat1: f64,
        lon2: f64,
        lat2: f64,
        max_nm: f64,
    ) -> f64 {
        if (lon1 - lon2).abs() > 180.0 {
            let (a, b) = split_at_antimeridian(lon1, lat1, lon2, lat2);
            return self
                .segment_min_distance_nm_wrapped(a.0, a.1, a.2, a.3, max_nm)
                .min(self.segment_min_distance_nm_wrapped(b.0, b.1, b.2, b.3, max_nm));
        }
        self.segment_min_distance_nm_wrapped(lon1, lat1, lon2, lat2, max_nm)
    }

    fn segment_min_distance_nm_wrapped(
        &self,
        lon1: f64,
        lat1: f64,
        lon2: f64,
        lat2: f64,
        max_nm: f64,
    ) -> f64 {
        let coslat = cos_lat_clamped((lat1 + lat2) / 2.0);
        let dlon = max_nm / (60.0 * coslat);
        let mut best = self.segment_min_distance_nm_planar(lon1, lat1, lon2, lat2, max_nm);
        if lon1.max(lon2) + dlon > 180.0 {
            best = best.min(self.segment_min_distance_nm_planar(
                lon1 - 360.0,
                lat1,
                lon2 - 360.0,
                lat2,
                max_nm,
            ));
        } else if lon1.min(lon2) - dlon < -180.0 {
            best = best.min(self.segment_min_distance_nm_planar(
                lon1 + 360.0,
                lat1,
                lon2 + 360.0,
                lat2,
                max_nm,
            ));
        }
        best
    }

    fn segment_min_distance_nm_planar(
        &self,
        lon1: f64,
        lat1: f64,
        lon2: f64,
        lat2: f64,
        max_nm: f64,
    ) -> f64 {
        let ref_lat = (lat1 + lat2) / 2.0;
        let coslat = cos_lat_clamped(ref_lat);
        let dlat = max_nm / 60.0;
        let dlon = max_nm / (60.0 * coslat);
        let envelope = AABB::from_corners(
            [lon1.min(lon2) - dlon, lat1.min(lat2) - dlat],
            [lon1.max(lon2) + dlon, lat1.max(lat2) + dlat],
        );

        let qa = nm_frame(Coord { x: lon1, y: lat1 }, lon1, ref_lat, coslat);
        let qb = nm_frame(Coord { x: lon2, y: lat2 }, lon1, ref_lat, coslat);
        let query = Line::new(qa, qb);

        let mut best = max_nm;
        for seg in self.tree.locate_in_envelope_intersecting(&envelope) {
            for line in seg.line.lines() {
                let ca = nm_frame(line.start, lon1, ref_lat, coslat);
                let cb = nm_frame(line.end, lon1, ref_lat, coslat);
                if query.intersects(&Line::new(ca, cb)) {
                    return 0.0;
                }
                let d = point_to_segment_dist(qa, ca, cb)
                    .min(point_to_segment_dist(qb, ca, cb))
                    .min(point_to_segment_dist(ca, qa, qb))
                    .min(point_to_segment_dist(cb, qa, qb));
                if d < best {
                    best = d;
                }
            }
        }
        best
    }
```

(`Coord`, `Line`, `AABB`, `Intersects` are already imported at the top of the file — verify, add if missing.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p asw-core distance_tests 2>&1 | tail -10`
Expected: 8 tests PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/asw-core/src/geo_index.rs
git commit -m "feat(core): nm-unit distance queries on CoastlineIndex"
```

---

### Task 3: Build pipeline — compute real shore distances; prune as core method

**Files:**
- Create: `crates/asw-build/src/shore.rs`
- Modify: `crates/asw-build/src/lib.rs`, `crates/asw-build/src/pipeline.rs`, `crates/asw-core/src/graph.rs`

**Interfaces:**
- Consumes: `CoastlineIndex::min_distance_nm` (Task 2), `quantize_shore_dist` / `SHORE_DIST_MAX_NM` (Task 1), `asw_core::h3::cell_center(cell) -> (lat, lng)`.
- Produces:
  - `pub fn compute_shore_distances(cells: &[(CellIndex, u32)], coastline: &CoastlineIndex) -> Vec<u8>` in `asw_build::shore` (output parallel to input order).
  - `pub fn prune_to_main_component(self) -> RoutingGraph` on `RoutingGraph` (moved from pipeline.rs, now preserving `shore_dist`).

- [ ] **Step 1: Write the failing test for `prune_to_main_component`**

In `crates/asw-core/src/graph.rs` tests:

```rust
#[test]
fn prune_keeps_main_component_and_shore_dist() {
    // 3-node chain (main) + 1 isolated node, distinct shore_dist values.
    let coords = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (10.0, 10.0)];
    let mut entries: Vec<(u64, f64, f64, u8)> = coords
        .iter()
        .enumerate()
        .map(|(i, &(lat, lng))| {
            let cell = h3o::LatLng::new(lat, lng)
                .unwrap()
                .to_cell(h3o::Resolution::Five);
            (u64::from(cell), lat, lng, (i as u8 + 1) * 10) // 10,20,30,40
        })
        .collect();
    entries.sort_by_key(|(h3, _, _, _)| *h3);

    let mut b = GraphBuilder::new();
    let mut ids = Vec::new();
    for &(h3, lat, lng, q) in &entries {
        ids.push(b.add_node(h3, lat, lng, q));
    }
    // Chain the first three entries (by sorted order); leave the last isolated.
    b.add_edge(ids[0], ids[1], 1.0);
    b.add_edge(ids[1], ids[2], 1.0);
    let g = b.build();

    let pruned = g.prune_to_main_component();
    assert_eq!(pruned.num_nodes, 3);
    // Every surviving node keeps the shore_dist of the entry with its H3 index.
    for (i, &h3) in pruned.node_h3.iter().enumerate() {
        let orig = entries.iter().find(|e| e.0 == h3).unwrap();
        assert_eq!(pruned.shore_dist[i], orig.3, "node {i} shore_dist mismatch");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p asw-core prune_keeps 2>&1 | tail -5`
Expected: compile error — no method `prune_to_main_component`.

- [ ] **Step 3: Move pruning into asw-core**

Add to `impl RoutingGraph` in `graph.rs` — this is the block from `pipeline.rs:98-158` reshaped into a method (asw-core already depends on `tracing` via geo_index's `info!`; verify, otherwise use plain `tracing::info!` path):

```rust
    /// Keep only the largest connected component, remapping node IDs.
    /// Returns self unchanged when the graph is already one component.
    pub fn prune_to_main_component(self) -> RoutingGraph {
        let labels = self.component_labels();
        let mut comp_sizes: std::collections::HashMap<u32, usize> =
            std::collections::HashMap::new();
        for &root in &labels {
            *comp_sizes.entry(root).or_insert(0) += 1;
        }
        let main_root = comp_sizes
            .iter()
            .max_by_key(|(_, count)| **count)
            .map(|(&root, _)| root)
            .unwrap_or(0);
        let main_count = comp_sizes.get(&main_root).copied().unwrap_or(0);
        let pruned_count = self.num_nodes as usize - main_count;

        if pruned_count == 0 {
            return self;
        }
        tracing::info!(
            "Pruning {} nodes in {} small components (keeping {} in main component)",
            pruned_count,
            comp_sizes.len() - 1,
            main_count,
        );

        let mut old_to_new: Vec<Option<u32>> = vec![None; self.num_nodes as usize];
        let mut new_builder = GraphBuilder::new();
        for old_id in 0..self.num_nodes {
            if labels[old_id as usize] == main_root {
                let h3 = self.node_h3[old_id as usize];
                let (lat, lon) = self.node_pos(old_id);
                let new_id =
                    new_builder.add_node(h3, lat, lon, self.shore_dist[old_id as usize]);
                old_to_new[old_id as usize] = Some(new_id);
            }
        }
        for old_src in 0..self.num_nodes {
            if labels[old_src as usize] != main_root {
                continue;
            }
            let new_src = old_to_new[old_src as usize].unwrap();
            for (old_dst, weight) in self.neighbors(old_src) {
                if let Some(new_dst) = old_to_new[old_dst as usize] {
                    new_builder.add_directed_edge(new_src, new_dst, weight);
                }
            }
        }
        new_builder.coastline_coords = self.coastline_coords;
        new_builder.build()
    }
```

In `pipeline.rs`, replace the whole `let graph = { ... }` pruning block (lines ~97-158) with:

```rust
    // Prune: keep only the largest connected component
    let graph = graph.prune_to_main_component();
    info!(
        "Final graph: {} nodes, {} edges",
        graph.num_nodes, graph.num_edges
    );
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p asw-core prune_keeps 2>&1 | tail -5` — PASS.
Run: `cargo test --workspace 2>&1 | tail -5` — all PASS.

- [ ] **Step 5: Write the failing test for `compute_shore_distances`**

Create `crates/asw-build/src/shore.rs` with the test first:

```rust
//! Per-node distance-to-shore computation (build time).

use asw_core::geo_index::CoastlineIndex;
use asw_core::graph::{quantize_shore_dist, SHORE_DIST_MAX_NM};
use asw_core::h3::cell_center;
use h3o::CellIndex;
use rayon::prelude::*;

/// Compute quantized straight-line distance to the nearest coastline for each
/// cell. Output order matches input order.
pub fn compute_shore_distances(
    cells: &[(CellIndex, u32)],
    coastline: &CoastlineIndex,
) -> Vec<u8> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use asw_core::geo_index::CoastlineSegment;
    use geo::LineString;

    #[test]
    fn near_and_far_cells() {
        // Vertical coastline at lon 28.0
        let line = LineString::from(vec![(28.0, 36.0), (28.0, 37.0)]);
        let coastline = CoastlineIndex::new(vec![CoastlineSegment::new(line)]);

        let near = asw_core::h3::lat_lng_to_cell(36.5, 28.05, h3o::Resolution::Nine).unwrap();
        let far = asw_core::h3::lat_lng_to_cell(36.5, 29.5, h3o::Resolution::Nine).unwrap();

        let result = compute_shore_distances(&[(near, 0), (far, 1)], &coastline);

        // Expected value computed from the actual cell center (cell centers
        // are offset from the query coords by up to ~100 m).
        let (lat, lon) = cell_center(near);
        let expected = quantize_shore_dist((lon - 28.0) * 60.0 * lat.to_radians().cos());
        assert_eq!(result[0], expected);
        assert_eq!(result[1], 255, "cell ~72 nm from shore must saturate");
    }
}
```

Register the module in `crates/asw-build/src/lib.rs`: add `pub mod shore;` alongside the existing `pub mod` lines.

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test -p asw-build near_and_far 2>&1 | tail -5`
Expected: FAIL — `todo!()` panic (or compile error if `CoastlineSegment`/`LineString` aren't exported — `CoastlineSegment` is `pub` in geo_index; `geo` is already an asw-build dependency, verify in its Cargo.toml).

- [ ] **Step 7: Implement**

Replace `todo!()`:

```rust
    cells
        .par_iter()
        .map(|(cell, _)| {
            let (lat, lon) = cell_center(*cell);
            quantize_shore_dist(coastline.min_distance_nm(lon, lat, SHORE_DIST_MAX_NM))
        })
        .collect()
```

- [ ] **Step 8: Run to verify it passes**

Run: `cargo test -p asw-build near_and_far 2>&1 | tail -5` — PASS.

- [ ] **Step 9: Wire into the pipeline**

In `pipeline.rs`, after `sorted_cells` is built and before the node loop:

```rust
    // Compute per-node distance to shore (straight-line, capped at 5.1 nm)
    let shore_dist = crate::shore::compute_shore_distances(&sorted_cells, &coastline_index);
    info!("Computed shore distances for {} cells", shore_dist.len());
```

and change the node loop (removing the Task-1 placeholder `255`):

```rust
    for (i, (cell, old_id)) in sorted_cells.iter().enumerate() {
        let (lat, lng) = cell_center(*cell);
        let new_id = builder.add_node(u64::from(*cell), lat, lng, shore_dist[i]);
        id_remap[*old_id as usize] = new_id;
    }
```

- [ ] **Step 10: Full test suite + commit**

Run: `cargo test --workspace 2>&1 | tail -5` — all PASS.

```bash
cargo fmt --all
git add -A crates
git commit -m "feat(build): compute per-node shore distances; move pruning into asw-core"
```

---

### Task 4: `ShorePenalty` in A*

**Files:**
- Modify: `crates/asw-core/src/routing.rs`

**Interfaces:**
- Consumes: `RoutingGraph.shore_dist`, `SHORE_DIST_UNIT_NM`.
- Produces:
  - `pub struct ShorePenalty { pub buffer_q: u8, pub k: f32 }` (`Copy`)
  - `ShorePenalty::from_nm(buffer_nm: f64) -> Option<ShorePenalty>` (None for ≤ 0; quantizes UP; `k = 15.0`)
  - `ShorePenalty::factor(&self, d: u8) -> f32`
  - `astar(graph, start, goal, buffers, shore: Option<ShorePenalty>)` — **signature change**

- [ ] **Step 1: Write the failing tests**

In `routing.rs` tests:

```rust
#[test]
fn shore_penalty_from_nm_and_factor() {
    assert!(ShorePenalty::from_nm(0.0).is_none());
    assert!(ShorePenalty::from_nm(-1.0).is_none());
    let p = ShorePenalty::from_nm(0.1).unwrap(); // 0.1 / 0.02 = 5
    assert_eq!(p.buffer_q, 5);
    let p2 = ShorePenalty::from_nm(0.001).unwrap(); // rounds UP to 1
    assert_eq!(p2.buffer_q, 1);
    let p3 = ShorePenalty::from_nm(99.0).unwrap(); // clamps to 255
    assert_eq!(p3.buffer_q, 255);

    assert_eq!(p.factor(5), 1.0); // at the buffer: no penalty
    assert_eq!(p.factor(255), 1.0); // far offshore: no penalty
    assert!((p.factor(0) - 16.0).abs() < 1e-6); // 1 + 15*1
    assert!((p.factor(2) - (1.0 + 15.0 * 0.6)).abs() < 1e-4);
}

/// Two corridors S->A->G (short, A hugs the shore) and S->B->G (long, B
/// offshore). Without a buffer the short corridor wins; with one, the long.
fn corridor_graph() -> (RoutingGraph, u32, u32, u32, u32) {
    let coords = [
        (0.0, 0.0, "S", 255u8),
        (1.0, 0.0, "A", 0u8), // on the shore
        (0.0, 1.0, "B", 255u8),
        (1.0, 1.0, "G", 255u8),
    ];
    let mut cells: Vec<(u64, f64, f64, u8, &str)> = coords
        .iter()
        .map(|&(lat, lng, label, q)| {
            let cell = h3o::LatLng::new(lat, lng)
                .unwrap()
                .to_cell(h3o::Resolution::Five);
            (u64::from(cell), lat, lng, q, label)
        })
        .collect();
    cells.sort_by_key(|(h3, _, _, _, _)| *h3);

    let mut b = GraphBuilder::new();
    let mut ids = std::collections::HashMap::new();
    for (h3, lat, lng, q, label) in &cells {
        let id = b.add_node(*h3, *lat, *lng, *q);
        ids.insert(*label, id);
    }
    b.add_edge(ids["S"], ids["A"], 5.0);
    b.add_edge(ids["A"], ids["G"], 5.0); // near-shore total: 10
    b.add_edge(ids["S"], ids["B"], 8.0);
    b.add_edge(ids["B"], ids["G"], 8.0); // offshore total: 16
    (b.build(), ids["S"], ids["A"], ids["B"], ids["G"])
}

#[test]
fn penalty_diverts_route_offshore() {
    let (g, s, a, b_node, goal) = corridor_graph();
    let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);

    // Without penalty: short near-shore corridor via A.
    let (path, cost) = astar(&g, s, goal, &mut buffers, None).unwrap();
    assert_eq!(path, vec![s, a, goal]);
    assert!((cost - 10.0).abs() < 1e-4);

    // With a 0.1 nm buffer: edge S->A costs 5 * 16 = 80 -> offshore wins.
    buffers.reset();
    let shore = ShorePenalty::from_nm(0.1);
    let (path, cost) = astar(&g, s, goal, &mut buffers, shore).unwrap();
    assert_eq!(path, vec![s, b_node, goal]);
    assert!((cost - 16.0).abs() < 1e-4);
}

#[test]
fn penalty_with_all_nodes_offshore_is_identity() {
    let (g, node_a, node_d) = diamond_graph(); // all nodes shore_dist=255
    let mut b1 = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
    let mut b2 = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
    let plain = astar(&g, node_a, node_d, &mut b1, None).unwrap();
    let with = astar(&g, node_a, node_d, &mut b2, ShorePenalty::from_nm(0.2)).unwrap();
    assert_eq!(plain.0, with.0);
    assert!((plain.1 - with.1).abs() < 1e-6);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p asw-core shore_penalty 2>&1 | tail -5`
Expected: compile error — `ShorePenalty` not found.

- [ ] **Step 3: Implement**

In `routing.rs` above `astar`:

```rust
/// Query-time shore clearance penalty. Edges into nodes closer to shore than
/// `buffer_q` get their weight multiplied by `1 + k * (1 - d/buffer_q)`.
#[derive(Debug, Clone, Copy)]
pub struct ShorePenalty {
    /// Requested clearance in shore_dist units (SHORE_DIST_UNIT_NM each).
    pub buffer_q: u8,
    /// Penalty strength at distance 0.
    pub k: f32,
}

impl ShorePenalty {
    pub const DEFAULT_K: f32 = 15.0;

    /// Build from a buffer in nautical miles. Returns None for buffer <= 0
    /// (and for NaN). Quantizes UP so the requested clearance is never
    /// understated.
    pub fn from_nm(buffer_nm: f64) -> Option<Self> {
        if !(buffer_nm > 0.0) {
            return None;
        }
        let q = (buffer_nm / crate::graph::SHORE_DIST_UNIT_NM)
            .ceil()
            .clamp(1.0, 255.0) as u8;
        Some(Self {
            buffer_q: q,
            k: Self::DEFAULT_K,
        })
    }

    /// Weight multiplier for an edge into a node `d` shore_dist units from shore.
    #[inline]
    pub fn factor(&self, d: u8) -> f32 {
        if d >= self.buffer_q {
            1.0
        } else {
            1.0 + self.k * (1.0 - d as f32 / self.buffer_q as f32)
        }
    }
}
```

Change `astar` signature and relaxation:

```rust
pub fn astar(
    graph: &RoutingGraph,
    start: u32,
    goal: u32,
    buffers: &mut crate::astar_pool::AstarBuffers,
    shore: Option<ShorePenalty>,
) -> Option<(Vec<u32>, f64)> {
```

and inside the neighbor loop, before `let tentative_g`:

```rust
            let weight = match shore {
                Some(sp) => weight * sp.factor(graph.shore_dist[neighbor as usize]),
                None => weight,
            };
```

Update existing call sites: the four `astar(...)` calls in `routing.rs` tests get `, None`; `compute_route` gets `, None` **temporarily** (Task 5 threads the real value).

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p asw-core 2>&1 | tail -5` — all PASS (including the untouched A* regression tests, now with `None`).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/asw-core/src/routing.rs
git commit -m "feat(core): graded shore penalty in A* (issue #26)"
```

---

### Task 5: Buffer-aware smoothing + `compute_route` threading

**Files:**
- Modify: `crates/asw-core/src/routing.rs`
- Modify (call sites): `crates/asw-serve/src/api.rs`, `crates/asw-cli/src/bench.rs`

**Interfaces:**
- Consumes: `segment_min_distance_nm` (Task 2), `shore_dist`, `SHORE_DIST_UNIT_NM`, `ShorePenalty` (Task 4).
- Produces:
  - `smooth(graph, path, coastline, shore_buffer_nm: f64) -> Vec<u32>` — **signature change**
  - `compute_route(..., shore_buffer_nm: f64)` — **signature change**, last parameter

- [ ] **Step 1: Write the failing tests**

In `routing.rs` tests:

```rust
/// Coastline at lon 28.0 (lat 36.45..36.55) and a 3-node dogleg around it.
/// Direct P0->P2 passes ~2.4 nm off the coast; the dogleg via P1 ~7 nm.
fn dogleg() -> (RoutingGraph, CoastlineIndex, Vec<u32>) {
    dogleg_with_shore(&[255, 255, 255])
}

fn dogleg_with_shore(shore_q: &[u8; 3]) -> (RoutingGraph, CoastlineIndex, Vec<u32>) {
    use geo::LineString;
    let coastline = CoastlineIndex::new(vec![crate::geo_index::CoastlineSegment::new(
        LineString::from(vec![(28.0, 36.45), (28.0, 36.55)]),
    )]);

    let coords = [(36.3, 28.05), (36.5, 28.15), (36.7, 28.05)];
    let mut cells: Vec<(u64, f64, f64, u8, usize)> = coords
        .iter()
        .enumerate()
        .map(|(i, &(lat, lng))| {
            let cell = h3o::LatLng::new(lat, lng)
                .unwrap()
                .to_cell(h3o::Resolution::Nine);
            (u64::from(cell), lat, lng, shore_q[i], i)
        })
        .collect();
    cells.sort_by_key(|(h3, _, _, _, _)| *h3);

    let mut b = GraphBuilder::new();
    let mut id_by_orig = [0u32; 3];
    for (h3, lat, lng, q, orig) in &cells {
        id_by_orig[*orig] = b.add_node(*h3, *lat, *lng, *q);
    }
    b.add_edge(id_by_orig[0], id_by_orig[1], 1.0);
    b.add_edge(id_by_orig[1], id_by_orig[2], 1.0);
    let path = id_by_orig.to_vec();
    (b.build(), coastline, path)
}

#[test]
fn smooth_without_buffer_cuts_the_corner() {
    let (g, coast, path) = dogleg();
    let smoothed = smooth(&g, &path, &coast, 0.0);
    assert_eq!(smoothed, vec![path[0], path[2]]);
}

#[test]
fn smooth_respects_buffer() {
    let (g, coast, path) = dogleg();
    // Direct line is ~2.41 nm off the coast: allowed at 2.0, blocked at 3.0.
    let loose = smooth(&g, &path, &coast, 2.0);
    assert_eq!(loose, vec![path[0], path[2]]);
    let strict = smooth(&g, &path, &coast, 3.0);
    assert_eq!(strict, path, "3 nm buffer must keep the dogleg waypoint");
}

#[test]
fn smooth_relaxes_near_endpoints() {
    // Path nodes themselves are close to shore (q=20 = 0.4 nm): the
    // threshold becomes min(3.0, 0.4) = 0.4 nm, so the direct line
    // (~2.4 nm off) is allowed even under a 3 nm buffer.
    let (g, coast, path) = dogleg_with_shore(&[20, 255, 20]);
    let smoothed = smooth(&g, &path, &coast, 3.0);
    assert_eq!(smoothed, vec![path[0], path[2]]);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p asw-core smooth_ 2>&1 | tail -5`
Expected: compile error — `smooth` takes 3 arguments.

- [ ] **Step 3: Implement buffer-aware `smooth`**

Restructure `smooth` — the algorithm is unchanged; the visibility check becomes a closure used at all three check sites (direct-to-end, exponential search, binary search):

```rust
/// Greedy line-of-sight smoothing.
///
/// Removes unnecessary waypoints by checking that direct lines between
/// waypoints (a) don't cross any coastline and (b) when `shore_buffer_nm > 0`,
/// don't come closer to the coastline than min(buffer, the raw path's own
/// minimum clearance over the skipped span). Rule (b) means smoothing never
/// brings the route closer to shore than penalized A* already accepted —
/// full buffer in open water, graceful degradation near endpoints/in coves.
pub fn smooth(
    graph: &RoutingGraph,
    path: &[u32],
    coastline: &CoastlineIndex,
    shore_buffer_nm: f64,
) -> Vec<u32> {
    if path.len() <= 2 {
        return path.to_vec();
    }
    let use_buffer = shore_buffer_nm > 0.0;

    let mut result = vec![path[0]];
    let mut current_idx = 0;
    let end_idx = path.len() - 1;

    while current_idx < end_idx {
        let (c_lat, c_lon) = graph.node_pos(path[current_idx]);

        // Running min of shore_dist from the anchor: range_min[j - current_idx]
        // = min shore_dist over path[current_idx..=j]. O(n) per anchor.
        let range_min: Vec<u8> = if use_buffer {
            let mut v = Vec::with_capacity(end_idx - current_idx + 1);
            let mut m = u8::MAX;
            for &node in &path[current_idx..=end_idx] {
                m = m.min(graph.shore_dist[node as usize]);
                v.push(m);
            }
            v
        } else {
            Vec::new()
        };

        let clear = |j: usize| -> bool {
            let (t_lat, t_lon) = graph.node_pos(path[j]);
            if coastline.crosses_land(c_lon, c_lat, t_lon, t_lat) {
                return false;
            }
            if use_buffer {
                let raw_min_nm = range_min[j - current_idx] as f64
                    * crate::graph::SHORE_DIST_UNIT_NM;
                let threshold = shore_buffer_nm.min(raw_min_nm);
                if threshold > 0.0
                    && coastline
                        .segment_min_distance_nm(c_lon, c_lat, t_lon, t_lat, threshold)
                        < threshold
                {
                    return false;
                }
            }
            true
        };

        // Try direct line to destination
        if clear(end_idx) {
            result.push(path[end_idx]);
            break;
        }

        // Exponential forward search: find boundary between clear and blocked
        let mut step = 1usize;
        let mut v_lo = current_idx + 1;
        let mut v_hi;
        loop {
            let test_idx = (current_idx + step).min(end_idx);
            if !clear(test_idx) {
                v_hi = test_idx;
                break;
            }
            v_lo = test_idx;
            if test_idx >= end_idx {
                v_lo = end_idx;
                v_hi = end_idx;
                break;
            }
            step *= 2;
        }

        if v_lo == end_idx {
            result.push(path[end_idx]);
            break;
        }

        // Binary search between v_lo (clear) and v_hi (blocked)
        while v_hi - v_lo > 1 {
            let mid = (v_lo + v_hi) / 2;
            if !clear(mid) {
                v_hi = mid;
            } else {
                v_lo = mid;
            }
        }

        if v_lo <= current_idx {
            v_lo = current_idx + 1;
        }
        result.push(path[v_lo]);
        current_idx = v_lo;
    }

    result
}
```

(Note: the original per-check `node_pos` calls for the end/test/mid points move inside `clear`. `CoastlineSegment` must be importable from tests — it is `pub`.)

- [ ] **Step 4: Thread through `compute_route`**

```rust
pub fn compute_route(
    graph: &RoutingGraph,
    from_lat: f64,
    from_lon: f64,
    to_lat: f64,
    to_lon: f64,
    coastline: &CoastlineIndex,
    node_knn: &dyn Fn(f64, f64) -> Option<(u32, f64)>,
    buffers: &mut crate::astar_pool::AstarBuffers,
    shore_buffer_nm: f64,
) -> Option<RouteResult> {
    let (start, _) = node_knn(from_lat, from_lon)?;
    let (goal, _) = node_knn(to_lat, to_lon)?;

    let shore = ShorePenalty::from_nm(shore_buffer_nm);
    let (raw_path, _distance_nm) = astar(graph, start, goal, buffers, shore)?;
    let raw_hops = raw_path.len();

    let smoothed = smooth(graph, &raw_path, coastline, shore_buffer_nm);
    // ... rest unchanged
```

Update call sites with `0.0` as the last argument for now (real wiring in Tasks 6–7): `crates/asw-serve/src/api.rs` (1 call), `crates/asw-cli/src/bench.rs` (4 calls).

- [ ] **Step 5: Run the full suite**

Run: `cargo test --workspace 2>&1 | tail -5` — all PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add -A crates
git commit -m "feat(core): buffer-aware smoothing, shore_buffer in compute_route"
```

---

### Task 6: API — `shore_buffer` query parameter

**Files:**
- Modify: `crates/asw-serve/src/api.rs`

**Interfaces:**
- Consumes: `compute_route(..., shore_buffer_nm)` (Task 5).
- Produces: `/route?from=..&to=..&shore_buffer=0.2` (nm, optional, default 0); 400 outside `[0, 5.0]` or non-finite; `RouteResponse.shore_buffer_nm: f64` echo.

- [ ] **Step 1: Write the failing tests**

In the `api.rs` tests module (uses the existing `test_state` helper and header pattern from the auth tests around lines 259–320):

```rust
    async fn ready_state() -> Arc<ServerState> {
        use asw_core::graph::GraphBuilder;
        let state = test_state();
        let coords = [(36.0, 28.0), (36.5, 28.5)];
        let mut entries: Vec<(u64, f64, f64)> = coords
            .iter()
            .map(|&(lat, lng)| {
                let cell = h3o::LatLng::new(lat, lng)
                    .unwrap()
                    .to_cell(h3o::Resolution::Five);
                (u64::from(cell), lat, lng)
            })
            .collect();
        entries.sort_by_key(|(h3, _, _)| *h3);
        let mut b = GraphBuilder::new();
        let mut ids = Vec::new();
        for &(h3, lat, lng) in &entries {
            ids.push(b.add_node(h3, lat, lng, 255));
        }
        b.add_edge(ids[0], ids[1], 30.0);
        let app = crate::state::AppState::new(b.build());
        *state.inner.write().await = Some(app);
        state
    }

    async fn get_route(query: &str) -> (HyperStatus, serde_json::Value) {
        let app = create_router(ready_state().await);
        let req = Request::get(format!("/route?{query}"))
            .header("X-Api-Key", "secret-key-1234567890")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn shore_buffer_out_of_range_is_400() {
        let (status, body) = get_route("from=36.0,28.0&to=36.5,28.5&shore_buffer=5.1").await;
        assert_eq!(status, HyperStatus::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("shore_buffer"));

        let (status, _) = get_route("from=36.0,28.0&to=36.5,28.5&shore_buffer=-0.1").await;
        assert_eq!(status, HyperStatus::BAD_REQUEST);
    }

    #[tokio::test]
    async fn shore_buffer_echoed_in_response() {
        let (status, body) = get_route("from=36.0,28.0&to=36.5,28.5&shore_buffer=0.2").await;
        assert_eq!(status, HyperStatus::OK, "body: {body}");
        assert!((body["shore_buffer_nm"].as_f64().unwrap() - 0.2).abs() < 1e-9);
    }

    #[tokio::test]
    async fn shore_buffer_defaults_to_zero() {
        let (status, body) = get_route("from=36.0,28.0&to=36.5,28.5").await;
        assert_eq!(status, HyperStatus::OK, "body: {body}");
        assert_eq!(body["shore_buffer_nm"].as_f64().unwrap(), 0.0);
    }
```

(`serde_json` is already a dependency of asw-serve — it's used for `geometry`. If `h3o` is dev-only missing in asw-serve, check `Cargo.toml`; state.rs tests already use `h3o`, so it's available.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p asw-serve shore_buffer 2>&1 | tail -5`
Expected: compile error — no field `shore_buffer` / `shore_buffer_nm`.

- [ ] **Step 3: Implement**

In `api.rs`:

```rust
#[derive(Deserialize)]
pub struct RouteQuery {
    /// "lat,lon"
    pub from: String,
    /// "lat,lon"
    pub to: String,
    /// Minimum distance from shore in nautical miles (0..=5.0, default 0).
    pub shore_buffer: Option<f64>,
}
```

`RouteResponse` gains:

```rust
    pub shore_buffer_nm: f64,
```

In `route_handler`, after parsing `to` and before calling `compute_route`:

```rust
    let shore_buffer_nm = params.shore_buffer.unwrap_or(0.0);
    if !shore_buffer_nm.is_finite() || !(0.0..=5.0).contains(&shore_buffer_nm) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid 'shore_buffer' parameter. Expected nautical miles in 0..5.0"
                    .into(),
            }),
        ));
    }
```

Pass `shore_buffer_nm` as the last argument of `compute_route` (replacing the Task-5 `0.0`), and add `shore_buffer_nm,` to the `RouteResponse` literal.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p asw-serve 2>&1 | tail -5` — all PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/asw-serve
git commit -m "feat(serve): shore_buffer query parameter on /route (issue #26)"
```

---

### Task 7: CLI — `--shore-buffer` on `asw bench`

**Files:**
- Modify: `crates/asw-cli/src/main.rs`, `crates/asw-cli/src/bench.rs`

**Interfaces:**
- Consumes: `compute_route(..., shore_buffer_nm)`.
- Produces: `asw bench --shore-buffer 0.2 ...`; `bench::run(graph, iterations, json, output, compare, shore_buffer_nm: f64)`.

- [ ] **Step 1: Add the flag**

In the `Commands::Bench` variant in `main.rs`:

```rust
        /// Shore clearance in nautical miles (0 = off); applies the routing penalty
        #[arg(long, default_value_t = 0.0)]
        shore_buffer: f64,
```

Destructure it in the match arm and pass to `bench::run(&graph, iterations, json, output.as_deref(), compare.as_deref(), shore_buffer)`.

- [ ] **Step 2: Thread through bench.rs**

- `pub fn run(...)` gains `shore_buffer_nm: f64` as last parameter.
- `resolve_routes(app: &AppState)` → `resolve_routes(app: &AppState, shore_buffer_nm: f64)`; `run_benchmark(...)` likewise.
- All four `compute_route(...)` calls: replace the Task-5 placeholder `0.0` with `shore_buffer_nm`.

- [ ] **Step 3: Verify it builds and behaves**

Run: `cargo build --release -p asw-cli 2>&1 | tail -3`
Expected: clean build.
Run: `./target/release/asw bench --help | grep shore`
Expected: the flag is listed with its help text.

Note: existing local graphs (`export/*.graph`) are v2 and will be **rejected** by the new loader — a real bench run needs a graph rebuilt in Task 9. Full planet baseline comparison happens only after the cloud rebuild.

- [ ] **Step 4: Run full suite and commit**

Run: `cargo test --workspace 2>&1 | tail -5` — all PASS.

```bash
cargo fmt --all
git add crates/asw-cli
git commit -m "feat(cli): --shore-buffer flag on asw bench"
```

---

### Task 8: Documentation

**Files:**
- Modify: `README.md`, `CHANGELOG.md`, `CLAUDE.md`

- [ ] **Step 1: CHANGELOG**

Under `## [Unreleased]` add:

```markdown
### Added

- `shore_buffer` query parameter on `/route` (nautical miles, 0–5.0): keeps routes a configurable clearance from the coastline via a graded A* cost penalty and buffer-aware path smoothing (#26)
- Per-node distance-to-shore stored in the graph (1 byte/node, 0.02 nm quantization, saturating at 5.1 nm)
- `--shore-buffer` flag on `asw bench`

### Changed

- **BREAKING:** graph format v2 → v3 (adds `shore_dist`) — existing graph files must be rebuilt
```

- [ ] **Step 2: README**

- Find the `/route` API documentation (grep for `X-Api-Key` / `from=`): add the `shore_buffer` parameter — optional, nautical miles, `0`–`5.0`, default `0`; describe semantics in one sentence: "soft clearance — the router strongly prefers water at least this far from the coastline, but can still enter harbors/coves when there is no alternative; not a hard guarantee."
- Find the known-limitations wording about depth data (grep for `depth`): note that `shore_buffer` partially mitigates it by keeping routes off headlands and uncharted near-shore hazards, and that it is not a substitute for charts.
- Grep for `v2`/`format` mentions of the graph format; update to v3 and note the rebuild requirement.
- Per project practice, audit README stats/tags for staleness while in there.

- [ ] **Step 3: CLAUDE.md**

Update the line `Graph format v2: bitcode + zstd-19 serialization...` to say v3 and mention `shore_dist` (1 byte/node quantized distance-to-shore).

- [ ] **Step 4: Commit**

```bash
git add README.md CHANGELOG.md CLAUDE.md
git commit -m "docs: shore_buffer parameter, graph format v3"
```

---

### Task 9: End-to-end verification (local marmaris build)

No new code — this task verifies the whole chain on a real region and produces evidence.

- [ ] **Step 1: Lint gate**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets 2>&1 | tail -5
cargo test --workspace 2>&1 | tail -5
```
Expected: no formatting diffs, no new clippy warnings, all tests pass.

- [ ] **Step 2: Build a marmaris graph with the new code**

```bash
cargo build --release -p asw-cli
./target/release/asw build --bbox marmaris --output export/marmaris-v3.graph
```
(If a local `land-polygons-split-4326` directory or zip exists from previous builds, pass it via `--shp`; otherwise the build downloads it. Requires osmium-tool for canal extraction — `brew install osmium-tool` if missing.)
Expected: log line `Computed shore distances for N cells`, graph saved.

- [ ] **Step 3: Route with and without buffer**

```bash
ASW_API_KEY=test-key ./target/release/asw serve --graph export/marmaris-v3.graph --port 3111 &
until curl -sf localhost:3111/ready; do sleep 1; done

# Cape-rounding route: Marmaris bay -> Bozburun, rounds the Turunç headlands
curl -s -H "X-Api-Key: test-key" "localhost:3111/route?from=36.84,28.27&to=36.71,27.99" > /tmp/route-0.json
curl -s -H "X-Api-Key: test-key" "localhost:3111/route?from=36.84,28.27&to=36.71,27.99&shore_buffer=0.2" > /tmp/route-02.json
```

Verify:
- Both return 200 with a LineString.
- `shore_buffer_nm` echoes 0 and 0.2 respectively.
- The buffered route's `distance_nm` is **≥** the unbuffered one (a wider cape rounding is longer).
- `shore_buffer=6` returns 400.

- [ ] **Step 4: Visual check**

```bash
./target/release/asw geojson --graph export/marmaris-v3.graph --bbox marmaris --coastline --output export/marmaris-v3.geojson
```
Overlay both route LineStrings on the coastline (export/viz.html or geojson.io): the buffered route must visibly stand off every headland; the unbuffered one may hug them.

- [ ] **Step 5: Local bench sanity**

```bash
./target/release/asw bench --graph export/marmaris-v3.graph --iterations 20 --json --output export/bench-shore-0.json
./target/release/asw bench --graph export/marmaris-v3.graph --iterations 20 --shore-buffer 0.2 --json --output export/bench-shore-02.json
```
Record both. `shore_buffer=0` timings must match pre-change levels for comparable routes; the 0.2 run documents the penalty's search-expansion cost. (Planet-baseline comparison happens after the cloud rebuild — out of this plan's scope.)

- [ ] **Step 6: Commit any fixes, then wrap up**

```bash
kill %1  # stop the serve process
git status  # must be clean apart from export/ (gitignored)
```

Follow-ups outside this plan (operational): planet rebuild on Hetzner (`asw cloud build`), republish graph artifacts + `-full` Docker image, update CHANGELOG to a versioned section on release, reply on issue #26.
