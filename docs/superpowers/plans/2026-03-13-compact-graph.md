# Compact Graph Format Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce the routing graph size ~50% on both disk and RAM by stripping `node_cells`, quantizing weights to u16, delta-varint encoding adjacency, and using fixed-point i32 coordinates.

**Architecture:** Replace the current `RoutingGraph` (parallel `Vec<f32>`/`Vec<u32>` arrays + bincode/zstd-3) with a compact format using interleaved varint-encoded edge data, i32 fixed-point coordinates, a passage bitset, and a `b"ASW\x01"` magic header with zstd-19 compression. The `neighbors()` iterator replaces both `edges()` and `edges_with_weights()`.

**Tech Stack:** Rust, bincode 1.3, zstd 0.13 (level 19), hand-rolled LEB128 varint (~20 lines)

**Spec:** `docs/superpowers/specs/2026-03-13-compact-graph-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `crates/asw-core/src/varint.rs` | Create | LEB128 encode/decode helpers |
| `crates/asw-core/src/graph.rs` | Rewrite | New `RoutingGraph` struct, `GraphBuilder`, `NeighborIter`, save/load with magic header |
| `crates/asw-core/src/lib.rs` | Modify | Add `pub mod varint;` |
| `crates/asw-core/src/routing.rs` | Modify | Use `neighbors()`, decode i32 coords |
| `crates/asw-build/src/pipeline.rs` | Modify | Sort nodes by H3 cell, produce new format |
| `crates/asw-serve/src/state.rs` | Modify | Decode i32 coords for R-tree and KNN |
| `crates/asw-cli/src/main.rs` | Modify | GeoJSON: reconstruct H3 cells, use `passage_mask`, decode edge_data |
| `crates/asw-cli/src/bench.rs` | Modify | Minimal — adapts to changed graph field accesses |

---

## Chunk 1: Core Graph Format

### Task 1: Varint encode/decode module

**Files:**
- Create: `crates/asw-core/src/varint.rs`
- Modify: `crates/asw-core/src/lib.rs:1-5`

- [ ] **Step 1: Write failing tests for varint encode/decode**

Create `crates/asw-core/src/varint.rs` with tests only:

```rust
/// LEB128 unsigned varint encoding/decoding for compact graph adjacency.

/// Encode a u32 as LEB128 varint, appending bytes to `buf`.
pub fn encode(value: u32, buf: &mut Vec<u8>) {
    todo!()
}

/// Decode a LEB128 varint from `data[pos..]`, returning (value, new_pos).
/// Panics on malformed input (truncated varint).
pub fn decode(data: &[u8], pos: usize) -> (u32, usize) {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_zero() {
        let mut buf = Vec::new();
        encode(0, &mut buf);
        assert_eq!(buf.len(), 1);
        let (val, pos) = decode(&buf, 0);
        assert_eq!(val, 0);
        assert_eq!(pos, 1);
    }

    #[test]
    fn roundtrip_single_byte() {
        // Values 0-127 fit in 1 byte
        let mut buf = Vec::new();
        encode(127, &mut buf);
        assert_eq!(buf.len(), 1);
        let (val, _) = decode(&buf, 0);
        assert_eq!(val, 127);
    }

    #[test]
    fn roundtrip_two_bytes() {
        // 128 requires 2 bytes
        let mut buf = Vec::new();
        encode(128, &mut buf);
        assert_eq!(buf.len(), 2);
        let (val, pos) = decode(&buf, 0);
        assert_eq!(val, 128);
        assert_eq!(pos, 2);
    }

    #[test]
    fn roundtrip_large() {
        let mut buf = Vec::new();
        encode(40_000_000, &mut buf);
        let (val, _) = decode(&buf, 0);
        assert_eq!(val, 40_000_000);
    }

    #[test]
    fn roundtrip_max() {
        let mut buf = Vec::new();
        encode(u32::MAX, &mut buf);
        assert_eq!(buf.len(), 5);
        let (val, _) = decode(&buf, 0);
        assert_eq!(val, u32::MAX);
    }

    #[test]
    fn multiple_values_sequential() {
        let mut buf = Vec::new();
        encode(100, &mut buf);
        encode(200, &mut buf);
        encode(300, &mut buf);
        let (v1, p1) = decode(&buf, 0);
        let (v2, p2) = decode(&buf, p1);
        let (v3, _) = decode(&buf, p2);
        assert_eq!((v1, v2, v3), (100, 200, 300));
    }
}
```

- [ ] **Step 2: Add module declaration**

In `crates/asw-core/src/lib.rs`, add after line 4 (`pub mod routing;`):

```rust
pub mod varint;
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p asw-core varint -- --nocapture`
Expected: FAIL — all tests panic with `todo!()`

- [ ] **Step 4: Implement varint encode/decode**

Replace the `todo!()` bodies in `crates/asw-core/src/varint.rs`:

```rust
pub fn encode(mut value: u32, buf: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

pub fn decode(data: &[u8], mut pos: usize) -> (u32, usize) {
    let mut result: u32 = 0;
    let mut shift = 0;
    loop {
        let byte = data[pos];
        pos += 1;
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    (result, pos)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p asw-core varint -- --nocapture`
Expected: All 6 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/asw-core/src/varint.rs crates/asw-core/src/lib.rs
git commit -m "feat: add LEB128 varint encode/decode module"
```

---

### Task 2: New RoutingGraph struct and NeighborIter

**Files:**
- Modify: `crates/asw-core/src/graph.rs:5-23` (RoutingGraph struct)
- Modify: `crates/asw-core/src/graph.rs:118-231` (impl RoutingGraph)

- [ ] **Step 1: Write failing test for NeighborIter**

Add to the test module in `crates/asw-core/src/graph.rs` (after line 323):

```rust
    #[test]
    fn neighbor_iter_decodes_edge_data() {
        // Manually encode 3 neighbors: targets [5, 10, 42], weights [1.5, 2.0, 3.5]
        let mut edge_data = Vec::new();
        // First target: 5 (varint), weight: 150 (u16 LE, 1.50 nm)
        crate::varint::encode(5, &mut edge_data);
        edge_data.extend_from_slice(&150u16.to_le_bytes());
        // Delta: 10 - 5 = 5 (varint), weight: 200 (u16 LE, 2.00 nm)
        crate::varint::encode(5, &mut edge_data);
        edge_data.extend_from_slice(&200u16.to_le_bytes());
        // Delta: 42 - 10 = 32 (varint), weight: 350 (u16 LE, 3.50 nm)
        crate::varint::encode(32, &mut edge_data);
        edge_data.extend_from_slice(&350u16.to_le_bytes());

        let end = edge_data.len() as u32;
        let graph = RoutingGraph {
            node_lats: vec![0; 1],
            node_lngs: vec![0; 1],
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
        // Node 0 and node 5 are passages, others are not
        let mut mask = vec![0u8; 1]; // 8 bits
        mask[0] = (1 << 0) | (1 << 5); // bits 0 and 5 set
        let graph = RoutingGraph {
            node_lats: vec![0; 8],
            node_lngs: vec![0; 8],
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
```

- [ ] **Step 2: Replace the RoutingGraph struct**

Replace lines 5-23 of `crates/asw-core/src/graph.rs`:

```rust
/// File layout: [b"ASW\x01" magic header][zstd-compressed bincode payload]
///
/// Compressed Sparse Row graph for maritime routing.
/// Coordinates are i32 fixed-point (degrees × 1e7).
/// Edge data is interleaved delta-varint targets + u16 weights.
#[derive(Serialize, Deserialize)]
pub struct RoutingGraph {
    pub node_lats: Vec<i32>,
    pub node_lngs: Vec<i32>,
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
```

- [ ] **Step 3: Add NeighborIter and helper methods**

Replace the existing `edges()`, `edges_with_weights()`, `node_pos()`, and `nearest_node()` methods (lines 134-176) with:

```rust
    /// Iterate neighbors of `node` as (target_id, weight_nm) pairs.
    /// Replaces both `edges()` and `edges_with_weights()`.
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
```

Add the `NeighborIter` struct outside the impl block (before `impl RoutingGraph`):

```rust
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
```

- [ ] **Step 4: Run new tests to verify they pass**

Run: `cargo test -p asw-core graph::tests::neighbor_iter -- --nocapture`
Run: `cargo test -p asw-core graph::tests::is_passage -- --nocapture`
Run: `cargo test -p asw-core graph::tests::node_pos_i32 -- --nocapture`
Expected: All 3 PASS

- [ ] **Step 5: Commit**

```bash
git add crates/asw-core/src/graph.rs
git commit -m "feat: new compact RoutingGraph with NeighborIter, passage_mask, i32 coords"
```

---

### Task 3: New GraphBuilder producing compact format

**Files:**
- Modify: `crates/asw-core/src/graph.rs:26-115` (GraphBuilder)

- [ ] **Step 1: Write failing test for the new builder**

Add to test module in `crates/asw-core/src/graph.rs`:

```rust
    #[test]
    fn builder_produces_compact_format() {
        let mut b = GraphBuilder::new();
        // lat/lng as f64 degrees, is_passage flag
        let n0 = b.add_node(51.5, -0.1, false); // London-ish
        let n1 = b.add_node(48.8, 2.3, false);  // Paris-ish
        let n2 = b.add_node(0.0, 0.0, true);    // synthetic passage
        b.add_edge(n0, n1, 186.0);
        b.add_edge(n0, n2, 50.0);

        let g = b.build();

        // Check i32 fixed-point encoding
        assert_eq!(g.node_lats[0], (51.5_f64 * 1e7).round() as i32);
        assert_eq!(g.node_lngs[1], (2.3_f64 * 1e7).round() as i32);

        // Check passage mask
        assert!(!g.is_passage(n0));
        assert!(!g.is_passage(n1));
        assert!(g.is_passage(n2));

        // Check edge data decodes correctly
        assert_eq!(g.num_nodes, 3);
        assert_eq!(g.num_edges, 4); // 2 bidirectional edges = 4 directed

        // n0 should have neighbors n1 and n2
        let n0_neighbors: Vec<(u32, f32)> = g.neighbors(n0).collect();
        assert_eq!(n0_neighbors.len(), 2);

        // n1 should have neighbor n0
        let n1_neighbors: Vec<(u32, f32)> = g.neighbors(n1).collect();
        assert_eq!(n1_neighbors.len(), 1);
        assert_eq!(n1_neighbors[0].0, n0);
        assert!((n1_neighbors[0].1 - 186.0).abs() < 0.01);
    }
```

- [ ] **Step 2: Rewrite GraphBuilder**

Replace the `GraphBuilder` struct and impl (lines 26-115):

```rust
pub struct GraphBuilder {
    /// (lat_deg, lng_deg, is_passage) per node
    nodes: Vec<(f64, f64, bool)>,
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

    /// Add a regular H3 node. Returns node ID.
    pub fn add_node(&mut self, lat: f64, lng: f64, is_passage: bool) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push((lat, lng, is_passage));
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

    pub fn build(self) -> RoutingGraph {
        let num_nodes = self.nodes.len() as u32;
        let num_edges = self.edges.len() as u32;

        // Encode coordinates as i32 fixed-point
        let node_lats: Vec<i32> = self
            .nodes
            .iter()
            .map(|(lat, _, _)| (*lat * 1e7).round() as i32)
            .collect();
        let node_lngs: Vec<i32> = self
            .nodes
            .iter()
            .map(|(_, lng, _)| (*lng * 1e7).round() as i32)
            .collect();

        // Build passage mask bitset
        let mask_len = (num_nodes as usize + 7) / 8;
        let mut passage_mask = vec![0u8; mask_len];
        for (i, (_, _, is_passage)) in self.nodes.iter().enumerate() {
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
                let weight_u16 = (weight_nm * 100.0).round() as u16;
                edge_data.extend_from_slice(&weight_u16.to_le_bytes());
                prev_target = target;
            }
        }
        offsets.push(edge_data.len() as u32);

        RoutingGraph {
            node_lats,
            node_lngs,
            passage_mask,
            offsets,
            edge_data,
            coastline_coords: self.coastline_coords,
            num_nodes,
            num_edges,
        }
    }
}
```

- [ ] **Step 3: Update the existing test helper `square_graph()`**

The existing `square_graph()` helper and tests reference the old API. Replace the test helper (around lines 238-249) and update all old tests to use the new builder API:

```rust
    fn square_graph() -> RoutingGraph {
        let mut b = GraphBuilder::new();
        let n0 = b.add_node(0.0, 0.0, false);
        let n1 = b.add_node(0.0, 1.0, false);
        let n2 = b.add_node(1.0, 0.0, false);
        let n3 = b.add_node(1.0, 1.0, false);
        b.add_edge(n0, n1, 1.0);
        b.add_edge(n1, n3, 1.0);
        b.add_edge(n0, n2, 2.0);
        b.add_edge(n2, n3, 2.0);
        b.build()
    }
```

Update `graph_builder_counts`:
```rust
    #[test]
    fn graph_builder_counts() {
        let g = square_graph();
        assert_eq!(g.num_nodes, 4);
        assert_eq!(g.num_edges, 8); // 4 bidirectional = 8 directed
    }
```

Update `graph_edges` → `graph_neighbors`:
```rust
    #[test]
    fn graph_neighbors() {
        let g = square_graph();
        let n0: Vec<(u32, f32)> = g.neighbors(0).collect();
        // n0 connects to n1 (weight 1.0) and n2 (weight 2.0), sorted by target
        assert_eq!(n0.len(), 2);
        assert_eq!(n0[0].0, 1); // n1
        assert_eq!(n0[1].0, 2); // n2
    }
```

Update `graph_edges_with_weights` → `graph_neighbor_weights`:
```rust
    #[test]
    fn graph_neighbor_weights() {
        let g = square_graph();
        let n0: Vec<(u32, f32)> = g.neighbors(0).collect();
        assert_eq!(n0[0], (1, 1.0));
        assert_eq!(n0[1], (2, 2.0));
    }
```

Update `graph_node_pos_roundtrip`:
```rust
    #[test]
    fn graph_node_pos_roundtrip() {
        let g = square_graph();
        let (lat, lng) = g.node_pos(3);
        assert!((lat - 1.0).abs() < 1e-6);
        assert!((lng - 1.0).abs() < 1e-6);
    }
```

Update `graph_connected_components_single`:
```rust
    #[test]
    fn graph_connected_components_single() {
        let g = square_graph();
        let components = g.connected_components();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0], 4);
    }
```

Update `graph_connected_components_isolated`:
```rust
    #[test]
    fn graph_connected_components_isolated() {
        let mut b = GraphBuilder::new();
        // Component 1: nodes 0-1
        let n0 = b.add_node(0.0, 0.0, false);
        let n1 = b.add_node(0.0, 1.0, false);
        b.add_edge(n0, n1, 1.0);
        // Component 2: nodes 2-3
        let n2 = b.add_node(1.0, 0.0, false);
        let n3 = b.add_node(1.0, 1.0, false);
        b.add_edge(n2, n3, 1.0);
        let g = b.build();
        let mut components = g.connected_components();
        components.sort();
        assert_eq!(components, vec![2, 2]);
    }
```

- [ ] **Step 4: Update `component_labels()` and `connected_components()` to use `neighbors()`**

In `component_labels()` (around line 192), replace the edge iteration that uses `edges_with_weights`:

Find the loop body that iterates edges (something like `for (neighbor, _weight) in self.edges_with_weights(node)`) and replace with:

```rust
        for node in 0..n {
            for (neighbor, _) in self.neighbors(node as u32) {
                union(&mut parent, &mut rank, node, neighbor as usize);
            }
        }
```

- [ ] **Step 5: Run all graph tests**

Run: `cargo test -p asw-core graph -- --nocapture`
Expected: All tests PASS (old tests updated + new tests)

- [ ] **Step 6: Commit**

```bash
git add crates/asw-core/src/graph.rs
git commit -m "feat: rewrite GraphBuilder for compact format with varint edge encoding"
```

---

### Task 4: Save/load with magic header and zstd-19

**Files:**
- Modify: `crates/asw-core/src/graph.rs:120-131` (save/load methods)

- [ ] **Step 1: Write failing round-trip test**

Update the existing `graph_save_load_roundtrip` test:

```rust
    #[test]
    fn graph_save_load_roundtrip() {
        let g = square_graph();

        let mut buf = Vec::new();
        g.save(&mut buf).unwrap();

        // Verify magic header
        assert_eq!(&buf[0..4], b"ASW\x01");

        let loaded = RoutingGraph::load(&buf[..]).unwrap();
        assert_eq!(loaded.num_nodes, g.num_nodes);
        assert_eq!(loaded.num_edges, g.num_edges);
        assert_eq!(loaded.node_lats, g.node_lats);
        assert_eq!(loaded.node_lngs, g.node_lngs);
        assert_eq!(loaded.passage_mask, g.passage_mask);
        assert_eq!(loaded.offsets, g.offsets);
        assert_eq!(loaded.edge_data, g.edge_data);

        // Verify routing works after load
        let neighbors: Vec<(u32, f32)> = loaded.neighbors(0).collect();
        assert_eq!(neighbors.len(), 2);
    }

    #[test]
    fn load_rejects_old_format() {
        // Simulate old format: starts with bincode length prefix, not ASW magic
        let fake_old = vec![4, 0, 0, 0, 0, 0, 0, 0]; // bincode Vec length
        let result = RoutingGraph::load(&fake_old[..]);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ASW"), "Error should mention ASW format: {}", err_msg);
    }
```

- [ ] **Step 2: Implement save/load with magic header**

Replace `save()` and `load()` in `impl RoutingGraph`:

```rust
    const MAGIC: &'static [u8; 4] = b"ASW\x01";

    pub fn save<W: Write>(&self, mut writer: W) -> anyhow::Result<()> {
        writer.write_all(Self::MAGIC)?;
        let encoder = zstd::Encoder::new(writer, 19)?.auto_finish();
        bincode::serialize_into(encoder, self)?;
        Ok(())
    }

    pub fn load<R: Read>(mut reader: R) -> anyhow::Result<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic[..3] != b"ASW" {
            anyhow::bail!(
                "Not an ASW graph file (expected ASW magic header). Rebuild required."
            );
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
        // Verify offsets are monotonically non-decreasing
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
            graph.passage_mask.len() == (n + 7) / 8,
            "passage_mask length {} != expected {}",
            graph.passage_mask.len(),
            (n + 7) / 8
        );

        Ok(graph)
    }
```

- [ ] **Step 3: Run round-trip tests**

Run: `cargo test -p asw-core graph::tests::graph_save_load -- --nocapture`
Run: `cargo test -p asw-core graph::tests::load_rejects -- --nocapture`
Expected: Both PASS

- [ ] **Step 4: Run full asw-core test suite**

Run: `cargo test -p asw-core -- --nocapture`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/asw-core/src/graph.rs
git commit -m "feat: save/load with ASW magic header, zstd-19, post-load validation"
```

---

## Chunk 2: Routing and Serve Updates

### Task 5: Update routing.rs

**Files:**
- Modify: `crates/asw-core/src/routing.rs:21-75` (astar function)
- Modify: `crates/asw-core/src/routing.rs:152-192` (compute_route)
- Modify: `crates/asw-core/src/routing.rs:199-210` (test helper)

- [ ] **Step 1: Update the `diamond_graph()` test helper**

Replace `diamond_graph()` (lines 199-210):

```rust
    fn diamond_graph() -> RoutingGraph {
        let mut b = GraphBuilder::new();
        let n0 = b.add_node(0.0, 0.0, false);
        let n1 = b.add_node(1.0, -1.0, false);
        let n2 = b.add_node(1.0, 1.0, false);
        let n3 = b.add_node(2.0, 0.0, false);
        b.add_edge(n0, n1, 1.0); // short path via n1
        b.add_edge(n1, n3, 1.0);
        b.add_edge(n0, n2, 5.0); // long path via n2
        b.add_edge(n2, n3, 5.0);
        b.build()
    }
```

- [ ] **Step 2: Update the `astar_unreachable` test helper**

The `astar_unreachable` test (around line 234) also calls `GraphBuilder::add_node()` with the old 2-arg signature. Update to 3-arg:

```rust
    #[test]
    fn astar_unreachable() {
        let mut b = GraphBuilder::new();
        let n0 = b.add_node(0.0, 0.0, false);
        let n1 = b.add_node(1.0, 1.0, false);
        // No edges — n1 is unreachable from n0
        let g = b.build();
        let result = astar(&g, n0, n1);
        assert!(result.is_none());
    }
```

- [ ] **Step 3: Update A* to use `neighbors()` and i32 coord decoding**

In the `astar` function (lines 21-75), find where it calls `graph.edges_with_weights(current)` and replace with `graph.neighbors(current)`. The iteration body stays the same since `neighbors()` yields `(u32, f32)` just like `edges_with_weights()` did.

Find where it calls `graph.node_pos()` for the heuristic — this should work unchanged since `node_pos()` was updated to decode i32.

The key change in `astar`: replace `graph.edges_with_weights(current)` with `graph.neighbors(current)`.

- [ ] **Step 4: Update `compute_route` if it accesses graph fields directly**

In `compute_route` (lines 152-192), verify it uses `graph.node_pos()` for coordinate lookups (not direct array access to `node_lats`/`node_lngs`). If it accesses `graph.node_lats[i] as f64` directly, replace with `graph.node_pos(i as u32)`.

- [ ] **Step 5: Run routing tests**

Run: `cargo test -p asw-core routing -- --nocapture`
Expected: All 3 tests PASS (astar_shortest_path, astar_same_node, astar_unreachable)

- [ ] **Step 6: Commit**

```bash
git add crates/asw-core/src/routing.rs
git commit -m "refactor: routing uses neighbors() iterator and i32 coord decoding"
```

---

### Task 6: Update state.rs (serve)

**Files:**
- Modify: `crates/asw-serve/src/state.rs:36-69` (AppState::new)
- Modify: `crates/asw-serve/src/state.rs:72-82` (nearest_node)

- [ ] **Step 1: Update AppState::new() to decode i32 coords**

In `AppState::new()` (lines 36-69), find where it builds the R-tree from node positions. It currently reads `graph.node_lats[i] as f64` and `graph.node_lngs[i] as f64`. Replace with `graph.node_pos(i as u32)`:

```rust
        let points: Vec<GeomWithData<[f64; 2], u32>> = (0..graph.num_nodes as u32)
            .map(|i| {
                let (lat, lng) = graph.node_pos(i);
                GeomWithData::new([lng, lat], i)
            })
            .collect();
```

- [ ] **Step 2: Update `component_labels` usage if it calls graph methods**

Verify `connected_components` / `component_labels` is called via `graph.component_labels()` — this was already updated in Task 3 to use `neighbors()`.

- [ ] **Step 3: Verify serve compiles**

Run: `cargo build -p asw-serve`
Expected: Compiles without errors

- [ ] **Step 4: Commit**

```bash
git add crates/asw-serve/src/state.rs
git commit -m "refactor: serve decodes i32 fixed-point coords for R-tree and KNN"
```

---

## Chunk 3: Build Pipeline

### Task 7: Update pipeline.rs

**Files:**
- Modify: `crates/asw-build/src/pipeline.rs:53-67` (graph building section)

The pipeline currently calls `GraphBuilder::add_node_with_cell()` and `add_edge()`. We need to:
1. Change `add_node_with_cell(lat, lng, cell)` → `add_node(lat, lng, is_passage)` where `is_passage = false` for H3 nodes
2. Sort nodes by H3 cell index before adding (for better compression)
3. Handle passage nodes from the passages module

- [ ] **Step 1: Read passages.rs to understand how passage nodes are added**

Check `crates/asw-core/src/passages.rs` to see how passages add nodes and edges to the graph builder. Note the current API calls.

- [ ] **Step 2: Update pipeline.rs node addition**

In `pipeline.rs` lines 53-67, the code iterates over `cells` (a `HashMap<CellIndex, u32>`) and adds nodes. Change the approach:

1. Collect cells into a `Vec` and sort by cell index (u64) for spatial ordering
2. Create a mapping from old cell → new node ID (after sorting)
3. Call `builder.add_node(lat, lng, false)` for H3 nodes
4. Passage nodes call `builder.add_node(lat, lng, true)`

Replace the graph building section:

```rust
    // Sort cells by H3 index for spatial ordering (better compression)
    let mut sorted_cells: Vec<(CellIndex, u32)> = cells.iter().map(|(&c, &id)| (c, id)).collect();
    sorted_cells.sort_by_key(|(cell, _)| u64::from(*cell));

    // Build node ID remapping: old_id -> new_id
    let mut id_remap = vec![0u32; sorted_cells.len()];
    let mut builder = GraphBuilder::new();
    for (i, (cell, old_id)) in sorted_cells.iter().enumerate() {
        let (lat, lng) = asw_core::h3::cell_center(*cell);
        let new_id = builder.add_node(lat, lng, false);
        debug_assert_eq!(new_id, i as u32);
        id_remap[*old_id as usize] = new_id;
    }
```

Then remap edge source/target IDs:

```rust
    // Add edges with remapped IDs
    for &(src, dst, weight) in &edges {
        builder.add_edge(id_remap[src as usize], id_remap[dst as usize], weight);
    }
```

Passage nodes are added after H3 nodes — they get `is_passage = true`:

```rust
    // Add passage nodes and edges (is_passage = true)
    // ... (adapted from current passage handling code)
```

- [ ] **Step 3: Update passage node addition**

Check how passages currently call `builder.add_node()` vs `builder.add_node_with_cell()`. Replace `add_node_with_cell(lat, lng, 0)` calls with `add_node(lat, lng, true)` and `add_node_with_cell(lat, lng, cell)` calls with `add_node(lat, lng, false)`.

- [ ] **Step 4: Store coastline_coords on the builder**

Verify `builder.coastline_coords = coastline_coords;` still works (the field name didn't change).

- [ ] **Step 5: Build and test**

Run: `cargo build --release -p asw-cli`
Expected: Compiles without errors

- [ ] **Step 6: Run unit tests**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/asw-build/src/pipeline.rs
git commit -m "feat: pipeline sorts nodes by H3 cell, produces compact graph format"
```

---

### Task 8: Update edges.rs (minimal)

**Files:**
- Modify: `crates/asw-build/src/edges.rs:12` (Edge type alias)

- [ ] **Step 1: Verify Edge type alias is still compatible**

The `Edge` type is `(u32, u32, f32)` — source, target, weight. This hasn't changed. The pipeline remaps IDs after edge building, so `edges.rs` doesn't need changes.

Run: `cargo build -p asw-build`
Expected: Compiles without errors

- [ ] **Step 2: Commit (skip if no changes needed)**

Only commit if changes were required.

---

## Chunk 4: CLI Updates

### Task 9: Update GeoJSON export

**Files:**
- Modify: `crates/asw-cli/src/main.rs:437-651` (export_geojson function)

This is the most involved CLI change: reconstruct H3 cells from i32 coords, use `passage_mask`, decode edges via `neighbors()`.

- [ ] **Step 1: Update hex polygon generation (lines 464-500)**

Replace `graph.node_cells[i]` lookup with on-the-fly H3 cell reconstruction:

```rust
    // Hex polygons — reconstruct H3 cells from i32 fixed-point coords
    let mut hex_count: u64 = 0;
    for i in 0..graph.num_nodes as usize {
        if graph.is_passage(i as u32) {
            continue; // synthetic node, no hex
        }

        let (lat, lng) = graph.node_pos(i as u32);

        // Bbox filter
        if let Some((min_lon, min_lat, max_lon, max_lat)) = bbox {
            if lng < min_lon || lng > max_lon || lat < min_lat || lat > max_lat {
                continue;
            }
        }

        // Reconstruct H3 cell: try resolutions 3-13, match by i32 round-trip
        // Range covers main cascade (3-10) plus passage zones (up to res-13 for Corinth)
        let stored_lat = graph.node_lats[i];
        let stored_lng = graph.node_lngs[i];
        let mut found_cell = None;
        for res in asw_core::H3_RES_BASE..=13 {
            let res_enum = h3o::Resolution::try_from(res).unwrap();
            if let Some(cell) = asw_core::h3::lat_lng_to_cell(lat, lng, res_enum) {
                let (clat, clng) = asw_core::h3::cell_center(cell);
                let clat_i32 = (clat * 1e7).round() as i32;
                let clng_i32 = (clng * 1e7).round() as i32;
                if clat_i32 == stored_lat && clng_i32 == stored_lng {
                    found_cell = Some(cell);
                    break;
                }
            }
        }

        let Some(cell) = found_cell else {
            tracing::warn!("Could not reconstruct H3 cell for node {i}");
            continue;
        };

        let boundary = asw_core::h3::cell_boundary(cell);
        let res = cell.resolution() as u8;
        // ... rest unchanged (color, feat, layers)
```

- [ ] **Step 2: Update passage edge iteration (lines 502-541)**

Replace direct `graph.offsets`/`graph.adjacency`/`graph.weights` access with `neighbors()` and `is_passage()`:

```rust
    // Passage edges
    for src in 0..graph.num_nodes as u32 {
        let src_is_passage = graph.is_passage(src);
        for (dst, weight_nm) in graph.neighbors(src) {
            let dst_is_passage = graph.is_passage(dst);
            if !src_is_passage && !dst_is_passage {
                continue;
            }
            if src >= dst {
                continue; // deduplicate bidirectional
            }
            let (src_lat, src_lon) = graph.node_pos(src);
            let (dst_lat, dst_lon) = graph.node_pos(dst);

            // Bbox filter for passage edges
            if let Some((min_lon, min_lat, max_lon, max_lat)) = bbox {
                let in_bbox = |lat: f64, lon: f64| {
                    lon >= min_lon && lon <= max_lon && lat >= min_lat && lat <= max_lat
                };
                if !in_bbox(src_lat, src_lon) && !in_bbox(dst_lat, dst_lon) {
                    continue;
                }
            }

            let feat = passage_feature_string(src_lon, src_lat, dst_lon, dst_lat, weight_nm);
            layers[LAYER_PASSAGES].push(feat);
        }
    }
```

- [ ] **Step 3: Update passage_feature_string if needed**

The function signature `fn passage_feature_string(src_lon: f64, src_lat: f64, dst_lon: f64, dst_lat: f64, weight: f32)` — verify the coordinate types match. `node_pos()` returns f64, so this should work.

- [ ] **Step 4: Update coastline coord access if needed**

`graph.coastline_coords` is still `Vec<Vec<(f32, f32)>>` — unchanged. Verify the coastline section (lines 546-568) compiles.

- [ ] **Step 5: Build**

Run: `cargo build -p asw-cli`
Expected: Compiles without errors

- [ ] **Step 6: Commit**

```bash
git add crates/asw-cli/src/main.rs
git commit -m "refactor: GeoJSON export reconstructs H3 cells, uses passage_mask and neighbors()"
```

---

### Task 10: Update bench.rs

**Files:**
- Modify: `crates/asw-cli/src/bench.rs`

- [ ] **Step 1: Check bench.rs for direct graph field access**

The bench module accesses `graph.num_nodes`, `graph.num_edges` for metadata, and uses `AppState` for routing. Since `AppState::new()` was updated in Task 6, and `RouteResult` fields (`distance_nm`, `raw_hops`, etc.) are unchanged, bench.rs should mostly compile as-is.

Check for any direct access to `graph.node_lats`, `graph.node_lngs`, `graph.node_cells`, `graph.adjacency`, or `graph.weights`. If found, update to use `graph.node_pos()` or `graph.neighbors()`.

- [ ] **Step 2: Fix any compilation errors**

Make minimal changes to fix compile errors. The bench module primarily uses `AppState` and `RouteResult` which are already updated.

- [ ] **Step 3: Build and test**

Run: `cargo build --release -p asw-cli`
Expected: Compiles without errors

- [ ] **Step 4: Commit (if changes needed)**

```bash
git add crates/asw-cli/src/bench.rs
git commit -m "refactor: bench adapts to compact graph format"
```

---

### Task 11: Final integration verification

- [ ] **Step 1: Full workspace test suite**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 2: Full release build**

Run: `cargo build --release -p asw-cli`
Expected: Clean compilation, no warnings

- [ ] **Step 3: Smoke test with a small region build (if shapefile available)**

If land polygon data is available:

```bash
./target/release/asw build --shp path/to/land-polygons-split-4326 --bbox dev-small --output export/compact-test.graph
```

Verify:
- Graph file has `ASW\x01` magic header: `xxd export/compact-test.graph | head -1`
- File is smaller than equivalent old-format graph
- GeoJSON export works: `./target/release/asw geojson --graph export/compact-test.graph --bbox dev-small --output export/compact-test.geojson`

- [ ] **Step 4: Commit any final fixes**

```bash
git commit -m "test: verify compact graph format end-to-end"
```
