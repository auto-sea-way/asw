# Shore Buffer — Design

**Date:** 2026-07-06
**Issue:** [#26 — Configurable minimum distance from shore](https://github.com/auto-sea-way/asw/issues/26)
**Status:** Approved

## Problem

Routes produced by the res-9 shoreline grid run right against headlands. OSM
coastline is a zero-depth line; rocks and shoals extend beyond it, worst at
capes — exactly where the optimal path touches land. The project has no depth
data, so a configurable distance from shore is the cheapest honest safety
margin ("assume any land has at least N of unsurveyed shoal around it").

The original issue asked for a build-time `--shore-buffer <METERS>` baked into
the graph. That was rejected (see issue reply) because it removes nodes in
coves/marinas where routes must start and end, erases channels narrower than
2× the buffer (conflicting with the passage-corridor cascade), and requires a
full rebuild per margin value.

## Decision summary

- Compute **distance-to-shore per node once at build time**, store it in the
  graph (one byte per node).
- Apply the margin at **query time** via a `/route` parameter, as a **graded
  soft cost penalty**, not a hard filter.
- Parameter unit: **nautical miles** (project convention).
- Graph format bumps **v2 → v3**; the loader rejects v2 with the existing
  "Rebuild required" message (follows current precedent, no dual-version
  code paths).
- Path smoothing becomes buffer-aware — without this, smoothing would cut
  capes right back and silently undo the feature.

Expected cost on the planet graph (~40M nodes): ~40 MB RSS (+1.1%), roughly
+1–2% file size after zstd (values are spatially coherent), a few minutes of
build time. Instance requirements unchanged.

## Data model (asw-core)

`RoutingGraph` gains one field:

```rust
/// Quantized straight-line distance from node center to nearest coastline.
/// Unit: 0.02 nm (~37 m). 255 = saturated (>= 5.1 nm). Rounded down.
pub shore_dist: Vec<u8>,
```

- Magic header becomes `ASW\x03`. Loader accepts only version 3.
- Load-time validation: `shore_dist.len() == num_nodes`.
- Quantization always **rounds down** so the stored value never overstates
  real clearance (conservative in the safety direction).
- `GraphBuilder::add_node(h3, lat, lng, shore_dist_q: u8)` — all call sites
  updated, including test graph constructors.
- The component-pruning rebuild in `pipeline.rs` copies `shore_dist` through
  the old→new ID remap.

## Distance computation (asw-core + asw-build)

New method on `CoastlineIndex`:

```rust
/// Min distance from point to any coastline segment, in nm, capped at max_nm.
/// Returns max_nm if no segment lies within the search envelope.
pub fn min_distance_nm(&self, lon: f64, lat: f64, max_nm: f64) -> f64
```

- Envelope query on the segment R-tree, search box expanded by `max_nm`
  converted to degrees with **cos(lat) correction** for longitude.
- Exact point-to-segment distance in a local equirectangular projection:
  `dx = Δlon·cos(lat)·60`, `dy = Δlat·60` (nm). At ≤ 5 nm scale the
  approximation error is far below one quantization step.

Pipeline: after cell generation (and after canal-water subtraction — so canal
banks count as shore), compute `min_distance_nm` for every node center in
parallel with rayon, quantize, and pass to `add_node`. Ocean nodes hit an
empty envelope and saturate immediately; cost concentrates on shoreline
nodes. Estimated minutes on cpx62 against the ~5 h planet build.

Straight-line distance is the deliberate semantic (not water-path distance):
a node behind a thin breakwater IS close to land for shoal purposes.

## Routing penalty (asw-core)

`astar()` gains an optional constraint:

```rust
pub struct ShorePenalty {
    pub buffer_q: u8, // buffer quantized to 0.02 nm units
    pub k: f32,       // penalty strength, default 15.0
}
```

In the relaxation loop, when the **target** node's `shore_dist < buffer_q`:

```
w' = w × (1 + k · (1 − d/buffer))
```

- Graded: cost rises smoothly toward the shore, so when the route must enter
  the buffer zone (cove entry, narrow strait) A* takes the outermost viable
  line instead of hugging the shore.
- Penalizing by target node means a path through n sub-buffer nodes
  accumulates n penalties — deeper/longer incursions cost proportionally more.
- `None` ⇒ one branch per relaxation, no other overhead.
- Admissibility: penalties only increase edge costs; the haversine heuristic
  underestimates the base cost ≤ penalized cost, so A* remains optimal with
  respect to penalized costs.

## Buffer-aware smoothing (asw-core)

Today `smooth()` accepts any shortcut that does not cross land — this would
cut capes back to the headland and undo the penalty. A naive "shortcut must
stay ≥ buffer from coast" instead breaks at endpoints (a route starting in a
marina has no valid segment at all). Rule adopted:

> A shortcut between `path[i]` and `path[j]` is acceptable iff it does not
> cross land AND its minimum distance to the coastline is
> `≥ min(buffer, min shore_dist of raw path nodes in [i..j])`.

Smoothing may never bring the route closer to shore than the penalized A*
path already was: full buffer enforced in open water, graceful degradation
near endpoints and in constrained water.

Implementation:

- Prefix-minimum array over the raw path's `shore_dist` values (smoothing
  only queries contiguous ranges).
- New `CoastlineIndex::segment_min_distance_nm(a, b, max_nm)`: envelope
  expanded by the threshold, exact segment-to-segment distance on candidate
  coastline segments, same equirectangular treatment as above.

## API (asw-serve)

- `/route` gains optional `shore_buffer` (nautical miles, f64, default 0 =
  exactly current behavior).
- Validation: `0 ≤ shore_buffer ≤ 5.0`, else 400 naming the valid range.
- Response echoes `shore_buffer_nm`.
- `compute_route()` threads the value to both `astar` and `smooth`.

## CLI (asw-cli)

- `asw bench` gains optional `--shore-buffer <NM>` so penalty overhead and
  search expansion can be measured.
- No `ASW_SHORE_BUFFER` env var — the parameter is per-request. A server-wide
  default can be added later if requested.

## Testing

Unit:
- Quantization: round-down, saturation at 255, roundtrip against known values.
- `min_distance_nm` / `segment_min_distance_nm` against hand-computed cases,
  including high-latitude cos(lat) correctness.
- Synthetic two-corridor graph: `shore_buffer=0` picks the short near-shore
  corridor; nonzero buffer picks the long offshore one.
- Smoothing threshold rule, including the endpoint-in-marina case (start node
  shore_dist ≈ 0 must still smooth).
- v3 load validation (length mismatch rejected; v2 file rejected with
  "Rebuild required").

Integration:
- Build the marmaris bbox locally; request a cape-rounding route with and
  without buffer; assert the returned LineString's min distance to the
  coastline is ≥ buffer away from endpoints. Visual check via `asw geojson`.

Bench (local, per project practice — compare against previous results):
- `shore_buffer=0` must match current performance within noise.
- Nonzero buffer: measure and record search expansion; not a regression,
  but must be known.

## Rollout

1. README: document the parameter, format v3, update the "no depth data"
   limitation wording (this feature partially mitigates it).
2. CHANGELOG: add under Unreleased.
3. Planet rebuild on Hetzner (cpx62), republish graph artifacts and the
   `-full` Docker image.
4. Reply on issue #26 when shipped.

## Out of scope

- Hard-filter / strict mode (`penalty|strict` flag) — can be layered on the
  same `shore_dist` data later if demanded.
- Server-wide default buffer.
- Build-time baked buffer (explicitly rejected, see issue reply).
- Depth data integration.
