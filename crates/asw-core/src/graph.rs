use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// File layout: [b"ASW\x01" magic header][zstd-compressed bincode payload]
///
/// Compressed Sparse Row graph for maritime routing.
/// Coordinates are i32 fixed-point (degrees × 1e7).
/// Edge data is interleaved delta-varint targets + u16 weights.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoutingGraph {
    pub node_lats: Vec<i32>,
    pub node_lngs: Vec<i32>,
    /// H3 resolution (3-15) for each node, 0 = passage node.
    pub node_resolutions: Vec<u8>,
    pub passage_mask: Vec<u8>,
    /// Byte offsets into `edge_data`. Length = num_nodes + 1.
    /// Invariant: `offsets[num_nodes] == edge_data.len()`
    pub offsets: Vec<u32>,
    /// Interleaved per-node: [varint target_delta][u16 weight_le] per edge.
    /// Targets sorted ascending, stored as deltas.
    pub edge_data: Vec<u8>,
    pub coastline_coords: Vec<Vec<(f32, f32)>>,
    pub num_nodes: u32,
    pub num_edges: u32,
}

/// Iterator over a node's neighbors, decoding interleaved varint+u16 edge data.
pub struct NeighborIter<'a> {
    data: &'a [u8],
    pos: usize,
    prev_target: u32,
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

        let weight_raw = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        let weight_nm = weight_raw as f32 / 100.0;

        Some((target, weight_nm))
    }
}

pub struct GraphBuilder {
    /// (lat_deg, lng_deg, is_passage, h3_resolution) per node
    /// h3_resolution: 0 for passage nodes, 3-15 for regular H3 nodes
    nodes: Vec<(f64, f64, bool, u8)>,
    /// (src, dst, weight_nm)
    edges: Vec<(u32, u32, f32)>,
    pub coastline_coords: Vec<Vec<(f32, f32)>>,
}

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            coastline_coords: Vec::new(),
        }
    }

    /// Add a node. Returns node ID.
    /// `resolution`: H3 resolution (3-15) for regular nodes, 0 for passage nodes.
    pub fn add_node(&mut self, lat: f64, lng: f64, is_passage: bool, resolution: u8) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push((lat, lng, is_passage, resolution));
        id
    }

    /// Add a bidirectional edge.
    pub fn add_edge(&mut self, src: u32, dst: u32, weight_nm: f32) {
        self.edges.push((src, dst, weight_nm));
        self.edges.push((dst, src, weight_nm));
    }

    /// Add a one-way edge.
    pub fn add_directed_edge(&mut self, src: u32, dst: u32, weight_nm: f32) {
        self.edges.push((src, dst, weight_nm));
    }

    /// Build the CSR graph.
    pub fn build(self) -> RoutingGraph {
        let num_nodes = self.nodes.len() as u32;
        let num_edges = self.edges.len() as u32;

        // Encode coordinates as i32 fixed-point
        let node_lats: Vec<i32> = self
            .nodes
            .iter()
            .map(|(lat, _, _, _)| (*lat * 1e7).round() as i32)
            .collect();
        let node_lngs: Vec<i32> = self
            .nodes
            .iter()
            .map(|(_, lng, _, _)| (*lng * 1e7).round() as i32)
            .collect();

        // Store H3 resolution per node
        let node_resolutions: Vec<u8> = self.nodes.iter().map(|(_, _, _, res)| *res).collect();

        // Build passage mask bitset
        let mask_len = (num_nodes as usize).div_ceil(8);
        let mut passage_mask = vec![0u8; mask_len];
        for (i, (_, _, is_passage, _)) in self.nodes.iter().enumerate() {
            if *is_passage {
                passage_mask[i / 8] |= 1 << (i % 8);
            }
        }

        // Group edges by source, sort targets ascending per source
        let mut adj: Vec<Vec<(u32, f32)>> = vec![Vec::new(); num_nodes as usize];
        for &(src, dst, w) in &self.edges {
            adj[src as usize].push((dst, w));
        }
        for list in &mut adj {
            list.sort_by_key(|&(target, _)| target);
        }

        // Encode edge_data: interleaved varint deltas + u16 LE weights
        let mut edge_data = Vec::new();
        let mut offsets = Vec::with_capacity(num_nodes as usize + 1);

        for list in &adj {
            offsets.push(edge_data.len() as u32);
            let mut prev_target = 0u32;
            for &(target, weight_nm) in list {
                let delta = target - prev_target;
                crate::varint::encode(delta, &mut edge_data);
                debug_assert!(
                    weight_nm <= 655.35,
                    "weight {weight_nm} nm exceeds u16 range (max 655.35 nm)"
                );
                let weight_u16 = (weight_nm * 100.0).round() as u16;
                edge_data.extend_from_slice(&weight_u16.to_le_bytes());
                prev_target = target;
            }
        }
        offsets.push(edge_data.len() as u32);

        RoutingGraph {
            node_lats,
            node_lngs,
            node_resolutions,
            passage_mask,
            offsets,
            edge_data,
            coastline_coords: self.coastline_coords,
            num_nodes,
            num_edges,
        }
    }
}

impl RoutingGraph {
    const MAGIC: &'static [u8; 4] = b"ASW\x01";

    /// Serialize: write magic header, then bincode+zstd-19 payload.
    pub fn save<W: Write>(&self, mut writer: W) -> anyhow::Result<()> {
        writer.write_all(Self::MAGIC)?;
        let encoder = zstd::Encoder::new(writer, 19)?.auto_finish();
        bincode::serialize_into(encoder, self)?;
        Ok(())
    }

    /// Deserialize: verify magic header, then bincode+zstd payload.
    pub fn load<R: Read>(mut reader: R) -> anyhow::Result<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic[..3] != b"ASW" {
            anyhow::bail!("Not an ASW graph file (expected ASW magic header). Rebuild required.");
        }
        if magic[3] != 1 {
            anyhow::bail!(
                "Unsupported ASW graph version {} (expected 1). Rebuild required.",
                magic[3]
            );
        }
        let decoder = zstd::Decoder::new(reader)?;
        let graph: Self = bincode::deserialize_from(decoder)?;

        // Post-deserialization validation
        let n = graph.num_nodes as usize;
        anyhow::ensure!(
            graph.offsets.len() == n + 1,
            "offsets length {} != num_nodes + 1 ({})",
            graph.offsets.len(),
            n + 1
        );
        anyhow::ensure!(
            graph.offsets[n] as usize == graph.edge_data.len(),
            "offsets sentinel {} != edge_data.len() {}",
            graph.offsets[n],
            graph.edge_data.len()
        );
        for i in 1..graph.offsets.len() {
            anyhow::ensure!(
                graph.offsets[i] >= graph.offsets[i - 1],
                "offsets not monotonic at index {}: {} < {}",
                i,
                graph.offsets[i],
                graph.offsets[i - 1]
            );
        }
        anyhow::ensure!(
            graph.node_lats.len() == n,
            "node_lats length {} != num_nodes {}",
            graph.node_lats.len(),
            n
        );
        anyhow::ensure!(
            graph.node_lngs.len() == n,
            "node_lngs length {} != num_nodes {}",
            graph.node_lngs.len(),
            n
        );
        anyhow::ensure!(
            graph.passage_mask.len() == n.div_ceil(8),
            "passage_mask length {} != expected {}",
            graph.passage_mask.len(),
            n.div_ceil(8)
        );
        anyhow::ensure!(
            graph.node_resolutions.len() == n,
            "node_resolutions length {} != num_nodes {}",
            graph.node_resolutions.len(),
            n
        );

        Ok(graph)
    }

    /// Iterate neighbors of `node` as (target_id, weight_nm) pairs.
    pub fn neighbors(&self, node: u32) -> NeighborIter<'_> {
        let start = self.offsets[node as usize] as usize;
        let end = self.offsets[node as usize + 1] as usize;
        NeighborIter {
            data: &self.edge_data[start..end],
            pos: 0,
            prev_target: 0,
        }
    }

    /// Check if a node is a passage/synthetic node.
    pub fn is_passage(&self, node: u32) -> bool {
        let idx = node as usize;
        self.passage_mask[idx / 8] & (1 << (idx % 8)) != 0
    }

    /// Decode i32 fixed-point coordinates to f64 (lat, lng) in degrees.
    pub fn node_pos(&self, node: u32) -> (f64, f64) {
        let i = node as usize;
        let lat = self.node_lats[i] as f64 / 1e7;
        let lng = self.node_lngs[i] as f64 / 1e7;
        (lat, lng)
    }

    /// Find the nearest node to a given (lat, lon) by brute-force.
    /// For the serve phase, use the R-tree in AppState instead.
    pub fn nearest_node(&self, lat: f64, lon: f64) -> Option<(u32, f64)> {
        if self.num_nodes == 0 {
            return None;
        }
        let mut best_id = 0u32;
        let mut best_dist = f64::MAX;
        for i in 0..self.num_nodes {
            let (nlat, nlng) = self.node_pos(i);
            let d = crate::h3::haversine_nm(lat, lon, nlat, nlng);
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
            for (neighbor, _) in self.neighbors(node as u32) {
                union(&mut parent, &mut rank, node, neighbor as usize);
            }
        }

        for i in 0..n {
            find(&mut parent, i);
        }
        parent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_graph() -> RoutingGraph {
        let mut b = GraphBuilder::new();
        let n0 = b.add_node(0.0, 0.0, false, 0);
        let n1 = b.add_node(0.0, 1.0, false, 0);
        let n2 = b.add_node(1.0, 0.0, false, 0);
        let n3 = b.add_node(1.0, 1.0, false, 0);
        b.add_edge(n0, n1, 1.0);
        b.add_edge(n1, n3, 1.0);
        b.add_edge(n0, n2, 2.0);
        b.add_edge(n2, n3, 2.0);
        b.build()
    }

    #[test]
    fn graph_builder_counts() {
        let g = square_graph();
        assert_eq!(g.num_nodes, 4);
        assert_eq!(g.num_edges, 8); // 4 bidirectional = 8 directed
    }

    #[test]
    fn graph_neighbors() {
        let g = square_graph();
        let n0: Vec<(u32, f32)> = g.neighbors(0).collect();
        assert_eq!(n0.len(), 2);
        assert_eq!(n0[0].0, 1); // n1
        assert_eq!(n0[1].0, 2); // n2
    }

    #[test]
    fn graph_neighbor_weights() {
        let g = square_graph();
        let n0: Vec<(u32, f32)> = g.neighbors(0).collect();
        assert_eq!(n0[0], (1, 1.0));
        assert_eq!(n0[1], (2, 2.0));
    }

    #[test]
    fn graph_node_pos_roundtrip() {
        let g = square_graph();
        let (lat, lng) = g.node_pos(3);
        assert!((lat - 1.0).abs() < 1e-6);
        assert!((lng - 1.0).abs() < 1e-6);
    }

    #[test]
    fn graph_connected_components_single() {
        let g = square_graph();
        let components = g.connected_components();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0], 4);
    }

    #[test]
    fn graph_connected_components_isolated() {
        let mut b = GraphBuilder::new();
        let n0 = b.add_node(0.0, 0.0, false, 0);
        let n1 = b.add_node(0.0, 1.0, false, 0);
        b.add_edge(n0, n1, 1.0);
        let n2 = b.add_node(1.0, 0.0, false, 0);
        let n3 = b.add_node(1.0, 1.0, false, 0);
        b.add_edge(n2, n3, 1.0);
        let g = b.build();
        let mut components = g.connected_components();
        components.sort();
        assert_eq!(components, vec![2, 2]);
    }

    #[test]
    fn graph_save_load_roundtrip() {
        let g = square_graph();
        let mut buf = Vec::new();
        g.save(&mut buf).unwrap();

        // Verify magic header
        assert_eq!(&buf[0..4], b"ASW\x01");

        let loaded = RoutingGraph::load(std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(loaded.num_nodes, g.num_nodes);
        assert_eq!(loaded.num_edges, g.num_edges);
        assert_eq!(loaded.node_lats, g.node_lats);
        assert_eq!(loaded.node_lngs, g.node_lngs);
        assert_eq!(loaded.node_resolutions, g.node_resolutions);
        assert_eq!(loaded.passage_mask, g.passage_mask);
        assert_eq!(loaded.offsets, g.offsets);
        assert_eq!(loaded.edge_data, g.edge_data);

        // Verify routing works after load
        let neighbors: Vec<(u32, f32)> = loaded.neighbors(0).collect();
        assert_eq!(neighbors.len(), 2);
    }

    #[test]
    fn load_rejects_old_format() {
        let fake_old = vec![4, 0, 0, 0, 0, 0, 0, 0];
        let result = RoutingGraph::load(std::io::Cursor::new(&fake_old));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("ASW"),
            "Error should mention ASW format: {}",
            err_msg
        );
    }

    #[test]
    fn neighbor_iter_decodes_edge_data() {
        let mut edge_data = Vec::new();
        crate::varint::encode(5, &mut edge_data);
        edge_data.extend_from_slice(&150u16.to_le_bytes());
        crate::varint::encode(5, &mut edge_data);
        edge_data.extend_from_slice(&200u16.to_le_bytes());
        crate::varint::encode(32, &mut edge_data);
        edge_data.extend_from_slice(&350u16.to_le_bytes());

        let end = edge_data.len() as u32;
        let graph = RoutingGraph {
            node_lats: vec![0; 1],
            node_lngs: vec![0; 1],
            node_resolutions: vec![0; 1],
            passage_mask: vec![0],
            offsets: vec![0, end],
            edge_data,
            coastline_coords: vec![],
            num_nodes: 1,
            num_edges: 3,
        };

        let neighbors: Vec<(u32, f32)> = graph.neighbors(0).collect();
        assert_eq!(neighbors.len(), 3);
        assert_eq!(neighbors[0], (5, 1.50));
        assert_eq!(neighbors[1], (10, 2.00));
        assert_eq!(neighbors[2], (42, 3.50));
    }

    #[test]
    fn is_passage_bitset() {
        let mut mask = vec![0u8; 1];
        mask[0] = (1 << 0) | (1 << 5);
        let graph = RoutingGraph {
            node_lats: vec![0; 8],
            node_lngs: vec![0; 8],
            node_resolutions: vec![0; 8],
            passage_mask: mask,
            offsets: vec![0; 9],
            edge_data: vec![],
            coastline_coords: vec![],
            num_nodes: 8,
            num_edges: 0,
        };
        assert!(graph.is_passage(0));
        assert!(!graph.is_passage(1));
        assert!(!graph.is_passage(4));
        assert!(graph.is_passage(5));
    }

    #[test]
    fn node_pos_i32_roundtrip() {
        let graph = RoutingGraph {
            node_lats: vec![(36.848_f64 * 1e7).round() as i32],
            node_lngs: vec![(28.268_f64 * 1e7).round() as i32],
            node_resolutions: vec![5],
            passage_mask: vec![0],
            offsets: vec![0, 0],
            edge_data: vec![],
            coastline_coords: vec![],
            num_nodes: 1,
            num_edges: 0,
        };
        let (lat, lng) = graph.node_pos(0);
        assert!((lat - 36.848).abs() < 1e-6);
        assert!((lng - 28.268).abs() < 1e-6);
    }

    #[test]
    fn builder_produces_compact_format() {
        let mut b = GraphBuilder::new();
        let n0 = b.add_node(51.5, -0.1, false, 5);
        let n1 = b.add_node(48.8, 2.3, false, 5);
        let n2 = b.add_node(0.0, 0.0, true, 0);
        b.add_edge(n0, n1, 186.0);
        b.add_edge(n0, n2, 50.0);

        let g = b.build();

        assert_eq!(g.node_lats[0], (51.5_f64 * 1e7).round() as i32);
        assert_eq!(g.node_lngs[1], (2.3_f64 * 1e7).round() as i32);
        assert!(!g.is_passage(n0));
        assert!(!g.is_passage(n1));
        assert!(g.is_passage(n2));
        assert_eq!(g.num_nodes, 3);
        assert_eq!(g.num_edges, 4);

        let n0_neighbors: Vec<(u32, f32)> = g.neighbors(n0).collect();
        assert_eq!(n0_neighbors.len(), 2);

        let n1_neighbors: Vec<(u32, f32)> = g.neighbors(n1).collect();
        assert_eq!(n1_neighbors.len(), 1);
        assert_eq!(n1_neighbors[0].0, n0);
        assert!((n1_neighbors[0].1 - 186.0).abs() < 0.01);
    }
}
