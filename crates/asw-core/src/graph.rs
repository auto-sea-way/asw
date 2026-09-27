use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Quantization unit for `shore_dist`: 0.02 nm (~37 m) per step.
pub const SHORE_DIST_UNIT_NM: f64 = 0.02;
/// Saturation ceiling: 255 units = 5.1 nm. Distances beyond this store 255.
pub const SHORE_DIST_MAX_NM: f64 = SHORE_DIST_UNIT_NM * 255.0;

/// Quantize a shore distance (nm) to `shore_dist` units, rounding DOWN so
/// the stored clearance never overstates the real one.
///
/// A tiny epsilon is added before flooring to counter f64 division error
/// at exact unit boundaries (e.g. `5.1 / 0.02` evaluates to
/// `254.99999999999997` rather than `255.0`), which would otherwise round
/// an exact boundary down to the wrong unit.
pub fn quantize_shore_dist(nm: f64) -> u8 {
    (nm / SHORE_DIST_UNIT_NM + 1e-9).floor().clamp(0.0, 255.0) as u8
}

/// File layout: [b"ASW\x03" magic header][zstd-compressed bitcode payload]
///
/// Compressed Sparse Row graph for maritime routing.
/// Nodes are H3 cell indices stored in sorted ascending order.
/// Edge data is varint target deltas; weights are recomputed at query time
/// as centre-to-centre haversine, so nothing per edge is stored but the id.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoutingGraph {
    /// H3 cell index for each node, sorted ascending. Array index = node ID.
    pub node_h3: Vec<u64>,
    /// Byte offsets into `edge_data`. Length = num_nodes + 1.
    /// Invariant: `offsets[num_nodes] == edge_data.len()`
    pub offsets: Vec<u32>,
    /// Per-node varint target deltas. Targets sorted ascending.
    pub edge_data: Vec<u8>,
    /// Quantized straight-line distance from node center to nearest coastline.
    /// Unit: SHORE_DIST_UNIT_NM (0.02 nm). 255 = saturated (>= 5.1 nm).
    /// Rounded down at build time. Length = num_nodes.
    pub shore_dist: Vec<u8>,
    pub coastline_runs: Vec<Vec<(f64, f64)>>,
    pub num_nodes: u32,
    pub num_edges: u32,
}

/// Iterator over a node's neighbors, decoding varint target deltas and
/// computing each edge's length from the two cell centres.
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

#[derive(Default)]
pub struct GraphBuilder {
    /// (h3_index, shore_dist_q) per node.
    nodes: Vec<(u64, u8)>,
    /// (src, dst)
    edges: Vec<(u32, u32)>,
    pub coastline_runs: Vec<Vec<(f64, f64)>>,
}

impl GraphBuilder {
    /// Add a node with its quantized shore distance. Returns node ID.
    pub fn add_node(&mut self, h3_index: u64, shore_dist_q: u8) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push((h3_index, shore_dist_q));
        id
    }

    /// Add a bidirectional edge. Weights are not stored: the router computes
    /// centre-to-centre haversine at query time.
    pub fn add_edge(&mut self, src: u32, dst: u32) {
        self.edges.push((src, dst));
        self.edges.push((dst, src));
    }

    /// Add a one-way edge.
    pub fn add_directed_edge(&mut self, src: u32, dst: u32) {
        self.edges.push((src, dst));
    }

    /// Keep only the largest connected component, renumbering node ids and
    /// preserving H3 order. Returns self unchanged when already connected.
    pub fn prune_to_main_component(mut self) -> Self {
        let n = self.nodes.len();
        let labels = component_labels(n, &self.edges);
        let mut comp_sizes: std::collections::HashMap<u32, usize> =
            std::collections::HashMap::new();
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

    /// Build the CSR graph.
    pub fn build(self) -> RoutingGraph {
        let num_nodes = self.nodes.len() as u32;

        let node_h3: Vec<u64> = self.nodes.iter().map(|(h3, _)| *h3).collect();
        let shore_dist: Vec<u8> = self.nodes.iter().map(|(_, q)| *q).collect();

        // Group edges by source, sort and dedup targets per source
        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); num_nodes as usize];
        for &(src, dst) in &self.edges {
            adj[src as usize].push(dst);
        }
        let mut num_edges = 0u32;
        let mut edge_data = Vec::new();
        let mut offsets = Vec::with_capacity(num_nodes as usize + 1);
        for list in &mut adj {
            list.sort_unstable();
            list.dedup();
            offsets.push(edge_data.len() as u32);
            let mut prev_target = 0u32;
            for &target in list.iter() {
                crate::varint::encode(target - prev_target, &mut edge_data);
                prev_target = target;
                num_edges += 1;
            }
        }
        offsets.push(edge_data.len() as u32);

        RoutingGraph {
            node_h3,
            offsets,
            edge_data,
            shore_dist,
            coastline_runs: self.coastline_runs,
            num_nodes,
            num_edges,
        }
    }
}

impl RoutingGraph {
    const MAGIC: &'static [u8; 4] = b"ASW\x03";

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
        if magic[3] != 3 {
            anyhow::bail!(
                "Unsupported ASW graph version {} (expected 3). Rebuild required.",
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
            graph.shore_dist.len() == n,
            "shore_dist length {} != num_nodes {}",
            graph.shore_dist.len(),
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
        let (src_lat, src_lon) = self.node_pos(node);
        NeighborIter {
            graph: self,
            data: &self.edge_data[start..end],
            pos: 0,
            prev_target: 0,
            src_lat,
            src_lon,
        }
    }

    /// Decode H3 cell center coordinates to f64 (lat, lng) in degrees.
    pub fn node_pos(&self, node: u32) -> (f64, f64) {
        let cell = h3o::CellIndex::try_from(self.node_h3[node as usize]).expect("invalid H3 index");
        crate::h3::cell_center(cell)
    }

    /// Drop coastline coordinate data to free memory after it has been
    /// used to build the CoastlineIndex.
    pub fn drop_coastline_runs(&mut self) {
        self.coastline_runs = Vec::new();
    }
}

/// Union-find component root per node over an edge list.
fn component_labels(n: usize, edges: &[(u32, u32)]) -> Vec<u32> {
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

    for &(a, b) in edges {
        union(&mut parent, &mut rank, a, b);
    }
    drop(rank);
    for i in 0..n as u32 {
        find(&mut parent, i);
    }
    parent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_graph() -> RoutingGraph {
        // Use real H3 cells at resolution 5, sorted by H3 index
        let c0 = h3o::LatLng::new(0.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(0.0, 1.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c2 = h3o::LatLng::new(1.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c3 = h3o::LatLng::new(1.0, 1.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);

        let mut cells: Vec<(u64, f64, f64)> = vec![
            (u64::from(c0), 0.0, 0.0),
            (u64::from(c1), 0.0, 1.0),
            (u64::from(c2), 1.0, 0.0),
            (u64::from(c3), 1.0, 1.0),
        ];
        cells.sort_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::default();
        let mut ids = Vec::new();
        for (h3, _, _) in &cells {
            ids.push(b.add_node(*h3, 255));
        }

        // Find which sorted index corresponds to which original cell
        let idx_of = |target_h3: u64| -> u32 {
            cells
                .iter()
                .position(|(h3, _, _)| *h3 == target_h3)
                .unwrap() as u32
        };

        let n0 = idx_of(u64::from(c0));
        let n1 = idx_of(u64::from(c1));
        let n2 = idx_of(u64::from(c2));
        let n3 = idx_of(u64::from(c3));

        b.add_edge(n0, n1);
        b.add_edge(n1, n3);
        b.add_edge(n0, n2);
        b.add_edge(n2, n3);
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
            assert!((-90.0..=90.0).contains(&lat), "lat out of range: {}", lat);
            assert!((-180.0..=180.0).contains(&lng), "lng out of range: {}", lng);
        }
    }

    #[test]
    fn graph_save_load_roundtrip() {
        let g = square_graph();
        let mut buf = Vec::new();
        g.save(&mut buf).unwrap();

        // Verify magic header
        assert_eq!(&buf[0..4], b"ASW\x03");

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
    fn node_pos_h3_decode() {
        let cell = h3o::LatLng::new(36.848, 28.268)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let graph = RoutingGraph {
            node_h3: vec![u64::from(cell)],
            offsets: vec![0, 0],
            edge_data: vec![],
            shore_dist: vec![255],
            coastline_runs: vec![],
            num_nodes: 1,
            num_edges: 0,
        };
        let (lat, lng) = graph.node_pos(0);
        // H3 cell centers are approximate, but should be close to input
        assert!(
            (lat - 36.848).abs() < 0.5,
            "lat {} too far from 36.848",
            lat
        );
        assert!(
            (lng - 28.268).abs() < 0.5,
            "lng {} too far from 28.268",
            lng
        );
    }

    #[test]
    fn builder_produces_compact_format() {
        let c0 = h3o::LatLng::new(51.5, -0.1)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(48.8, 2.3)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c2 = h3o::LatLng::new(10.0, 10.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);

        let mut cells: Vec<(u64, f64, f64)> = vec![
            (u64::from(c0), 51.5, -0.1),
            (u64::from(c1), 48.8, 2.3),
            (u64::from(c2), 10.0, 10.0),
        ];
        cells.sort_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::default();
        let mut ids = Vec::new();
        for (h3, _, _) in &cells {
            ids.push(b.add_node(*h3, 255));
        }

        let idx_of = |target_h3: u64| -> u32 {
            cells
                .iter()
                .position(|(h3, _, _)| *h3 == target_h3)
                .unwrap() as u32
        };

        let n0 = idx_of(u64::from(c0));
        let n1 = idx_of(u64::from(c1));
        let n2 = idx_of(u64::from(c2));

        b.add_edge(n0, n1);
        b.add_edge(n0, n2);

        let g = b.build();

        assert_eq!(g.num_nodes, 3);
        assert_eq!(g.num_edges, 4);

        let n0_neighbors: Vec<(u32, f32)> = g.neighbors(n0).collect();
        assert_eq!(n0_neighbors.len(), 2);

        let n1_neighbors: Vec<(u32, f32)> = g.neighbors(n1).collect();
        assert_eq!(n1_neighbors.len(), 1);
        assert_eq!(n1_neighbors[0].0, n0);
        let (lat0, lon0) = g.node_pos(n0);
        let (lat1, lon1) = g.node_pos(n1);
        let expected = crate::h3::haversine_nm(lat1, lon1, lat0, lon0);
        assert!((n1_neighbors[0].1 as f64 - expected).abs() < 1e-3);
    }

    #[test]
    fn neighbor_weights_are_centre_to_centre_haversine() {
        let g = square_graph();
        for n in 0..g.num_nodes {
            let (lat, lon) = g.node_pos(n);
            for (t, w) in g.neighbors(n) {
                let (tlat, tlon) = g.node_pos(t);
                let expected = crate::h3::haversine_nm(lat, lon, tlat, tlon);
                assert!(
                    (w as f64 - expected).abs() < 1e-3,
                    "edge {n}->{t}: {w} vs {expected}"
                );
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

    #[test]
    fn quantize_rounds_down_and_saturates() {
        assert_eq!(quantize_shore_dist(0.0), 0);
        assert_eq!(quantize_shore_dist(0.019), 0); // rounds down, not nearest
        assert_eq!(quantize_shore_dist(0.02), 1);
        assert_eq!(quantize_shore_dist(0.199), 9); // 9.95 -> 9
        assert_eq!(quantize_shore_dist(5.1), 255);
        assert_eq!(quantize_shore_dist(99.0), 255); // saturates
        assert_eq!(quantize_shore_dist(-1.0), 0); // clamps
    }

    #[test]
    fn load_rejects_v2_files() {
        let bytes = b"ASW\x02whatever".to_vec();
        let err = RoutingGraph::load(&bytes[..]).unwrap_err();
        assert!(
            err.to_string().contains("Unsupported ASW graph version 2"),
            "got: {err}"
        );
    }

    #[test]
    fn prune_keeps_main_component_and_shore_dist() {
        // 3-node chain (main) + 1 isolated node, distinct shore_dist values.
        let coords = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (10.0, 10.0)];
        let mut entries: Vec<(u64, f64, f64, u8)> = coords
            .iter()
            .enumerate()
            .map(|(i, &(lat, lng))| {
                let cell = h3o::LatLng::new(lat, lng)
                    .unwrap()
                    .to_cell(h3o::Resolution::Five);
                (u64::from(cell), lat, lng, (i as u8 + 1) * 10) // 10,20,30,40
            })
            .collect();
        entries.sort_by_key(|(h3, _, _, _)| *h3);

        let mut b = GraphBuilder::default();
        let mut ids = Vec::new();
        for &(h3, _, _, q) in &entries {
            ids.push(b.add_node(h3, q));
        }
        // Chain the first three entries (by sorted order); leave the last isolated.
        b.add_edge(ids[0], ids[1]);
        b.add_edge(ids[1], ids[2]);
        let pruned = b.prune_to_main_component().build();
        assert_eq!(pruned.num_nodes, 3);
        // Every surviving node keeps the shore_dist of the entry with its H3 index.
        for (i, &h3) in pruned.node_h3.iter().enumerate() {
            let orig = entries.iter().find(|e| e.0 == h3).unwrap();
            assert_eq!(pruned.shore_dist[i], orig.3, "node {i} shore_dist mismatch");
        }
    }

    #[test]
    fn shore_dist_survives_save_load_roundtrip() {
        // Reuse the same construction pattern as the existing roundtrip test,
        // but with distinct shore_dist values per node.
        let c0 = h3o::LatLng::new(0.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(5.0, 5.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let mut cells = vec![
            (u64::from(c0), 0.0, 0.0, 7u8),
            (u64::from(c1), 5.0, 5.0, 200u8),
        ];
        cells.sort_by_key(|(h3, _, _, _)| *h3);

        let mut b = GraphBuilder::default();
        let mut ids = Vec::new();
        for &(h3, _, _, q) in &cells {
            ids.push(b.add_node(h3, q));
        }
        b.add_edge(ids[0], ids[1]);
        let g = b.build();

        let mut buf = Vec::new();
        g.save(&mut buf).unwrap();
        let loaded = RoutingGraph::load(&buf[..]).unwrap();
        assert_eq!(loaded.shore_dist, g.shore_dist);
        assert_eq!(loaded.shore_dist.len(), loaded.num_nodes as usize);
    }
}
