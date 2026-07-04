# Sea Route Demo — Design Spec

**Date:** 2026-07-04
**Status:** Approved
**Scope:** Standalone experimental web-app demonstrating interactive auto-routing on a CARTO + OpenSeaMap map, backed by the asw-serve API. Lives in a new branch, mostly outside the main repo tree. Not integrated into CI, README, or the normal build pipeline.

## Goal

A fullscreen browser map (CARTO basemap + OpenSeaMap seamarks overlay) with one tool: click to lay down route waypoints, get the shortest sea route between them via asw's routing engine, and see live nautical-mile distances as you edit the route.

## Architecture

Two independent pieces, both living under `experiments/sea-route-demo/` in the new branch, run separately by hand (no docker-compose orchestration):

1. **Backend** — the existing `asw-serve` crate, unmodified in behavior except for one small addition: a permissive CORS layer. Packaged via a new, self-contained multi-stage Dockerfile that builds from source.
2. **Frontend** — a static `index.html` + one JS file (no build step, no framework), served by any plain static file server (`python -m http.server` or `npx serve`) on a different port than the backend.

### Why two independent pieces instead of docker-compose

This is a throwaway experiment. Keeping the frontend as plain static files you can open with any file server avoids adding orchestration config for something that's two `docker run` / `python -m http.server` commands.

## Backend

### CORS addition (real change to `crates/asw-serve`)

`crates/asw-serve/src/api.rs` currently sets no CORS headers at all (confirmed by reading the source — no `tower-http` dependency, no `Access-Control-*` handling anywhere). A browser `fetch()` from a static frontend on a different origin/port would be blocked outright.

**Fix:** add `tower-http` (`cors` feature) as a dependency of `asw-serve`, and layer a permissive `CorsLayer` (any origin, `GET` only, matching the API's read-only surface) onto the router in `create_router`. This is a small, generically useful change — any local frontend experiment would hit the same wall — not scoped only to this demo.

This is the only change to the real crates in this spec.

### Packaging: self-contained Dockerfile

The repo's root `Dockerfile` only `COPY`s a pre-built `asw-linux-${TARGETARCH}` binary — it assumes CI has already cross-compiled the binary and is not buildable standalone from a plain `docker build .` on a dev machine.

Rather than modify the release Dockerfile or depend on cross-compilation tooling, add a new multi-stage Dockerfile under `experiments/sea-route-demo/`:

- Stage 1 (builder): official `rust` image, `cargo build --release -p asw-cli` against the current working tree (including the CORS patch).
- Stage 2 (runtime): same `gcr.io/distroless/static-debian12` base as the real Dockerfile, `COPY` the freshly built binary, same `ASW_GRAPH` / `ASW_HOST` / `ASW_PORT` env vars and `/data` volume convention as the existing image.

This avoids depending on a cross-compilation toolchain (`cross`, musl target) and avoids any dependency on the publicly-pullable `ghcr.io/auto-sea-way/asw:latest` image, which predates the CORS change.

### Running it

```bash
docker build -t asw-demo -f experiments/sea-route-demo/Dockerfile .
docker run --rm \
  -v "$(pwd)/export/planet.graph:/data/asw.graph:ro" \
  -e ASW_API_KEY=dev-local-key \
  -p 3000:3000 \
  asw-demo
```

Uses the already-existing `export/planet.graph` (confirmed present locally, ~738 MB) — no new graph build needed, no bbox restriction, routing works anywhere on the planet.

## Frontend

### Stack

Plain HTML + JS, MapLibre GL JS loaded via CDN `<script>` tag. No bundler, no package.json, no build step — matches the "just an experiment" framing.

### Map layers

- Base: CARTO Voyager raster tiles (`https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}{r}.png`).
- Overlay: OpenSeaMap seamarks raster tiles on top, for nautical chart symbols.

### Backend connection

Frontend reads the API base URL, port, and API key from a small constants block at the top of the JS file (e.g. `const API_BASE = 'http://localhost:3000'; const API_KEY = 'dev-local-key';`) — no env/config plumbing needed for a local single-user experiment. Every request sends `X-Api-Key: <API_KEY>`.

### Route tool interaction model

- A toggle button activates "route tool" mode.
- **Add point:** while active, click anywhere on the map to append a new waypoint at the end of the route.
- **Move point:** drag an existing waypoint marker. While dragging, the one or two segments adjacent to that point are recalculated live, debounced ~200ms, so the route updates continuously without hammering the API on every mousemove frame.
- **Delete point:** click an existing waypoint marker to remove it; its two adjacent segments collapse into one new segment (or the route just gets one point shorter, if it's an endpoint).
- **Undo:** Ctrl+Z, plus a small undo button, pops the last mutation (add/move/delete) off an in-memory history stack of waypoint-array snapshots. No redo.

### Route calculation

`asw-serve`'s `/route` endpoint takes exactly one `from`/`to` pair per call (`GET /route?from=lat,lon&to=lat,lon`, confirmed from `crates/asw-serve/src/api.rs` — no native multi-waypoint support). The frontend therefore:

1. Maintains waypoints as a plain array `[{lat, lon}, ...]`.
2. Issues one `/route` call per consecutive pair `(waypoints[i], waypoints[i+1])`.
3. Concatenates the returned `geometry.coordinates` (GeoJSON `LineString`, `[lon, lat]` order) into one polyline per segment, and stitches segments into the full displayed route.
4. Uses each response's `distance_nm` (already rounded to 1 decimal by the server) directly — no unit conversion, matching the project's nautical-miles-only convention.
5. Nearest-node snapping happens entirely server-side (`AppState::nearest_node`) — the frontend sends raw click lat/lon, never computes or sends snapped coordinates.
6. On any edit, only the affected segment(s) are re-fetched — not the whole route.

### Distance labels

- Each waypoint marker shows a persistent text label with the **cumulative** distance from the route start: nothing on waypoint 1, then the running total of all prior segment distances (nm) for every subsequent point.
- Each segment shows a persistent text label at its geometric midpoint (midpoint of the returned polyline, not the straight line between endpoints) with that segment's own distance (nm).
- Labels update immediately whenever their underlying segment's route response changes.

### Error handling

If a `/route` call for a segment fails (404 "no route found", network error, or non-2xx), that segment renders as a dashed red line directly between its two waypoints (straight line, since no real geometry exists) with a small inline error note near its midpoint (e.g. "no route found"). The rest of the route continues to render and function normally — a broken segment never blocks editing elsewhere.

## Testing

No automated test suite — this is a throwaway experiment, not a shipped feature. Verification is manual:

1. Build the Docker image, run it against `export/planet.graph`, confirm `/health` and `/ready` respond and `/info` reports node/edge counts with the API key.
2. Load the frontend, confirm the CARTO + OpenSeaMap layers render fullscreen.
3. Click through the golden path: activate route tool, add 3+ points spanning open water, confirm segments and cumulative labels appear correctly.
4. Drag a middle point, confirm only its two adjacent segments and their labels update live.
5. Delete a point, confirm the route and labels recompute correctly.
6. Undo several times (Ctrl+Z and button), confirm state reverts correctly at each step.
7. Add a point on land (or otherwise unreachable), confirm the dashed-red-line error case renders and doesn't break the rest of the route.

## Out of scope

- No integration into the main README, CI, or build pipeline.
- No docker-compose or single-command orchestration.
- No persistence (routes are lost on page reload).
- No mobile/touch support consideration.
- No redo (undo-only history).
