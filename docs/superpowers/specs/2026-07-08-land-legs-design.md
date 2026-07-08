# Land legs: flag overland stitch segments, water-only distance

Date: 2026-07-08
Status: approved

## Problem

Since true-endpoint routing (v0.6.0, PR #35), route geometry starts and ends
at the exact requested coordinates. When a pin sits on land, the first/last
segment connects it straight to the water — the approved "shoreline clip"
behavior. Rendered on a map, that segment is indistinguishable from the sea
route (see the Dover pin in `benchmarks/bench-routes.geojson`: a ~2 nm line
across the town), and it inflates `distance_nm` with distance a vessel never
sails.

## Decisions

1. Route responses identify which segments cross land, so clients can render
   them differently. Geometry stays a single `LineString`.
2. `distance_nm` counts only water segments. Flagged land legs contribute
   zero distance.
3. Benchmark route coordinates stay exactly as they are. The inland pins
   (English Channel, Dover Strait, Kiel, Suez, ...) become the visual
   fixture that shows land-leg rendering works.
4. No graph rebuild: detection happens at query time against the coastline
   index already stored in graph v3.

## Design

### asw-core

`smooth_indices` already contains the only branch where a segment is kept
despite failing the `crosses_land` test (the "even the next hop is blocked"
case — land pins and the peninsula edge case). That branch records the
segment instead of discarding the knowledge.

- `RouteResult` gains `land_legs: Vec<usize>`: value `s` means the segment
  `coordinates[s] -> coordinates[s+1]` crosses land.
- `distance_nm` skips flagged segments.
- The direct-line shortcut is unaffected: it only fires when the pin-to-pin
  line is land-free, so shortcut routes always have `land_legs: []`.
- A stitch leg from a pin in open water is a water segment: not flagged,
  still counted in the distance.
- Degenerate case — both pins on land snapped to the same node: geometry
  `pin -> node -> pin`, both legs flagged, `distance_nm` 0.0. Legitimate.

### asw-serve

`/route` response gains `"land_legs": [0, 17]` — always present, `[]` when
the whole route is water. `distance_nm` becomes water-only. Additive field;
existing clients keep working, but reported distances for land-pin routes
drop (changelog + README API section note this).

### Benchmarks

The bench geojson writer emits each route's land legs as separate
`LineString` features with simplestyle `"stroke": "#e5484d"` (GitHub's
preview renders stroke colors), while the water route keeps the default
styling. Distance columns in BENCHMARKS.md/README become water-only; Kiel
(~95 nm with overland stitches) is expected to drop to canal-transit
reality (~85-87 nm).

## Testing

- Core: land-pin route flags the first segment and excludes it from
  distance (expectations built from `g.node_pos()`, not input coordinates —
  known fixture gotcha); water pins produce empty `land_legs` and unchanged
  distance; same-node land pins return 0.0 nm with both legs flagged.
- HTTP: `land_legs` present and correct in the JSON response.
- Full bench re-run refreshes BENCHMARKS.md, README tables, and
  bench-routes.geojson.

## Out of scope

- Point-in-polygon "pin is on land" detection (would need a `LandIndex` at
  serve time; `crosses_land` against the coastline index covers the cases
  that matter and fits the existing architecture).
- Clipping geometry at the shoreline (would reverse the true-endpoint
  property shipped in v0.6.0).
- A separate `land_distance_nm` response field (YAGNI until someone asks).

## Release

Ships as v0.6.1: no graph format change, additive API field. The release
can reuse the `draft-graph-v060` graph artifact.
