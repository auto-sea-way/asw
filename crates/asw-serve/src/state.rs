use rstar::{primitives::GeomWithData, RTree};
use asw_core::geo_index::CoastlineIndex;
use asw_core::graph::RoutingGraph;

/// Shared application state for the HTTP server.
pub struct AppState {
    pub graph: RoutingGraph,
    pub coastline: CoastlineIndex,
    pub node_tree: RTree<GeomWithData<[f64; 2], u32>>,
    /// Component root for each node; nodes in the main component share `main_component`.
    component_labels: Vec<usize>,
    main_component: usize,
}

impl AppState {
    pub fn new(graph: RoutingGraph) -> Self {
        // Build coastline R-tree from serialized coords
        let coastline = CoastlineIndex::from_serialized(&graph.coastline_coords);

        // Precompute connected components — find the largest
        let component_labels = graph.component_labels();
        let mut comp_sizes = std::collections::HashMap::new();
        for &root in &component_labels {
            *comp_sizes.entry(root).or_insert(0usize) += 1;
        }
        let main_component = comp_sizes
            .into_iter()
            .max_by_key(|&(_, size)| size)
            .map(|(root, _)| root)
            .unwrap_or(0);

        // Build node position R-tree for KNN snap
        let points: Vec<GeomWithData<[f64; 2], u32>> = (0..graph.num_nodes)
            .map(|i| {
                let lat = graph.node_lats[i as usize] as f64;
                let lon = graph.node_lngs[i as usize] as f64;
                GeomWithData::new([lon, lat], i)
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
                let dist = asw_core::h3::haversine_km(lat, lon, nlat, nlon);
                return Some((node_id, dist));
            }
        }
        None
    }
}
