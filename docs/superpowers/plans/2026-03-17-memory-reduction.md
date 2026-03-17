# Memory Reduction Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce server memory from ~6.2 GiB steady / 8.7 GiB peak to ~4.2 GiB steady / 4.2 GiB peak for the global 40M-node routing graph.

**Architecture:** Replace 4 node identity vecs + 1.1 GB RTree with a single sorted `Vec<u64>` of H3 indices that serves as both coordinate store and spatial index. Add pre-allocated A* buffer pool to eliminate per-request allocation spikes. Switch serialization from deprecated bincode 1 to bitcode.

**Tech Stack:** Rust, h3o, bitcode, tokio, axum

**Spec:** `docs/superpowers/specs/2026-03-17-memory-reduction-design.md`

---

## Chunk 1: Switch serialization from bincode to bitcode

This is the lowest-risk change and touches the serialization boundary only. Do it first so all subsequent graph format changes use the new serializer.

### Task 1: Replace bincode with bitcode in workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace deps, line 19)
- Modify: `crates/asw-core/Cargo.toml` (line 13)

- [ ] **Step 1: Update workspace Cargo.toml**

In `Cargo.toml`, replace:
```toml
bincode = "1"
```
with:
```toml
bitcode = "0.6"
```

- [ ] **Step 2: Update asw-core Cargo.toml**

In `crates/asw-core/Cargo.toml`, replace:
```toml
bincode.workspace = true
```
with:
```toml
bitcode.workspace = true
```

- [ ] **Step 3: Verify it compiles (expect errors in graph.rs)**

Run: `cargo check -p asw-core 2>&1 | head -20`
Expected: Compile errors in `graph.rs` where `bincode::` is referenced. This confirms the dependency swap worked.

### Task 2: Migrate graph serialization to bitcode

**Files:**
- Modify: `crates/asw-core/src/graph.rs:10-25` (add Encode/Decode derives)
- Modify: `crates/asw-core/src/graph.rs:170-195` (save/load methods)

- [ ] **Step 1: Add bitcode derives to RoutingGraph**

In `graph.rs`, add `bitcode::Encode` and `bitcode::Decode` to the derive list on `RoutingGraph` (line 9). Also add `use bitcode;` at the top if not already imported via serde.

Note: bitcode works via serde — the existing `#[derive(Serialize, Deserialize)]` is sufficient. No additional derives needed. Just change the function calls.

- [ ] **Step 2: Update save() method**

In `graph.rs` lines 174-178, replace:
```rust
pub fn save<W: Write>(&self, mut writer: W) -> anyhow::Result<()> {
    writer.write_all(Self::MAGIC)?;
    let encoder = zstd::Encoder::new(writer, 19)?.auto_finish();
    bincode::serialize_into(encoder, self)?;
    Ok(())
}
```
with:
```rust
pub fn save<W: Write>(&self, mut writer: W) -> anyhow::Result<()> {
    writer.write_all(Self::MAGIC)?;
    let encoded = bitcode::serde::serialize(self)?;
    let mut encoder = zstd::Encoder::new(writer, 19)?;
    encoder.write_all(&encoded)?;
    encoder.finish()?;
    Ok(())
}
```

Add `use std::io::Write;` if not already imported.

- [ ] **Step 3: Update load() method**

In `graph.rs` lines 182-195, replace:
```rust
let decoder = zstd::Decoder::new(reader)?;
let graph: Self = bincode::deserialize_from(decoder)?;
```
with:
```rust
let mut decoder = zstd::Decoder::new(reader)?;
let mut buf = Vec::new();
decoder.read_to_end(&mut buf)?;
let graph: Self = bitcode::serde::deserialize(&buf)?;
```

Add `use std::io::Read;` if not already imported (it likely is, since `read_exact` is used above).

- [ ] **Step 4: Bump magic version**

In `graph.rs` line 171, change:
```rust
const MAGIC: &'static [u8; 4] = b"ASW\x01";
```
to:
```rust
const MAGIC: &'static [u8; 4] = b"ASW\x02";
```

In the load method (around line 188), update the version check:
```rust
if magic[3] != 2 {
    anyhow::bail!(
        "Unsupported ASW graph version {} (expected 2). Rebuild required.",
        magic[3]
    );
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p asw-core`
Expected: All tests pass. The save/load roundtrip test (`test_graph_save_load_roundtrip`) validates the new serializer.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/asw-core/Cargo.toml crates/asw-core/src/graph.rs
git commit -m "refactor: switch graph serialization from bincode to bitcode"
```

---

## Chunk 2: Replace node coordinate vecs with `node_h3: Vec<u64>`

This changes the core graph struct. All consumers of `node_lats`, `node_lngs`, `node_resolutions`, and `passage_mask` must be updated.

### Task 3: Update RoutingGraph struct and builder

**Files:**
- Modify: `crates/asw-core/src/graph.rs:10-25` (struct fields)
- Modify: `crates/asw-core/src/graph.rs:54-168` (GraphBuilder)

- [ ] **Step 1: Change RoutingGraph struct**

Replace the struct definition (lines 10-25) with:
```rust
#[derive(Serialize, Deserialize)]
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
```

- [ ] **Step 2: Update GraphBuilder**

Replace the builder struct (lines 54-61) with:
```rust
pub struct GraphBuilder {
    /// (h3_index, lat_deg, lng_deg) per node — lat/lng kept for edge weight calculation
    nodes: Vec<(u64, f64, f64)>,
    /// (src, dst, weight_nm)
    edges: Vec<(u32, u32, f32)>,
    pub coastline_coords: Vec<Vec<(f32, f32)>>,
}
```

- [ ] **Step 3: Update GraphBuilder::new()**

Update `new()` (line 70) to match the new fields:
```rust
pub fn new() -> Self {
    Self {
        nodes: Vec::new(),
        edges: Vec::new(),
        coastline_coords: Vec::new(),
    }
}
```

- [ ] **Step 4: Update GraphBuilder::add_node()**

Replace `add_node` (line 80) with:
```rust
/// Add a node. Returns node ID.
pub fn add_node(&mut self, h3_index: u64, lat: f64, lng: f64) -> u32 {
    let id = self.nodes.len() as u32;
    self.nodes.push((h3_index, lat, lng));
    id
}
```

- [ ] **Step 5: Update GraphBuilder::build()**

Adapt the existing `build()` method (lines 98-168) minimally. **Do NOT rewrite the CSR encoding from scratch** — the varint encoding logic is delicate and must be preserved exactly.

Changes:
- Replace `node_lats`/`node_lngs` construction with `node_h3`
- Remove `node_resolutions` and `passage_mask` construction
- Keep ALL edge encoding logic (varint + u16 weight at `* 100.0` scale) byte-identical

Specifically, replace the node-related lines (approx 100-124) with:
```rust
let node_h3: Vec<u64> = self.nodes.iter().map(|(h3, _, _)| *h3).collect();
```

And update the final struct construction to use the new fields:
```rust
RoutingGraph {
    node_h3,
    offsets,
    edge_data,
    coastline_coords: self.coastline_coords,
    num_nodes,
    num_edges,
}
```

**Important:** The weight encoding uses `* 100.0` (not `* 10.0`). Do not change this — it matches the decoder at line 48 which divides by `100.0`.

- [ ] **Step 6: Update node_pos() method**

Replace `node_pos()` (lines 266-271) with:
```rust
/// Decode node position from H3 cell index to (lat, lng) in degrees.
pub fn node_pos(&self, node: u32) -> (f64, f64) {
    let h3 = self.node_h3[node as usize];
    let cell = h3o::CellIndex::try_from(h3).expect("invalid H3 index");
    let ll = h3o::LatLng::from(cell);
    (ll.lat(), ll.lng())
}
```

Note: Use `h3o::LatLng::from(cell)`, not `cell.to_lat_lng()` — this matches the existing `cell_center()` helper in `crates/asw-core/src/h3.rs:5`.

- [ ] **Step 7: Remove is_passage() and nearest_node()**

Delete `is_passage()` (lines 260-263) — no longer needed, no callers after passage_mask removal.

Delete `nearest_node()` (lines 275-290) — brute-force fallback, replaced by H3 lookup in AppState.

- [ ] **Step 8: Update drop_coastline_coords()**

This method (line 306) stays unchanged — it still clears `self.coastline_coords`.

- [ ] **Step 9: Update load() validation**

Replace the post-deserialization validation (lines 197-244) with:
```rust
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

// Validate H3 indices
for (i, &h3) in graph.node_h3.iter().enumerate() {
    anyhow::ensure!(
        h3o::CellIndex::try_from(h3).is_ok(),
        "invalid H3 index at node {}",
        i
    );
}

// Validate strict sorted order (no duplicates)
for w in graph.node_h3.windows(2) {
    anyhow::ensure!(
        w[0] < w[1],
        "node_h3 not strictly sorted: {} >= {}",
        w[0],
        w[1]
    );
}

// Validate edge targets
for src in 0..graph.num_nodes {
    for (target, _) in graph.neighbors(src) {
        anyhow::ensure!(
            target < graph.num_nodes,
            "edge target {} >= num_nodes {}",
            target,
            graph.num_nodes
        );
    }
}
```

- [ ] **Step 10: Update all tests in graph.rs**

The following tests need updating to use the new struct:

`square_graph()` helper (line 367): Use `add_node(h3_index, lat, lng)` instead of `add_node(lat, lng, false, 5)`. Use real H3 indices from `h3o::CellIndex`:
```rust
fn square_graph() -> RoutingGraph {
    let mut b = GraphBuilder::new();
    // Use real H3 res-5 cells for test points
    let c0 = h3o::LatLng::new(51.5, 0.0).expect("valid").to_cell(h3o::Resolution::Five);
    let c1 = h3o::LatLng::new(51.5, 2.0).expect("valid").to_cell(h3o::Resolution::Five);
    let c2 = h3o::LatLng::new(52.5, 0.0).expect("valid").to_cell(h3o::Resolution::Five);
    let c3 = h3o::LatLng::new(52.5, 2.0).expect("valid").to_cell(h3o::Resolution::Five);
    // Sort by H3 index to match production behavior
    let mut cells = vec![(c0, 51.5, 0.0), (c1, 51.5, 2.0), (c2, 52.5, 0.0), (c3, 52.5, 2.0)];
    cells.sort_by_key(|(c, _, _)| u64::from(*c));
    let n0 = b.add_node(u64::from(cells[0].0), cells[0].1, cells[0].2);
    let n1 = b.add_node(u64::from(cells[1].0), cells[1].1, cells[1].2);
    let n2 = b.add_node(u64::from(cells[2].0), cells[2].1, cells[2].2);
    let n3 = b.add_node(u64::from(cells[3].0), cells[3].1, cells[3].2);
    b.add_edge(n0, n1, 10.0);
    b.add_edge(n1, n3, 10.0);
    b.add_edge(n0, n2, 15.0);
    b.add_edge(n2, n3, 15.0);
    b.build()
}
```

Delete `test_is_passage_bitset` — passage_mask no longer exists.

Update all other tests that directly construct `RoutingGraph` structs (tests at lines 473, 524, 542) to use the new field layout.

Update `test_graph_save_load_roundtrip` to check `node_h3` instead of `node_lats`/`node_lngs`/`node_resolutions`/`passage_mask`.

Update `test_load_rejects_old_format` to check for version 2.

- [ ] **Step 11: Run tests**

Run: `cargo test -p asw-core`
Expected: All tests pass (except possibly routing tests if they depend on exact coordinates — check next).

- [ ] **Step 12: Commit**

```bash
git add crates/asw-core/src/graph.rs
git commit -m "refactor: replace node coordinate vecs with node_h3 Vec<u64>"
```

### Task 4: Update build pipeline to emit H3 indices

**Files:**
- Modify: `crates/asw-build/src/pipeline.rs:52-74` (graph building section)

- [ ] **Step 1: Update node addition in pipeline.rs**

The current code (lines 52-74) sorts cells by H3 index and adds nodes with `add_node(lat, lng, false, res)`. Update to:

```rust
// Sort cells by H3 index for binary-search lookup at serve time.
let mut sorted_cells: Vec<_> = cells.keys().copied().collect();
sorted_cells.sort_by_key(|c| u64::from(*c));

// Build node-ID mapping: cell → sequential node ID
let cell_to_node: HashMap<CellIndex, u32> = sorted_cells
    .iter()
    .enumerate()
    .map(|(i, &cell)| (cell, i as u32))
    .collect();

let mut builder = GraphBuilder::new();
for &cell in &sorted_cells {
    let center = cell.to_lat_lng();
    builder.add_node(u64::from(cell), center.lat(), center.lng());
}
```

Use the existing `cell_center()` helper from `asw_core::h3` which calls `LatLng::from(cell)`.

**Note:** `crates/asw-build/src/cells.rs` and `crates/asw-build/src/edges.rs` need no changes — they work with `CellIndex` values and `cell_center()`, which are unaffected by the graph struct changes.

- [ ] **Step 2: Verify edge addition is unchanged**

The edge addition loop (lines 69-71) uses `cell_to_node` mapping which is unchanged in structure. Verify it still works:
```rust
for (src_cell, dst_cell, weight) in &edges {
    let src = cell_to_node[src_cell];
    let dst = cell_to_node[dst_cell];
    builder.add_edge(src, dst, *weight);
}
```

- [ ] **Step 3: Run build pipeline test**

Run: `cargo test -p asw-build`
Expected: Tests pass. If there are integration tests that build a graph, they should work with the new format.

- [ ] **Step 4: Commit**

```bash
git add crates/asw-build/src/pipeline.rs
git commit -m "refactor: emit H3 cell indices in build pipeline"
```

### Task 5: Update GeoJSON export

**Files:**
- Modify: `crates/asw-cli/src/main.rs:465-681` (export_geojson function)
- Modify: `crates/asw-cli/src/main.rs:391-443` (helper functions)

- [ ] **Step 1: Update hex polygon export**

In `export_geojson()` (around line 492), the current code reconstructs H3 cells from fixed-point coordinates and resolution. Replace with direct H3 index usage:

```rust
for i in 0..graph.num_nodes {
    let h3 = graph.node_h3[i as usize];
    let cell = h3o::CellIndex::try_from(h3).expect("valid H3");
    let res = cell.resolution() as u8;
    let boundary = cell.boundary();
    // ... render hex polygon using boundary
}
```

- [ ] **Step 2: Remove passage edges layer**

Delete the passage edges export section (around lines 544-574) and the `passage_feature_string()` helper (lines 416-428). No nodes have passage flag set in production graphs.

Remove the passage layer from the file writing section (around lines 608-647).

- [ ] **Step 3: Update hex_feature_string() if needed**

The `hex_feature_string()` function (line 391) takes boundary coordinates and resolution. It may need adjustment to accept `h3o::Boundary` directly instead of reconstructed coordinates. Check the current signature and adapt.

- [ ] **Step 4: Run the CLI build**

Run: `cargo build -p asw-cli`
Expected: Compiles cleanly.

- [ ] **Step 5: Commit**

```bash
git add crates/asw-cli/src/main.rs
git commit -m "refactor: use H3 indices for GeoJSON export, remove passage layer"
```

---

## Chunk 3: Replace RTree with H3 binary search in AppState

### Task 6: Update AppState to use H3 lookup

**Files:**
- Modify: `crates/asw-serve/src/state.rs:1-112` (full rewrite of AppState)
- Modify: `crates/asw-serve/Cargo.toml` (remove rstar)

- [ ] **Step 1: Remove rstar from asw-serve**

In `crates/asw-serve/Cargo.toml`, remove:
```toml
rstar.workspace = true
```

- [ ] **Step 2: Rewrite AppState struct**

Replace the AppState definition and implementation (lines 48-112):

```rust
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

        // No RTree construction — binary search on graph.node_h3 is the spatial index.

        Self {
            graph,
            coastline,
            component_labels,
            main_component,
        }
    }

    /// Find nearest node in the main connected component using H3 index lookup.
    ///
    /// Iterates from finest to coarsest resolution, converting the input
    /// lat/lon to an H3 cell and searching for it (or its k-ring neighbors)
    /// in the sorted node_h3 vec via binary search.
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

    /// Binary search for an H3 index in the sorted node_h3 vec.
    /// Returns the node ID (array index) if found.
    fn h3_lookup(&self, h3: u64) -> Option<u32> {
        self.graph
            .node_h3
            .binary_search(&h3)
            .ok()
            .map(|i| i as u32)
    }
}
```

- [ ] **Step 3: Remove rstar imports from state.rs**

Remove the `use rstar::...` import at the top of `state.rs` (line 3).

- [ ] **Step 4: Add tests for nearest_node H3 lookup**

Add tests in `state.rs` to verify the H3 binary search works:

```rust
#[cfg(test)]
mod nearest_node_tests {
    use super::*;

    fn test_graph() -> RoutingGraph {
        let mut b = asw_core::graph::GraphBuilder::new();
        // Create a small graph with known H3 cells
        let c0 = h3o::LatLng::new(51.5, 0.0).unwrap().to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(51.5, 2.0).unwrap().to_cell(h3o::Resolution::Five);
        let mut cells = vec![(c0, 51.5, 0.0), (c1, 51.5, 2.0)];
        cells.sort_by_key(|(c, _, _)| u64::from(*c));
        for (c, lat, lng) in &cells {
            b.add_node(u64::from(*c), *lat, *lng);
        }
        b.add_edge(0, 1, 10.0);
        b.build()
    }

    #[test]
    fn nearest_node_finds_exact_cell() {
        let graph = test_graph();
        let app = AppState::new(graph);
        // Query at exact cell center should find a node
        let result = app.nearest_node(51.5, 0.0);
        assert!(result.is_some());
    }

    #[test]
    fn nearest_node_finds_nearby_cell() {
        let graph = test_graph();
        let app = AppState::new(graph);
        // Query slightly offset from cell center should still snap
        let result = app.nearest_node(51.501, 0.001);
        assert!(result.is_some());
    }

    #[test]
    fn nearest_node_returns_none_for_empty_graph() {
        let b = asw_core::graph::GraphBuilder::new();
        let graph = b.build();
        let app = AppState::new(graph);
        assert!(app.nearest_node(51.5, 0.0).is_none());
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p asw-serve`
Expected: All tests pass.

- [ ] **Step 6: Run workspace build**

Run: `cargo build --workspace`
Expected: Clean compilation. This catches any remaining references to removed fields.

- [ ] **Step 6: Commit**

```bash
git add crates/asw-serve/src/state.rs crates/asw-serve/Cargo.toml
git commit -m "feat: replace RTree with H3 binary search for nearest-node lookup"
```

### Task 7: Update bench module

**Files:**
- Modify: `crates/asw-cli/src/bench.rs:169-211` (resolve_routes)
- Modify: `crates/asw-cli/src/bench.rs:636-654` (run function, AppState construction)

- [ ] **Step 1: Update bench to use new AppState**

The bench module constructs `AppState` at line 652-654. Since `AppState::new()` signature is unchanged (takes `RoutingGraph`), this should compile without changes.

The `resolve_routes()` function (line 169) calls `app.nearest_node(lat, lon)` which has the same signature. Verify it compiles:

Run: `cargo build -p asw-cli`
Expected: Clean compilation.

- [ ] **Step 2: Commit (if changes needed)**

```bash
git add crates/asw-cli/src/bench.rs
git commit -m "refactor: adapt bench module to new AppState"
```

---

## Chunk 4: A* buffer pool

### Task 8: Create AstarBufferPool

**Files:**
- Create: `crates/asw-core/src/astar_pool.rs`
- Modify: `crates/asw-core/src/lib.rs` (add module)

- [ ] **Step 1: Write the buffer pool module**

Create `crates/asw-core/src/astar_pool.rs`:

```rust
/// Pre-allocated buffers for A* search to avoid per-request allocation.
pub struct AstarBuffers {
    pub g_score: Vec<f32>,
    pub came_from: Vec<u32>,
    pub closed: Vec<bool>,
}

impl AstarBuffers {
    /// Create a new buffer set sized for `num_nodes`.
    pub fn new(num_nodes: usize) -> Self {
        Self {
            g_score: vec![f32::MAX; num_nodes],
            came_from: vec![u32::MAX; num_nodes],
            closed: vec![false; num_nodes],
        }
    }

    /// Reset all buffers to initial state (no reallocation).
    pub fn reset(&mut self) {
        self.g_score.fill(f32::MAX);
        self.came_from.fill(u32::MAX);
        self.closed.fill(false);
    }
}

/// A pool of reusable A* buffer sets backed by a tokio channel.
pub struct AstarPool {
    tx: tokio::sync::mpsc::Sender<AstarBuffers>,
    rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<AstarBuffers>>,
}

impl AstarPool {
    /// Create a pool with `size` pre-allocated buffer sets.
    pub fn new(num_nodes: usize, size: usize) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(size);
        for _ in 0..size {
            tx.try_send(AstarBuffers::new(num_nodes)).expect("channel has capacity");
        }
        Self {
            tx,
            rx: tokio::sync::Mutex::new(rx),
        }
    }

    /// Acquire a buffer set from the pool. Waits if none available.
    pub async fn acquire(&self) -> AstarBuffers {
        self.rx.lock().await.recv().await.expect("pool channel closed")
    }

    /// Return a buffer set to the pool after use.
    pub async fn release(&self, mut buf: AstarBuffers) {
        buf.reset();
        let _ = self.tx.send(buf).await;
    }
}
```

- [ ] **Step 2: Add module to lib.rs**

In `crates/asw-core/src/lib.rs`, add:
```rust
pub mod astar_pool;
```

- [ ] **Step 3: Add tokio dependency to asw-core**

In `crates/asw-core/Cargo.toml`, add:
```toml
tokio = { version = "1", features = ["sync"] }
```

- [ ] **Step 4: Add tests for AstarBuffers and AstarPool**

Add at the bottom of `astar_pool.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_new_initializes_correctly() {
        let buf = AstarBuffers::new(100);
        assert_eq!(buf.g_score.len(), 100);
        assert_eq!(buf.came_from.len(), 100);
        assert_eq!(buf.closed.len(), 100);
        assert!(buf.g_score.iter().all(|&v| v == f32::MAX));
        assert!(buf.came_from.iter().all(|&v| v == u32::MAX));
        assert!(buf.closed.iter().all(|&v| !v));
    }

    #[test]
    fn buffers_reset_clears_state() {
        let mut buf = AstarBuffers::new(10);
        buf.g_score[0] = 0.0;
        buf.came_from[0] = 5;
        buf.closed[0] = true;
        buf.reset();
        assert_eq!(buf.g_score[0], f32::MAX);
        assert_eq!(buf.came_from[0], u32::MAX);
        assert!(!buf.closed[0]);
    }

    #[tokio::test]
    async fn pool_acquire_and_release() {
        let pool = AstarPool::new(10, 2);
        let buf = pool.acquire().await;
        assert_eq!(buf.g_score.len(), 10);
        pool.release(buf).await;
        // Should be able to acquire again
        let _buf = pool.acquire().await;
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p asw-core`
Expected: Compiles and all tests pass (including new pool tests).

- [ ] **Step 6: Commit**

```bash
git add crates/asw-core/src/astar_pool.rs crates/asw-core/src/lib.rs crates/asw-core/Cargo.toml
git commit -m "feat: add AstarPool for pre-allocated A* search buffers"
```

### Task 9: Integrate buffer pool into A* and AppState

**Files:**
- Modify: `crates/asw-core/src/routing.rs:21-75` (astar function)
- Modify: `crates/asw-serve/src/state.rs` (AppState)
- Modify: `crates/asw-serve/src/api.rs` (route handler)

- [ ] **Step 1: Update astar() to accept buffers**

In `routing.rs`, change the `astar` function signature (line 21) to accept mutable buffers:

```rust
pub fn astar(
    graph: &RoutingGraph,
    start: u32,
    goal: u32,
    buffers: &mut crate::astar_pool::AstarBuffers,
) -> Option<(Vec<u32>, f64)> {
    let g_score = &mut buffers.g_score;
    let came_from = &mut buffers.came_from;
    let closed = &mut buffers.closed;

    g_score[start as usize] = 0.0;
    // ... rest of algorithm unchanged, just use the references above
    // instead of the local Vec allocations
```

Remove the three `vec![]` allocations (lines 23-25) since they now come from the buffer.

Important: at the end of the function, do NOT reset the buffers — the caller (pool) handles that via `release()`.

- [ ] **Step 2: Update compute_route() to accept buffers**

In `routing.rs`, update `compute_route()` (line 152) to thread the buffers through:

```rust
pub fn compute_route(
    graph: &RoutingGraph,
    from_lat: f64,
    from_lon: f64,
    to_lat: f64,
    to_lon: f64,
    coastline: &CoastlineIndex,
    node_knn: impl Fn(f64, f64) -> Option<(u32, f64)>,
    buffers: &mut crate::astar_pool::AstarBuffers,
) -> Option<RouteResult> {
```

Pass `buffers` to `astar()` inside the function.

- [ ] **Step 3: Add pool to AppState**

In `state.rs`, add the pool field to `AppState`:

```rust
pub struct AppState {
    pub graph: RoutingGraph,
    pub coastline: CoastlineIndex,
    component_labels: Vec<u32>,
    main_component: u32,
    astar_pool: asw_core::astar_pool::AstarPool,
}
```

In `AppState::new()`, after building component_labels, create the pool:
```rust
let astar_pool = asw_core::astar_pool::AstarPool::new(graph.num_nodes as usize, 2);
```

Add a method to expose pool access:
```rust
pub async fn with_astar_buffers<F, T>(&self, f: F) -> T
where
    F: FnOnce(&mut asw_core::astar_pool::AstarBuffers) -> T,
{
    let mut buf = self.astar_pool.acquire().await;
    let result = f(&mut buf);
    self.astar_pool.release(buf).await;
    result
}
```

- [ ] **Step 4: Update route handler in api.rs**

In `api.rs`, the `route_handler` calls `compute_route()`. Update it to acquire buffers from the pool:

```rust
let result = app
    .with_astar_buffers(|buffers| {
        asw_core::routing::compute_route(
            &app.graph,
            from_lat, from_lon,
            to_lat, to_lon,
            &app.coastline,
            knn,
            buffers,
        )
    })
    .await;
```

- [ ] **Step 5: Update routing tests**

In `routing.rs` tests, create a temporary `AstarBuffers` for each test:
```rust
let mut buffers = crate::astar_pool::AstarBuffers::new(graph.num_nodes as usize);
let result = astar(&graph, 0, 3, &mut buffers);
```

- [ ] **Step 6: Update bench module**

In `bench.rs`, the `run_benchmark()` function calls `compute_route()`. Update to pass buffers. Since bench doesn't use tokio async, create `AstarBuffers` directly without the pool:
```rust
let mut buffers = asw_core::astar_pool::AstarBuffers::new(graph.num_nodes as usize);
// Pass &mut buffers to compute_route()
```

- [ ] **Step 7: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/asw-core/src/routing.rs crates/asw-serve/src/state.rs crates/asw-serve/src/api.rs crates/asw-cli/src/bench.rs
git commit -m "feat: integrate A* buffer pool into routing and serve"
```

---

## Chunk 5: Final integration and verification

### Task 10: Build a test graph and verify

**Files:** None (verification only)

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Run fmt**

Run: `cargo fmt --all -- --check`
Expected: No formatting issues.

- [ ] **Step 4: Build a small test graph**

Run: `cargo run --release -p asw-cli -- build --shp path/to/land-polygons-split-4326 --bbox dev-small --output export/test-v2.graph`

This builds a new graph in v2 format with bitcode serialization. Verify it works.

- [ ] **Step 5: Test serve with new graph**

Run: `ASW_API_KEY=test cargo run --release -p asw-cli -- serve --graph export/test-v2.graph --port 3000`

Then test endpoints:
```bash
curl http://localhost:3000/health
curl http://localhost:3000/ready  # wait until 200
curl -H "X-Api-Key: test" "http://localhost:3000/route?from=50.5,1.0&to=51.0,2.0"
curl -H "X-Api-Key: test" http://localhost:3000/info
```

- [ ] **Step 6: Verify old graph rejection**

Run: `cargo run --release -p asw-cli -- serve --graph export/asw.graph --port 3000`
Expected: Error: "Unsupported ASW graph version 1 (expected 2). Rebuild required."

- [ ] **Step 7: Commit any final fixes**

```bash
git add -u
git commit -m "fix: final adjustments from integration testing"
```

### Task 11: Update documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md` (if graph format documented)
- Modify: `docs/deployment.md` (if memory requirements documented)

- [ ] **Step 1: Update CLAUDE.md**

Note the graph format change (v1 → v2) and new memory characteristics.

- [ ] **Step 2: Update deployment docs**

Update minimum memory requirements from 10 GiB to 6 GiB.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md README.md docs/deployment.md
git commit -m "docs: update memory requirements and graph format version"
```
