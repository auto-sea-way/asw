use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// File layout: [b"ASW\x02" magic header][zstd-compressed bitcode payload]
///
/// Compressed Sparse Row graph for maritime routing.
/// Nodes are H3 cell indices stored in sorted ascending order.
/// Edge data is interleaved delta-varint targets + u16 weights.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoutingGraph {
    /// H3 cell index for each node, sorted ascending. Array index = node ID.
    pub node_h3: Vec<u64>,
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
    /// (h3_index, lat_deg, lng_deg) per node.
    /// lat/lng kept temporarily for edge weight calculation in the build pipeline.
    nodes: Vec<(u64, f64, f64)>,
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
    pub fn add_node(&mut self, h3_index: u64, lat: f64, lng: f64) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push((h3_index, lat, lng));
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

        let node_h3: Vec<u64> = self.nodes.iter().map(|(h3, _, _)| *h3).collect();

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
            node_h3,
            offsets,
            edge_data,
            coastline_coords: self.coastline_coords,
            num_nodes,
            num_edges,
        }
    }
}

impl RoutingGraph {
    const MAGIC: &'static [u8; 4] = b"ASW\x02";

    /// Serialize: write magic header, then bitcode+zstd-19 payload.
    pub fn save<W: Write>(&self, mut writer: W) -> anyhow::Result<()> {
        writer.write_all(Self::MAGIC)?;
        let encoded = bitcode::serialize(self)?;
        let mut encoder = zstd::Encoder::new(writer, 19)?;
        encoder.write_all(&encoded)?;
        encoder.finish()?;
        Ok(())
    }

    /// Deserialize: verify magic header, then bitcode+zstd payload.
    pub fn load<R: Read>(mut reader: R) -> anyhow::Result<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic[..3] != b"ASW" {
            anyhow::bail!("Not an ASW graph file (expected ASW magic header). Rebuild required.");
        }
        if magic[3] != 2 {
            anyhow::bail!(
                "Unsupported ASW graph version {} (expected 2). Rebuild required.",
                magic[3]
            );
        }
        let mut decoder = zstd::Decoder::new(reader)?;
        let mut buf = Vec::new();
        decoder.read_to_end(&mut buf)?;
        let graph: Self = bitcode::deserialize(&buf)?;

        // Post-deserialization validation
        let n = graph.num_nodes as usize;
        anyhow::ensure!(
            graph.node_h3.len() == n,
            "node_h3 length {} != num_nodes {}",
            graph.node_h3.len(),
            n
        );
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
                "offsets not monotonic at index {}",
                i
            );
        }
        // Validate H3 indices
        for (i, &h3) in graph.node_h3.iter().enumerate() {
            anyhow::ensure!(
                h3o::CellIndex::try_from(h3).is_ok(),
                "invalid H3 index at node {}",
                i
            );
        }
        // Validate strict sorted order
        for w in graph.node_h3.windows(2) {
            anyhow::ensure!(
                w[0] < w[1],
                "node_h3 not strictly sorted: {} >= {}",
                w[0],
                w[1]
            );
        }

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

    /// Decode H3 cell center coordinates to f64 (lat, lng) in degrees.
    pub fn node_pos(&self, node: u32) -> (f64, f64) {
        let h3 = self.node_h3[node as usize];
        let cell = h3o::CellIndex::try_from(h3).expect("invalid H3 index");
        let ll = h3o::LatLng::from(cell);
        (ll.lat(), ll.lng())
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

    /// Drop coastline coordinate data to free memory after it has been
    /// used to build the CoastlineIndex.
    pub fn drop_coastline_coords(&mut self) {
        self.coastline_coords = Vec::new();
    }

    /// Returns a Vec where `result[i]` is the component root for node `i`.
    /// Uses u32 to halve memory vs usize (40M nodes * 4 bytes = 160 MB).
    pub fn component_labels(&self) -> Vec<u32> {
        let n = self.num_nodes as usize;
        debug_assert!(n <= u32::MAX as usize);
        let mut parent: Vec<u32> = (0..n as u32).collect();
        let mut rank = vec![0u8; n];

        fn find(parent: &mut [u32], x: u32) -> u32 {
            let mut root = x;
            while parent[root as usize] != root {
                root = parent[root as usize];
            }
            // Path compression
            let mut cur = x;
            while cur != root {
                let next = parent[cur as usize];
                parent[cur as usize] = root;
                cur = next;
            }
            root
        }

        fn union(parent: &mut [u32], rank: &mut [u8], a: u32, b: u32) {
            let ra = find(parent, a);
            let rb = find(parent, b);
            if ra == rb {
                return;
            }
            if rank[ra as usize] < rank[rb as usize] {
                parent[ra as usize] = rb;
            } else if rank[ra as usize] > rank[rb as usize] {
                parent[rb as usize] = ra;
            } else {
                parent[rb as usize] = ra;
                rank[ra as usize] += 1;
            }
        }

        for node in 0..n {
            for (neighbor, _) in self.neighbors(node as u32) {
                union(&mut parent, &mut rank, node as u32, neighbor);
            }
        }
        drop(rank);

        for i in 0..n as u32 {
            find(&mut parent, i);
        }
        parent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_graph() -> RoutingGraph {
        // Use real H3 cells at resolution 5, sorted by H3 index
        let c0 = h3o::LatLng::new(0.0, 0.0).unwrap().to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(0.0, 1.0).unwrap().to_cell(h3o::Resolution::Five);
        let c2 = h3o::LatLng::new(1.0, 0.0).unwrap().to_cell(h3o::Resolution::Five);
        let c3 = h3o::LatLng::new(1.0, 1.0).unwrap().to_cell(h3o::Resolution::Five);

        let mut cells: Vec<(u64, f64, f64)> = vec![
            (u64::from(c0), 0.0, 0.0),
            (u64::from(c1), 0.0, 1.0),
            (u64::from(c2), 1.0, 0.0),
            (u64::from(c3), 1.0, 1.0),
        ];
        cells.sort_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::new();
        let mut ids = Vec::new();
        for (h3, lat, lng) in &cells {
            ids.push(b.add_node(*h3, *lat, *lng));
        }

        // Find which sorted index corresponds to which original cell
        let idx_of = |target_h3: u64| -> u32 {
            cells.iter().position(|(h3, _, _)| *h3 == target_h3).unwrap() as u32
        };

        let n0 = idx_of(u64::from(c0));
        let n1 = idx_of(u64::from(c1));
        let n2 = idx_of(u64::from(c2));
        let n3 = idx_of(u64::from(c3));

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
        // Just check node 0 has 2 neighbors
        let n0: Vec<(u32, f32)> = g.neighbors(0).collect();
        assert_eq!(n0.len(), 2);
    }

    #[test]
    fn graph_node_pos_h3_roundtrip() {
        let g = square_graph();
        // Each node should decode to a valid lat/lng
        for i in 0..g.num_nodes {
            let (lat, lng) = g.node_pos(i);
            assert!(lat >= -90.0 && lat <= 90.0, "lat out of range: {}", lat);
            assert!(lng >= -180.0 && lng <= 180.0, "lng out of range: {}", lng);
        }
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
        let c0 = h3o::LatLng::new(0.0, 0.0).unwrap().to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(0.0, 1.0).unwrap().to_cell(h3o::Resolution::Five);
        let c2 = h3o::LatLng::new(1.0, 0.0).unwrap().to_cell(h3o::Resolution::Five);
        let c3 = h3o::LatLng::new(1.0, 1.0).unwrap().to_cell(h3o::Resolution::Five);

        let mut cells: Vec<(u64, f64, f64)> = vec![
            (u64::from(c0), 0.0, 0.0),
            (u64::from(c1), 0.0, 1.0),
            (u64::from(c2), 1.0, 0.0),
            (u64::from(c3), 1.0, 1.0),
        ];
        cells.sort_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::new();
        for (h3, lat, lng) in &cells {
            b.add_node(*h3, *lat, *lng);
        }

        let idx_of = |target_h3: u64| -> u32 {
            cells.iter().position(|(h3, _, _)| *h3 == target_h3).unwrap() as u32
        };

        let n0 = idx_of(u64::from(c0));
        let n1 = idx_of(u64::from(c1));
        let n2 = idx_of(u64::from(c2));
        let n3 = idx_of(u64::from(c3));

        b.add_edge(n0, n1, 1.0);
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
        assert_eq!(&buf[0..4], b"ASW\x02");

        let loaded = RoutingGraph::load(std::io::Cursor::new(&buf)).unwrap();
        assert_eq!(loaded.num_nodes, g.num_nodes);
        assert_eq!(loaded.num_edges, g.num_edges);
        assert_eq!(loaded.node_h3, g.node_h3);
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

        // Use a real H3 cell for the dummy node
        let cell = h3o::LatLng::new(0.0, 0.0).unwrap().to_cell(h3o::Resolution::Five);
        let graph = RoutingGraph {
            node_h3: vec![u64::from(cell)],
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
    fn node_pos_h3_decode() {
        let cell = h3o::LatLng::new(36.848, 28.268).unwrap().to_cell(h3o::Resolution::Five);
        let graph = RoutingGraph {
            node_h3: vec![u64::from(cell)],
            offsets: vec![0, 0],
            edge_data: vec![],
            coastline_coords: vec![],
            num_nodes: 1,
            num_edges: 0,
        };
        let (lat, lng) = graph.node_pos(0);
        // H3 cell centers are approximate, but should be close to input
        assert!((lat - 36.848).abs() < 0.5, "lat {} too far from 36.848", lat);
        assert!((lng - 28.268).abs() < 0.5, "lng {} too far from 28.268", lng);
    }

    #[test]
    fn builder_produces_compact_format() {
        let c0 = h3o::LatLng::new(51.5, -0.1).unwrap().to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(48.8, 2.3).unwrap().to_cell(h3o::Resolution::Five);
        let c2 = h3o::LatLng::new(10.0, 10.0).unwrap().to_cell(h3o::Resolution::Five);

        let mut cells: Vec<(u64, f64, f64)> = vec![
            (u64::from(c0), 51.5, -0.1),
            (u64::from(c1), 48.8, 2.3),
            (u64::from(c2), 10.0, 10.0),
        ];
        cells.sort_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::new();
        let mut ids = Vec::new();
        for (h3, lat, lng) in &cells {
            ids.push(b.add_node(*h3, *lat, *lng));
        }

        let idx_of = |target_h3: u64| -> u32 {
            cells.iter().position(|(h3, _, _)| *h3 == target_h3).unwrap() as u32
        };

        let n0 = idx_of(u64::from(c0));
        let n1 = idx_of(u64::from(c1));
        let n2 = idx_of(u64::from(c2));

        b.add_edge(n0, n1, 186.0);
        b.add_edge(n0, n2, 50.0);

        let g = b.build();

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
