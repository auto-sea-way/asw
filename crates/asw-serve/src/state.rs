use asw_core::geo_index::CoastlineIndex;
use asw_core::graph::RoutingGraph;
use rstar::{primitives::GeomWithData, RTree};

/// Wrapper that tracks readiness — the HTTP server starts before the graph is loaded.
pub struct ServerState {
    pub inner: tokio::sync::RwLock<Option<AppState>>,
    pub graph_path: String,
}

impl ServerState {
    pub fn new(graph_path: String) -> Self {
        Self {
            inner: tokio::sync::RwLock::new(None),
            graph_path,
        }
    }

    pub fn set_ready(&self, app: AppState) {
        // Use blocking_lock since this is called from spawn_blocking
        *self.inner.blocking_write() = Some(app);
    }
}

/// Shared application state for the HTTP server.
pub struct AppState {
    pub graph: RoutingGraph,
    pub coastline: CoastlineIndex,
    pub node_tree: RTree<GeomWithData<[f64; 2], u32>>,
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

        // Step 3: Node position R-tree for KNN snap.
        let points: Vec<GeomWithData<[f64; 2], u32>> = (0..graph.num_nodes)
            .map(|i| {
                let (lat, lng) = graph.node_pos(i);
                GeomWithData::new([lng, lat], i)
            })
            .collect();
        let node_tree = RTree::bulk_load(points);

        Self {
            graph,
            coastline,
            node_tree,
            component_labels,
            main_component,
        }
    }

    /// Find nearest node in the main connected component using R-tree KNN.
    pub fn nearest_node(&self, lat: f64, lon: f64) -> Option<(u32, f64)> {
        for candidate in self.node_tree.nearest_neighbor_iter(&[lon, lat]) {
            let node_id = candidate.data;
            if self.component_labels[node_id as usize] == self.main_component {
                let (nlat, nlon) = self.graph.node_pos(node_id);
                let dist = asw_core::h3::haversine_nm(lat, lon, nlat, nlon);
                return Some((node_id, dist));
            }
        }
        None
    }
}
