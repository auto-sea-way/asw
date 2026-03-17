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

        Self {
            graph,
            coastline,
            component_labels,
            main_component,
        }
    }

    /// Find nearest node in the main connected component using H3 binary search.
    ///
    /// Iterates from finest resolution down to coarsest, checking the exact cell
    /// and its k-ring(1) neighbors at each level.
    pub fn nearest_node(&self, lat: f64, lon: f64) -> Option<(u32, f64)> {
        let ll = h3o::LatLng::new(lat, lon).ok()?;

        // Iterate from finest resolution (passage corridors) to coarsest (ocean)
        for res_u8 in (3..=13).rev() {
            let res = h3o::Resolution::try_from(res_u8).ok()?;
            let cell = ll.to_cell(res);
            let h3_val = u64::from(cell);

            // Check exact cell
            if let Some(node_id) = self.h3_lookup(h3_val) {
                if self.component_labels[node_id as usize] == self.main_component {
                    let (nlat, nlon) = self.graph.node_pos(node_id);
                    let dist = asw_core::h3::haversine_nm(lat, lon, nlat, nlon);
                    return Some((node_id, dist));
                }
            }

            // Check k-ring(1) neighbors
            for neighbor in cell.grid_disk::<Vec<_>>(1) {
                let nh3 = u64::from(neighbor);
                if nh3 == h3_val {
                    continue; // Already checked
                }
                if let Some(node_id) = self.h3_lookup(nh3) {
                    if self.component_labels[node_id as usize] == self.main_component {
                        let (nlat, nlon) = self.graph.node_pos(node_id);
                        let dist = asw_core::h3::haversine_nm(lat, lon, nlat, nlon);
                        return Some((node_id, dist));
                    }
                }
            }
        }

        None
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
}
