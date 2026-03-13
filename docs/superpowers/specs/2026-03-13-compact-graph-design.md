# Compact Graph Format

**Date:** 2026-03-13
**Status:** Approved

## Summary

Reduce the routing graph size on both disk and in memory by stripping non-essential fields, quantizing weights, delta-varint encoding adjacency lists, and using fixed-point coordinates. Targets ~50% RAM reduction and ~50-60% disk reduction for the planet graph.

## Context

The current planet graph (40.4M nodes, 305M edges) is 2.3 GB on disk (bincode + zstd level 3) and ~3.25 GB in RAM. The largest contributors are the adjacency and weights arrays (each ~1.2 GB), followed by `node_cells` (~323 MB) which is not used for routing.

This design assumes the km-to-nm refactor (see `2026-03-13-km-to-nm-design.md`) is either completed or applied concurrently — all weights are in nautical miles.

## Changes

### 1. Strip `node_cells`

Remove `node_cells: Vec<u64>` from `RoutingGraph` and `GraphBuilder`. This field stores H3 cell indices and is only used by `asw geojson` for hex boundary visualization and passage node identification.

**Savings:** ~323 MB RAM, significant disk reduction.

**GeoJSON reconstruction:** The `asw geojson` command reconstructs H3 cells on-the-fly by trying `latlng_to_cell(res)` for resolutions 3–10 against the stored lat/lng. The matching resolution is the one whose cell center (converted to f32) equals the stored position. This is a one-time cost at export time.

### 2. Add passage node bitset

Replace the `node_cells[i] == 0` check with a compact bitset.

```rust
pub passage_mask: Vec<u8>  // 1 bit per node
```

Helper method:
```rust
pub fn is_passage(&self, node: u32) -> bool {
    let byte = self.passage_mask[node as usize / 8];
    byte & (1 << (node as usize % 8)) != 0
}
```

~5 MB for 40.4M nodes vs 323 MB for `node_cells`.

### 3. Quantize weights to u16

Store edge weights as `u16` instead of `f32`.

- **Encoding:** `stored = (distance_nm * 100.0).round() as u16`
- **Decoding:** `distance_nm = stored as f32 / 100.0`
- **Step size:** 0.01 nm (~18.5 meters)
- **Range:** 0–655.35 nm (covers all edges; largest res-3 edges are ~32 nm)

The A* loop decodes on access. The quantization error is irrelevant for maritime routing.

**Savings:** 305M edges × 2 bytes saved = ~610 MB RAM.

### 4. Delta-varint encode adjacency lists

Replace `adjacency: Vec<u32>` and `weights: Vec<f32>` with a single interleaved byte buffer.

```rust
pub edge_data: Vec<u8>   // interleaved: [varint target delta][u16 weight LE] per edge
pub offsets: Vec<u32>    // byte offsets into edge_data (not element offsets)
```

**Encoding per node's adjacency list:**
1. Sort targets ascending
2. First target stored as-is (varint), subsequent targets as delta from previous
3. Each entry is `[varint delta][u16 weight little-endian]`

**Varint format:** Standard unsigned LEB128 (1 byte for values 0–127, 2 bytes for 128–16383, etc.). Average delta for spatially-ordered nodes is expected to be 1–2 bytes.

**Iteration:** A* already iterates all neighbors sequentially — no random access into a single node's neighbor list is needed. Decode loop:
```rust
fn neighbors(&self, node: u32) -> NeighborIter {
    let start = self.offsets[node as usize] as usize;
    let end = self.offsets[node as usize + 1] as usize;
    NeighborIter { data: &self.edge_data[start..end], prev_target: 0 }
}
```

**Estimated savings:** ~1.5–2 bytes per target (varint delta) + 2 bytes per weight = ~3.5–4 bytes per edge vs 8 bytes before. For 305M edges: ~450–600 MB vs 1,220 MB for adjacency alone, plus 610 MB already saved on weights.

### 5. Fixed-point i32 coordinates

Store lat/lng as `i32` instead of `f32`, using fixed-point encoding.

- **Encoding:** `stored = (degrees * 1e7).round() as i32`
- **Decoding:** `degrees = stored as f64 / 1e7`
- **Precision:** 1e-7 degrees ≈ ~1.1 cm (exceeds f32 effective precision)
- **Range:** ±214.7 degrees (covers all valid coordinates)

Same 4 bytes per value, so no direct RAM savings. The benefit is compression: nearby nodes produce similar i32 values that delta-encode well under zstd, whereas f32 IEEE 754 bit patterns look like noise to compressors.

**Node ordering:** Sort nodes by H3 cell index during build. H3 uses a Hilbert-curve-derived spatial ordering, so consecutive nodes will be geographically close, producing small deltas in the coordinate arrays.

### 6. Compression

- **Zstd level:** Bump from 3 to 19. Load time is not a priority; better compression ratio is preferred.
- **Serialization:** Bincode for the overall struct. The `edge_data: Vec<u8>` is already pre-encoded; bincode writes raw bytes.

### 7. Format versioning

Add `version: u8` as the first field. New format is version 1. On load, check the version and return a clear error for unrecognized versions instead of a bincode deserialization panic.

## New RoutingGraph struct

```rust
#[derive(Serialize, Deserialize)]
pub struct RoutingGraph {
    pub version: u8,
    pub node_lats: Vec<i32>,           // fixed-point, degrees × 1e7
    pub node_lngs: Vec<i32>,           // fixed-point, degrees × 1e7
    pub passage_mask: Vec<u8>,         // 1 bit per node
    pub offsets: Vec<u32>,             // byte offsets into edge_data
    pub edge_data: Vec<u8>,            // interleaved varint deltas + u16 weights
    pub coastline_coords: Vec<Vec<(f32, f32)>>,
    pub num_nodes: u32,
    pub num_edges: u32,
}
```

## Estimated impact (planet graph)

| Component | Before | After (RAM) |
|-----------|--------|-------------|
| node_lats | 162 MB | 162 MB |
| node_lngs | 162 MB | 162 MB |
| node_cells | 323 MB | 0 |
| passage_mask | 0 | 5 MB |
| offsets | 162 MB | 162 MB |
| adjacency | 1,220 MB | ~450-600 MB |
| weights | 1,220 MB | 0 (in edge_data) |
| **Total (excl. coastline)** | **~3,250 MB** | **~1,550-1,700 MB** |

**Disk (zstd-19):** estimated ~800 MB–1.1 GB, down from 2.3 GB.

**Query cost increase:** One varint decode + one u16-to-f32 multiply per edge, one i32-to-f64 multiply per coordinate read in heuristic. All negligible vs haversine trig.

## Affected crates

- **asw-core/src/graph.rs** — New struct, encoding/decoding helpers, neighbor iterator, save/load with version check
- **asw-core/src/routing.rs** — Use neighbor iterator, decode i32 coords for haversine
- **asw-core/src/geo_index.rs** — Adapt coastline index to unchanged coastline_coords
- **asw-build/src/pipeline.rs** — Build new format: sort nodes spatially, delta-varint encode edges, generate passage_mask, encode fixed-point coords
- **asw-build/src/edges.rs** — Produce sorted adjacency per node
- **asw-serve/src/state.rs** — Decode i32 coords for R-tree construction
- **asw-cli/src/main.rs** — GeoJSON: reconstruct H3 cells from coords, use passage_mask
- **asw-cli/src/bench.rs** — Adapt to new RouteResult field names if needed

## Not changed

- **Coastline coords** — kept as `Vec<Vec<(f32, f32)>>` in the routing graph (needed for path smoothing at serve time)
- **Passage definitions** — specified by coordinates, unaffected
- **H3 grid logic** — build-time only, uses degrees/coordinates

## Testing

- Existing unit tests updated for new struct
- Round-trip test: build graph → save → load → verify all fields decode correctly
- Routing regression: same routes produce same results (within 0.01 nm quantization tolerance)
- `cargo test` and `cargo build --release` must pass
- Benchmark planet graph: verify file size reduction and query time is not degraded
