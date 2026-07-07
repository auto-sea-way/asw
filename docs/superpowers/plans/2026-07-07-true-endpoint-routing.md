# True-Endpoint Routing Implementation Plan

> **Revised 2026-07-07 after the shore-buffer merge (PR #34).** This version is
> written against post-merge `main` (`bf54049`): `smooth` is buffer-aware
> (per-node `shore_dist` + `segment_min_distance_nm`), `compute_route` and
> `/route` carry `shore_buffer_nm`. The direct-line shortcut is gated on shore
> clearance, and coordinate smoothing carries a parallel `shore_dist` slice so
> stitched pins participate in the buffer rules.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Routes start and end exactly at the requested coordinates; clear open-water point pairs return a direct 2-point route without a graph search; both honor a requested shore buffer.

**Architecture:** Query-time change in `asw-core::routing`: (1) `smooth_indices` — the existing buffer-aware line-of-sight smoothing generalized to raw `[lon, lat]` coordinates plus a parallel quantized `shore_dist` slice, returning kept indices; the node-based `smooth` becomes a thin wrapper over it. (2) `compute_route` gains a direct-line shortcut (skipped or clearance-checked when a shore buffer is requested) and stitches the true pin coordinates onto the A* path before smoothing; pins get their own quantized shore distance via `CoastlineIndex::min_distance_nm`. No graph format change, no `asw-serve` API change.

**Tech Stack:** Rust workspace. Crates touched: `asw-core` (routing), `asw-serve` (tests only). Spec: `docs/superpowers/specs/2026-07-07-true-endpoint-routing-design.md`.

## Global Constraints

- Branch: `feature/true-endpoint-routing`, rebased onto `origin/main` at `bf54049` (shore-buffer merged).
- `cargo` is not on the default PATH. Every shell session: `export PATH="$HOME/.cargo/bin:$PATH"`.
- All distances in nautical miles (project standard, never km).
- Coordinate convention: `[f64; 2]` arrays are `[lon, lat]` (GeoJSON order). `haversine_nm(lat1, lon1, lat2, lon2)`, `crosses_land(lon1, lat1, lon2, lat2)`, `min_distance_nm(lon, lat, max_nm)`, `segment_min_distance_nm(lon1, lat1, lon2, lat2, max_nm)` keep their existing argument orders — lat/lon orders differ between them, be careful.
- Quantized shore distances: `u8` in `SHORE_DIST_UNIT_NM` (0.02 nm) units, saturating at 255 (= 5.1 nm); `crate::graph::quantize_shore_dist(nm) -> u8`.
- Run `cargo fmt --all` before every commit (CI rejects unformatted code).
- End every commit message with the trailer line: `Claude-Session: https://claude.ai/code/session_01AMx13pMriCc6KubjyQJe1q`
- Land-pin policy (approved in spec): never error; a pin that cannot see its snapped node keeps the direct (land-clipping) segment.

---

### Task 1: `smooth_indices` — coordinate-based, buffer-aware smoothing core

**Files:**
- Modify: `crates/asw-core/src/routing.rs` (add `smooth_indices`; shrink `smooth` to a wrapper over it)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `CoastlineIndex::crosses_land(lon1, lat1, lon2, lat2) -> bool`, `CoastlineIndex::segment_min_distance_nm(lon1, lat1, lon2, lat2, max_nm) -> f64`, the private `RangeMin` sparse table, `crate::graph::SHORE_DIST_UNIT_NM` — all existing.
- Produces: `pub fn smooth_indices(coords: &[[f64; 2]], shore_dist: &[u8], coastline: &CoastlineIndex, shore_buffer_nm: f64) -> Vec<usize>` — `coords` are `[lon, lat]`, `shore_dist[i]` is the quantized shore distance of `coords[i]` (only read when `shore_buffer_nm > 0`, but the slice must always be the same length as `coords`). Returns strictly increasing indices into `coords`, always containing `0` and `coords.len() - 1`. `pub fn smooth(...)` keeps its exact current signature and behavior (its three existing tests must pass unchanged). Task 2 calls `smooth_indices`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/asw-core/src/routing.rs` (it already has `use super::*;`, `use crate::graph::GraphBuilder;`, and the `dogleg`/`dogleg_with_shore` fixtures):

```rust
    /// Vertical coastline "wall" at `lon`, spanning `lat_min..lat_max`.
    fn wall_index(lon: f64, lat_min: f64, lat_max: f64) -> CoastlineIndex {
        let line = geo::LineString::from(vec![(lon, lat_min), (lon, lat_max)]);
        CoastlineIndex::new(vec![crate::geo_index::CoastlineSegment::new(line)])
    }

    /// Closed square ring around (0, 0), side 0.2 degrees — a tiny "island"
    /// used to simulate a pin on land.
    fn island_around_origin() -> CoastlineIndex {
        let ring = geo::LineString::from(vec![
            (-0.1, -0.1),
            (0.1, -0.1),
            (0.1, 0.1),
            (-0.1, 0.1),
            (-0.1, -0.1),
        ]);
        CoastlineIndex::new(vec![crate::geo_index::CoastlineSegment::new(ring)])
    }

    #[test]
    fn smooth_indices_collapses_clear_path() {
        let coastline = CoastlineIndex::new(vec![]);
        let coords = [[0.0, 0.0], [0.3, 0.1], [0.6, -0.1], [1.0, 0.0]];
        let out = smooth_indices(&coords, &[255; 4], &coastline, 0.0);
        assert_eq!(out, vec![0, 3]);
    }

    #[test]
    fn smooth_indices_keeps_necessary_corner() {
        // Wall at lon 0.5 (lat -1..1); the path detours over its top at lat 1.5.
        // Only the corner above the wall must survive smoothing.
        let coastline = wall_index(0.5, -1.0, 1.0);
        let coords = [[0.0, 0.0], [0.2, 0.5], [0.5, 1.5], [0.8, 0.5], [1.0, 0.0]];
        let out = smooth_indices(&coords, &[255; 5], &coastline, 0.0);
        assert_eq!(out, vec![0, 2, 4]);
    }

    #[test]
    fn smooth_indices_short_input_passthrough() {
        let coastline = wall_index(0.5, -1.0, 1.0);
        let coords = [[0.0, 0.0], [1.0, 0.0]];
        assert_eq!(
            smooth_indices(&coords, &[255; 2], &coastline, 0.0),
            vec![0, 1]
        );
    }

    #[test]
    fn smooth_indices_blocked_next_hop_still_progresses() {
        // First point sits inside a ring "island": it can see nothing, so the
        // smoother must keep the (land-clipping) segment to the next point and
        // continue — this is the approved land-pin behavior.
        let coastline = island_around_origin();
        let coords = [[0.0, 0.0], [0.5, 0.0], [1.0, 0.0]];
        let out = smooth_indices(&coords, &[255; 3], &coastline, 0.0);
        assert_eq!(*out.first().unwrap(), 0);
        assert_eq!(*out.last().unwrap(), 2);
    }

    #[test]
    fn smooth_indices_respects_buffer() {
        // Same geometry as the node-based dogleg tests: coastline at lon 28.0
        // (lat 36.45..36.55), direct P0->P2 line ~2.41 nm off the coast.
        let coastline = wall_index(28.0, 36.45, 36.55);
        let coords = [[28.05, 36.3], [28.15, 36.5], [28.05, 36.7]];
        let loose = smooth_indices(&coords, &[255; 3], &coastline, 2.0);
        assert_eq!(loose, vec![0, 2]);
        let strict = smooth_indices(&coords, &[255; 3], &coastline, 3.0);
        assert_eq!(strict, vec![0, 1, 2], "3 nm buffer must keep the dogleg");
    }

    #[test]
    fn smooth_indices_relaxes_for_near_shore_endpoints() {
        // Endpoints themselves are close to shore (q=20 = 0.4 nm): threshold
        // becomes min(3.0, 0.4) = 0.4 nm, so the ~2.4 nm direct line passes.
        let coastline = wall_index(28.0, 36.45, 36.55);
        let coords = [[28.05, 36.3], [28.15, 36.5], [28.05, 36.7]];
        let out = smooth_indices(&coords, &[20, 255, 20], &coastline, 3.0);
        assert_eq!(out, vec![0, 2]);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p asw-core smooth_indices`
Expected: compile error — `smooth_indices` not found.

- [ ] **Step 3: Implement `smooth_indices`, shrink `smooth` to a wrapper**

In `crates/asw-core/src/routing.rs`, replace the entire body of `smooth` and add `smooth_indices` below it. The algorithm is the current `smooth` verbatim, with `graph.node_pos(path[j])` replaced by `coords[j]` and `graph.shore_dist[node]` by `shore_dist[j]`:

```rust
/// Greedy line-of-sight smoothing over a node path (see `smooth_indices`,
/// which this delegates to after decoding node positions and shore
/// distances).
pub fn smooth(
    graph: &RoutingGraph,
    path: &[u32],
    coastline: &CoastlineIndex,
    shore_buffer_nm: f64,
) -> Vec<u32> {
    let coords: Vec<[f64; 2]> = path
        .iter()
        .map(|&n| {
            let (lat, lon) = graph.node_pos(n);
            [lon, lat]
        })
        .collect();
    let shore_dist: Vec<u8> = path
        .iter()
        .map(|&n| graph.shore_dist[n as usize])
        .collect();
    smooth_indices(&coords, &shore_dist, coastline, shore_buffer_nm)
        .into_iter()
        .map(|i| path[i])
        .collect()
}

/// Greedy line-of-sight smoothing over raw `[lon, lat]` coordinates.
///
/// Removes waypoints whose direct predecessor→successor line (a) doesn't
/// cross any coastline and (b) when `shore_buffer_nm > 0`, doesn't come
/// closer to the coastline than min(buffer, the input's own minimum
/// clearance over the skipped span, per `shore_dist`). Rule (b) means
/// smoothing never brings the route closer to shore than the raw input
/// already accepted — full buffer in open water, graceful degradation near
/// endpoints/in coves.
///
/// `shore_dist[i]` is the quantized (`SHORE_DIST_UNIT_NM` units) shore
/// distance of `coords[i]`; it must have the same length as `coords` and is
/// only consulted when `shore_buffer_nm > 0`. Returns strictly increasing
/// indices into `coords`, always keeping the first and last. When even the
/// next hop is blocked (e.g. a pin on land), the blocked segment is kept and
/// smoothing continues from that point.
pub fn smooth_indices(
    coords: &[[f64; 2]],
    shore_dist: &[u8],
    coastline: &CoastlineIndex,
    shore_buffer_nm: f64,
) -> Vec<usize> {
    if coords.len() <= 2 {
        return (0..coords.len()).collect();
    }
    let use_buffer = shore_buffer_nm > 0.0;
    debug_assert!(
        !use_buffer || shore_dist.len() == coords.len(),
        "shore_dist must be parallel to coords when a buffer is requested"
    );

    // Range-min over the whole input's shore_dist, built once per call (not
    // per anchor) so smoothing is O(n log n) overall instead of O(n^2).
    let range_min = if use_buffer {
        Some(RangeMin::build(shore_dist))
    } else {
        None
    };

    let mut result = vec![0];
    let mut current_idx = 0;
    let end_idx = coords.len() - 1;

    while current_idx < end_idx {
        let [c_lon, c_lat] = coords[current_idx];

        let clear = |j: usize| -> bool {
            let [t_lon, t_lat] = coords[j];
            if coastline.crosses_land(c_lon, c_lat, t_lon, t_lat) {
                return false;
            }
            if let Some(rm) = &range_min {
                let raw_min_nm =
                    rm.query(current_idx, j) as f64 * crate::graph::SHORE_DIST_UNIT_NM;
                let threshold = shore_buffer_nm.min(raw_min_nm);
                if threshold > 0.0
                    && coastline.segment_min_distance_nm(c_lon, c_lat, t_lon, t_lat, threshold)
                        < threshold
                {
                    return false;
                }
            }
            true
        };

        // Try direct line to destination
        if clear(end_idx) {
            result.push(end_idx);
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
            result.push(end_idx);
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

        // v_lo is the farthest visible point. Ensure we make progress even
        // when the very next hop is blocked (land pin case).
        if v_lo <= current_idx {
            v_lo = current_idx + 1;
        }
        result.push(v_lo);
        current_idx = v_lo;
    }

    result
}
```

The `RangeMin` struct stays where it is — `smooth_indices` uses it.

- [ ] **Step 4: Run the core test suite**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p asw-core`
Expected: all PASS — the 6 new `smooth_indices` tests AND the pre-existing `smooth_without_buffer_cuts_the_corner`, `smooth_respects_buffer`, `smooth_relaxes_near_endpoints` (they now exercise the wrapper and must be bit-identical).

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all
git add crates/asw-core/src/routing.rs
git commit -m "refactor(core): coordinate-based buffer-aware smoothing core (smooth_indices)

Claude-Session: https://claude.ai/code/session_01AMx13pMriCc6KubjyQJe1q"
```

---

### Task 2: `compute_route` — buffer-gated shortcut + endpoint stitching

**Files:**
- Modify: `crates/asw-core/src/routing.rs` (rewrite `compute_route`, add private `direct_line_ok`)
- Test: same file, `tests` module

**Interfaces:**
- Consumes: `smooth_indices` from Task 1; the Task 1 test helpers `wall_index(lon, lat_min, lat_max) -> CoastlineIndex` and `island_around_origin() -> CoastlineIndex`; `astar(graph, start, goal, buffers, Option<ShorePenalty>)`, `ShorePenalty::from_nm`, `haversine_nm(lat1, lon1, lat2, lon2)`, `RoutingGraph::node_pos(node) -> (lat, lon)`, `graph.shore_dist: Vec<u8>`, `CoastlineIndex::min_distance_nm(lon, lat, max_nm)`, `CoastlineIndex::segment_min_distance_nm(lon1, lat1, lon2, lat2, max_nm)`, `crate::graph::{quantize_shore_dist, SHORE_DIST_UNIT_NM}` — all existing.
- Produces: `compute_route` with an **unchanged signature** (callers in `asw-serve/src/api.rs` and `asw-cli/src/bench.rs` keep compiling untouched). Behavior contract for later tasks: `RouteResult.coordinates` always begins with `[from_lon, from_lat]` and ends with `[to_lon, to_lat]`; the shortcut path reports `raw_hops == 2, smooth_hops == 2`; with `shore_buffer_nm > 0` the shortcut only fires when the direct line keeps `min(buffer, both pins' own shore clearance)` from the coastline.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/asw-core/src/routing.rs`:

```rust
    /// Build a chain graph from (lat, lon) waypoints; returns the graph and
    /// node ids in input order. Nodes are inserted sorted by H3 index (the
    /// builder requires it for binary-search lookup), edges connect the
    /// waypoints in input order.
    fn chain_graph(coords: &[(f64, f64)]) -> (RoutingGraph, Vec<u32>) {
        let mut cells: Vec<(u64, f64, f64, usize)> = coords
            .iter()
            .enumerate()
            .map(|(i, &(lat, lon))| {
                let cell = h3o::LatLng::new(lat, lon)
                    .unwrap()
                    .to_cell(h3o::Resolution::Five);
                (u64::from(cell), lat, lon, i)
            })
            .collect();
        cells.sort_by_key(|(h3, _, _, _)| *h3);

        let mut b = GraphBuilder::new();
        let mut ids = vec![0u32; coords.len()];
        for (h3, lat, lon, orig) in &cells {
            ids[*orig] = b.add_node(*h3, *lat, *lon, 255);
        }
        for w in ids.windows(2) {
            b.add_edge(w[0], w[1], 60.0);
        }
        (b.build(), ids)
    }

    #[test]
    fn shortcut_returns_direct_route_when_line_is_clear() {
        let coastline = CoastlineIndex::new(vec![]);
        let g = GraphBuilder::new().build();
        // knn returning None proves the shortcut runs BEFORE snapping.
        let knn = |_: f64, _: f64| -> Option<(u32, f64)> { None };
        let mut buffers = crate::astar_pool::AstarBuffers::new(1);
        let r =
            compute_route(&g, 0.0, 0.0, 0.3, 0.2, &coastline, &knn, &mut buffers, 0.0).unwrap();
        assert_eq!(r.coordinates, vec![[0.0, 0.0], [0.2, 0.3]]);
        assert!((r.distance_nm - haversine_nm(0.0, 0.0, 0.3, 0.2)).abs() < 1e-9);
        assert_eq!(r.raw_hops, 2);
        assert_eq!(r.smooth_hops, 2);
    }

    #[test]
    fn shortcut_handles_identical_points() {
        let coastline = CoastlineIndex::new(vec![]);
        let g = GraphBuilder::new().build();
        let knn = |_: f64, _: f64| -> Option<(u32, f64)> { None };
        let mut buffers = crate::astar_pool::AstarBuffers::new(1);
        let r =
            compute_route(&g, 0.5, 0.5, 0.5, 0.5, &coastline, &knn, &mut buffers, 0.0).unwrap();
        assert!(r.distance_nm.abs() < 1e-9);
        assert_eq!(r.coordinates.len(), 2);
    }

    #[test]
    fn shortcut_respects_shore_buffer() {
        // Dogleg geometry: wall at lon 28.0 (lat 36.45..36.55); the direct
        // pin-to-pin line at lon 28.05 passes ~2.41 nm off the coast.
        let coastline = wall_index(28.0, 36.45, 36.55);
        let g = GraphBuilder::new().build();
        let knn = |_: f64, _: f64| -> Option<(u32, f64)> { None };
        let mut buffers = crate::astar_pool::AstarBuffers::new(1);

        // 2 nm buffer: 2.41 nm clearance suffices -> shortcut fires.
        let loose = compute_route(
            &g, 36.3, 28.05, 36.7, 28.05, &coastline, &knn, &mut buffers, 2.0,
        );
        assert!(loose.is_some_and(|r| r.coordinates.len() == 2));

        // 3 nm buffer: clearance violated -> shortcut declined, falls through
        // to snapping (knn None -> no route). Proves the gate.
        let strict = compute_route(
            &g, 36.3, 28.05, 36.7, 28.05, &coastline, &knn, &mut buffers, 3.0,
        );
        assert!(strict.is_none());
    }

    #[test]
    fn shortcut_relaxes_for_near_shore_pins() {
        // From-pin is only ~0.39 nm off the wall, so the threshold degrades
        // to min(3.0, pin clearance) and the direct line (whose closest
        // approach IS the from-pin) passes despite the 3 nm buffer.
        let coastline = wall_index(28.0, 36.45, 36.55);
        let g = GraphBuilder::new().build();
        let knn = |_: f64, _: f64| -> Option<(u32, f64)> { None };
        let mut buffers = crate::astar_pool::AstarBuffers::new(1);
        let r = compute_route(
            &g, 36.5, 28.008, 36.5, 28.4, &coastline, &knn, &mut buffers, 3.0,
        );
        assert!(
            r.is_some_and(|r| r.coordinates.len() == 2),
            "graceful degradation: pins closer than the buffer keep their own clearance"
        );
    }

    #[test]
    fn stitched_route_starts_and_ends_at_pins() {
        // Wall at lon 0.5 (lat -1..1). Chain S(0,0) -> M(1.5,0.5) -> G(0,1)
        // goes over the top of the wall. Pins are offset from S and G.
        let coastline = wall_index(0.5, -1.0, 1.0);
        let (g, ids) = chain_graph(&[(0.0, 0.0), (1.5, 0.5), (0.0, 1.0)]);
        let (s, goal) = (ids[0], ids[2]);
        let knn = move |_lat: f64, lon: f64| -> Option<(u32, f64)> {
            Some(if lon < 0.5 { (s, 0.0) } else { (goal, 0.0) })
        };
        let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let r = compute_route(&g, 0.0, -0.1, 0.0, 1.1, &coastline, &knn, &mut buffers, 0.0)
            .unwrap();

        assert_eq!(r.coordinates.first().unwrap(), &[-0.1, 0.0]);
        assert_eq!(r.coordinates.last().unwrap(), &[1.1, 0.0]);
        // The detour over the wall must be longer than the blocked direct line.
        assert!(r.distance_nm > haversine_nm(0.0, -0.1, 0.0, 1.1));
        // No segment of the returned polyline may cross the coastline
        // (endpoints here are on open water, so no clipping is expected).
        for w in r.coordinates.windows(2) {
            assert!(
                !coastline.crosses_land(w[0][0], w[0][1], w[1][0], w[1][1]),
                "smoothed segment {:?} -> {:?} crosses land",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn same_node_snap_yields_positive_distance() {
        // Both pins snap to the single node N above the wall; the direct
        // pin-to-pin line is blocked. Expect [from, N, to] with the exact
        // two-leg distance — this is the "0.00 NM inside one deep-ocean
        // hexagon" regression test.
        let coastline = wall_index(0.5, -1.0, 1.0);
        let (g, ids) = chain_graph(&[(1.5, 0.5)]);
        let n = ids[0];
        let knn = move |_: f64, _: f64| -> Option<(u32, f64)> { Some((n, 0.0)) };
        let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let r = compute_route(&g, 0.0, 0.4, 0.0, 0.6, &coastline, &knn, &mut buffers, 0.0)
            .unwrap();

        let expected = haversine_nm(0.0, 0.4, 1.5, 0.5) + haversine_nm(1.5, 0.5, 0.0, 0.6);
        assert!(
            (r.distance_nm - expected).abs() < 1e-6,
            "distance {} != two-leg sum {expected}",
            r.distance_nm
        );
        assert_eq!(r.coordinates.len(), 3);
        assert_eq!(r.coordinates[0], [0.4, 0.0]);
        assert_eq!(r.coordinates[2], [0.6, 0.0]);
    }

    #[test]
    fn land_pin_still_returns_route() {
        // From-pin sits inside a ring island at (0,0); to-pin is on open
        // water. Approved behavior: no error, route starts at the pin and
        // the first segment clips the island.
        let coastline = island_around_origin();
        let (g, ids) = chain_graph(&[(0.0, 0.5), (0.0, 2.0)]);
        let (s, goal) = (ids[0], ids[1]);
        let knn = move |_lat: f64, lon: f64| -> Option<(u32, f64)> {
            Some(if lon < 1.0 { (s, 0.0) } else { (goal, 0.0) })
        };
        let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let r = compute_route(&g, 0.0, 0.0, 0.0, 2.2, &coastline, &knn, &mut buffers, 0.0)
            .expect("land pin must still produce a route");
        assert_eq!(r.coordinates.first().unwrap(), &[0.0, 0.0]);
        assert_eq!(r.coordinates.last().unwrap(), &[2.2, 0.0]);
        assert!(r.distance_nm > 0.0);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p asw-core routing`
Expected: the 7 new tests FAIL (shortcut tests return `None` because `node_knn` returns `None`; stitched tests fail the first/last coordinate assertions because coordinates are node centers).

- [ ] **Step 3: Rewrite `compute_route`, add `direct_line_ok`**

Replace the entire `compute_route` function and add the private helper above it:

```rust
/// Can the straight line between the requested points be used as the route?
///
/// Requires (a) no coastline crossing and (b) when a shore buffer is
/// requested, that the line keeps min(buffer, both endpoints' own shore
/// clearance) from the coastline — the same graceful-degradation rule
/// `smooth_indices` applies, with the two pins as the whole "path". The
/// small epsilon absorbs float noise when the closest approach is exactly
/// an endpoint (its point clearance and the segment clearance are then the
/// same geometric quantity computed via different code paths).
fn direct_line_ok(
    coastline: &CoastlineIndex,
    from_lat: f64,
    from_lon: f64,
    to_lat: f64,
    to_lon: f64,
    shore_buffer_nm: f64,
) -> bool {
    if coastline.crosses_land(from_lon, from_lat, to_lon, to_lat) {
        return false;
    }
    if shore_buffer_nm <= 0.0 {
        return true;
    }
    let from_d = coastline.min_distance_nm(from_lon, from_lat, shore_buffer_nm);
    let to_d = coastline.min_distance_nm(to_lon, to_lat, shore_buffer_nm);
    let threshold = shore_buffer_nm.min(from_d).min(to_d);
    threshold <= 0.0
        || coastline.segment_min_distance_nm(from_lon, from_lat, to_lon, to_lat, threshold)
            >= threshold - 1e-9
}

/// Compute a full route: direct-line shortcut → snap → A* → stitch true
/// endpoints → smooth → build result.
#[allow(clippy::too_many_arguments)]
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
    // Direct-line shortcut: if the straight line between the requested
    // points is usable, no graph search is needed. This also covers points
    // that snap to the same deep-ocean node, which would otherwise produce
    // a single-point route with distance 0.
    if direct_line_ok(coastline, from_lat, from_lon, to_lat, to_lon, shore_buffer_nm) {
        return Some(RouteResult {
            distance_nm: haversine_nm(from_lat, from_lon, to_lat, to_lon),
            raw_hops: 2,
            smooth_hops: 2,
            coordinates: vec![[from_lon, from_lat], [to_lon, to_lat]],
        });
    }

    let (start, _) = node_knn(from_lat, from_lon)?;
    let (goal, _) = node_knn(to_lat, to_lon)?;

    let shore = ShorePenalty::from_nm(shore_buffer_nm);
    let (raw_path, _distance_nm) = astar(graph, start, goal, buffers, shore)?;
    let raw_hops = raw_path.len();

    // Stitch the true endpoints onto the node path so the route starts and
    // ends exactly at the requested coordinates. Smoothing from the pin
    // itself removes the dog-leg to the snapped node center (which can be
    // tens of nm away on res-3 deep-ocean cells). Pins get their own
    // quantized shore distance so buffer-aware smoothing treats them like
    // any near-shore waypoint (graceful degradation, not a hard failure).
    let pin_q = |lon: f64, lat: f64| -> u8 {
        if shore_buffer_nm > 0.0 {
            crate::graph::quantize_shore_dist(coastline.min_distance_nm(
                lon,
                lat,
                255.0 * crate::graph::SHORE_DIST_UNIT_NM,
            ))
        } else {
            u8::MAX
        }
    };

    let mut coords: Vec<[f64; 2]> = Vec::with_capacity(raw_path.len() + 2);
    let mut shore_dist: Vec<u8> = Vec::with_capacity(raw_path.len() + 2);
    coords.push([from_lon, from_lat]);
    shore_dist.push(pin_q(from_lon, from_lat));
    for &n in &raw_path {
        let (lat, lon) = graph.node_pos(n);
        coords.push([lon, lat]);
        shore_dist.push(graph.shore_dist[n as usize]);
    }
    coords.push([to_lon, to_lat]);
    shore_dist.push(pin_q(to_lon, to_lat));

    let kept = smooth_indices(&coords, &shore_dist, coastline, shore_buffer_nm);
    let smoothed: Vec<[f64; 2]> = kept.into_iter().map(|i| coords[i]).collect();
    let smooth_hops = smoothed.len();

    // Compute actual distance along smoothed path
    let mut smooth_dist = 0.0;
    for w in smoothed.windows(2) {
        smooth_dist += haversine_nm(w[0][1], w[0][0], w[1][1], w[1][0]);
    }

    Some(RouteResult {
        distance_nm: smooth_dist,
        raw_hops,
        smooth_hops,
        coordinates: smoothed,
    })
}
```

- [ ] **Step 4: Run the full core test suite**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p asw-core`
Expected: all tests PASS (7 new + all existing astar/shore-penalty/smooth tests).

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all
git add crates/asw-core/src/routing.rs
git commit -m "feat(core): true-endpoint routing — buffer-gated shortcut and pin stitching

Routes now start and end exactly at the requested coordinates. Clear
open-water pairs return a direct 2-point route without a graph search
(gated on shore clearance when a buffer is requested); fixes floating
polylines and 0.00 nm same-cell routes in deep water.

Claude-Session: https://claude.ai/code/session_01AMx13pMriCc6KubjyQJe1q"
```

---

### Task 3: `asw-serve` integration tests

**Files:**
- Modify: `crates/asw-serve/src/api.rs` (tests module only — no production code changes)

**Interfaces:**
- Consumes: `compute_route` behavior contract from Task 2 (geometry endpoints == requested pins; shortcut when no coastline blocks); `GraphBuilder.coastline_coords: Vec<Vec<(f32, f32)>>` public field (coords are `(lon, lat)` pairs, consumed by `AppState::new` via `CoastlineIndex::from_serialized`); existing test helpers `mark_ready`, `ready_state_with_small_graph`, `get_route`.
- Produces: nothing consumed later; final test coverage at the HTTP layer.

- [ ] **Step 1: Parameterize the ready-state test helper**

In `crates/asw-serve/src/api.rs` tests, extract the body of `ready_state_with_small_graph` into a coastline-accepting variant; the original delegates (keep its doc comment):

```rust
    /// Like `ready_state_with_small_graph`, but with an injected coastline
    /// (lon, lat) polyline so tests can block specific lines of sight.
    async fn ready_state_with_graph(coastline: Vec<Vec<(f32, f32)>>) -> Arc<ServerState> {
        use crate::state::AppState;
        use asw_core::graph::GraphBuilder;

        let coords = [(36.848, 28.268), (36.9, 28.3), (37.0, 28.5)];
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
        entries.dedup_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::new();
        b.coastline_coords = coastline;
        let mut ids = Vec::new();
        for &(h3, lat, lng) in &entries {
            ids.push(b.add_node(h3, lat, lng, 255));
        }
        for i in 0..ids.len().saturating_sub(1) {
            b.add_edge(ids[i], ids[i + 1], 1.0);
        }
        let graph = b.build();

        let state = Arc::new(ServerState::new(
            "test.graph".into(),
            "secret-key-1234567890".into(),
        ));
        mark_ready(&state, AppState::new(graph)).await;
        state
    }

    async fn ready_state_with_small_graph() -> Arc<ServerState> {
        ready_state_with_graph(Vec::new()).await
    }
```

- [ ] **Step 2: Fix the 404 test (it would now hit the shortcut)**

`route_returns_404_when_no_route_found_once_ready` uses an **empty graph with an empty coastline** — after Task 2 the direct-line shortcut turns that into a 200. Keep the test's intent (no snappable node ⇒ 404) by blocking the direct line with a coastline wall between the two query points. Replace the graph construction inside the test with:

```rust
        let mut b = GraphBuilder::new();
        // Wall crossing the from->to line (lon 28.4, lat 36.0..37.5), so the
        // direct-line shortcut cannot answer; with no nodes to snap to, the
        // route must still surface as 404.
        b.coastline_coords = vec![vec![(28.4, 36.0), (28.4, 37.5)]];
        mark_ready(&state, AppState::new(b.build())).await;
```

(and update the test's doc comment accordingly; the imports `AppState`/`GraphBuilder` are already inside the test).

- [ ] **Step 3: Add the new integration tests**

```rust
    /// Deep-water fix, shortcut path: two points far from any graph node
    /// with a clear line of sight return the two requested points exactly
    /// and a positive distance (was: polyline floating at distant node
    /// centers, or 0.00 nm for same-cell pairs).
    #[tokio::test]
    async fn route_open_water_returns_requested_points_exactly() {
        let app = create_router(ready_state_with_small_graph().await);
        let req = Request::get("/route?from=36.5,28.0&to=36.6,28.1")
            .header("X-Api-Key", "secret-key-1234567890")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["distance_nm"].as_f64().unwrap() > 0.0);
        let coords = json["geometry"]["coordinates"].as_array().unwrap();
        assert_eq!(coords.first().unwrap(), &serde_json::json!([28.0, 36.5]));
        assert_eq!(coords.last().unwrap(), &serde_json::json!([28.1, 36.6]));
    }

    /// Deep-water fix, stitched path: a wall blocks the direct line, so the
    /// route goes through the graph — but the geometry must still start and
    /// end exactly at the requested coordinates, not at node centers.
    #[tokio::test]
    async fn route_geometry_starts_and_ends_at_requested_points() {
        // Wall at lon 28.35 (lat 36.88..36.92): blocks pin->pin and
        // pin->node3, but not node2->pin_to — forcing a stitched route.
        let state = ready_state_with_graph(vec![vec![(28.35, 36.88), (28.35, 36.92)]]).await;
        let app = create_router(state);
        let req = Request::get("/route?from=36.84,28.26&to=37.01,28.51")
            .header("X-Api-Key", "secret-key-1234567890")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["distance_nm"].as_f64().unwrap() > 0.0);
        let coords = json["geometry"]["coordinates"].as_array().unwrap();
        assert_eq!(coords.first().unwrap(), &serde_json::json!([28.26, 36.84]));
        assert_eq!(coords.last().unwrap(), &serde_json::json!([28.51, 37.01]));
        assert!(coords.len() >= 3, "expected a stitched route, got {coords:?}");
    }
```

- [ ] **Step 4: Run the serve test suite**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p asw-serve`
Expected: all tests PASS. Pre-existing tests worth watching (they now take the shortcut path since test graphs carry no coastline; their assertions still hold):
- `route_returns_200_with_valid_route_once_ready` (distance > 0, `raw_hops >= 1`, LineString)
- `concurrent_route_requests_beyond_pool_capacity_all_succeed`
- `shore_buffer_echoed_in_response` / `shore_buffer_defaults_to_zero` (via `get_route`; with an empty coastline, `min_distance_nm` caps at the buffer so `direct_line_ok` passes and the response echoes the buffer)

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all
git add crates/asw-serve/src/api.rs
git commit -m "test(serve): cover true-endpoint routing at the HTTP layer

Claude-Session: https://claude.ai/code/session_01AMx13pMriCc6KubjyQJe1q"
```

---

### Task 4: Docs, workspace-wide checks, benchmarks

**Files:**
- Modify: `CHANGELOG.md` (Unreleased section)
- Modify: `CLAUDE.md` (Key Design Decisions list)
- Modify: `README.md` (only if it describes route/snapping behavior — audit step below)

**Interfaces:**
- Consumes: behavior shipped in Tasks 1–3.
- Produces: release notes; no code.

- [ ] **Step 1: Update CHANGELOG.md**

Under `## [Unreleased]` add:

```markdown
### Fixed

- Deep-water routes: geometry now starts and ends exactly at the requested coordinates instead of at snapped node centers (on res-3 ocean cells the nearest node can be tens of nm away, leaving the polyline visibly detached from the route markers); two points inside the same cell no longer return a single-point 0.00 nm route

### Changed

- Direct-line shortcut: when the straight line between the requested points does not cross land — and keeps the requested `shore_buffer` clearance, degraded to the endpoints' own shore distance when they start closer — `/route` returns a 2-point great-circle route without a graph search (faster for open-water queries)
- A pin on land (or blocked from its snapped node) still returns a route: the first/last segment keeps the direct connection to the graph (small shoreline clip) instead of erroring
- `asw_core::routing::smooth` is now a thin wrapper over the new coordinate-based `smooth_indices` (same algorithm, same buffer semantics)
```

- [ ] **Step 2: Update CLAUDE.md Key Design Decisions**

Add one bullet to the existing list:

```markdown
- Query-time endpoint stitching: routes start/end at the exact requested coordinates; clear line-of-sight pairs short-circuit to a direct great-circle leg without a graph search (shore-buffer aware; no graph densification needed for deep water)
```

- [ ] **Step 3: Audit README.md**

Run: `grep -n -i "snap\|route\|smooth\|shore" README.md`
Check every hit against the new behavior. If any sentence says routes are built "between nearest graph nodes" or similar, rewrite it to say routes start/end at the requested coordinates. If nothing is stale, make no edit.

- [ ] **Step 4: Workspace-wide verification**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: all clean, all tests PASS (this also compiles `asw-cli`'s `bench.rs` against the unchanged `compute_route` signature).

- [ ] **Step 5: Local benchmark (if a local graph exists)**

If `export/asw.graph` (or another local graph file) exists:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release -p asw-cli
./target/release/asw bench --graph export/asw.graph
```

Compare against the previous local benchmark results. Expected: open-water routes faster (shortcut skips A*); no regression on coastal routes (one extra `crosses_land` call per query is noise). If no local graph file exists, note that in the final report instead of running.

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md CLAUDE.md README.md
git commit -m "docs: changelog and design notes for true-endpoint routing

Claude-Session: https://claude.ai/code/session_01AMx13pMriCc6KubjyQJe1q"
```

---

## Completion

After all tasks: use superpowers:verification-before-completion, then superpowers:finishing-a-development-branch. The user prefers a PR to `main` (not a direct merge). PR body must end with:
`https://claude.ai/code/session_01AMx13pMriCc6KubjyQJe1q`
