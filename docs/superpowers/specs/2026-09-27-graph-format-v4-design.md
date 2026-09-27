# Graph format v4 — memory-mapped planet graph

- **Date:** 2026-09-27
- **Status:** design approved in brainstorming, awaiting spec review
- **Driver:** Rhumb Logbook needs the whole planet graph on the phone, opened in a
  background relaunch with a ~10 s budget and tens of MB resident. The contract is
  section 3 of `rhumb/docs/superpowers/specs/2026-09-27-automatic-logbook-design.md`,
  with the amendments listed in section 8 below.
- **Scope of this spec:** the file format, the asw-core refactor, the build pipeline and
  the server. The mobile binding (`asw-mobile`, UniFFI, xcframework and AAR CI) is a
  separate spec that builds on this one.

## 1. Why v3 cannot do it

v3 is zstd over bitcode, decoded into owned vectors at load. The server then builds an
rstar R-tree over every coastline segment and allocates two A* buffer sets sized to the
node count, filled with `f32::MAX` and `u32::MAX`. Result on the planet: 60-90 s load and
~4.1 GiB RSS. Nothing in v3 can be read before the whole file is decoded, so no tuning of
v3 gets it onto a phone.

## 2. Decisions

- One flat little-endian file, memory-mapped. No compression inside the file. Every
  section is a plain array or a self-delimiting varint stream, so a query touches only the
  pages it needs.
- Node ids stay plain sorted `u64`. The "64-entry blocks with u16 deltas" idea from the
  Rhumb contract does not work: measured on the Marmaris graph, 0 of 1342 blocks fit and
  only 53 % of consecutive deltas fit u16. `ponytail:` u64 ids cost 315 MB on the planet;
  a per-block varint scheme is the upgrade path if the file must shrink.
- No stored edge weights. Verified: every v3 weight is centre-to-centre haversine within
  the u16 rounding (max deviation 0.005 nm). Weight is recomputed at query time.
- No converter. The build emits v4, the planet is rebuilt once on Hetzner, v3 loading is
  deleted in the same PR.
- The server uses the same mmap reader with `populate` (MAP_POPULATE) so the file is read
  in at open. No separate in-memory mode.
- `isWater` uses crossing parity, not a boolean crossing test (section 6).
- Coastline points are stored as `i32` microdegrees, no delta coding. Same size as the v3
  `f32` pairs and exact to 0.11 m. `ponytail:` i16 deltas halve this section; add if the
  planet file must shrink.
- No checksum inside the file. The distribution manifest (Rhumb side) and the GitHub
  release asset carry a sha256.
- Endianness: little-endian hosts only, asserted at compile time. All release targets
  (x86_64, aarch64) qualify.

## 3. File layout

Magic `ASW\x04`. All integers little-endian. Every section starts at an 8-byte aligned
offset; the writer pads with zeros. The whole file must be readable by `mmap` at its
natural page alignment, which satisfies the 8-byte requirement.

```
offset  size  field
0       4     magic "ASW\x04"
4       4     reserved (0)
8       64    graph version string: u8 length + up to 63 bytes UTF-8, zero padded.
              Set by the build: "<crate version> <UTC build date>", e.g. "0.7.0 2026-10-03".
72      4     num_nodes  u32
76      4     num_edges  u32   (directed edge records, as today)
80      4     num_coast_runs u32
84      4     reserved (0)
88      9*2*8 section table: 9 entries of (offset u64, length_bytes u64); header ends at 232
```

Sections, in table order:

| # | name | element | count | notes |
| --- | --- | --- | --- | --- |
| 0 | `node_h3` | u64 | num_nodes | sorted ascending, strict. Array index = node id. |
| 1 | `offsets` | u32 | num_nodes + 1 | byte offsets into `edge_targets`; last = section length |
| 2 | `edge_targets` | varint stream | | per node: target ids as ascending varint deltas, same encoding as v3 minus the u16 weight |
| 3 | `shore_dist` | u8 | num_nodes | unchanged from v3 |
| 4 | `coast_runs` | u32 | num_coast_runs + 1 | point index where each run starts; last = total points |
| 5 | `coast_bbox` | i32 × 4 | num_coast_runs | min_lon, min_lat, max_lon, max_lat per run, microdegrees; the grid lists runs by cell, the bbox prunes inside a cell |
| 6 | `coast_points` | (i32 lon, i32 lat) | total points | microdegrees |
| 7 | `grid_offsets` | u32 | 3600 × 1800 + 1 | index into `grid_ids` per 0.1° cell, row-major by lat band then lon |
| 8 | `grid_ids` | u32 | | run ids whose bounding box touches the cell |

Grid cell for (lon, lat): `col = floor((lon + 180) / 0.1)` clamped to 0..3599,
`row = floor((lat + 90) / 0.1)` clamped to 0..1799. A run is listed in every cell its
bounding box overlaps. Runs never cross the antimeridian (guaranteed by the split source
dataset, as today); query segments that do are split at the seam before lookup, exactly
as `crosses_land` does now.

Planet estimate (39.4 M nodes, 299.5 M edge records): ids 315 MB, offsets 158 MB,
targets ~420 MB (1.4 B/edge measured on Marmaris), shore 39 MB, coastline unknown
(~150-300 MB), grid offsets 26 MB. About 1.1-1.2 GB on disk, roughly 500-600 MB as a
zstd download.

## 4. asw-core

**`graph.rs`.** `RoutingGraph` becomes a view over bytes:

```rust
pub struct RoutingGraph { bytes: Bytes, /* parsed header + section slices */ }
enum Bytes { Mmap(memmap2::Mmap), Owned(Vec<u8>) }
```

- `RoutingGraph::open(path, populate: bool)` maps the file, validates magic, header,
  section table bounds and alignment. No per-node validation loop.
- `RoutingGraph::from_bytes(Vec<u8>)` for tests and small graphs.
- `GraphBuilder::build()` returns the v4 byte image directly (`Vec<u8>`), and
  `RoutingGraph::save(path)` writes bytes. The builder is the only writer, so
  the validation that v3 did at load (H3 validity, strict sort, monotonic offsets) runs
  here, once, at build time.
- Accessors replace the public fields: `num_nodes()`, `num_edges()`, `node_h3(i)`,
  `shore_dist(i)`, `node_pos(i)`, `neighbors(i)`, `version()`, `coast_run(i)`,
  `h3_lookup(h3)`.
- `neighbors(i)` keeps its `(u32, f32)` item type. The weight is
  `haversine_nm(node_pos(i), node_pos(target))`, with the source position decoded once per
  iterator. Routing code does not change. `ponytail:` one `cell_center` decode per edge
  relaxation; cache positions in the A* buffers if the bench shows it.
- Typed section access is one small unsafe helper that asserts alignment and length and
  returns `&[T]` for `u8`, `u32`, `u64`, `i32`.

**`coast.rs`.** `CoastlineIndex<'a>` becomes a grid-backed view over five slices
(`coast_runs`, `coast_bbox`, `coast_points`, `grid_offsets`, `grid_ids`). `RoutingGraph::coastline()`
returns it borrowed from the mapped file; the build constructs it from the slices it has
just produced. `AppState` therefore holds only the graph and the A* pool. Same public queries as today: `crosses_land`, `min_distance_deg`,
`min_distance_nm`, `segment_min_distance_nm`, with the antimeridian handling carried
over unchanged. New: `crossing_count(lon1, lat1, lon2, lat2) -> usize` using the
half-open vertex rule. The rstar-backed `CoastlineIndex` and `from_serialized` are
deleted. `LandIndex` (polygons, rstar) stays, and moves to asw-build together with rstar.

**`nearest_node`.** The snapping ladder and its `H3_EDGE_NM` table move from
`asw-serve/src/state.rs` to `asw-core` as `RoutingGraph::nearest_node(lat, lon)`. The
mobile `route` and `is_water` both need it.

**`is_water(lat, lon) -> bool`** in `routing.rs`: snap to the nearest node, count coastline
crossings from the node centre to the point, even = water. No node found = land.

**`astar_pool.rs`.** All five buffers are zero-filled (`vec![0; n]`), so the allocator
hands out lazy zero pages and resident memory grows with the search, not the graph.
`touch()` already resets a slot on first use in a generation, so the initial values were
never read. `AstarPool` allocates its buffer sets on first `acquire`, not at
construction.

**Dependencies.** asw-core drops `bitcode`, `zstd`, `serde`, `rstar`, `rayon`, and gains
`memmap2`. `rayon` was only used by `LandIndex::subtract_water`, which moves to asw-build
with the rest of `LandIndex`.

## 5. asw-build, asw-serve, asw-cli

**Build.** `pipeline.rs` builds the coastline grid sections first and wraps them in the
core `CoastlineIndex`, so `cells.rs` and `shore.rs` use the same grid code the router
uses; the R-tree coastline index is gone. `GraphBuilder` receives runs and points and
emits the v4 image. The version string is set from the crate version and the UTC date.

**Serve.** `AppState::new` receives an opened `RoutingGraph`; the coastline index is a
view, no build step. The CLI opens the file with `populate = true`. `/info` gains
`graph_version`. Readiness semantics are unchanged: `/ready` returns 200 once the file is
mapped and populated.

**CLI.** `asw build` writes v4. `asw geojson` and `asw bench` use the accessors. Loading
a v3 file fails with "Unsupported ASW graph version 3. Rebuild required."

**Docker and release.** Unchanged mechanics. The full image grows from ~740 MB to
~1.2 GB. The planet is rebuilt on Hetzner and uploaded to `draft-graph-v070`.

## 6. isWater and parity

A point is snapped to the nearest water node. The node centre is known water. A straight
line from the centre to the point crosses the coastline zero or more times; each crossing
flips water and land, so an even count means the point is on water. The boolean test the
Rhumb contract proposed fails in marinas: a basin narrower than a res-10 cell has no water
node inside it, the nearest node is outside the mole, and the line crosses the mole ring
twice. Parity says water; the boolean said land. A point genuinely on land enters the
land ring once and never leaves it: odd, land. Inland lakes and rivers have no coastline,
so they are reported as land, which matches the Rhumb decision to keep them out of scope.

## 7. Testing and verification

- Round-trip unit tests through `GraphBuilder::build` and `RoutingGraph::from_bytes` on
  the existing small fixtures; existing tests that constructed `RoutingGraph` fields by
  hand move to the builder.
- Header and section-table rejection tests: wrong magic, v3 file, truncated file,
  misaligned section.
- `crossing_count` tests: ring crossed twice, single crossing, line through a vertex.
- Grid index tests carried over from the R-tree index, including the antimeridian cases.
- Bench: `asw bench` 20 routes on the Marmaris graph before and after, then on the planet
  once rebuilt. Distances are expected to shift in the second decimal because weights are
  no longer rounded to 0.01 nm; the baseline JSON files are regenerated the same day.
- Memory: server RSS after load and after the diverse route mix, recorded in the README
  table. Planet open time with and without `populate`.
- Docs: README, CLAUDE.md and CHANGELOG updated; the stale "res-9 leaf" wording becomes
  res-10 (`H3_RES_LEAF`).

## 8. Amendments to the Rhumb contract

1. Node ids are plain u64, not u16-delta blocks. Planet file ~1.1-1.2 GB, not ~1 GB.
2. `isWater` uses crossing parity.
3. No `asw convert`; the first v4 planet comes from a rebuild.
4. `nearest_node` moves to asw-core; A* buffers become zero-filled and lazily allocated.
   Both are required for the "resident proportional to the search" promise.
5. The file carries no checksum; the manifest does.
6. The mobile binding is a separate spec.
