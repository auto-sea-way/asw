use asw_core::geo_index::CoastlineIndex;
use asw_core::graph::RoutingGraph;

/// Wrapper that tracks readiness — the HTTP server starts before the graph is loaded.
pub struct ServerState {
    pub inner: tokio::sync::RwLock<Option<AppState>>,
    pub graph_path: String,
    api_key: String,
}

impl ServerState {
    pub fn new(graph_path: String, api_key: String) -> Self {
        assert!(
            !api_key.trim().is_empty(),
            "API key must not be empty or whitespace-only"
        );
        Self {
            inner: tokio::sync::RwLock::new(None),
            graph_path,
            api_key,
        }
    }

    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn set_ready(&self, app: AppState) {
        // Use blocking_lock since this is called from spawn_blocking
        *self.inner.blocking_write() = Some(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_state_stores_api_key() {
        let state = ServerState::new("test.graph".into(), "test-key-1234".into());
        assert_eq!(state.api_key(), "test-key-1234");
        assert_eq!(state.graph_path, "test.graph");
    }
}

/// Shared application state for the HTTP server.
pub struct AppState {
    pub graph: RoutingGraph,
    pub coastline: CoastlineIndex,
    /// Component root for each node; nodes in the main component share `main_component`.
    component_labels: Vec<u32>,
    main_component: u32,
    /// Pre-allocated A* search buffer pool (2 buffer sets for concurrent requests).
    astar_pool: asw_core::astar_pool::AstarPool,
}

impl AppState {
    /// Build AppState from a RoutingGraph.
    ///
    /// Initialization is sequenced to minimize peak memory: each heavy
    /// allocation is completed (and its temporaries dropped) before the
    /// next one begins.
    pub fn new(mut graph: RoutingGraph) -> Self {
        // Step 1: Build coastline R-tree, then free the raw coords from the graph.
        let coastline = CoastlineIndex::from_serialized(&graph.coastline_coords);
        graph.drop_coastline_coords();

        // Step 2: Connected components (u32 parent vec = 160 MB for 40M nodes).
        let component_labels = graph.component_labels();
        let main_component = {
            let mut comp_sizes = std::collections::HashMap::new();
            for &root in &component_labels {
                *comp_sizes.entry(root).or_insert(0usize) += 1;
            }
            comp_sizes
                .into_iter()
                .max_by_key(|&(_, size)| size)
                .map(|(root, _)| root)
                .unwrap_or(0)
        };

        let astar_pool = asw_core::astar_pool::AstarPool::new(graph.num_nodes as usize, 2);

        Self {
            graph,
            coastline,
            component_labels,
            main_component,
            astar_pool,
        }
    }

    /// Approximate H3 edge length in nautical miles, indexed by resolution (3..=13).
    /// Used for early-termination: skip coarser resolutions when current best is already
    /// closer than one cell edge.
    const H3_EDGE_NM: [f64; 14] = [
        0.0, 0.0, 0.0,   // res 0-2: unused
        35.0,  // res 3
        13.0,  // res 4
        5.0,   // res 5
        1.9,   // res 6
        0.7,   // res 7
        0.27,  // res 8
        0.10,  // res 9
        0.038, // res 10
        0.014, // res 11
        0.005, // res 12
        0.002, // res 13
    ];

    /// Maximum k-ring expansion per resolution tier.
    fn k_max(res: u8) -> u32 {
        match res {
            9..=13 => 30,
            6..=8 => 20,
            3..=5 => 15,
            _ => 3,
        }
    }

    /// Search a single resolution with k-ring up to `k_max`, updating `best`.
    /// Returns true if any main-component node was found at this resolution.
    fn search_resolution(
        &self,
        ll: &h3o::LatLng,
        lat: f64,
        lon: f64,
        res_u8: u8,
        k_max: u32,
        best: &mut Option<(u32, f64)>,
    ) -> bool {
        let res = match h3o::Resolution::try_from(res_u8) {
            Ok(r) => r,
            Err(_) => return false,
        };
        let cell = ll.to_cell(res);
        for k in 0..=k_max {
            let mut found_at_k = false;
            for neighbor in cell.grid_disk::<Vec<_>>(k) {
                let nh3 = u64::from(neighbor);
                if let Some(node_id) = self.h3_lookup(nh3) {
                    if self.component_labels[node_id as usize] == self.main_component {
                        let (nlat, nlon) = self.graph.node_pos(node_id);
                        let dist = asw_core::h3::haversine_nm(lat, lon, nlat, nlon);
                        if best.is_none_or(|(_, d)| dist < d) {
                            *best = Some((node_id, dist));
                            found_at_k = true;
                        }
                    }
                }
            }
            if found_at_k {
                return true;
            }
        }
        false
    }

    /// Find nearest node in the main connected component using H3 binary search.
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
        for res_u8 in (3..=13).rev() {
            let edge_nm = Self::H3_EDGE_NM[res_u8 as usize];
            if let Some((_, d)) = best {
                if d < edge_nm * 0.4 {
                    continue;
                }
            }
            self.search_resolution(&ll, lat, lon, res_u8, 3, &mut best);
        }

        // Pass 2: adaptive k at common resolutions (3-9). Passage corridors (10-13)
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
        let res3 = h3o::Resolution::try_from(3u8).ok()?;
        let cell3 = ll.to_cell(res3);
        for k in 0..=50u32 {
            for neighbor in cell3.grid_disk::<Vec<_>>(k) {
                let nh3 = u64::from(neighbor);
                if let Some(node_id) = self.h3_lookup(nh3) {
                    if self.component_labels[node_id as usize] == self.main_component {
                        let (nlat, nlon) = self.graph.node_pos(node_id);
                        let dist = asw_core::h3::haversine_nm(lat, lon, nlat, nlon);
                        if best.is_none_or(|(_, d)| dist < d) {
                            best = Some((node_id, dist));
                        }
                    }
                }
            }
            if best.is_some() {
                return best;
            }
        }

        None
    }

    /// Acquire a buffer set from the pool, run `f`, then release the buffer.
    pub async fn with_astar_buffers<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut asw_core::astar_pool::AstarBuffers) -> T,
    {
        let mut buf = self.astar_pool.acquire();
        let result = f(&mut buf);
        self.astar_pool.release(buf);
        result
    }

    /// Binary search for an H3 cell index in the sorted `node_h3` array.
    fn h3_lookup(&self, h3: u64) -> Option<u32> {
        self.graph.node_h3.binary_search(&h3).ok().map(|i| i as u32)
    }
}

#[cfg(test)]
mod app_state_tests {
    use super::*;
    use asw_core::graph::GraphBuilder;

    /// Build a small test graph with nodes sorted by H3 index.
    fn test_graph(cells: &[(f64, f64)]) -> RoutingGraph {
        let mut entries: Vec<(u64, f64, f64)> = cells
            .iter()
            .map(|&(lat, lng)| {
                let cell = h3o::LatLng::new(lat, lng)
                    .unwrap()
                    .to_cell(h3o::Resolution::Five);
                (u64::from(cell), lat, lng)
            })
            .collect();
        entries.sort_by_key(|(h3, _, _)| *h3);
        // Deduplicate by H3 index (nearby coords may map to same cell)
        entries.dedup_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::new();
        let mut ids = Vec::new();
        for &(h3, lat, lng) in &entries {
            ids.push(b.add_node(h3, lat, lng));
        }
        // Connect all nodes in a chain so they share one component
        for i in 0..ids.len().saturating_sub(1) {
            b.add_edge(ids[i], ids[i + 1], 1.0);
        }
        b.build()
    }

    #[test]
    fn exact_cell_match_finds_node() {
        let graph = test_graph(&[(36.848, 28.268), (37.0, 28.5), (36.5, 28.0)]);
        let state = AppState::new(graph);

        // Query at the exact position of the first node — should snap to it
        let result = state.nearest_node(36.848, 28.268);
        assert!(result.is_some(), "should find a node near (36.848, 28.268)");
        let (_, dist) = result.unwrap();
        // Distance should be very small (cell center offset)
        assert!(dist < 5.0, "distance {} nm should be < 5 nm", dist);
    }

    #[test]
    fn nearby_offset_snaps_to_closest() {
        let graph = test_graph(&[(36.848, 28.268), (37.0, 28.5), (36.5, 28.0)]);
        let state = AppState::new(graph);

        // Query slightly offset from first node — should still find a node
        let result = state.nearest_node(36.85, 28.27);
        assert!(result.is_some(), "should find a node near offset position");
        let (_, dist) = result.unwrap();
        assert!(dist < 10.0, "distance {} nm should be < 10 nm", dist);
    }

    #[test]
    fn empty_graph_returns_none() {
        let b = GraphBuilder::new();
        let graph = b.build();
        let state = AppState::new(graph);

        let result = state.nearest_node(36.848, 28.268);
        assert!(result.is_none(), "empty graph should return None");
    }

    /// Build a graph with two components: a main chain and an isolated node.
    /// The isolated node is NOT connected to the chain.
    fn graph_with_isolated_node() -> (RoutingGraph, (f64, f64), (f64, f64)) {
        // Main component: 3 nodes forming a chain, ~1 degree apart
        let main_a = (36.0, 28.0);
        let main_b = (36.5, 28.5);
        let main_c = (37.0, 29.0);
        // Isolated node: close to main_a but NOT connected.
        // Offset by ~0.3° (~18nm) to ensure distinct res-5 cells.
        let isolated = (36.3, 28.3);

        let mut entries: Vec<(u64, f64, f64, bool)> = Vec::new();
        for &(lat, lng) in &[main_a, main_b, main_c] {
            let cell = h3o::LatLng::new(lat, lng)
                .unwrap()
                .to_cell(h3o::Resolution::Five);
            entries.push((u64::from(cell), lat, lng, true));
        }
        {
            let cell = h3o::LatLng::new(isolated.0, isolated.1)
                .unwrap()
                .to_cell(h3o::Resolution::Five);
            entries.push((u64::from(cell), isolated.0, isolated.1, false));
        }
        entries.sort_by_key(|(h3, _, _, _)| *h3);
        entries.dedup_by_key(|(h3, _, _, _)| *h3);

        let mut b = GraphBuilder::new();
        let mut ids = Vec::new();
        let mut is_main = Vec::new();
        for &(h3, lat, lng, main) in &entries {
            ids.push(b.add_node(h3, lat, lng));
            is_main.push(main);
        }
        // Connect only the main-component nodes in a chain
        let main_ids: Vec<u32> = ids
            .iter()
            .zip(is_main.iter())
            .filter(|(_, &m)| m)
            .map(|(&id, _)| id)
            .collect();
        for i in 0..main_ids.len().saturating_sub(1) {
            b.add_edge(main_ids[i], main_ids[i + 1], 1.0);
        }
        (b.build(), isolated, main_a)
    }

    #[test]
    fn skips_isolated_component_snaps_to_main() {
        let (graph, isolated_pos, _main_pos) = graph_with_isolated_node();
        let state = AppState::new(graph);

        // Query at the isolated node's position — should snap to main component instead
        let result = state.nearest_node(isolated_pos.0, isolated_pos.1);
        assert!(result.is_some(), "should find a main-component node");
        let (node_id, _dist) = result.unwrap();
        assert_eq!(
            state.component_labels[node_id as usize], state.main_component,
            "snapped node must be in main component"
        );
    }

    /// Build a graph with nodes at two different resolutions.
    /// A res-9 node near the query and a res-5 node farther away.
    fn graph_multi_resolution() -> RoutingGraph {
        let mut b = GraphBuilder::new();

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

        let mut entries = vec![
            (u64::from(far_cell), far_pos.0, far_pos.1),
            (u64::from(near_cell), near_pos.0, near_pos.1),
        ];
        entries.sort_by_key(|(h3, _, _)| *h3);
        entries.dedup_by_key(|(h3, _, _)| *h3);

        let mut ids = Vec::new();
        for &(h3, lat, lng) in &entries {
            ids.push(b.add_node(h3, lat, lng));
        }
        // Connect all so they're in the same component
        for i in 0..ids.len().saturating_sub(1) {
            b.add_edge(ids[i], ids[i + 1], 1.0);
        }
        b.build()
    }

    #[test]
    fn prefers_closer_node_across_resolutions() {
        let graph = graph_multi_resolution();
        let state = AppState::new(graph);

        // Query near the res-9 node
        let query = (36.0, 28.0);
        let result = state.nearest_node(query.0, query.1);
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

        let mut b = GraphBuilder::new();
        b.add_node(u64::from(ocean_cell), ocean_pos.0, ocean_pos.1);
        let graph = b.build();
        let state = AppState::new(graph);

        // Query 5 degrees away (~300 nm) — simulates a remote island
        let result = state.nearest_node(42.0, 16.0);
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

        let mut b = GraphBuilder::new();
        b.add_node(u64::from(ocean_cell), ocean_pos.0, ocean_pos.1);
        let graph = b.build();
        let state = AppState::new(graph);

        // Query from deep inland (Belgrade, Serbia — ~400nm from Mediterranean)
        let result = state.nearest_node(44.8, 20.5);
        assert!(result.is_some(), "should find ocean node from inland point");
    }
}
