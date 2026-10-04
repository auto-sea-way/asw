//! Nearest navigable node: two-pass adaptive k-ring search over the sorted
//! H3 node array. Shared by the server, the bench and `is_water`.

use crate::graph::RoutingGraph;

impl RoutingGraph {
    /// Approximate H3 edge length in nautical miles, indexed by resolution (3..=14).
    /// Used for early-termination: skip this resolution if current best is already
    /// closer than the cell edge length.
    const H3_EDGE_NM: [f64; 15] = [
        0.0, 0.0, 0.0,    // res 0-2: unused
        35.0,   // res 3
        13.0,   // res 4
        5.0,    // res 5
        1.9,    // res 6
        0.7,    // res 7
        0.27,   // res 8
        0.10,   // res 9
        0.038,  // res 10
        0.014,  // res 11
        0.005,  // res 12
        0.002,  // res 13
        0.0007, // res 14
    ];

    /// Maximum k-ring expansion per resolution tier.
    fn k_max(res: u8) -> u32 {
        match res {
            9..=14 => 30,
            6..=8 => 20,
            3..=5 => 15,
            _ => 3,
        }
    }

    /// First eager-disk bound for `search_resolution`'s geometric doubling.
    const DISK_DOUBLING_START: u32 = 4;

    /// Search a single resolution with k-ring up to `k_max`, updating `best`.
    ///
    /// Geometric-doubling implementation: `grid_disk_distances` is EAGER — it
    /// materializes the whole disk out to its bound before yielding anything,
    /// so a single call at `k_max` pays for the full disk even when the
    /// nearest node sits at small k (the res-3 fallback's k_max=50 disk is
    /// ~7,651 cells; a k≈12 hit needs only ~469). Instead, the eager call is
    /// issued with a growing bound (4, 8, 16, 32, ..., capped at `k_max`);
    /// each step processes only the cells beyond the previous bound, and the
    /// search stops requesting larger disks as soon as a step yields a hit.
    /// Since disk size is quadratic in k, a hit just past a step bound can
    /// cost up to ~5x the minimal disk, but the worst (no-match) case is
    /// bounded at ~1.56x a single full-k_max eager call.
    ///
    /// A per-ring `grid_ring(k)` loop was tried first and abandoned: its
    /// pentagon-safe fallback re-runs a full O(k²) BFS per ring, and at res-3
    /// k_max=50 the disk covers a large fraction of all ~41k res-3 cells, so
    /// most rings hit the fallback — measured 13-18x slower than one eager
    /// call on exhaustive (no-match) searches. The eager
    /// `grid_disk_distances` API handles pentagon distortion once per call
    /// (fast path first, one safe BFS on failure), keeping the worst case at
    /// ~1.56x the single eager call.
    ///
    /// Both the `_fast` and `_safe` backing iterators yield cells grouped by
    /// ascending grid distance (BFS order), so within each step cells arrive
    /// ring 0, ring 1, ... — the grouping logic below relies on this.
    ///
    /// Early-return semantics are identical to the old per-k code: as soon as
    /// a fully-scanned k-level contains at least one match,
    /// `best` (updated to the closest such match) is final for this call and
    /// we stop — larger k are never examined, and larger disks are never
    /// requested. A k-level is never split across steps (each step's bound is
    /// a whole ring count), so "finish the whole current k before returning"
    /// holds at doubling boundaries too.
    fn search_resolution(
        &self,
        ll: &h3o::LatLng,
        lat: f64,
        lon: f64,
        res_u8: u8,
        k_max: u32,
        best: &mut Option<(u32, f64)>,
    ) {
        let res = match h3o::Resolution::try_from(res_u8) {
            Ok(r) => r,
            Err(_) => return,
        };
        let cell = ll.to_cell(res);

        // Rings 0..=processed_up_to were fully scanned in previous steps.
        let mut processed_up_to: Option<u32> = None;
        let mut bound = Self::DISK_DOUBLING_START.min(k_max);
        loop {
            let disk: Vec<(h3o::CellIndex, u32)> = cell.grid_disk_distances(bound);
            let first_new_k = processed_up_to.map_or(0, |p| p + 1);

            let mut current_k = first_new_k;
            let mut found_at_current_k = false;
            for (neighbor, k) in disk {
                if k < first_new_k {
                    continue; // Already scanned in a previous, smaller step.
                }
                if k != current_k {
                    // Ring `current_k` is fully scanned — stop here if it had
                    // a match, matching the old per-k early return.
                    if found_at_current_k {
                        return;
                    }
                    current_k = k;
                    found_at_current_k = false;
                }

                if let Some(node_id) = self.h3_lookup(u64::from(neighbor)) {
                    found_at_current_k = true;
                    let (nlat, nlon) = self.node_pos(node_id);
                    let dist = crate::h3::haversine_nm(lat, lon, nlat, nlon);
                    if best.is_none_or(|(_, d)| dist < d) {
                        *best = Some((node_id, dist));
                    }
                }
            }
            // The step's last ring (k == bound) is now fully scanned; if it
            // had a match, stop without requesting a larger disk.
            if found_at_current_k {
                return;
            }

            if bound >= k_max {
                return; // Entire k_max disk scanned, no early exit triggered.
            }
            processed_up_to = Some(bound);
            bound = bound.saturating_mul(2).min(k_max);
        }
    }

    /// Find the nearest node via two-pass adaptive k-ring expansion. The graph
    /// is pruned to one connected component at build time, so any node is
    /// routable.
    ///
    /// Two-pass approach:
    /// - Pass 1 (fast): k=3 at each resolution, fine→coarse. Handles 99% of queries.
    /// - Pass 2 (refine): adaptive k proportional to pass-1 distance, fine→coarse.
    ///   Only does work when pass 1 found a distant candidate that finer resolutions
    ///   could beat with larger k. Skips via early termination when pass 1 was close.
    pub fn nearest_node(&self, lat: f64, lon: f64) -> Option<(u32, f64)> {
        let ll = h3o::LatLng::new(lat, lon).ok()?;
        let mut best: Option<(u32, f64)> = None;

        // Pass 1: fast scan with small k
        for res_u8 in (3..=14).rev() {
            let edge_nm = Self::H3_EDGE_NM[res_u8 as usize];
            if let Some((_, d)) = best {
                if d < edge_nm * 0.4 {
                    continue;
                }
            }
            self.search_resolution(&ll, lat, lon, res_u8, 3, &mut best);
        }

        // Pass 2: adaptive k at common resolutions (3-9). Passage corridors (10-14)
        // have tiny cells where d/edge explodes — they're covered by pass 1's k=3.
        // Skip entirely if pass 1 found a node within 0.5nm (excellent snap).
        let needs_pass2 = match best {
            Some((_, d)) => d >= 0.5,
            None => true,
        };
        if needs_pass2 {
            for res_u8 in (3..=9).rev() {
                let edge_nm = Self::H3_EDGE_NM[res_u8 as usize];
                if let Some((_, d)) = best {
                    if d < edge_nm * 0.4 {
                        continue;
                    }
                }
                let k_limit = if let Some((_, d)) = best {
                    let k = ((d / edge_nm) as u32 + 2).min(Self::k_max(res_u8));
                    if k <= 3 {
                        continue; // Already covered by pass 1
                    }
                    k
                } else {
                    Self::k_max(res_u8) // No candidate yet: full search
                };
                self.search_resolution(&ll, lat, lon, res_u8, k_limit, &mut best);
            }
        }

        if best.is_some() {
            return best;
        }

        // Exhaustive fallback: search res-3 with large k (covers most of the planet).
        // This only finds nodes indexed at res-3 (deep ocean cells). Production graphs
        // always contain res-3 nodes; test graphs may not.
        //
        // `k_max=50` here is an upper bound, not the typical cost:
        // `search_resolution`'s geometric doubling stops requesting larger disks
        // once a step yields a match (typically k≈12 in practice), so this
        // usually walks a fraction of the full 7,651-cell k=50 disk (up to ~5x
        // the minimal needed disk when a hit lands just past a step bound).
        self.search_resolution(&ll, lat, lon, 3, 50, &mut best);

        best
    }
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphBuilder;

    /// Test helper: build a graph from H3 indices — sorted and deduplicated,
    /// every node shore_dist=255, all nodes chained with unit edges.
    fn chain_graph(h3s: &[u64]) -> RoutingGraph {
        let mut h3s = h3s.to_vec();
        h3s.sort_unstable();
        h3s.dedup();
        let mut b = GraphBuilder::default();
        let mut ids = Vec::new();
        for &h3 in &h3s {
            ids.push(b.add_node(h3, 255));
        }
        for i in 0..ids.len().saturating_sub(1) {
            b.add_edge(ids[i], ids[i + 1]);
        }
        b.build()
    }

    /// Build a small test graph with nodes sorted by H3 index.
    fn test_graph(cells: &[(f64, f64)]) -> RoutingGraph {
        let h3s: Vec<u64> = cells
            .iter()
            .map(|&(lat, lng)| {
                u64::from(
                    h3o::LatLng::new(lat, lng)
                        .unwrap()
                        .to_cell(h3o::Resolution::Five),
                )
            })
            .collect();
        chain_graph(&h3s)
    }

    #[test]
    fn exact_cell_match_finds_node() {
        let graph = test_graph(&[(36.848, 28.268), (37.0, 28.5), (36.5, 28.0)]);

        // Query at the exact position of the first node — should snap to it
        let result = graph.nearest_node(36.848, 28.268);
        assert!(result.is_some(), "should find a node near (36.848, 28.268)");
        let (_, dist) = result.unwrap();
        // Distance should be very small (cell center offset)
        assert!(dist < 5.0, "distance {} nm should be < 5 nm", dist);
    }

    #[test]
    fn nearby_offset_snaps_to_closest() {
        let graph = test_graph(&[(36.848, 28.268), (37.0, 28.5), (36.5, 28.0)]);

        // Query slightly offset from first node — should still find a node
        let result = graph.nearest_node(36.85, 28.27);
        assert!(result.is_some(), "should find a node near offset position");
        let (_, dist) = result.unwrap();
        assert!(dist < 10.0, "distance {} nm should be < 10 nm", dist);
    }

    #[test]
    fn empty_graph_returns_none() {
        let b = GraphBuilder::default();
        let graph = b.build();

        let result = graph.nearest_node(36.848, 28.268);
        assert!(result.is_none(), "empty graph should return None");
    }

    /// Build a graph with nodes at two different resolutions.
    /// A res-9 node near the query and a res-5 node farther away.
    fn graph_multi_resolution() -> RoutingGraph {
        // Res-5 node far from query point (1 degree away)
        let far_pos = (37.0, 29.0);
        let far_cell = h3o::LatLng::new(far_pos.0, far_pos.1)
            .unwrap()
            .to_cell(h3o::Resolution::Five);

        // Res-9 node close to query point (0.05 degrees away)
        let near_pos = (36.05, 28.05);
        let near_cell = h3o::LatLng::new(near_pos.0, near_pos.1)
            .unwrap()
            .to_cell(h3o::Resolution::Nine);

        chain_graph(&[u64::from(far_cell), u64::from(near_cell)])
    }

    #[test]
    fn prefers_closer_node_across_resolutions() {
        let graph = graph_multi_resolution();

        // Query near the res-9 node
        let query = (36.0, 28.0);
        let result = graph.nearest_node(query.0, query.1);
        assert!(result.is_some(), "should find a node");
        let (_, dist) = result.unwrap();
        // Should snap to the nearby res-9 node (~3 nm away), not the far res-5 node (~60 nm away)
        assert!(
            dist < 10.0,
            "distance {} nm — should snap to nearby res-9 node, not far res-5 node",
            dist
        );
    }

    #[test]
    fn remote_query_finds_distant_node() {
        // Single res-3 ocean node at (40.0, 20.0)
        let ocean_pos = (40.0, 20.0);
        let ocean_cell = h3o::LatLng::new(ocean_pos.0, ocean_pos.1)
            .unwrap()
            .to_cell(h3o::Resolution::Three);

        let mut b = GraphBuilder::default();
        b.add_node(u64::from(ocean_cell), 255);
        let graph = b.build();

        // Query 5 degrees away (~300 nm) — simulates a remote island
        let result = graph.nearest_node(42.0, 16.0);
        assert!(result.is_some(), "should find a node even 300nm away");
        let (_, dist) = result.unwrap();
        assert!(dist < 500.0, "distance {} nm should be < 500 nm", dist);
    }

    #[test]
    fn deep_inland_finds_ocean_node() {
        // Single res-3 ocean node in the Mediterranean
        let ocean_pos = (36.0, 18.0);
        let ocean_cell = h3o::LatLng::new(ocean_pos.0, ocean_pos.1)
            .unwrap()
            .to_cell(h3o::Resolution::Three);

        let mut b = GraphBuilder::default();
        b.add_node(u64::from(ocean_cell), 255);
        let graph = b.build();

        // Query from deep inland (Belgrade, Serbia — ~400nm from Mediterranean)
        let result = graph.nearest_node(44.8, 20.5);
        assert!(result.is_some(), "should find ocean node from inland point");
    }

    /// Regression test for the single-scan `search_resolution` rewrite
    /// (finding 12): when a k-level contains more than one main-component
    /// match, `nearest_node` must return the *closest* of them, not merely
    /// the first one encountered while walking the single sorted
    /// `grid_disk_distances` scan.
    #[test]
    fn same_ring_picks_closest_of_multiple_candidates() {
        let origin_pos = (10.0, 20.0);
        let origin_cell = h3o::LatLng::new(origin_pos.0, origin_pos.1)
            .unwrap()
            .to_cell(h3o::Resolution::Five);

        // Two distinct ring-1 neighbors of the (unindexed) origin cell — the
        // origin itself is never added as a node, so ring 0 is empty and the
        // search must resolve the tie within ring 1.
        let ring1: Vec<h3o::CellIndex> = origin_cell.grid_ring(1);
        assert!(ring1.len() >= 2, "expected at least two ring-1 neighbors");
        let a = ring1[0];
        let b = ring1[1];
        let a_ll = h3o::LatLng::from(a);
        let b_ll = h3o::LatLng::from(b);

        let graph = chain_graph(&[u64::from(a), u64::from(b)]);

        let result = graph.nearest_node(origin_pos.0, origin_pos.1);
        assert!(result.is_some(), "should find a ring-1 candidate");
        let (_, dist) = result.unwrap();

        let dist_a = crate::h3::haversine_nm(origin_pos.0, origin_pos.1, a_ll.lat(), a_ll.lng());
        let dist_b = crate::h3::haversine_nm(origin_pos.0, origin_pos.1, b_ll.lat(), b_ll.lng());
        let expected = dist_a.min(dist_b);

        assert!(
            (dist - expected).abs() < 1e-9,
            "expected closest candidate at {expected} nm, got {dist} nm"
        );
    }

    /// Regression test for the incremental-k rewrite of `search_resolution`:
    /// a near match at ring 2 (inside the first doubling step, bound=4) must
    /// win over a farther main-component match at ring 5 (inside the second
    /// step), and the search must stop after ring 2's k-level is fully
    /// scanned — i.e. the ring-5 node's mere existence in the graph must not
    /// change the result. This exercises the "stop at the first k with a hit"
    /// contract across a doubling-step boundary.
    #[test]
    fn nearer_ring_wins_over_farther_ring_and_stops_early() {
        let origin_pos = (10.0, 20.0);
        let origin_cell = h3o::LatLng::new(origin_pos.0, origin_pos.1)
            .unwrap()
            .to_cell(h3o::Resolution::Five);

        let ring2: Vec<h3o::CellIndex> = origin_cell.grid_ring(2);
        let ring5: Vec<h3o::CellIndex> = origin_cell.grid_ring(5);
        let near = ring2[0];
        let far = ring5[0];
        let near_ll = h3o::LatLng::from(near);
        let far_ll = h3o::LatLng::from(far);

        let graph = chain_graph(&[u64::from(near), u64::from(far)]);

        let result = graph.nearest_node(origin_pos.0, origin_pos.1);
        assert!(result.is_some(), "should find the ring-2 candidate");
        let (_, dist) = result.unwrap();

        let dist_near =
            crate::h3::haversine_nm(origin_pos.0, origin_pos.1, near_ll.lat(), near_ll.lng());
        let dist_far =
            crate::h3::haversine_nm(origin_pos.0, origin_pos.1, far_ll.lat(), far_ll.lng());
        assert!(
            dist_near < dist_far,
            "test setup: ring-2 must be closer than ring-5"
        );

        assert!(
            (dist - dist_near).abs() < 1e-9,
            "expected nearer ring-2 candidate at {dist_near} nm, got {dist} nm (far candidate at {dist_far} nm must not win)"
        );
    }

    /// Doubling-boundary regression test: a match sitting EXACTLY at a
    /// doubling step's bound (k=8, the last ring of the 4→8 step) must be
    /// found and must win over a match at k=9 (the first ring of the next
    /// step, 8→16). This pins down two off-by-one hazards at the seam:
    /// a step skipping its own boundary ring (k=8 dropped by both the
    /// bound=8 and bound=16 steps would lose the node entirely), and a step
    /// returning early before its boundary ring is fully scanned (the k=9
    /// node must never be examined once k=8 has a hit).
    #[test]
    fn hit_exactly_at_doubling_boundary_k8_wins_over_k9() {
        // Sanity-check the boundary assumption this test encodes: with the
        // doubling schedule 4, 8, 16, ... k=8 is the last ring of step 2.
        assert_eq!(RoutingGraph::DISK_DOUBLING_START, 4);

        let origin_pos = (10.0, 20.0);
        let origin_cell = h3o::LatLng::new(origin_pos.0, origin_pos.1)
            .unwrap()
            .to_cell(h3o::Resolution::Five);

        let ring8: Vec<h3o::CellIndex> = origin_cell.grid_ring(8);
        let ring9: Vec<h3o::CellIndex> = origin_cell.grid_ring(9);
        let boundary = ring8[0];
        let beyond = ring9[0];
        let boundary_ll = h3o::LatLng::from(boundary);
        let beyond_ll = h3o::LatLng::from(beyond);

        let graph = chain_graph(&[u64::from(boundary), u64::from(beyond)]);

        let result = graph.nearest_node(origin_pos.0, origin_pos.1);
        assert!(
            result.is_some(),
            "should find the ring-8 boundary candidate"
        );
        let (_, dist) = result.unwrap();

        let dist_boundary = crate::h3::haversine_nm(
            origin_pos.0,
            origin_pos.1,
            boundary_ll.lat(),
            boundary_ll.lng(),
        );
        let dist_beyond =
            crate::h3::haversine_nm(origin_pos.0, origin_pos.1, beyond_ll.lat(), beyond_ll.lng());
        assert!(
            dist_boundary < dist_beyond,
            "test setup: ring-8 must be closer than ring-9"
        );

        assert!(
            (dist - dist_boundary).abs() < 1e-9,
            "expected boundary ring-8 candidate at {dist_boundary} nm, got {dist} nm (ring-9 candidate at {dist_beyond} nm must not win)"
        );
    }

    #[test]
    fn nearest_node_finds_res14_cells() {
        // A res-14 cell in a narrow channel and a res-10 cell at its mouth.
        let fine = h3o::LatLng::new(44.6929, 14.3921)
            .unwrap()
            .to_cell(h3o::Resolution::Fourteen);
        let coarse = h3o::LatLng::new(44.6950, 14.3890)
            .unwrap()
            .to_cell(h3o::Resolution::Ten);
        let g = chain_graph(&[u64::from(fine), u64::from(coarse)]);
        let (node, d) = g.nearest_node(44.6929, 14.3921).unwrap();
        assert_eq!(g.node_h3(node), u64::from(fine));
        assert!(d < 0.002, "{d}");
    }
}
