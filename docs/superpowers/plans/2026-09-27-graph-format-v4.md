# Graph Format v4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the bitcode+zstd v3 graph file with a flat, memory-mappable v4 file that the server opens in seconds and a phone can open in milliseconds, with the coastline index and node snapping inside asw-core.

**Architecture:** `RoutingGraph` becomes a view over a byte buffer (mmap or owned) with a 232-byte header, nine 8-byte-aligned little-endian sections, and accessor methods. Edge weights are no longer stored; `neighbors()` computes centre-to-centre haversine. The coastline lives in the file as i32 microdegree runs with a 0.1° grid index, and `CoastlineIndex` is a borrowed view over those sections. `nearest_node` moves from asw-serve into asw-core, and `is_water` is added using crossing parity.

**Tech Stack:** Rust 2021 workspace, `memmap2` (new), `h3o`, `geo` (kept for `Line::intersects`), `rstar` and `rayon` (asw-build only after this plan).

**Spec:** `docs/superpowers/specs/2026-09-27-graph-format-v4-design.md`

## Global Constraints

- Branch `feat/graph-v4`, based on `chore/ponytail-audit-cuts` (PR #50). Rebase onto `main` once #50 merges.
- Magic `ASW\x04`. All integers little-endian. Every section starts at an 8-byte aligned offset. Compile-time assert `target_endian = "little"`.
- Node ids are plain sorted `u64`. No stored edge weights. No compression inside the file. No checksum inside the file.
- Coastline points are `i32` microdegrees, no delta coding. Grid is 3600 × 1800 cells of 0.1°.
- No `asw convert`. Loading a v3 file fails with `Unsupported ASW graph version 3. Rebuild required.`
- asw-core ends with dependencies: `h3o`, `geo`, `memmap2`, `tracing`, `anyhow`, `serde` only if still needed by something else (it is not; remove it). `rstar` and `rayon` move to asw-build.
- Distances are nautical miles everywhere. Never kilometres.
- Run `cargo fmt --all` before every commit. `cargo clippy --workspace --all-targets -- -D warnings` must pass at the end of every task.
- Commit messages: plain conventional commits, no attribution trailers of any kind.
- This Mac mini has 8 GB RAM. Never load the planet graph here with the v3 binary. The planet rebuild runs on Hetzner.

## Review Focus

1. A route whose A* touches millions of nodes on the phone: `AstarBuffers` must be zero-filled so untouched pages stay non-resident. Pinned by `buffers_new_is_zero_filled` in Task 6.
2. A fix berthed in a marina basin narrower than a res-10 cell: `is_water` must return water although the line to the nearest node crosses the mole ring twice. Pinned by `is_water_marina_behind_mole_is_water` in Task 5.
3. A v3 file, a truncated v4 file, or a file whose section table points past the end: `open` must fail with a clear error, never panic or read out of bounds. Pinned by `from_bytes_rejects_*` tests in Task 4.
4. A coastline query envelope that overflows lon ±180 after the wrap retry shifts it by 360: the grid lookup must clamp to the grid, not index out of bounds. Pinned by `grid_clamps_envelope_beyond_the_seam` in Task 2.
5. A route through the Panama res-13 corridor: dropping the 0.01 nm weight clamp must not make canal edges free. Haversine of two adjacent res-13 cells is ~0.0033 nm, strictly positive. Pinned by `res13_edge_weight_is_true_distance` in Task 3.

---

## File Structure

| File | Responsibility after this plan |
| --- | --- |
| `crates/asw-core/src/graph.rs` | v4 header and section layout, `Bytes`, `RoutingGraph` reader and accessors, `GraphBuilder` writer and pruning |
| `crates/asw-core/src/coast.rs` (new) | `CoastlineSections` (build side), `CoastlineIndex<'a>` grid-backed queries incl. `crossing_count` |
| `crates/asw-core/src/snap.rs` (new) | `RoutingGraph::nearest_node` and helpers (moved from asw-serve) |
| `crates/asw-core/src/routing.rs` | unchanged algorithms, `is_water` added, `CoastlineIndex<'_>` signatures |
| `crates/asw-core/src/astar_pool.rs` | zero-filled buffers, lazy pool |
| `crates/asw-core/src/geo_index.rs` | deleted (LandIndex moves to asw-build, CoastlineIndex to coast.rs) |
| `crates/asw-build/src/land_index.rs` (new) | `LandIndex`, `LandPolygon`, transmeridian helpers (moved from core) |
| `crates/asw-build/src/coastline.rs` | returns `Vec<Vec<(f64, f64)>>` runs |
| `crates/asw-build/src/pipeline.rs` | builds `CoastlineSections` first, uses grid index, writes v4 bytes with a version string |
| `crates/asw-build/src/edges.rs` | edges without cost |
| `crates/asw-serve/src/state.rs` | `AppState { graph, astar_pool }` only |
| `crates/asw-serve/src/api.rs` | `graph_version` in `/info`, `app.graph.coastline()` |
| `crates/asw-cli/src/main.rs`, `bench.rs` | `RoutingGraph::open`, accessors |

---

### Task 1: Move `LandIndex` into asw-build

**Files:**
- Create: `crates/asw-build/src/land_index.rs`
- Modify: `crates/asw-core/src/geo_index.rs` (remove `LandPolygon`, `LandIndex`, `has_transmeridian_coords`, `transmeridian_variants`, `shift_polygon`, the `land_index_intersects_polygon_handles_unwrapped_transmeridian_ring` test, the `rayon` and `BooleanOps`/`MultiPolygon`/`Contains`/`Point` imports)
- Modify: `crates/asw-build/src/lib.rs`, `shapefile.rs:2`, `edges.rs:2`, `cells.rs:2`
- Modify: `crates/asw-core/Cargo.toml` (remove `rayon`)

**Interfaces:**
- Produces: `asw_build::land_index::{LandIndex, LandPolygon}` with the exact same API as today (`new`, `is_water`, `intersects_polygon`, `contains_polygon`, `polygon_count`, `polygons`, `subtract_water`).

- [ ] **Step 1: Create the new module by moving code**

Create `crates/asw-build/src/land_index.rs` with the following content taken verbatim from `geo_index.rs`: the `LandPolygon` struct and impls (lines 9-29), `LandIndex` (lines 54-209), `has_transmeridian_coords`, `transmeridian_variants`, `shift_polygon`, and a private copy of `corners`. Header of the new file:

```rust
//! Land polygon R-tree used only at build time.

use geo::algorithm::bool_ops::BooleanOps;
use geo::{BoundingRect, Contains, Coord, Intersects, LineString, MultiPolygon, Point, Polygon};
use rayon::prelude::*;
use rstar::{Envelope, RTree, RTreeObject, AABB};
use tracing::info;

/// Rect→AABB-corners adapter over geo's `BoundingRect`.
fn corners<G: BoundingRect<f64, Output = Option<geo::Rect<f64>>>>(g: &G) -> ([f64; 2], [f64; 2]) {
    let r = g.bounding_rect().expect("non-empty geometry");
    ([r.min().x, r.min().y], [r.max().x, r.max().y])
}
```

Move the test `land_index_intersects_polygon_handles_unwrapped_transmeridian_ring` into a `#[cfg(test)] mod tests` at the bottom of the new file.

- [ ] **Step 2: Register the module and fix imports**

`crates/asw-build/src/lib.rs`: add `pub mod land_index;`.

Replace imports:
- `shapefile.rs:2` → `use crate::land_index::{LandIndex, LandPolygon};`
- `edges.rs:2` → `use crate::land_index::LandIndex;`
- `cells.rs:2` → `use asw_core::geo_index::CoastlineIndex;` and `use crate::land_index::LandIndex;`

- [ ] **Step 3: Strip core**

In `crates/asw-core/src/geo_index.rs` delete everything moved in Step 1. Keep `CoastlineSegment`, `CoastlineIndex`, `split_at_antimeridian`, `corners`, `cos_lat_clamped`, `nm_lon_radius`, `with_wrap_retry`, `nm_frame`, `point_to_segment_dist`, and both remaining test modules. Reduce the imports to:

```rust
use geo::{BoundingRect, Coord, Intersects, Line, LineString};
use rstar::{RTree, RTreeObject, AABB};
```

Remove `rayon.workspace = true` from `crates/asw-core/Cargo.toml`.

- [ ] **Step 4: Build and test the workspace**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace 2>&1 | tail -20`
Expected: all tests pass, including `land_index::tests::land_index_intersects_polygon_handles_unwrapped_transmeridian_ring`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates
git commit -m "refactor(build): move LandIndex out of asw-core"
```

---

### Task 2: Grid-backed coastline index in asw-core

**Files:**
- Create: `crates/asw-core/src/coast.rs`
- Delete: `crates/asw-core/src/geo_index.rs`
- Modify: `crates/asw-core/src/lib.rs`, `routing.rs`, `graph.rs` (coastline field type), `crates/asw-core/Cargo.toml` (remove `rstar`)
- Modify: `crates/asw-build/src/coastline.rs`, `pipeline.rs`, `cells.rs`, `shore.rs`
- Modify: `crates/asw-serve/src/state.rs`, `api.rs`
- Modify: `docs/superpowers/specs/2026-09-27-graph-format-v4-design.md` (add the `coast_bbox` section, fix the rayon sentence)

**Interfaces:**
- Produces:
  - `asw_core::coast::CoastlineSections { runs: Vec<u32>, bbox: Vec<i32>, points: Vec<i32>, grid_offsets: Vec<u32>, grid_ids: Vec<u32> }`, `CoastlineSections::from_runs(&[Vec<(f64, f64)>]) -> Self`, `CoastlineSections::index(&self) -> CoastlineIndex<'_>`
  - `asw_core::coast::CoastlineIndex<'a>` (Copy) with `from_slices(runs, bbox, points, grid_offsets, grid_ids)`, `run_count()`, `run_points(run) -> impl Iterator<Item = (f64, f64)>`, `crosses_land`, `crossing_count`, `min_distance_deg`, `min_distance_nm`, `segment_min_distance_nm`. Same semantics as the R-tree versions.
  - Constants `GRID_STEP_DEG`, `GRID_COLS`, `GRID_ROWS`, `GRID_CELLS`.
- `GraphBuilder.coastline_runs: Vec<Vec<(f64, f64)>>` replaces `coastline_coords`. Transitional until Task 4: `RoutingGraph.coastline_runs: Vec<Vec<(f64, f64)>>` is still serialized with bitcode.

- [ ] **Step 1: Write the failing tests for the sections builder and the grid**

Create `crates/asw-core/src/coast.rs` with only the tests first (the module compiles once Step 3 lands; run them then):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> CoastlineSections {
        CoastlineSections::from_runs(&[vec![(10.0, -1.0), (10.0, 1.0)]])
    }

    #[test]
    fn from_runs_encodes_points_bbox_and_grid() {
        let s = CoastlineSections::from_runs(&[
            vec![(10.0, -1.0), (10.0, 1.0)],
            vec![(0.5, 0.5)], // dropped: fewer than 2 points
            vec![(-0.05, 0.0), (0.05, 0.0)],
        ]);
        assert_eq!(s.runs, vec![0, 2, 4]);
        assert_eq!(s.points, vec![10_000_000, -1_000_000, 10_000_000, 1_000_000, -50_000, 0, 50_000, 0]);
        assert_eq!(&s.bbox[0..4], &[10_000_000, -1_000_000, 10_000_000, 1_000_000]);
        assert_eq!(s.grid_offsets.len(), GRID_CELLS + 1);
        assert_eq!(*s.grid_offsets.last().unwrap() as usize, s.grid_ids.len());
        // The wall spans 21 lat rows (lat -1.0..=1.0 at 0.1°) in one lon column.
        assert_eq!(s.grid_ids.iter().filter(|&&id| id == 0).count(), 21);
        // The short run straddles lon 0 (cols 1799 and 1800), one row.
        assert_eq!(s.grid_ids.iter().filter(|&&id| id == 1).count(), 2);
    }

    #[test]
    fn empty_sections_answer_every_query_as_open_water() {
        let s = CoastlineSections::from_runs(&[]);
        let idx = s.index();
        assert_eq!(idx.run_count(), 0);
        assert!(!idx.crosses_land(0.0, 0.0, 1.0, 1.0));
        assert_eq!(idx.crossing_count(0.0, 0.0, 1.0, 1.0), 0);
        assert_eq!(idx.min_distance_nm(28.0, 36.5, 5.1), 5.1);
        assert_eq!(idx.min_distance_deg(0.0, 0.0, 1.0), f64::MAX);
    }

    #[test]
    fn crosses_land_normal_case() {
        let s = wall();
        let idx = s.index();
        assert!(idx.crosses_land(5.0, 0.0, 15.0, 0.0));
        assert!(!idx.crosses_land(5.0, 0.0, 5.0, 1.0));
    }

    #[test]
    fn crosses_land_antimeridian_no_false_positive_from_far_land() {
        let s = CoastlineSections::from_runs(&[vec![(0.0, -1.0), (0.0, 1.0)]]);
        assert!(!s.index().crosses_land(179.9, 0.0, -179.9, 0.0));
    }

    #[test]
    fn crosses_land_antimeridian_detects_real_crossing_near_seam() {
        let s = CoastlineSections::from_runs(&[vec![(179.95, -1.0), (179.95, 1.0)]]);
        assert!(s.index().crosses_land(179.9, 0.0, -179.9, 0.0));
    }

    #[test]
    fn grid_clamps_envelope_beyond_the_seam() {
        // A query shifted by +360 (wrap retry) must not index outside the grid.
        let s = CoastlineSections::from_runs(&[vec![(179.98, -0.5), (179.98, 0.5)]]);
        let d = s.index().min_distance_deg(-179.99, 0.0, 0.05);
        assert!((d - 0.03).abs() < 1e-9, "expected ~0.03 deg across the seam, got {d}");
        let d2 = s.index().min_distance_deg(-179.99 + 360.0, 0.0, 0.05);
        assert!(d2.is_finite());
    }

    #[test]
    fn min_distance_deg_matches_point_to_segment_distance() {
        let s = CoastlineSections::from_runs(&[vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)]]);
        let idx = s.index();
        assert!(idx.min_distance_deg(0.0, 0.5, 5.0) < 1e-9);
        assert!((idx.min_distance_deg(0.5, 0.5, 5.0) - 0.5).abs() < 1e-9);
        assert_eq!(idx.min_distance_deg(50.0, 50.0, 0.5), f64::MAX);
    }

    #[test]
    fn min_distance_nm_mid_latitude_and_cap() {
        let s = CoastlineSections::from_runs(&[vec![(28.0, 36.0), (28.0, 37.0)]]);
        let idx = s.index();
        let expected = 0.1 * 60.0 * (36.5f64).to_radians().cos();
        let d = idx.min_distance_nm(28.1, 36.5, 5.1);
        assert!((d - expected).abs() < 0.05, "got {d}, expected {expected}");
        assert_eq!(idx.min_distance_nm(29.0, 36.5, 5.1), 5.1);
    }

    #[test]
    fn segment_min_distance_nm_parallel_and_crossing() {
        let s = CoastlineSections::from_runs(&[vec![(28.0, 36.0), (28.0, 37.0)]]);
        let idx = s.index();
        let expected = 0.05 * 60.0 * (36.5f64).to_radians().cos();
        let d = idx.segment_min_distance_nm(28.05, 36.4, 28.05, 36.6, 5.1);
        assert!((d - expected).abs() < 0.05, "got {d}, expected {expected}");
        assert_eq!(idx.segment_min_distance_nm(27.9, 36.5, 28.1, 36.5, 5.1), 0.0);
    }

    #[test]
    fn distances_across_antimeridian() {
        let s = CoastlineSections::from_runs(&[vec![(179.98, -0.5), (179.98, 0.5)]]);
        let d = s.index().min_distance_nm(-179.99, 0.0, 5.1);
        assert!((d - 1.8).abs() < 0.05, "got {d}, expected 1.8");
        let s2 = CoastlineSections::from_runs(&[vec![(179.98, 0.05), (179.98, 0.2)]]);
        let d2 = s2.index().segment_min_distance_nm(179.9, 0.0, -179.9, 0.0, 5.1);
        assert!((d2 - 3.0).abs() < 0.05, "got {d2}, expected 3.0");
    }

    /// Diamond island around the origin; the query line y=0 passes exactly
    /// through its west vertex. The half-open rule must count that once.
    fn diamond() -> CoastlineSections {
        CoastlineSections::from_runs(&[vec![(-0.1, 0.0), (0.0, -0.1), (0.1, 0.0), (0.0, 0.1), (-0.1, 0.0)]])
    }

    #[test]
    fn crossing_count_parity_inside_and_through_island() {
        let s = diamond();
        let idx = s.index();
        assert_eq!(idx.crossing_count(-1.0, 0.0, 0.0, 0.0), 1, "into the island: odd");
        assert_eq!(idx.crossing_count(-1.0, 0.0, 1.0, 0.0), 2, "through the island: even");
        assert_eq!(idx.crossing_count(-1.0, 0.5, 1.0, 0.5), 0, "misses the island");
    }

    #[test]
    fn crossing_count_touching_vertex_is_even() {
        // y = 0.1 touches the north vertex without entering.
        let s = diamond();
        assert_eq!(s.index().crossing_count(-1.0, 0.1, 1.0, 0.1) % 2, 0);
    }

    #[test]
    fn crossing_count_across_antimeridian() {
        let s = CoastlineSections::from_runs(&[vec![(179.95, -1.0), (179.95, 1.0)]]);
        assert_eq!(s.index().crossing_count(179.9, 0.0, -179.9, 0.0), 1);
    }
}
```

- [ ] **Step 2: Add the module and confirm the tests fail to compile**

Add `pub mod coast;` to `crates/asw-core/src/lib.rs`.

Run: `cargo test -p asw-core coast 2>&1 | head -5`
Expected: compile error, `CoastlineSections` not found.

- [ ] **Step 3: Implement `coast.rs`**

Above the tests in `crates/asw-core/src/coast.rs`:

```rust
//! Coastline stored as i32 microdegree runs plus a 0.1° grid index over run
//! bounding boxes. `CoastlineSections` is the build-side owner of the five
//! arrays; `CoastlineIndex` is a borrowed view over them (from a
//! `CoastlineSections` in tests and the build, from the mapped file at
//! query time) and answers the router's geometry questions.

use geo::{Coord, Intersects, Line};

pub const GRID_STEP_DEG: f64 = 0.1;
pub const GRID_COLS: usize = 3600;
pub const GRID_ROWS: usize = 1800;
pub const GRID_CELLS: usize = GRID_COLS * GRID_ROWS;
const MICRO: f64 = 1e6;

fn micro(deg: f64) -> i32 {
    (deg * MICRO).round() as i32
}

fn grid_col(lon: f64) -> usize {
    ((lon + 180.0) / GRID_STEP_DEG)
        .floor()
        .clamp(0.0, (GRID_COLS - 1) as f64) as usize
}

fn grid_row(lat: f64) -> usize {
    ((lat + 90.0) / GRID_STEP_DEG)
        .floor()
        .clamp(0.0, (GRID_ROWS - 1) as f64) as usize
}

/// The coastline sections exactly as written to a v4 graph file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CoastlineSections {
    /// Point index where run `i` starts. Length = run count + 1.
    pub runs: Vec<u32>,
    /// Per run: min_lon, min_lat, max_lon, max_lat in microdegrees.
    pub bbox: Vec<i32>,
    /// Interleaved lon, lat in microdegrees.
    pub points: Vec<i32>,
    /// CSR offsets into `grid_ids`, one per grid cell plus a sentinel.
    pub grid_offsets: Vec<u32>,
    /// Run ids whose bbox touches the cell.
    pub grid_ids: Vec<u32>,
}

impl CoastlineSections {
    /// Runs are (lon, lat) polylines in degrees. Runs with fewer than two
    /// points are dropped.
    pub fn from_runs(runs: &[Vec<(f64, f64)>]) -> Self {
        let mut out = Self {
            runs: vec![0],
            ..Self::default()
        };
        let mut cell_run: Vec<(u32, u32)> = Vec::new();
        for run in runs.iter().filter(|r| r.len() >= 2) {
            let id = (out.runs.len() - 1) as u32;
            let (mut min_lon, mut min_lat) = (f64::MAX, f64::MAX);
            let (mut max_lon, mut max_lat) = (f64::MIN, f64::MIN);
            for &(lon, lat) in run {
                out.points.push(micro(lon));
                out.points.push(micro(lat));
                min_lon = min_lon.min(lon);
                max_lon = max_lon.max(lon);
                min_lat = min_lat.min(lat);
                max_lat = max_lat.max(lat);
            }
            out.runs.push((out.points.len() / 2) as u32);
            out.bbox
                .extend([micro(min_lon), micro(min_lat), micro(max_lon), micro(max_lat)]);
            for row in grid_row(min_lat)..=grid_row(max_lat) {
                for col in grid_col(min_lon)..=grid_col(max_lon) {
                    cell_run.push(((row * GRID_COLS + col) as u32, id));
                }
            }
        }
        cell_run.sort_unstable();
        out.grid_offsets = vec![0u32; GRID_CELLS + 1];
        for &(cell, _) in &cell_run {
            out.grid_offsets[cell as usize + 1] += 1;
        }
        for i in 0..GRID_CELLS {
            out.grid_offsets[i + 1] += out.grid_offsets[i];
        }
        out.grid_ids = cell_run.into_iter().map(|(_, id)| id).collect();
        out
    }

    pub fn index(&self) -> CoastlineIndex<'_> {
        CoastlineIndex::from_slices(
            &self.runs,
            &self.bbox,
            &self.points,
            &self.grid_offsets,
            &self.grid_ids,
        )
    }
}

/// Borrowed view over the coastline sections.
#[derive(Clone, Copy)]
pub struct CoastlineIndex<'a> {
    runs: &'a [u32],
    bbox: &'a [i32],
    points: &'a [i32],
    grid_offsets: &'a [u32],
    grid_ids: &'a [u32],
}

impl<'a> CoastlineIndex<'a> {
    pub fn from_slices(
        runs: &'a [u32],
        bbox: &'a [i32],
        points: &'a [i32],
        grid_offsets: &'a [u32],
        grid_ids: &'a [u32],
    ) -> Self {
        Self {
            runs,
            bbox,
            points,
            grid_offsets,
            grid_ids,
        }
    }

    pub fn run_count(&self) -> usize {
        self.runs.len().saturating_sub(1)
    }

    /// Points of one run as (lon, lat) degrees.
    pub fn run_points(&self, run: usize) -> impl Iterator<Item = (f64, f64)> + 'a {
        let (s, e) = (self.runs[run] as usize * 2, self.runs[run + 1] as usize * 2);
        self.points[s..e]
            .chunks_exact(2)
            .map(|p| (p[0] as f64 / MICRO, p[1] as f64 / MICRO))
    }

    fn lines(&self, run: usize) -> impl Iterator<Item = Line<f64>> + 'a {
        let (s, e) = (self.runs[run] as usize * 2, self.runs[run + 1] as usize * 2);
        self.points[s..e].windows(4).step_by(2).map(|w| {
            Line::new(
                Coord {
                    x: w[0] as f64 / MICRO,
                    y: w[1] as f64 / MICRO,
                },
                Coord {
                    x: w[2] as f64 / MICRO,
                    y: w[3] as f64 / MICRO,
                },
            )
        })
    }

    /// Sorted, deduplicated ids of runs whose bbox intersects the envelope
    /// (degrees; may overflow ±180 after a wrap retry, the grid clamps).
    fn candidates(&self, min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> Vec<usize> {
        let (mn_lon, mn_lat, mx_lon, mx_lat) =
            (micro(min_lon), micro(min_lat), micro(max_lon), micro(max_lat));
        let mut ids = Vec::new();
        for row in grid_row(min_lat)..=grid_row(max_lat) {
            for col in grid_col(min_lon)..=grid_col(max_lon) {
                let cell = row * GRID_COLS + col;
                let (s, e) = (self.grid_offsets[cell] as usize, self.grid_offsets[cell + 1] as usize);
                for &id in &self.grid_ids[s..e] {
                    let b = &self.bbox[id as usize * 4..id as usize * 4 + 4];
                    if b[0] > mx_lon || b[2] < mn_lon || b[1] > mx_lat || b[3] < mn_lat {
                        continue;
                    }
                    ids.push(id as usize);
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Does the segment cross any coastline? Antimeridian-aware: a query
    /// crossing lon ±180 is split at the seam first.
    pub fn crosses_land(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> bool {
        if (lon1 - lon2).abs() > 180.0 {
            let (a, b) = split_at_antimeridian(lon1, lat1, lon2, lat2);
            return self.crosses_land_planar(a.0, a.1, a.2, a.3)
                || self.crosses_land_planar(b.0, b.1, b.2, b.3);
        }
        self.crosses_land_planar(lon1, lat1, lon2, lat2)
    }

    fn crosses_land_planar(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> bool {
        let line = Line::new(Coord { x: lon1, y: lat1 }, Coord { x: lon2, y: lat2 });
        self.candidates(lon1.min(lon2), lat1.min(lat2), lon1.max(lon2), lat1.max(lat2))
            .into_iter()
            .any(|run| self.lines(run).any(|l| line.intersects(&l)))
    }

    /// Number of coastline edges the segment P->T crosses, half-open so a
    /// shared vertex on the line counts once and a touch counts zero or
    /// two. Parity from a known-water P tells whether T is on water.
    pub fn crossing_count(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> usize {
        if (lon1 - lon2).abs() > 180.0 {
            let (a, b) = split_at_antimeridian(lon1, lat1, lon2, lat2);
            return self.crossing_count_planar(a.0, a.1, a.2, a.3)
                + self.crossing_count_planar(b.0, b.1, b.2, b.3);
        }
        self.crossing_count_planar(lon1, lat1, lon2, lat2)
    }

    fn crossing_count_planar(&self, px: f64, py: f64, tx: f64, ty: f64) -> usize {
        let (dx, dy) = (tx - px, ty - py);
        // Strictly left of P->T; a point on the line counts as right.
        let left = |x: f64, y: f64| dx * (y - py) - dy * (x - px) > 0.0;
        let mut n = 0;
        for run in self.candidates(px.min(tx), py.min(ty), px.max(tx), py.max(ty)) {
            for l in self.lines(run) {
                let (a, b) = (l.start, l.end);
                if left(a.x, a.y) == left(b.x, b.y) {
                    continue;
                }
                let (ex, ey) = (b.x - a.x, b.y - a.y);
                let denom = dx * ey - dy * ex;
                if denom == 0.0 {
                    continue;
                }
                let t = ((a.x - px) * ey - (a.y - py) * ex) / denom;
                if (0.0..1.0).contains(&t) {
                    n += 1;
                }
            }
        }
        n
    }

    /// Minimum distance in degrees within `radius_deg`, `f64::MAX` if none.
    pub fn min_distance_deg(&self, lon: f64, lat: f64, radius_deg: f64) -> f64 {
        with_wrap_retry(lon, lon, radius_deg, |shift| {
            self.min_distance_deg_planar(lon + shift, lat, radius_deg)
        })
    }

    fn min_distance_deg_planar(&self, lon: f64, lat: f64, radius_deg: f64) -> f64 {
        let pt = Coord { x: lon, y: lat };
        let mut best = f64::MAX;
        for run in self.candidates(lon - radius_deg, lat - radius_deg, lon + radius_deg, lat + radius_deg) {
            for l in self.lines(run) {
                best = best.min(point_to_segment_dist(pt, l.start, l.end));
            }
        }
        best
    }

    /// Minimum distance in nm, capped at `max_nm`.
    pub fn min_distance_nm(&self, lon: f64, lat: f64, max_nm: f64) -> f64 {
        let coslat = cos_lat_clamped(lat);
        let dlon = nm_lon_radius(max_nm, coslat);
        with_wrap_retry(lon, lon, dlon, |shift| {
            self.min_distance_nm_planar(lon + shift, lat, max_nm, coslat)
        })
    }

    fn min_distance_nm_planar(&self, lon: f64, lat: f64, max_nm: f64, coslat: f64) -> f64 {
        let dlat = max_nm / 60.0;
        let dlon = max_nm / (60.0 * coslat);
        let origin = Coord { x: 0.0, y: 0.0 };
        let mut best = max_nm;
        for run in self.candidates(lon - dlon, lat - dlat, lon + dlon, lat + dlat) {
            for l in self.lines(run) {
                let a = nm_frame(l.start, lon, lat, coslat);
                let b = nm_frame(l.end, lon, lat, coslat);
                best = best.min(point_to_segment_dist(origin, a, b));
            }
        }
        best
    }

    /// Minimum distance in nm from a query segment to the coastline, capped
    /// at `max_nm`; 0.0 on intersection.
    pub fn segment_min_distance_nm(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64, max_nm: f64) -> f64 {
        if (lon1 - lon2).abs() > 180.0 {
            let (a, b) = split_at_antimeridian(lon1, lat1, lon2, lat2);
            return self
                .segment_min_distance_nm_wrapped(a.0, a.1, a.2, a.3, max_nm)
                .min(self.segment_min_distance_nm_wrapped(b.0, b.1, b.2, b.3, max_nm));
        }
        self.segment_min_distance_nm_wrapped(lon1, lat1, lon2, lat2, max_nm)
    }

    fn segment_min_distance_nm_wrapped(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64, max_nm: f64) -> f64 {
        let coslat = cos_lat_clamped((lat1 + lat2) / 2.0);
        let dlon = nm_lon_radius(max_nm, coslat);
        with_wrap_retry(lon1.min(lon2), lon1.max(lon2), dlon, |shift| {
            self.segment_min_distance_nm_planar(lon1 + shift, lat1, lon2 + shift, lat2, max_nm)
        })
    }

    fn segment_min_distance_nm_planar(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64, max_nm: f64) -> f64 {
        let ref_lat = (lat1 + lat2) / 2.0;
        let coslat = cos_lat_clamped(ref_lat);
        let dlat = max_nm / 60.0;
        let dlon = max_nm / (60.0 * coslat);
        let qa = nm_frame(Coord { x: lon1, y: lat1 }, lon1, ref_lat, coslat);
        let qb = nm_frame(Coord { x: lon2, y: lat2 }, lon1, ref_lat, coslat);
        let query = Line::new(qa, qb);
        let mut best = max_nm;
        for run in self.candidates(
            lon1.min(lon2) - dlon,
            lat1.min(lat2) - dlat,
            lon1.max(lon2) + dlon,
            lat1.max(lat2) + dlat,
        ) {
            for l in self.lines(run) {
                let ca = nm_frame(l.start, lon1, ref_lat, coslat);
                let cb = nm_frame(l.end, lon1, ref_lat, coslat);
                if query.intersects(&Line::new(ca, cb)) {
                    return 0.0;
                }
                let d = point_to_segment_dist(qa, ca, cb)
                    .min(point_to_segment_dist(qb, ca, cb))
                    .min(point_to_segment_dist(ca, qa, qb))
                    .min(point_to_segment_dist(cb, qa, qb));
                best = best.min(d);
            }
        }
        best
    }
}
```

Below that, paste verbatim from `geo_index.rs`: `LonLatSegment`, `split_at_antimeridian`, `cos_lat_clamped`, `nm_lon_radius`, `with_wrap_retry`, `nm_frame`, `point_to_segment_dist` (with their doc comments).

- [ ] **Step 4: Run the coast tests**

Run: `cargo test -p asw-core coast 2>&1 | tail -20`
Expected: all 13 tests pass. If `crossing_count_touching_vertex_is_even` fails, check the `left` closure uses `> 0.0` (strict), not `>= 0.0`.

- [ ] **Step 5: Delete the R-tree index and repoint every user**

Delete `crates/asw-core/src/geo_index.rs` and remove `pub mod geo_index;` from `lib.rs`. Remove `rstar.workspace = true` from `crates/asw-core/Cargo.toml`.

`crates/asw-core/src/graph.rs`: rename the field on both `GraphBuilder` and `RoutingGraph` from `coastline_coords: Vec<Vec<(f32, f32)>>` to `coastline_runs: Vec<Vec<(f64, f64)>>`, update `drop_coastline_coords` → `drop_coastline_runs`, and the `build()`, `prune_to_main_component` and test uses. The bitcode serialization keeps working (it is transitional; Task 4 removes it).

`crates/asw-core/src/routing.rs`:
- `use crate::geo_index::CoastlineIndex;` → `use crate::coast::CoastlineIndex;`
- `coastline: &CoastlineIndex` → `coastline: &CoastlineIndex<'_>` in `smooth_indices`, `direct_line_ok`, `compute_route`.
- Test helpers: return owned sections and call `.index()` at the use site. Replace `wall_index` and `island_around_origin` with:

```rust
    fn wall(lon: f64, lat_min: f64, lat_max: f64) -> crate::coast::CoastlineSections {
        crate::coast::CoastlineSections::from_runs(&[vec![(lon, lat_min), (lon, lat_max)]])
    }

    fn island_around_origin() -> crate::coast::CoastlineSections {
        crate::coast::CoastlineSections::from_runs(&[vec![
            (-0.1, -0.1),
            (0.1, -0.1),
            (0.1, 0.1),
            (-0.1, 0.1),
            (-0.1, -0.1),
        ]])
    }
```

and in every test: `let coastline = wall_index(..);` → `let sections = wall(..); let coastline = sections.index();` (same for `island_around_origin()`, and `CoastlineIndex::new(vec![])` → `let sections = crate::coast::CoastlineSections::from_runs(&[]); let coastline = sections.index();`). The two ring fixtures built inline with `geo::LineString` (`compute_route_flags_and_excludes_land_leg`, `compute_route_same_node_both_legs_flagged_zero_distance`) become `CoastlineSections::from_runs(&[vec![(28.25, 36.83), (28.27, 36.83), (28.27, 36.85), (28.25, 36.85), (28.25, 36.83)]])`.

`crates/asw-build/src/coastline.rs`: return runs only.

```rust
use asw_core::COASTLINE_SUBDIVIDE_MAX;
use geo::{Coord, LineString, Polygon};
use rayon::prelude::*;
use tracing::info;

/// Extract coastline runs (lon, lat) from land polygons, subdivided to at
/// most COASTLINE_SUBDIVIDE_MAX vertices so the grid index stays selective.
pub fn extract_coastline(polygons: &[Polygon<f64>]) -> Vec<Vec<(f64, f64)>> {
    info!("Extracting coastline from {} polygons...", polygons.len());
    let runs: Vec<Vec<(f64, f64)>> = polygons
        .par_iter()
        .flat_map_iter(|poly| {
            let mut rings = subdivide_ring(poly.exterior());
            for hole in poly.interiors() {
                rings.extend(subdivide_ring(hole));
            }
            rings
                .into_iter()
                .map(|ls| ls.coords().map(|c| (c.x, c.y)).collect::<Vec<_>>())
        })
        .collect();
    info!("{} coastline runs after subdivision", runs.len());
    runs
}
```

(`subdivide_ring` unchanged.)

`crates/asw-build/src/pipeline.rs` step 2 becomes:

```rust
    info!("Extracting coastline segments...");
    let land_polygons = land.polygons();
    let mut coastline_runs = crate::coastline::extract_coastline(&land_polygons);
    let full_sections = asw_core::coast::CoastlineSections::from_runs(&coastline_runs);
    let coastline_index = full_sections.index();
    info!("Coastline: {} runs", coastline_index.run_count());
```

Keep the bbox clip on `coastline_runs` (change `(lon as f64)` to `lon`). Later `builder.coastline_runs = coastline_runs;`. Update `use asw_core::geo_index::CoastlineIndex;` → remove; `cells.rs:2` and `shore.rs:3` import `asw_core::coast::CoastlineIndex` and take `&CoastlineIndex<'_>`. `shore.rs` test: build `CoastlineSections::from_runs(&[vec![(28.0, 36.0), (28.0, 37.0)]])` and pass `&sections.index()`.

`crates/asw-serve/src/state.rs`: `AppState` becomes

```rust
pub struct AppState {
    pub graph: RoutingGraph,
    /// Transitional (until the graph file carries the sections itself).
    pub coast: asw_core::coast::CoastlineSections,
    pub(crate) astar_pool: asw_core::astar_pool::AstarPool,
}
```

with `AppState::new`: `let coast = CoastlineSections::from_runs(&graph.coastline_runs); graph.drop_coastline_runs();`. `api.rs`: `&app.coastline` → `&app.coast.index()`; the test helper `ready_state_with_graph(coastline: Vec<Vec<(f64, f64)>>)` sets `graph.coastline_runs = coastline;` and `route_returns_404_when_no_route_found_once_ready` sets `b.coastline_runs = vec![vec![(28.4, 36.0), (28.4, 37.5)]];`. `main.rs:317` logs `app_state.coast.index().run_count()`. `bench.rs:148,188`: `&app.coast.index()`. `main.rs` geojson: iterate `graph.coastline_runs` and change `coastline_feature_string(seg: &[(f64, f64)])` to map `|&(lon, lat)| [lon, lat]`.

- [ ] **Step 6: Build, test, clippy**

Run: `cargo test --workspace 2>&1 | tail -20 && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: green. The routing tests that previously passed through the R-tree must still pass unchanged in their assertions.

- [ ] **Step 7: Amend the spec**

In the spec's section 3 table insert after `coast_runs`: `| 5 | coast_bbox | i32 × 4 | num_coast_runs | min_lon, min_lat, max_lon, max_lat per run, microdegrees; the grid lists runs by cell, the bbox prunes inside a cell |`, renumber the later sections 6-8, change "8 entries" to "9 entries" and the header size from 216 to 232 bytes (offset 88 + 9 × 16 = 232). In section 4 replace "`rayon` is not used in asw-core today; nothing to gate." with "`rayon` was only used by `LandIndex::subtract_water`, which moves to asw-build with the rest of `LandIndex`."

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): grid-backed coastline index with crossing parity"
```

---

### Task 3: Drop stored edge weights

**Files:**
- Modify: `crates/asw-core/src/graph.rs` (`GraphBuilder`, `NeighborIter`, `prune_to_main_component`, tests)
- Modify: `crates/asw-core/src/routing.rs` (tests only)
- Modify: `crates/asw-build/src/edges.rs`, `pipeline.rs`
- Modify: `crates/asw-serve/src/state.rs` (`chain_graph`)

**Interfaces:**
- Produces: `GraphBuilder::add_edge(&mut self, src: u32, dst: u32)`, `GraphBuilder::add_directed_edge(&mut self, src: u32, dst: u32)`, `GraphBuilder::prune_to_main_component(self) -> GraphBuilder`. `RoutingGraph::neighbors(node) -> impl Iterator<Item = (u32, f32)>` where the `f32` is haversine nm between the two cell centres. `RoutingGraph::prune_to_main_component` is removed.

- [ ] **Step 1: Write the failing tests in `graph.rs`**

Replace `neighbor_iter_decodes_edge_data`, `quantized_weight_never_zero_for_tiny_res13_edge` and `build_hard_errors_on_weight_overflowing_u16` with:

```rust
    #[test]
    fn neighbor_weights_are_centre_to_centre_haversine() {
        let g = square_graph();
        for n in 0..g.num_nodes {
            let (lat, lon) = g.node_pos(n);
            for (t, w) in g.neighbors(n) {
                let (tlat, tlon) = g.node_pos(t);
                let expected = crate::h3::haversine_nm(lat, lon, tlat, tlon);
                assert!((w as f64 - expected).abs() < 1e-3, "edge {n}->{t}: {w} vs {expected}");
            }
        }
    }

    #[test]
    fn res13_edge_weight_is_true_distance() {
        let center = h3o::LatLng::new(9.08, -79.68)
            .unwrap()
            .to_cell(h3o::Resolution::Thirteen);
        let neighbor = crate::h3::neighbors(center)[0];
        let (lat0, lon0) = crate::h3::cell_center(center);
        let (lat1, lon1) = crate::h3::cell_center(neighbor);
        let true_dist_nm = crate::h3::haversine_nm(lat0, lon0, lat1, lon1);
        assert!(true_dist_nm < 0.005);

        let mut cells = [u64::from(center), u64::from(neighbor)];
        cells.sort_unstable();
        let mut b = GraphBuilder::default();
        let n0 = b.add_node(cells[0], 255);
        let n1 = b.add_node(cells[1], 255);
        b.add_edge(n0, n1);
        let g = b.build();
        let (_, w) = g.neighbors(n0).next().unwrap();
        assert!(w > 0.0, "canal edges must never be free");
        assert!((w as f64 - true_dist_nm).abs() < 1e-6);
    }
```

In `square_graph` and `builder_produces_compact_format` drop the weight arguments (`b.add_edge(n0, n1);` etc.). In `builder_produces_compact_format` replace the `186.0` assertion with the haversine of the two node positions (tolerance `1e-3`). In `graph_save_load_roundtrip` keep the field comparisons.

`prune_keeps_main_component_and_shore_dist`: after adding edges call `let b = b.prune_to_main_component(); let g = b.build();` and keep its assertions on `g`.

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p asw-core graph 2>&1 | head -20`
Expected: compile errors on `add_edge` arity.

- [ ] **Step 3: Implement**

`GraphBuilder`:

```rust
#[derive(Default)]
pub struct GraphBuilder {
    /// (h3_index, shore_dist_q) per node, in the order added (must be strictly ascending by H3).
    nodes: Vec<(u64, u8)>,
    /// (src, dst)
    edges: Vec<(u32, u32)>,
    pub coastline_runs: Vec<Vec<(f64, f64)>>,
}

impl GraphBuilder {
    pub fn add_node(&mut self, h3_index: u64, shore_dist_q: u8) -> u32 { /* unchanged */ }

    /// Add a bidirectional edge. Weights are not stored: the router computes
    /// centre-to-centre haversine at query time.
    pub fn add_edge(&mut self, src: u32, dst: u32) {
        self.edges.push((src, dst));
        self.edges.push((dst, src));
    }

    pub fn add_directed_edge(&mut self, src: u32, dst: u32) {
        self.edges.push((src, dst));
    }

    /// Keep only the largest connected component, renumbering node ids and
    /// preserving H3 order. Returns self unchanged when already connected.
    pub fn prune_to_main_component(mut self) -> Self {
        let n = self.nodes.len();
        let labels = component_labels(n, &self.edges);
        let mut comp_sizes: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        for &root in &labels {
            *comp_sizes.entry(root).or_insert(0) += 1;
        }
        let Some((&main_root, &main_count)) = comp_sizes.iter().max_by_key(|(_, c)| **c) else {
            return self;
        };
        if main_count == n {
            return self;
        }
        tracing::info!(
            "Pruning {} nodes in {} small components (keeping {} in main component)",
            n - main_count,
            comp_sizes.len() - 1,
            main_count,
        );
        let mut old_to_new: Vec<Option<u32>> = vec![None; n];
        let mut kept = Vec::with_capacity(main_count);
        for (old, node) in self.nodes.iter().enumerate() {
            if labels[old] == main_root {
                old_to_new[old] = Some(kept.len() as u32);
                kept.push(*node);
            }
        }
        self.edges = self
            .edges
            .iter()
            .filter_map(|&(s, d)| Some((old_to_new[s as usize]?, old_to_new[d as usize]?)))
            .collect();
        self.nodes = kept;
        self
    }

    pub fn build(self) -> RoutingGraph { /* as today, minus the weight bytes */ }
}

/// Union-find component root per node over an edge list.
fn component_labels(n: usize, edges: &[(u32, u32)]) -> Vec<u32> {
    /* the body of the old RoutingGraph::component_labels, iterating `edges` instead of `self.neighbors` */
}
```

In `build()`, the encode loop becomes:

```rust
        for list in &adj {
            offsets.push(edge_data.len() as u32);
            let mut prev_target = 0u32;
            for &target in list {
                crate::varint::encode(target - prev_target, &mut edge_data);
                prev_target = target;
            }
        }
```

with `adj: Vec<Vec<u32>>` (sorted and deduplicated per source: `list.sort_unstable(); list.dedup();`).

`NeighborIter`:

```rust
pub struct NeighborIter<'a> {
    graph: &'a RoutingGraph,
    data: &'a [u8],
    pos: usize,
    prev_target: u32,
    src_lat: f64,
    src_lon: f64,
}

impl<'a> Iterator for NeighborIter<'a> {
    type Item = (u32, f32);

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.data.len() {
            return None;
        }
        let (delta, new_pos) = crate::varint::decode(self.data, self.pos);
        self.pos = new_pos;
        let target = self.prev_target + delta;
        self.prev_target = target;
        // ponytail: one cell-centre decode per relaxation; cache positions in
        // the A* buffers if the bench says this dominates.
        let (lat, lon) = self.graph.node_pos(target);
        let weight_nm = crate::h3::haversine_nm(self.src_lat, self.src_lon, lat, lon) as f32;
        Some((target, weight_nm))
    }
}
```

and `neighbors()` fills `src_lat, src_lon` from `self.node_pos(node)`. Delete `RoutingGraph::prune_to_main_component` and `RoutingGraph::component_labels`. Update the file-level doc comment: "Edge data is varint target deltas; weights are recomputed from cell centres."

`crates/asw-build/src/edges.rs`: `pub type Edge = (u32, u32);`, delete the two `cost` computations and their `cell_center`/`haversine_nm` imports if now unused, `edges.push((src_id, dst_id))`, `edges.push((a, b))`, and the filter at line 149 maps `(src, dst)`. `pipeline.rs`: `builder.add_edge(id_remap[src as usize], id_remap[dst as usize]);`, and replace the build+prune block with:

```rust
    let builder = builder.prune_to_main_component();
    let graph = builder.build();
    info!("Final graph: {} nodes, {} edges", graph.num_nodes, graph.num_edges);
```

`crates/asw-serve/src/state.rs:292`: `b.add_edge(ids[i], ids[i + 1]);`.

- [ ] **Step 4: Fix the routing tests that assumed hand-set weights**

In `crates/asw-core/src/routing.rs` tests add a helper next to `diamond_graph`:

```rust
    /// Sum of centre-to-centre haversine along a node path.
    fn path_len(g: &RoutingGraph, path: &[u32]) -> f64 {
        path.windows(2)
            .map(|w| {
                let (a1, o1) = g.node_pos(w[0]);
                let (a2, o2) = g.node_pos(w[1]);
                haversine_nm(a1, o1, a2, o2)
            })
            .sum()
    }
```

- `diamond_graph`: `b.add_edge(ids["A"], ids["B"]);` etc. (four edges, no weights).
- `astar_shortest_path`: replace the `cost - 10.0` assertion with `assert!((cost - path_len(&g, &path)).abs() < 1e-3);`.
- `corridor_graph`: move B offshore so the offshore corridor is strictly longer: `(0.0, 1.6, "B", 255u8)`. Edges without weights. `penalty_diverts_route_offshore`: keep the two path assertions; replace the cost assertions with `assert!((cost - path_len(&g, &path)).abs() < 1e-3);` for the no-penalty case and, for the penalty case, `assert!(cost > path_len(&g, &path) - 1e-3);` (the penalised cost is never below the geometric length).
- `chain_graph` (routing tests, line ~787) and the chain in `compute_route_flags_and_excludes_land_leg`: `b.add_edge(w[0], w[1]);` / `b.add_edge(ids[i], ids[i + 1]);`.

- [ ] **Step 5: Test and clippy**

Run: `cargo test --workspace 2>&1 | tail -20 && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: green.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): compute edge weights from cell centres, drop stored weights"
```

---

### Task 4: v4 byte layout, mmap reader, writer

**Files:**
- Modify: `crates/asw-core/src/graph.rs` (storage, header, reader, writer, accessors, tests)
- Modify: `crates/asw-core/Cargo.toml` (remove `serde`, `bitcode`, `zstd`, `ordered-float`; add `memmap2`), root `Cargo.toml` (`memmap2 = "0.9"` in workspace deps)
- Modify: `crates/asw-core/src/routing.rs`, `crates/asw-serve/src/state.rs`, `api.rs`, `crates/asw-cli/src/main.rs`, `bench.rs`, `crates/asw-build/src/pipeline.rs` (accessor call sites)

**Interfaces:**
- Produces:
  - `RoutingGraph::open(path: &Path, populate: bool) -> anyhow::Result<RoutingGraph>`
  - `RoutingGraph::from_bytes(bytes: Vec<u8>) -> anyhow::Result<RoutingGraph>`
  - `RoutingGraph::save(&self, path: &Path) -> anyhow::Result<()>` (writes the mapped bytes)
  - `GraphBuilder::build_bytes(self) -> Vec<u8>`, `GraphBuilder::build(self) -> RoutingGraph` (= `from_bytes(build_bytes()).expect(..)`), `GraphBuilder.version: String`
  - Accessors: `num_nodes() -> u32`, `num_edges() -> u32`, `version() -> &str`, `node_h3(i: u32) -> u64`, `node_h3s() -> &[u64]`, `shore_dist(i: u32) -> u8`, `node_pos(i)`, `neighbors(i)`, `h3_lookup(h3: u64) -> Option<u32>`, `coastline() -> CoastlineIndex<'_>`
  - Public fields on `RoutingGraph` are gone.

- [ ] **Step 1: Write the failing tests**

Replace the `graph_save_load_roundtrip`, `load_rejects_old_format`, `load_rejects_v2_files`, `node_pos_h3_decode` tests with:

```rust
    #[test]
    fn build_bytes_layout_header() {
        let mut b = GraphBuilder::default();
        b.version = "0.7.0 2026-10-03".into();
        let bytes = b.build_bytes();
        assert_eq!(&bytes[0..4], b"ASW\x04");
        assert_eq!(bytes[8] as usize, "0.7.0 2026-10-03".len());
        assert_eq!(&bytes[9..25], b"0.7.0 2026-10-03");
        assert_eq!(bytes.len() % 8, 0);
        let g = RoutingGraph::from_bytes(bytes).unwrap();
        assert_eq!(g.version(), "0.7.0 2026-10-03");
        assert_eq!(g.num_nodes(), 0);
        assert_eq!(g.coastline().run_count(), 0);
    }

    #[test]
    fn save_open_roundtrip_through_mmap() {
        let g = square_graph();
        let dir = std::env::temp_dir().join(format!("asw-v4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("square.graph");
        g.save(&path).unwrap();
        let loaded = RoutingGraph::open(&path, false).unwrap();
        assert_eq!(loaded.num_nodes(), g.num_nodes());
        assert_eq!(loaded.num_edges(), g.num_edges());
        assert_eq!(loaded.node_h3s(), g.node_h3s());
        let a: Vec<(u32, f32)> = loaded.neighbors(0).collect();
        let b: Vec<(u32, f32)> = g.neighbors(0).collect();
        assert_eq!(a, b);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn coastline_roundtrips_through_the_file() {
        let mut b = GraphBuilder::default();
        b.coastline_runs = vec![vec![(28.4, 36.0), (28.4, 37.5)]];
        let g = b.build();
        let coast = g.coastline();
        assert_eq!(coast.run_count(), 1);
        assert!(coast.crosses_land(28.3, 36.5, 28.5, 36.5));
        let pts: Vec<(f64, f64)> = coast.run_points(0).collect();
        assert_eq!(pts, vec![(28.4, 36.0), (28.4, 37.5)]);
    }

    #[test]
    fn from_bytes_rejects_v3_and_garbage() {
        let err = RoutingGraph::from_bytes(b"ASW\x03whatever".to_vec()).unwrap_err();
        assert!(err.to_string().contains("Unsupported ASW graph version 3"), "got: {err}");
        assert!(RoutingGraph::from_bytes(vec![4, 0, 0, 0]).is_err());
        assert!(RoutingGraph::from_bytes(Vec::new()).is_err());
    }

    #[test]
    fn from_bytes_rejects_truncated_file() {
        let bytes = square_graph_bytes();
        let cut = bytes[..bytes.len() - 8].to_vec();
        let err = RoutingGraph::from_bytes(cut).unwrap_err();
        assert!(err.to_string().contains("beyond end of file"), "got: {err}");
    }

    #[test]
    fn from_bytes_rejects_misaligned_section() {
        let mut bytes = square_graph_bytes();
        // Section 0 offset lives at header byte 88; nudge it by 4.
        let off = u64::from_le_bytes(bytes[88..96].try_into().unwrap());
        bytes[88..96].copy_from_slice(&(off + 4).to_le_bytes());
        let err = RoutingGraph::from_bytes(bytes).unwrap_err();
        assert!(err.to_string().contains("aligned"), "got: {err}");
    }

    #[test]
    fn from_bytes_rejects_wrong_section_length() {
        let mut bytes = square_graph_bytes();
        // num_nodes at byte 72: claim one node more than stored.
        let n = u32::from_le_bytes(bytes[72..76].try_into().unwrap());
        bytes[72..76].copy_from_slice(&(n + 1).to_le_bytes());
        assert!(RoutingGraph::from_bytes(bytes).is_err());
    }

    #[test]
    #[should_panic(expected = "strictly ascending")]
    fn builder_panics_on_unsorted_nodes() {
        let c0 = h3o::LatLng::new(0.0, 0.0).unwrap().to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(1.0, 1.0).unwrap().to_cell(h3o::Resolution::Five);
        let (lo, hi) = (u64::from(c0).min(u64::from(c1)), u64::from(c0).max(u64::from(c1)));
        let mut b = GraphBuilder::default();
        b.add_node(hi, 255);
        b.add_node(lo, 255);
        let _ = b.build_bytes();
    }
```

Add the helper `fn square_graph_bytes() -> Vec<u8>` that builds the same graph as `square_graph` but returns `b.build_bytes()`. Change the existing tests to accessors: `g.num_nodes` → `g.num_nodes()`, `g.num_edges` → `g.num_edges()`, `loaded.node_h3` → `loaded.node_h3s()`.

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p asw-core graph 2>&1 | head -10`
Expected: compile errors (`build_bytes`, `open`, `from_bytes` missing).

- [ ] **Step 3: Implement the layout**

Rewrite the storage part of `crates/asw-core/src/graph.rs`. Keep `SHORE_DIST_UNIT_NM`, `SHORE_DIST_MAX_NM`, `quantize_shore_dist`, `NeighborIter`, `GraphBuilder` (from Task 3) as they are, and add:

```rust
use crate::coast::{CoastlineIndex, CoastlineSections, GRID_CELLS};
use std::path::Path;

const _: () = assert!(cfg!(target_endian = "little"), "v4 graph files are little-endian");

const MAGIC: [u8; 4] = *b"ASW\x04";
const VERSION_OFF: usize = 8; // u8 length + 63 bytes
const NUM_NODES_OFF: usize = 72;
const NUM_EDGES_OFF: usize = 76;
const NUM_RUNS_OFF: usize = 80;
const TABLE_OFF: usize = 88;
const SECTION_COUNT: usize = 9;
const HEADER_LEN: usize = TABLE_OFF + SECTION_COUNT * 16; // 232

const SEC_NODE_H3: usize = 0;
const SEC_OFFSETS: usize = 1;
const SEC_EDGE_TARGETS: usize = 2;
const SEC_SHORE_DIST: usize = 3;
const SEC_COAST_RUNS: usize = 4;
const SEC_COAST_BBOX: usize = 5;
const SEC_COAST_POINTS: usize = 6;
const SEC_GRID_OFFSETS: usize = 7;
const SEC_GRID_IDS: usize = 8;

/// Integer types that may be viewed directly in the mapped file.
pub trait Plain: Copy + private::Sealed {}
mod private {
    pub trait Sealed {}
}
macro_rules! plain {
    ($($t:ty),*) => { $(impl private::Sealed for $t {} impl Plain for $t {})* };
}
plain!(u8, u32, u64, i32);

fn cast_slice<T: Plain>(bytes: &[u8]) -> &[T] {
    let size = std::mem::size_of::<T>();
    assert_eq!(bytes.len() % size, 0, "section length is not a multiple of the element size");
    assert_eq!(bytes.as_ptr() as usize % std::mem::align_of::<T>(), 0, "section is not aligned");
    // SAFETY: T is a plain integer (sealed), length and alignment checked above,
    // and the returned slice borrows `bytes`.
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const T, bytes.len() / size) }
}

fn bytes_of<T: Plain>(data: &[T]) -> &[u8] {
    // SAFETY: T is a plain integer with no padding.
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data)) }
}

enum Bytes {
    Mmap(memmap2::Mmap),
    /// u64-backed so every 8-aligned section offset stays aligned in memory.
    Owned { buf: Vec<u64>, len: usize },
}

impl Bytes {
    fn owned(src: &[u8]) -> Self {
        let mut buf = vec![0u64; src.len().div_ceil(8)];
        // SAFETY: the u64 buffer is at least src.len() bytes long.
        let dst = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, src.len()) };
        dst.copy_from_slice(src);
        Bytes::Owned { buf, len: src.len() }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Bytes::Mmap(m) => &m[..],
            Bytes::Owned { buf, len } => &bytes_of(buf)[..*len],
        }
    }
}

#[derive(Clone, Copy)]
struct Section {
    off: usize,
    len: usize,
}

/// File layout: 232-byte header, then nine 8-byte-aligned little-endian
/// sections (see the v4 design spec). The struct is a view over the bytes;
/// nothing is decoded at open time.
pub struct RoutingGraph {
    bytes: Bytes,
    version: String,
    num_nodes: u32,
    num_edges: u32,
    num_runs: u32,
    sections: [Section; SECTION_COUNT],
}
```

Reader:

```rust
impl RoutingGraph {
    /// Memory-map a v4 file. `populate` asks the kernel to read the whole
    /// file in at open (MAP_POPULATE on Linux, MADV_WILLNEED elsewhere).
    pub fn open(path: &Path, populate: bool) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path)?;
        let mut opts = memmap2::MmapOptions::new();
        if populate {
            opts.populate();
        }
        // SAFETY: the file is only ever replaced whole by rename; a mapping of
        // the old inode stays valid until dropped.
        let mmap = unsafe { opts.map(&file)? };
        #[cfg(unix)]
        if populate {
            let _ = mmap.advise(memmap2::Advice::WillNeed);
        }
        Self::parse(Bytes::Mmap(mmap))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> anyhow::Result<Self> {
        Self::parse(Bytes::owned(&bytes))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::write(path, self.bytes.as_slice())?;
        Ok(())
    }

    fn parse(bytes: Bytes) -> anyhow::Result<Self> {
        let b = bytes.as_slice();
        anyhow::ensure!(b.len() >= 4 && &b[..3] == b"ASW", "Not an ASW graph file (expected ASW magic header). Rebuild required.");
        anyhow::ensure!(b[3] == 4, "Unsupported ASW graph version {} (expected 4). Rebuild required.", b[3]);
        anyhow::ensure!(b.len() >= HEADER_LEN, "graph header truncated");
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let vlen = b[VERSION_OFF] as usize;
        anyhow::ensure!(vlen <= 63, "graph version string too long");
        let version = std::str::from_utf8(&b[VERSION_OFF + 1..VERSION_OFF + 1 + vlen])?.to_string();
        let num_nodes = u32_at(NUM_NODES_OFF);
        let num_edges = u32_at(NUM_EDGES_OFF);
        let num_runs = u32_at(NUM_RUNS_OFF);
        let mut sections = [Section { off: 0, len: 0 }; SECTION_COUNT];
        for (i, s) in sections.iter_mut().enumerate() {
            let off = u64_at(TABLE_OFF + i * 16) as usize;
            let len = u64_at(TABLE_OFF + i * 16 + 8) as usize;
            anyhow::ensure!(off % 8 == 0, "section {i} is not 8-byte aligned");
            anyhow::ensure!(off >= HEADER_LEN && off.saturating_add(len) <= b.len(), "section {i} extends beyond end of file");
            *s = Section { off, len };
        }
        let n = num_nodes as usize;
        let r = num_runs as usize;
        let expect = |i: usize, len: usize, what: &str| -> anyhow::Result<()> {
            anyhow::ensure!(sections[i].len == len, "{what} section length {} != expected {len}", sections[i].len);
            Ok(())
        };
        expect(SEC_NODE_H3, n * 8, "node_h3")?;
        expect(SEC_OFFSETS, (n + 1) * 4, "offsets")?;
        expect(SEC_SHORE_DIST, n, "shore_dist")?;
        expect(SEC_COAST_RUNS, (r + 1) * 4, "coast_runs")?;
        expect(SEC_COAST_BBOX, r * 16, "coast_bbox")?;
        expect(SEC_GRID_OFFSETS, (GRID_CELLS + 1) * 4, "grid_offsets")?;
        anyhow::ensure!(sections[SEC_COAST_POINTS].len % 8 == 0, "coast_points length not a multiple of 8");
        anyhow::ensure!(sections[SEC_GRID_IDS].len % 4 == 0, "grid_ids length not a multiple of 4");
        let g = Self { bytes, version, num_nodes, num_edges, num_runs, sections };
        let offsets: &[u32] = g.section(SEC_OFFSETS);
        anyhow::ensure!(offsets[n] as usize == g.sections[SEC_EDGE_TARGETS].len, "offsets sentinel != edge_targets length");
        let runs: &[u32] = g.section(SEC_COAST_RUNS);
        anyhow::ensure!(runs[r] as usize * 8 == g.sections[SEC_COAST_POINTS].len, "coast_runs sentinel != coast_points length");
        let grid: &[u32] = g.section(SEC_GRID_OFFSETS);
        anyhow::ensure!(grid[GRID_CELLS] as usize * 4 == g.sections[SEC_GRID_IDS].len, "grid_offsets sentinel != grid_ids length");
        Ok(g)
    }

    fn section<T: Plain>(&self, i: usize) -> &[T] {
        let s = self.sections[i];
        cast_slice(&self.bytes.as_slice()[s.off..s.off + s.len])
    }

    pub fn num_nodes(&self) -> u32 { self.num_nodes }
    pub fn num_edges(&self) -> u32 { self.num_edges }
    pub fn version(&self) -> &str { &self.version }
    pub fn node_h3s(&self) -> &[u64] { self.section(SEC_NODE_H3) }
    pub fn node_h3(&self, node: u32) -> u64 { self.node_h3s()[node as usize] }
    pub fn shore_dist(&self, node: u32) -> u8 { self.section::<u8>(SEC_SHORE_DIST)[node as usize] }

    /// Binary search for an exact H3 cell index.
    pub fn h3_lookup(&self, h3: u64) -> Option<u32> {
        self.node_h3s().binary_search(&h3).ok().map(|i| i as u32)
    }

    pub fn coastline(&self) -> CoastlineIndex<'_> {
        CoastlineIndex::from_slices(
            self.section(SEC_COAST_RUNS),
            self.section(SEC_COAST_BBOX),
            self.section(SEC_COAST_POINTS),
            self.section(SEC_GRID_OFFSETS),
            self.section(SEC_GRID_IDS),
        )
    }

    pub fn neighbors(&self, node: u32) -> NeighborIter<'_> {
        let offsets: &[u32] = self.section(SEC_OFFSETS);
        let (start, end) = (offsets[node as usize] as usize, offsets[node as usize + 1] as usize);
        let (src_lat, src_lon) = self.node_pos(node);
        NeighborIter { graph: self, data: &self.section::<u8>(SEC_EDGE_TARGETS)[start..end], pos: 0, prev_target: 0, src_lat, src_lon }
    }

    pub fn node_pos(&self, node: u32) -> (f64, f64) {
        let cell = h3o::CellIndex::try_from(self.node_h3(node)).expect("invalid H3 index");
        crate::h3::cell_center(cell)
    }
}
```

Writer, in `GraphBuilder` (add `pub version: String` to the struct):

```rust
    /// Encode the v4 file image. Panics on builder misuse (unsorted or
    /// invalid H3 ids): the builder is the only writer, so this is the one
    /// place the invariants are checked.
    pub fn build_bytes(self) -> Vec<u8> {
        for w in self.nodes.windows(2) {
            assert!(w[0].0 < w[1].0, "nodes must be added in strictly ascending H3 order");
        }
        for &(h3, _) in &self.nodes {
            assert!(h3o::CellIndex::try_from(h3).is_ok(), "invalid H3 index {h3:#x}");
        }
        assert!(self.version.len() <= 63, "version string longer than 63 bytes");
        let num_nodes = self.nodes.len() as u32;
        let node_h3: Vec<u64> = self.nodes.iter().map(|(h3, _)| *h3).collect();
        let shore_dist: Vec<u8> = self.nodes.iter().map(|(_, q)| *q).collect();

        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); num_nodes as usize];
        for &(src, dst) in &self.edges {
            adj[src as usize].push(dst);
        }
        let mut edge_targets = Vec::new();
        let mut offsets = Vec::with_capacity(num_nodes as usize + 1);
        let mut num_edges = 0u32;
        for list in &mut adj {
            list.sort_unstable();
            list.dedup();
            offsets.push(edge_targets.len() as u32);
            let mut prev = 0u32;
            for &target in list.iter() {
                crate::varint::encode(target - prev, &mut edge_targets);
                prev = target;
                num_edges += 1;
            }
        }
        offsets.push(edge_targets.len() as u32);

        let coast = CoastlineSections::from_runs(&self.coastline_runs);

        let mut out = vec![0u8; HEADER_LEN];
        let mut table: Vec<(u64, u64)> = Vec::with_capacity(SECTION_COUNT);
        fn push<T: Plain>(out: &mut Vec<u8>, table: &mut Vec<(u64, u64)>, data: &[T]) {
            while out.len() % 8 != 0 {
                out.push(0);
            }
            let bytes = bytes_of(data);
            table.push((out.len() as u64, bytes.len() as u64));
            out.extend_from_slice(bytes);
        }
        push(&mut out, &mut table, &node_h3);
        push(&mut out, &mut table, &offsets);
        push(&mut out, &mut table, &edge_targets);
        push(&mut out, &mut table, &shore_dist);
        push(&mut out, &mut table, &coast.runs);
        push(&mut out, &mut table, &coast.bbox);
        push(&mut out, &mut table, &coast.points);
        push(&mut out, &mut table, &coast.grid_offsets);
        push(&mut out, &mut table, &coast.grid_ids);
        while out.len() % 8 != 0 {
            out.push(0);
        }

        out[..4].copy_from_slice(&MAGIC);
        out[VERSION_OFF] = self.version.len() as u8;
        out[VERSION_OFF + 1..VERSION_OFF + 1 + self.version.len()].copy_from_slice(self.version.as_bytes());
        out[NUM_NODES_OFF..NUM_NODES_OFF + 4].copy_from_slice(&num_nodes.to_le_bytes());
        out[NUM_EDGES_OFF..NUM_EDGES_OFF + 4].copy_from_slice(&num_edges.to_le_bytes());
        out[NUM_RUNS_OFF..NUM_RUNS_OFF + 4].copy_from_slice(&((coast.runs.len() - 1) as u32).to_le_bytes());
        for (i, (off, len)) in table.iter().enumerate() {
            out[TABLE_OFF + i * 16..TABLE_OFF + i * 16 + 8].copy_from_slice(&off.to_le_bytes());
            out[TABLE_OFF + i * 16 + 8..TABLE_OFF + i * 16 + 16].copy_from_slice(&len.to_le_bytes());
        }
        out
    }

    pub fn build(self) -> RoutingGraph {
        RoutingGraph::from_bytes(self.build_bytes()).expect("builder wrote an invalid graph image")
    }
```

Delete the old `save`/`load`, the `Serialize`/`Deserialize` derives, `drop_coastline_runs`, and the public fields. Cargo: remove `serde`, `bitcode`, `zstd`, `ordered-float` from `crates/asw-core/Cargo.toml`; add `memmap2.workspace = true`; add `memmap2 = "0.9"` to `[workspace.dependencies]` in the root `Cargo.toml`.

- [ ] **Step 4: Repoint call sites**

- `routing.rs`: `graph.shore_dist[n as usize]` → `graph.shore_dist(n)`; tests: `g.num_nodes as usize` → `g.num_nodes() as usize`; `(0..g.num_nodes)` → `(0..g.num_nodes())`.
- `crates/asw-serve/src/state.rs`: `AppState { pub graph: RoutingGraph, pub(crate) astar_pool }`; `AppState::new(graph)` builds only the pool (Task 6 makes it lazy); remove the `coast` field; `nearest_node`'s `self.graph.node_h3.binary_search` → `self.graph.h3_lookup(h3)` (delete the local `h3_lookup`). Tests: `g.num_nodes` → `g.num_nodes()`.
- `api.rs`: `&app.coast.index()` → `&app.graph.coastline()`; `app.graph.num_nodes` → `app.graph.num_nodes()`; test helpers build the graph through `GraphBuilder` with `b.coastline_runs = ...` (see Task 2) — `chain_graph` returns a built graph, so change `ready_state_with_graph` to build its own `GraphBuilder` with the three cells, `add_edge` between consecutive ids, and `coastline_runs = coastline`.
- `bench.rs:148,188`: `&app.graph.coastline()`; `graph.num_nodes` → `graph.num_nodes()` everywhere; line 553: `let graph = RoutingGraph::open(&graph_path, true).context("Failed to open graph")?;` (drop the `File`/`BufReader` lines).
- `main.rs` serve loader: replace the file/reader/load lines with `let routing_graph = asw_core::graph::RoutingGraph::open(&graph_file, true).context("Failed to open graph")?;` and log `routing_graph.num_nodes(), routing_graph.num_edges()`; the coastline log line becomes `info!("Coastline: {} runs, graph version {}", app_state.graph.coastline().run_count(), app_state.graph.version());`.
- `main.rs` geojson: `RoutingGraph::open(graph_path, false)`; `graph.node_h3[i]` → `graph.node_h3(i as u32)`; coastline loop: `let coast = graph.coastline(); for run in 0..coast.run_count() { let seg: Vec<(f64, f64)> = coast.run_points(run).collect(); ... }`.
- `pipeline.rs`: set `builder.version = format!("{} {}", env!("CARGO_PKG_VERSION"), time::OffsetDateTime::now_utc().date());` (add `time = { version = "0.3", features = ["formatting"] }` to asw-build if not present; it is already a workspace-level dependency of asw-cli, so add the same line to `crates/asw-build/Cargo.toml`) and replace the save block with `std::fs::write(output_path, builder.build_bytes()).context("Failed to write graph")?;`. Log nodes/edges via `RoutingGraph::open(output_path, false)?` after writing (one line, also proves the file opens).

- [ ] **Step 5: Test and clippy**

Run: `cargo test --workspace 2>&1 | tail -30 && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: green. `cargo tree -p asw-core` shows no `bitcode`, `zstd`, `serde`, `rstar`, `rayon`.

- [ ] **Step 6: Build Marmaris and open it**

Run:
```bash
cargo build --release -p asw-cli
./target/release/asw build --shp /Volumes/2TB/Projects/auto-sea-way/data/land-polygons-split-4326 --bbox marmaris --output export/marmaris-v4.graph
./target/release/asw geojson --graph export/marmaris-v4.graph --bbox marmaris --coastline --output export/marmaris-v4.geojson
ls -la export/marmaris-v4.graph
```
(If the shapefile directory is elsewhere, `asw build` without `--shp` downloads it into `--workdir`; use `--workdir /Volumes/2TB/Projects/auto-sea-way/data`.)
Expected: a file of roughly 4-6 MB (Marmaris v3 was 1.26 MB compressed), geojson export runs, no panic.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): memory-mapped graph format v4"
```

---

### Task 5: `nearest_node` into asw-core, `is_water` by parity

**Files:**
- Create: `crates/asw-core/src/snap.rs`
- Modify: `crates/asw-core/src/lib.rs`, `routing.rs`
- Modify: `crates/asw-serve/src/state.rs` (remove the moved code and tests), `api.rs:126`, `crates/asw-cli/src/bench.rs:137,173`

**Interfaces:**
- Produces: `RoutingGraph::nearest_node(&self, lat: f64, lon: f64) -> Option<(u32, f64)>` (same semantics as `AppState::nearest_node` today), `asw_core::routing::is_water(graph: &RoutingGraph, lat: f64, lon: f64) -> bool`.

- [ ] **Step 1: Write the failing `is_water` tests in `routing.rs`**

```rust
    /// One res-5 water node at (36.5, 28.3); a thin mole ring between it and
    /// a berth at (36.5, 28.0). Parity: two crossings, still water.
    fn mole_graph() -> RoutingGraph {
        let cell = h3o::LatLng::new(36.5, 28.3).unwrap().to_cell(h3o::Resolution::Five);
        let mut b = GraphBuilder::default();
        b.add_node(u64::from(cell), 255);
        b.coastline_runs = vec![vec![
            (28.10, 36.40),
            (28.11, 36.40),
            (28.11, 36.60),
            (28.10, 36.60),
            (28.10, 36.40),
        ]];
        b.build()
    }

    #[test]
    fn is_water_marina_behind_mole_is_water() {
        let g = mole_graph();
        assert!(is_water(&g, 36.5, 28.0));
        assert!(is_water(&g, 36.5, 28.3));
    }

    #[test]
    fn is_water_inside_island_is_land() {
        let cell = h3o::LatLng::new(36.5, 28.3).unwrap().to_cell(h3o::Resolution::Five);
        let mut b = GraphBuilder::default();
        b.add_node(u64::from(cell), 255);
        b.coastline_runs = vec![vec![(27.9, 36.4), (28.1, 36.4), (28.1, 36.6), (27.9, 36.6), (27.9, 36.4)]];
        let g = b.build();
        assert!(!is_water(&g, 36.5, 28.0), "inside the island ring");
        assert!(is_water(&g, 36.5, 28.3), "open water next to the node");
    }

    #[test]
    fn is_water_without_any_node_is_land() {
        let g = GraphBuilder::default().build();
        assert!(!is_water(&g, 36.5, 28.0));
    }
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p asw-core is_water 2>&1 | head -5`
Expected: `is_water` not found.

- [ ] **Step 3: Move the snapping ladder**

Create `crates/asw-core/src/snap.rs` containing, moved verbatim from `crates/asw-serve/src/state.rs` into `impl RoutingGraph`: `H3_EDGE_NM`, `k_max`, `DISK_DOUBLING_START`, `search_resolution` (with `self.h3_lookup(..)` and `self.node_pos(..)` instead of `self.graph.`), and `nearest_node`, with their doc comments. Then move the whole `app_state_tests` module into `snap.rs` as `mod tests`, replacing `AppState::new(graph)` + `state.nearest_node(..)` with `graph.nearest_node(..)`, and `chain_graph` (the serve test helper) as a private test helper in `snap.rs`:

```rust
    fn chain_graph(h3s: &[u64]) -> RoutingGraph {
        let mut h3s = h3s.to_vec();
        h3s.sort_unstable();
        h3s.dedup();
        let mut b = crate::graph::GraphBuilder::default();
        let ids: Vec<u32> = h3s.iter().map(|&h| b.add_node(h, 255)).collect();
        for w in ids.windows(2) {
            b.add_edge(w[0], w[1]);
        }
        b.build()
    }
```

Add `pub mod snap;` to `lib.rs`. In `state.rs` delete the moved items, the `h3o` import if unused, and `chain_graph` if nothing in asw-serve still uses it (api.rs tests build their own graph after Task 4). Call sites: `api.rs` `app.nearest_node(lat, lon)` → `app.graph.nearest_node(lat, lon)`; `bench.rs:137,173` the same.

Add to `routing.rs`:

```rust
/// Is the point on navigable water? Snap to the nearest water node (its
/// centre is known water) and count coastline crossings on the way to the
/// point: even means water. No node within the snapping ladder means land.
pub fn is_water(graph: &RoutingGraph, lat: f64, lon: f64) -> bool {
    let Some((node, _)) = graph.nearest_node(lat, lon) else {
        return false;
    };
    let (nlat, nlon) = graph.node_pos(node);
    graph.coastline().crossing_count(nlon, nlat, lon, lat) % 2 == 0
}
```

- [ ] **Step 4: Test and clippy**

Run: `cargo test --workspace 2>&1 | tail -20 && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: green, including the nine moved `nearest_node` tests now under `asw_core::snap::tests`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(core): move nearest_node into core, add is_water by crossing parity"
```

---

### Task 6: Zero-filled A* buffers, lazy pool

**Files:**
- Modify: `crates/asw-core/src/astar_pool.rs`
- Modify: `crates/asw-serve/src/state.rs` (`AppState::new` comment)

**Interfaces:**
- `AstarBuffers::new(n)` and `AstarPool::new(num_nodes, size)` keep their signatures. `AstarPool::new` no longer allocates.

- [ ] **Step 1: Write the failing tests**

In `astar_pool.rs` tests, add:

```rust
    #[test]
    fn buffers_new_is_zero_filled() {
        // Zero fills go through calloc, so untouched pages are never resident.
        let buf = AstarBuffers::new(1000);
        assert!(buf.g_score.iter().all(|&g| g == 0.0));
        assert!(buf.came_from.iter().all(|&c| c == 0));
        assert!(buf.gen.iter().all(|&g| g == 0));
    }

    #[test]
    fn pool_new_allocates_nothing_until_acquire() {
        let pool = AstarPool::new(1_000_000, 2);
        assert_eq!(pool.buffers.lock().unwrap().len(), 0);
        let buf = pool.acquire();
        assert_eq!(buf.g_score.len(), 1_000_000);
        pool.release(buf);
        assert_eq!(pool.buffers.lock().unwrap().len(), 1);
    }
```

In `reset_handles_generation_wraparound` change `assert_eq!(buf.g_score[4], f32::MAX);` to `assert_eq!(buf.g_score[4], 0.0);` (an untouched slot holds the zero fill).

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -p asw-core astar_pool 2>&1 | tail -8`
Expected: `buffers_new_is_zero_filled` and `pool_new_allocates_nothing_until_acquire` fail.

- [ ] **Step 3: Implement**

```rust
    pub fn new(num_nodes: usize) -> Self {
        // All-zero fills: `vec![0; n]` uses calloc, so pages are mapped lazily
        // and resident memory grows with the search, not the graph. The
        // initial values are never read: `touch()` initialises a slot on
        // first use in a generation.
        Self {
            g_score: vec![0.0; num_nodes],
            came_from: vec![0; num_nodes],
            closed: vec![false; num_nodes],
            h_score: vec![0.0; num_nodes],
            gen: vec![0; num_nodes],
            current_gen: 1,
        }
    }
```

`AstarPool::new`: `Self { buffers: Mutex::new(Vec::with_capacity(size)), num_nodes }` and update its doc comment: "Buffer sets are allocated on first `acquire` and kept after `release`, so a server that never routes never pays for them."

- [ ] **Step 4: Test, clippy, commit**

Run: `cargo test --workspace 2>&1 | tail -10 && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3`
Expected: green.

```bash
cargo fmt --all
git add -A
git commit -m "perf(core): zero-fill A* buffers and allocate them lazily"
```

---

### Task 7: `/info` graph version, docs, version bump

**Files:**
- Modify: `crates/asw-serve/src/api.rs` (`InfoResponse`)
- Modify: `README.md`, `CLAUDE.md`, `CHANGELOG.md`, `docs/deployment.md`
- Modify: all five `crates/*/Cargo.toml` (`version = "0.7.0"`), `Cargo.lock`

- [ ] **Step 1: Add `graph_version` to `/info`**

```rust
#[derive(Serialize)]
struct InfoResponse {
    nodes: u32,
    edges: u32,
    graph_path: String,
    version: String,
    /// Version string stored in the graph file header.
    graph_version: String,
}
```

and in `info_handler`: `graph_version: app.graph.version().to_string(),`. Add to the existing `/info` happy-path test (or add one using `ready_state_with_small_graph`) an assertion that the JSON body contains `"graph_version"`.

- [ ] **Step 2: Docs**

- `README.md:37`: "Wait for the `/ready` endpoint to return 200 (a few seconds while the graph file is mapped and read in)".
- `README.md:53` and `CLAUDE.md:47`: "res-3 deep ocean through res-10 shoreline".
- `README.md:57`: "**Serialize** graph to a flat memory-mapped binary file (format v4: sorted H3 ids, varint edge targets, per-node shore distance, coastline runs with a 0.1° grid index; no stored weights, no compression)".
- `README.md:76`: "Graph v4 format (memory-mapped). Graphs built with v3 or earlier must be rebuilt — older files are rejected at load time."
- `README.md:155` and `CLAUDE.md:63`: replace the memory paragraph with the numbers measured in Task 8 (file cache ≈ file size, plus A* pages touched). Until measured, write "to be measured".
- `README.md` Full Planet Build table: `Graph file size` and memory rows get the Task 8 numbers.
- `README.md:126` land detection sentence: unchanged.
- `CLAUDE.md:51`: "Graph format v4: flat little-endian sections, memory-mapped (`memmap2`), sorted `u64` node ids for O(log n) lookup, per-node `shore_dist: u8`, coastline runs as i32 microdegrees with a 0.1° grid index, edge weights recomputed as centre-to-centre haversine".
- `CHANGELOG.md` under `## [Unreleased]`, add:

```markdown
### Changed

- **BREAKING:** graph format v3 → v4. The file is now flat and memory-mapped: the server opens the planet in seconds instead of 60-90 s, and resident memory drops from ~4.1 GiB to the working set. Edge weights are no longer stored (recomputed from cell centres, distances move in the second decimal); the coastline lives in the file as microdegree runs with a 0.1° grid index. v3 files are rejected; the planet graph was rebuilt.
- `nearest_node` moved from asw-serve into asw-core; `LandIndex` moved from asw-core into asw-build.
- `/info` reports `graph_version`, the version string stored in the graph header.

### Added

- `asw_core::routing::is_water(graph, lat, lon)`: water test by coastline crossing parity from the nearest water node, correct inside marina basins narrower than a leaf cell.
```

- `docs/deployment.md`: search for `60-90`, `4.1`, `swap` and update the same way as the README.

- [ ] **Step 3: Version bump**

Set `version = "0.7.0"` in all five crate manifests, run `cargo build` to refresh `Cargo.lock`, check `./target/release/asw --version` after `cargo build --release -p asw-cli`.

- [ ] **Step 4: Test, clippy, commit**

Run: `cargo test --workspace 2>&1 | tail -10 && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3`

```bash
cargo fmt --all
git add -A
git commit -m "chore: v0.7.0, graph_version in /info, docs for format v4"
```

---

### Task 8: Verification on real graphs

**Files:**
- Modify: `README.md`, `CLAUDE.md` (measured numbers), `export/` (gitignored artefacts)

- [ ] **Step 1: Regional bench, before and after**

Build a main-branch binary in a separate target dir and bench it on the v3 Marmaris graph; bench the new binary on the v4 Marmaris graph built in Task 4:

```bash
git worktree add /Volumes/2TB/Projects/asw-main main
(cd /Volumes/2TB/Projects/asw-main && CARGO_TARGET_DIR=/Volumes/2TB/Projects/asw-main/target cargo build --release -p asw-cli)
/Volumes/2TB/Projects/asw-main/target/release/asw bench --graph export/marmaris-v3.graph --json --output export/bench-marmaris-v3.json
./target/release/asw bench --graph export/marmaris-v4.graph --json --output export/bench-marmaris-v4.json --compare export/bench-marmaris-v3.json
git worktree remove /Volumes/2TB/Projects/asw-main
```

Expected: identical hop counts on every route, distances within 0.1 nm, p50 timings within 2× of v3. If timings are more than 2× slower, profile with `cargo flamegraph` on one long route and, only if `cell_center` dominates, cache node positions in `AstarBuffers` (a `Vec<(f32, f32)>` filled in `touch_and_cache_h`, used by `NeighborIter`).

- [ ] **Step 2: Planet rebuild on Hetzner**

```bash
asw cloud build --output export/asw-v4.graph --keep-server
```

This is the normal ~5 h build. It emits a v4 file. Upload it to a new draft release:

```bash
gh release create draft-graph-v070 --draft --title "Planet graph v4 (v0.7.0)" export/asw-v4.graph#asw.graph
```

Then `asw cloud teardown`.

- [ ] **Step 3: Planet server measurements (on Hetzner before teardown, or on any Linux box with ≥ 4 GB)**

```bash
asw serve --graph asw.graph --port 3000 &
sleep 20; curl -s localhost:3000/ready; ps -o rss= -p $(pgrep -f 'asw serve')
./asw bench --graph asw.graph --json --output bench-planet-v4.json --compare export/bench-main-today.json
ps -o rss= -p $(pgrep -f 'asw serve')
```

Record: seconds to `/ready` with populate, RSS after open, RSS after the bench, planet file size. Put them in the README table and the CLAUDE.md memory line. Compare the bench distances and hops against `export/bench-main-today.json`; hop counts must match, distances move in the second decimal only. Commit the doc numbers:

```bash
git add README.md CLAUDE.md
git commit -m "docs: measured planet numbers for graph format v4"
```

- [ ] **Step 4: Open the PR**

Push `feat/graph-v4`, open a PR against `main` (after PR #50 has merged, rebase first). PR body: what changed, the bench table, the planet numbers, the note that the release must use `graph_tag=draft-graph-v070`.

---

## Self-review notes

- Spec coverage: file layout (Task 4), core view and accessors (Task 4), coastline grid and parity (Task 2), `nearest_node` move and `is_water` (Task 5), A* zero fill and lazy pool (Task 6), build emits v4 with version string (Task 4), server populate and `/info` (Tasks 4, 7), no converter and v3 rejection (Task 4), docs and res-10 wording (Task 7), bench and planet rebuild (Task 8), spec amendments for `coast_bbox` and rayon (Task 2). The mobile crate is out of scope by the spec.
- Type consistency: `CoastlineIndex<'a>` everywhere after Task 2; `add_edge(src, dst)` after Task 3; `num_nodes()` accessor after Task 4; `graph.nearest_node` after Task 5.
- Review Focus items 1-5 are pinned by `buffers_new_is_zero_filled` (Task 6), `is_water_marina_behind_mole_is_water` (Task 5), `from_bytes_rejects_*` (Task 4), `grid_clamps_envelope_beyond_the_seam` (Task 2), `res13_edge_weight_is_true_distance` (Task 3).
