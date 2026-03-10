use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Compressed Sparse Row graph for maritime routing.
#[derive(Serialize, Deserialize)]
pub struct RoutingGraph {
    /// Latitude of each node (degrees)
    pub node_lats: Vec<f32>,
    /// Longitude of each node (degrees)
    pub node_lngs: Vec<f32>,
    /// CSR row offsets: offsets[i]..offsets[i+1] are edges from node i
    pub offsets: Vec<u32>,
    /// Target node IDs (parallel to weights)
    pub adjacency: Vec<u32>,
    /// Edge cost in km (parallel to adjacency)
    pub weights: Vec<f32>,
    /// Coastline segments for smoothing: each is Vec<(lon, lat)> as f32
    pub coastline_coords: Vec<Vec<(f32, f32)>>,
    /// H3 cell index for each node (0 = synthetic/passage node)
    pub node_cells: Vec<u64>,
    pub num_nodes: u32,
    pub num_edges: u32,
}

/// Builder for constructing a RoutingGraph from edge lists.
pub struct GraphBuilder {
    pub node_lats: Vec<f32>,
    pub node_lngs: Vec<f32>,
    pub node_cells: Vec<u64>,
    /// Edges as (source, target, weight_km)
    pub edges: Vec<(u32, u32, f32)>,
    pub coastline_coords: Vec<Vec<(f32, f32)>>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self {
            node_lats: Vec::new(),
            node_lngs: Vec::new(),
            node_cells: Vec::new(),
            edges: Vec::new(),
            coastline_coords: Vec::new(),
        }
    }

    /// Add a node, returns its ID.
    pub fn add_node(&mut self, lat: f32, lng: f32) -> u32 {
        let id = self.node_lats.len() as u32;
        self.node_lats.push(lat);
        self.node_lngs.push(lng);
        self.node_cells.push(0);
        id
    }

    /// Add a node with an associated H3 cell, returns its ID.
    pub fn add_node_with_cell(&mut self, lat: f32, lng: f32, cell: u64) -> u32 {
        let id = self.node_lats.len() as u32;
        self.node_lats.push(lat);
        self.node_lngs.push(lng);
        self.node_cells.push(cell);
        id
    }

    /// Add a bidirectional edge.
    pub fn add_edge(&mut self, src: u32, dst: u32, weight_km: f32) {
        self.edges.push((src, dst, weight_km));
        self.edges.push((dst, src, weight_km));
    }

    /// Add a directed edge.
    pub fn add_directed_edge(&mut self, src: u32, dst: u32, weight_km: f32) {
        self.edges.push((src, dst, weight_km));
    }

    /// Build the CSR graph.
    pub fn build(mut self) -> RoutingGraph {
        let num_nodes = self.node_lats.len() as u32;
        let num_edges = self.edges.len() as u32;

        // Sort edges by source
        self.edges.sort_unstable_by_key(|e| e.0);

        let mut offsets = vec![0u32; num_nodes as usize + 1];
        let mut adjacency = Vec::with_capacity(self.edges.len());
        let mut weights = Vec::with_capacity(self.edges.len());

        for &(src, dst, w) in &self.edges {
            offsets[src as usize + 1] += 1;
            adjacency.push(dst);
            weights.push(w);
        }

        // Prefix sum
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }

        RoutingGraph {
            node_lats: self.node_lats,
            node_lngs: self.node_lngs,
            offsets,
            adjacency,
            weights,
            coastline_coords: self.coastline_coords,
            node_cells: self.node_cells,
            num_nodes,
            num_edges,
        }
    }
}

impl RoutingGraph {
    /// Serialize to a writer via bincode.
    pub fn save<W: Write>(&self, writer: W) -> anyhow::Result<()> {
        bincode::serialize_into(writer, self)?;
        Ok(())
    }

    /// Deserialize from a reader via bincode.
    pub fn load<R: Read>(reader: R) -> anyhow::Result<Self> {
        let graph = bincode::deserialize_from(reader)?;
        Ok(graph)
    }

    /// Get edges from a node: iterator of (target_id, weight_km).
    pub fn edges(&self, node: u32) -> &[u32] {
        let start = self.offsets[node as usize] as usize;
        let end = self.offsets[node as usize + 1] as usize;
        &self.adjacency[start..end]
    }

    /// Get edges with weights from a node.
    pub fn edges_with_weights(&self, node: u32) -> impl Iterator<Item = (u32, f32)> + '_ {
        let start = self.offsets[node as usize] as usize;
        let end = self.offsets[node as usize + 1] as usize;
        self.adjacency[start..end]
            .iter()
            .zip(self.weights[start..end].iter())
            .map(|(&target, &weight)| (target, weight))
    }

    /// Get node position as (lat, lon).
    pub fn node_pos(&self, node: u32) -> (f64, f64) {
        (
            self.node_lats[node as usize] as f64,
            self.node_lngs[node as usize] as f64,
        )
    }

    /// Find the nearest node to a given (lat, lon) by brute-force.
    /// For the serve phase, we'll use an R-tree instead.
    pub fn nearest_node(&self, lat: f64, lon: f64) -> Option<(u32, f64)> {
        if self.num_nodes == 0 {
            return None;
        }
        let mut best_id = 0u32;
        let mut best_dist = f64::MAX;
        for i in 0..self.num_nodes {
            let nlat = self.node_lats[i as usize] as f64;
            let nlng = self.node_lngs[i as usize] as f64;
            let d = crate::h3::haversine_km(lat, lon, nlat, nlng);
            if d < best_dist {
                best_dist = d;
                best_id = i;
            }
        }
        Some((best_id, best_dist))
    }

    /// Connected components via union-find. Returns sorted component sizes (largest first).
    pub fn connected_components(&self) -> Vec<usize> {
        let labels = self.component_labels();
        let mut comp_sizes = std::collections::HashMap::new();
        for &root in &labels {
            *comp_sizes.entry(root).or_insert(0usize) += 1;
        }
        let mut sizes: Vec<usize> = comp_sizes.values().copied().collect();
        sizes.sort_unstable_by(|a, b| b.cmp(a));
        sizes
    }

    /// Returns a Vec where `result[i]` is the component root for node `i`.
    /// All nodes in the same component share the same root value.
    pub fn component_labels(&self) -> Vec<usize> {
        let n = self.num_nodes as usize;
        let mut parent: Vec<usize> = (0..n).collect();
        let mut rank = vec![0u8; n];

        fn find(parent: &mut [usize], x: usize) -> usize {
            if parent[x] != x {
                parent[x] = find(parent, parent[x]);
            }
            parent[x]
        }

        fn union(parent: &mut [usize], rank: &mut [u8], a: usize, b: usize) {
            let ra = find(parent, a);
            let rb = find(parent, b);
            if ra == rb {
                return;
            }
            if rank[ra] < rank[rb] {
                parent[ra] = rb;
            } else if rank[ra] > rank[rb] {
                parent[rb] = ra;
            } else {
                parent[rb] = ra;
                rank[ra] += 1;
            }
        }

        for node in 0..n {
            for (target, _) in self.edges_with_weights(node as u32) {
                union(&mut parent, &mut rank, node, target as usize);
            }
        }

        // Flatten all parents to roots
        for i in 0..n {
            find(&mut parent, i);
        }
        parent
    }
}
