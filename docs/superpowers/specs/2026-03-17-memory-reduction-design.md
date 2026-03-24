# Memory Reduction Design

**Date:** 2026-03-17
**Status:** Approved
**Goal:** Reduce server memory usage for the global graph (40M nodes, 305M edges)

## Problem

The server currently uses ~6.2 GiB steady-state and ~8.7 GiB peak when loading the global routing graph from an 843 MB file. Each route request temporarily allocates ~360 MB for A* search buffers. This limits deployment to machines with 10+ GiB RAM.

## Measured Memory Breakdown (Current)

| Component | Size | % |
|-----------|------|---|
| `edge_data` (varint + u16, 305M edges) | 2,300 MB | 37% |
| `node_tree` RTree (40M f64 points + index) | 1,100 MB | 18% |
| `node_lats` + `node_lngs` (i32) | 320 MB | 5% |
| `offsets` (u32) | 160 MB | 3% |
| `component_labels` (u32) | 160 MB | 3% |
| `coastline` RTree | 300 MB | 5% |
| `node_resolutions` (u8) | 40 MB | 1% |
| `passage_mask` (u8 bitset) | 5 MB | <1% |
| Allocator overhead / fragmentation | 850 MB | 14% |
| **Total steady-state** | **~5,235 MB** | |
| **Peak (during RTree construction)** | **~8,700 MB** | |
| **Per-route A* buffers (temporary)** | **360 MB** | |

## Changes

### 1. Replace node_tree RTree with sorted `node_h3` lookup

**Saves ~1,100 MB steady, ~1.8 GB peak. Zero additional memory.**

The node_tree RTree exists for nearest-node snapping (KNN): given a user's lat/lon, find the closest graph node in the main connected component. Currently this is a 1.1 GB RTree of 40M `(f64, f64, u32)` entries, which requires ~1.8 GB during construction (temporary Vec + RTree bulk load).

All graph nodes are H3 cells. The build pipeline already sorts nodes by H3 index. If `node_h3: Vec<u64>` is kept sorted, binary search on it returns the array index, which IS the node ID. No separate lookup structure is needed — `node_h3` serves double duty as both coordinate storage and spatial index.

At query time:
- Convert input lat/lon to H3 cell index at each resolution (res-13 down to res-3)
- Binary search `node_h3` for the cell
- If not found, expand to k-ring(1) neighbors at that resolution
- Filter to main component (same as current)

H3 cell computation + binary search is O(log n), faster than RTree KNN iteration. No speed penalty.

**Peak reduction:** No intermediate construction — `node_h3` is deserialized directly from the graph file.

### 2. Replace node coordinate vecs with H3 index vec

**Saves 45 MB steady.**

Current fields for node identity:
- `node_lats: Vec<i32>` (160 MB)
- `node_lngs: Vec<i32>` (160 MB)
- `node_resolutions: Vec<u8>` (40 MB)
- `passage_mask: Vec<u8>` (5 MB)

Replace all four with `node_h3: Vec<u64>` (320 MB).

- Lat/lon decoded on-the-fly via `h3o::CellIndex::lat_lng()` (~10ns per call)
- Resolution via `h3o::CellIndex::resolution()`
- `passage_mask` removed — the build pipeline never creates passage nodes (`is_passage` is always `false` in `pipeline.rs:64`). The passage system works entirely through resolution cascade: passage corridors get refined to finer H3 cells (res-11/13), which are regular nodes. The `passage_mask` is all-zeros in every production graph.
- GeoJSON export uses H3 index directly for hex polygons; the passage edges layer is removed (no nodes are flagged as passage nodes)
- Post-deserialization validation: verify all H3 indices are valid cells via `h3o::CellIndex::try_from()`

### 3. A* buffer pool

**Eliminates per-request 360 MB allocation spikes. Adds 720 MB fixed to steady state.**

Current: each route allocates `g_score: Vec<f32>` (160 MB), `came_from: Vec<u32>` (160 MB), `closed: Vec<bool>` (40 MB) — all sized to 40M nodes, allocated and freed per request.

Replace with a pool of 2 pre-allocated buffer sets:
- Pool stored in `AppState` behind a tokio channel (`mpsc` bounded channel of size 2)
- On request: recv a buffer set, use it, reset with `fill()`, send it back
- Pool size of 2 supports 2 concurrent routes; additional requests wait on the channel
- No allocation jitter, predictable memory profile
- Pool size is appropriate for the expected concurrency (single-purpose maritime routing server, typically 1-2 concurrent requests)

### 4. Switch serialization from bincode 1 to bitcode

**No memory impact. Correctness improvement.**

`bincode` v1 is deprecated and unmaintained. Replace with `bitcode` — serde-compatible, fastest Rust serializer, produces smaller output (better zstd compression).

Bump graph format version: `ASW\x01` to `ASW\x02`. Old graphs produce a clear "rebuild required" error.

## Expected Results

| Metric | Current | After | Change |
|--------|---------|-------|--------|
| Steady-state (idle) | ~5,235 MB | ~3,440 MB | -34% |
| Steady-state (with A* pool) | ~5,235 MB | ~4,160 MB | -20% |
| Peak (during load) | ~8,700 MB | ~4,200 MB | -52% |
| Per-request allocation | 360 MB | 0 MB | eliminated |
| Min deployment RAM | 10 GiB | 5-6 GiB | -40-50% |

Breakdown of new steady-state (idle):
- `node_h3: Vec<u64>` — 320 MB
- `edge_data: Vec<u8>` — 2,300 MB
- `offsets: Vec<u32>` — 160 MB
- `coastline` RTree — 300 MB
- `component_labels: Vec<u32>` — 160 MB
- Allocator overhead — ~200 MB (less fragmentation: no RTree, fewer vecs)
- **Total: ~3,440 MB**

The key insight: `node_h3` is both the coordinate store AND the spatial index, eliminating 1.1 GB of RTree and 365 MB of separate coordinate vecs, replaced by a single 320 MB vec.

## RoutingGraph struct (before and after)

**Before:**
```rust
pub struct RoutingGraph {
    pub node_lats: Vec<i32>,          // 160 MB
    pub node_lngs: Vec<i32>,          // 160 MB
    pub node_resolutions: Vec<u8>,    // 40 MB
    pub passage_mask: Vec<u8>,        // 5 MB
    pub offsets: Vec<u32>,            // 160 MB
    pub edge_data: Vec<u8>,           // 2,300 MB
    pub coastline_coords: Vec<Vec<(f32, f32)>>,
    pub num_nodes: u32,
    pub num_edges: u32,
}
```

**After:**
```rust
pub struct RoutingGraph {
    /// Sorted by H3 index. Array index = node ID.
    pub node_h3: Vec<u64>,            // 320 MB
    pub offsets: Vec<u32>,            // 160 MB
    pub edge_data: Vec<u8>,           // 2,300 MB
    pub coastline_coords: Vec<Vec<(f32, f32)>>,
    pub num_nodes: u32,
    pub num_edges: u32,
}
```

## AppState struct (before and after)

**Before:**
```rust
pub struct AppState {
    pub graph: RoutingGraph,
    pub coastline: CoastlineIndex,
    pub node_tree: RTree<GeomWithData<[f64; 2], u32>>,  // 1,100 MB
    component_labels: Vec<u32>,
    main_component: u32,
}
```

**After:**
```rust
pub struct AppState {
    pub graph: RoutingGraph,
    pub coastline: CoastlineIndex,
    // No spatial index needed — binary search on graph.node_h3 serves as lookup
    component_labels: Vec<u32>,
    main_component: u32,
    astar_pool: AstarBufferPool,        // 720 MB (2 buffer sets)
}
```

## H3 Nearest-Node Algorithm

```
fn nearest_node(lat, lon, main_component):
    // Iterate from finest to coarsest resolution.
    // Max res-13 covers passage corridors (Suez, Corinth, etc.)
    for resolution in [13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3]:
        cell = lat_lng_to_cell(lat, lon, resolution)
        if node_id = node_h3.binary_search(cell):
            if component_labels[node_id] == main_component:
                return node_id
        for neighbor in cell.grid_ring(1):
            if node_id = node_h3.binary_search(neighbor):
                if component_labels[node_id] == main_component:
                    return node_id
    return None
```

Note: the resolution range must cover the full range used in the graph (res-3 ocean through res-13 Corinth Canal). Starting at the finest resolution ensures the closest possible node is found first.

## Graph Format Version

- Current: `ASW\x01` — bincode 1 + zstd-19, fields: node_lats, node_lngs, node_resolutions, passage_mask, offsets, edge_data, coastline_coords, num_nodes, num_edges
- New: `ASW\x02` — bitcode + zstd-19, fields: node_h3 (sorted), offsets, edge_data, coastline_coords, num_nodes, num_edges
- Validation: all H3 indices checked with `h3o::CellIndex::try_from()` during load

Loading `ASW\x01` files will produce: "Unsupported ASW graph version 1 (expected 2). Rebuild required."

## Node ID Stability

Sorting `node_h3` by H3 index changes node IDs compared to the current graph. This means:
- `offsets` and `edge_data` must be rebuilt to match the new node ordering
- This happens naturally during `asw build` — no manual migration needed
- Existing `.graph` files are incompatible (version bump enforces rebuild)

## Future Work

- **Edge data compression** — `edge_data` at 2.3 GB is 45% of memory. Block-based compression or adjacency list restructuring could cut this significantly.
- **Sparse A* buffers** — HashMap-based search would eliminate the 720 MB pool at the cost of ~1.5-2x slower routing.
- **Memory-mapped graph** — mmap a flat binary format for OS-managed paging.
- **u32 offsets limit** — `offsets: Vec<u32>` limits `edge_data` to 4 GiB. Not an issue today (2.3 GB) but worth noting if edge data grows.

## Files Changed

- `crates/asw-core/src/graph.rs` — RoutingGraph struct, serialization, `node_pos()` via H3, remove passage_mask/resolutions/lats/lngs, add `node_h3`, update all tests (roundtrip, passage bitset, neighbor iter, node_pos)
- `crates/asw-core/src/routing.rs` — A* to accept buffer pool handle, return buffers after use
- `crates/asw-core/Cargo.toml` — bincode → bitcode
- `crates/asw-serve/src/state.rs` — AppState: remove node_tree and rstar import, add astar_pool, nearest_node via H3 binary search
- `crates/asw-serve/Cargo.toml` — remove `rstar` dependency
- `crates/asw-build/src/pipeline.rs` — GraphBuilder to store H3 cell indices, ensure sorted output
- `crates/asw-build/src/cells.rs` — adapt resolution handling if needed
- `crates/asw-build/src/edges.rs` — adapt to new node format
- `crates/asw-cli/src/main.rs` — GeoJSON export using H3 indices, remove passage edges layer
- `crates/asw-cli/src/bench.rs` — adapt to new AppState API (nearest_node, buffer pool)
- `Cargo.toml` — workspace: bincode → bitcode
