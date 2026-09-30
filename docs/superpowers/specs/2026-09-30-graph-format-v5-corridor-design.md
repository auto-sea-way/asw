# Graph format v5 — coarse graph and corridor search

- **Date:** 2026-09-30
- **Status:** implemented; planet v5 built 2026-09-30 (38,886,917 nodes, 1,425 MB)
- **Driver:** long routes take seconds. Rotterdam to Singapore settles 8.9M of the
  39.4M planet nodes (4.4 s), Shanghai to Rotterdam 16.6M (8 s). Phones are slower still.

## 1. Why A* is slow here

The heuristic is great-circle distance. When a continent forces a detour, the true
route is much longer than the great circle (8,242 nm against 5,703 nm for
Rotterdam–Singapore), and A* settles every node whose `g + h` stays under the route
length: most of Europe, the Atlantic and the Mediterranean. 76 % of settled nodes are
res-10 coastline cells. Per-node cost is fine; the node count is the problem.

Measured alternatives (planet v4, same machine):

| Approach | Result |
|---|---|
| Landmarks (ALT) stored per res-3 region | Correct only with node reopening; bounds jump at region borders, millions of reopenings, slower than plain A* |
| ALT stored per node | Works, but ~80 MB per landmark; 8 landmarks add 630 MB |
| Weighted A* (`h * (1 + ε)`) | 9× on open ocean, but continent detours barely move (8.9M → 8.1M settled at ε = 0.1) |
| **Coarse path, then fine A* inside a corridor** | 5–10× fewer settled nodes on long routes, final smoothed distance within ±0.2 % |

## 2. Coarse graph

- **Region** of a node: its res-3 H3 parent (the node itself if it is res 3 or coarser).
- **Coarse node:** one connected piece of water inside one region, found with union-find
  over edges whose two ends share a region. Splitting by piece stops the coarse graph
  from tunnelling across an isthmus (a res-3 cell can hold both the Pacific and the
  Caribbean side of Panama).
- **Coarse edge:** between two coarse nodes when any fine edge joins them. Weight at
  query time is the great-circle distance between the two centroids.
- **Centroid:** normalised mean of the unit vectors of the member cell centres.
- Built in `GraphBuilder::build_bytes`, so every graph (build pipeline, tests) carries it.
- Planet: 41,257 coarse nodes, 198,926 coarse edges, about 2 MB.

## 3. File layout

Magic `ASW\x05`. Header unchanged except `SECTION_COUNT` 10 → 16 (header 344 bytes).
Sections 0–9 are exactly v4. New sections, all per coarse node unless noted, coarse
nodes ordered by (region, smallest member node id):

| # | Section | Type | Content |
|---|---|---|---|
| 10 | `coarse_region` | u64 | region H3 id (non-decreasing) |
| 11 | `coarse_rep` | u32 | smallest member node id |
| 12 | `coarse_size` | u32 | member node count |
| 13 | `coarse_pos` | i32 × 2 | centroid lat, lon in microdegrees |
| 14 | `coarse_offsets` | u32 × (n + 1) | CSR into `coarse_targets` |
| 15 | `coarse_targets` | u32 | coarse neighbour ids, ascending per node |

v4 files are rejected with a "rebuild required" message, as v3 was.

## 4. Query

1. Direct-line shortcut, snapping and the closed-area check are unchanged.
2. **Coarse node of an endpoint.** Binary-search its region in `coarse_region`. One
   coarse node in the region: done. Otherwise flood-fill from the endpoint inside the
   region, tracking the smallest id, but stop after `limit + 1` nodes, where `limit` is
   the size of the second-largest piece in the region. Stopping early means the
   endpoint is in the largest piece; finishing means the smallest id is that piece's
   `coarse_rep`. The flood is bounded by small pieces, never by the 470k-node giant.
3. Same coarse node for both endpoints: plain A* (short route, nothing to gain).
4. **Coarse A*** from start to goal coarse node, centroid great-circle weights and
   heuristic. With `arctic = false`, coarse nodes whose centroid is in a closed area are
   skipped (the two endpoint nodes excepted). The sea north of 80° N has no nodes.
5. **Corridor:** regions of the coarse path plus `CORRIDOR_RINGS` (1) rings of coarse
   neighbours, as a sorted `Vec<u64>` of region ids.
6. **Fine A*** as today, except that a node outside the corridor or in a closed area is
   marked closed on first touch, so it is never expanded and the test runs once per
   node instead of once per edge.
7. No coarse path, or no fine path inside the corridor: fall back to the full search.
   The worst case is today's behaviour.

Routes stay optimal inside the corridor, not globally. Measured on eight routes, the
final smoothed distance moved by −0.1 % to +0.2 %, below the hexagon zigzag that
smoothing removes anyway.

## 5. No nodes north of 80° N

The build drops cells whose centre is north of `ICE_CAP_LAT` before building edges.
They were never routable (the query-time check from the Arctic fix stays, for safety).
Saves 533k nodes, about 16 MB. `is_water` north of 80° N now returns false: no node is
in snapping range, and the ice cap is not navigable water.

## 6. Out of scope

- Converter from v4: the planet is rebuilt on Hetzner, as for v4.
- Per-node landmark data, contraction hierarchies.
- Clipping the coastline sections at 80° N (1.5 MB; they keep `is_water` and crossing
  tests exact near the cap).

## 7. Results (planet v5, same Mac, same graph data as a v4 file for `main`)

| Route | main | v5 | Distance |
|---|---|---|---|
| Rotterdam–Singapore | 4.50 s | 0.81 s | 8,242.6 → 8,245.7 nm |
| Shanghai–Rotterdam | 8.54 s (full search, Arctic closed) | 1.19 s | 10,379.5 → 10,366.9 nm |
| Tokyo–Seattle | 699 ms | 419 ms | 4,269.0 → 4,286.7 nm |
| North Atlantic | 579 ms | 286 ms | same |
| Tasman Sea | 49.6 ms | 33.0 ms | same |
| Routes under 500 nm | | ±1 % | same |

Two findings from tuning:

- Below 500 nm the coarse search cost more than it saved (Corinth 1.4 → 2.9 ms), so
  those routes skip it (`CORRIDOR_MIN_NM`).
- Passing the corridor into `touch_and_cache_h` stopped it from being inlined and made
  every search 5–7 % slower. `#[inline(always)]` removed that; measured by running
  `main` on the v5 data converted to a v4 file, so graph data was ruled out.
