# True-endpoint routing (deep-water route fix)

Date: 2026-07-07
Status: approved

## Problem

`compute_route` snaps `from`/`to` to the nearest graph node and returns a
polyline of graph node positions only. The user's actual coordinates are never
part of the returned geometry.

- Near the coast (res-9 nodes, ~0.1 nm spacing) the snap error is invisible.
- In open water (res-3 nodes, ~59 nm edge length) the nearest node center can
  be tens of nm from the requested point, so the polyline visibly floats away
  from both route markers.
- When both points snap to the same res-3 node, A* returns a single-node path
  and the route has distance 0.00 nm.

This is a query-time algorithm gap. The graph build is correct and is not
changed.

## Decision summary

- Approach A: endpoint stitching + direct-line shortcut, implemented in
  `asw-core/src/routing.rs::compute_route`. No graph format change, no
  `asw-serve` API change.
- Land-adjacent pins (option 2): always snap to navigable water and return a
  route. A pin on land keeps its direct segment to the first graph node (small
  visual shoreline clip); no error is returned.

## Data flow

```
(from, to) pins
   |
   +- crosses_land(from, to)?  --no-->  return [from, to], dist = haversine
   |yes
   +- snap both pins to nearest graph nodes (unchanged)
   +- A* between snapped nodes (unchanged)
   +- coordinate sequence: [from_pin] + path node coords + [to_pin]
   +- line-of-sight smoothing over that sequence (starts at the pin itself)
   +- distance = haversine sum over final smoothed coordinates
```

The smoothing pass starting at the pin removes the dog-leg to the snapped hex
center: the pin connects to the farthest A* path point it can see over water.

## Code changes

1. **`smooth` refactor** (`asw-core/src/routing.rs`): operate on coordinates
   (`&[[f64; 2]]` or `&[(f64, f64)]`) instead of node IDs. Algorithm
   (exponential + binary line-of-sight search) unchanged. The existing
   "ensure progress" step already tolerates a blocked next hop, which
   implements the option-2 behavior for land pins for free.
2. **`compute_route`**: add the direct-line shortcut; prepend/append the true
   pin coordinates before smoothing; compute distance over the smoothed
   coordinates. Signature unchanged.
3. **`asw-serve`**: no changes. Response shape unchanged; geometry now starts
   and ends exactly at the requested coordinates.
4. **`raw_hops` / `smooth_hops`**: keep their meaning. The shortcut path
   reports 2/2.

## Edge cases

- Both pins snap to the same node with land between them: sequence is
  `[from, node, to]`; smoothing keeps required corners; distance is correct
  instead of 0.00.
- `from == to`: shortcut returns a degenerate 2-point route, 0.0 nm.
- Pin behind a peninsula relative to its snapped node: same as a land pin —
  one clipping segment, route still returned.
- Antimeridian-crossing segments: same behavior as the existing smoothing
  line-of-sight checks; not made worse by this change.

## Shore-buffer interaction (note for feature/shore-buffer)

`ShorePenalty` is not yet wired into `compute_route`. When it is, the
direct-line shortcut MUST be skipped (or clearance-checked using the nm-unit
`CoastlineIndex` distance queries) whenever a shore buffer is requested —
otherwise the shortcut could return a line closer to shore than the requested
clearance.

## Testing

Unit tests in `routing.rs` against a small synthetic `CoastlineIndex`:

1. Clear line between pins: 2-point route, distance == haversine.
2. Same-node snap with land between: real route, distance > 0.
3. Stitched routes start/end exactly at the pin coordinates.
4. Pin on land: route still returned, starts at the pin.
5. Distance includes the endpoint legs (pin to first/last graph node).

Integration test in `asw-serve/src/api.rs`: `/route` with two open-water
points far from any node returns 200, distance > 0, geometry endpoints equal
the requested coordinates.

Afterwards: run local benchmarks and compare with previous results. The
shortcut is expected to make open-water queries faster; no regression
elsewhere.
