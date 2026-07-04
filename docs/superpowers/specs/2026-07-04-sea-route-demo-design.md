# Sea Route Demo — Design Spec

**Date:** 2026-07-04
**Status:** Approved
**Scope:** Standalone experimental web-app demonstrating interactive auto-routing on a CARTO + OpenSeaMap map, backed by the asw-serve API. Lives in a new branch, mostly outside the main repo tree. Not integrated into CI, README, or the normal build pipeline.

**Revision note:** this replaces an earlier same-day draft. That draft picked MapLibre GL, a hand-run (non-compose) Dockerfile, and per-segment recompute. This revision keeps that draft's two real technical findings — the missing CORS layer and the need for a from-source Dockerfile — but switches to Leaflet, docker-compose orchestration, and a simpler recompute-everything-on-change strategy, per a follow-up design conversation the same day.

## Goal

A fullscreen browser map (CARTO basemap + OpenSeaMap seamarks overlay) with one tool: click to lay down route waypoints, get the shortest sea route between them via asw's routing engine, and see live nautical-mile distances as you edit the route.

## Architecture

Everything lives under `examples/map-routing/` in the new branch:

- `index.html` — the entire frontend (vanilla JS + Leaflet, no build step, no framework)
- `Dockerfile` — multi-stage build of `asw-serve` from source (see below)
- `docker-compose.yml` — one service, builds the above Dockerfile, mounts the existing `export/planet.graph`, reads `ASW_API_KEY` from the repo-root `.env`
- `README.md` — how to run it

## Backend

### CORS addition (real change to `crates/asw-serve`)

`crates/asw-serve/src/api.rs` currently sets no CORS headers at all (confirmed by reading the source — no `tower-http` dependency, no `Access-Control-*` handling). A browser `fetch()` from `index.html` (opened as a static file, origin `null`/`file://`, calling `http://localhost:3000`) would be blocked outright by the browser without CORS headers.

**Fix:** add `tower-http` (`cors` feature) as a dependency of `asw-serve`, and layer a permissive `CorsLayer` (any origin, `GET` only, matching the API's read-only surface) onto the router in `create_router` (`crates/asw-serve/src/api.rs`). Small, generically useful change — any local frontend experiment hits the same wall — not scoped only to this demo.

This is the only change to the real crates in this spec.

### Packaging: from-source Dockerfile

The repo's root `Dockerfile` only `COPY`s a pre-built `asw-linux-${TARGETARCH}` binary — it assumes CI has already cross-compiled it, and can't be built standalone with a plain `docker build .` on a dev machine. The published `ghcr.io/auto-sea-way/asw` images also predate the CORS patch above, so they can't be used as-is either.

`examples/map-routing/Dockerfile`:
- Stage 1 (builder): official `rust` image, `cargo build --release -p asw-cli` against the current working tree (including the CORS patch).
- Stage 2 (runtime): same `gcr.io/distroless/static-debian12` base as the real Dockerfile, `COPY` the freshly built binary, same `ASW_GRAPH` / `ASW_HOST` / `ASW_PORT` env vars and `/data` volume convention as the existing image.

### `docker-compose.yml`

```yaml
services:
  asw:
    build:
      context: ../..
      dockerfile: examples/map-routing/Dockerfile
    volumes:
      - ../../export/planet.graph:/data/asw.graph:ro
    environment:
      - ASW_API_KEY=${ASW_API_KEY}
    ports:
      - "3000:3000"
    env_file:
      - ../../.env
```

Uses the already-existing `export/planet.graph` (confirmed present locally, ~738 MB, global) — no new graph build or download needed. `docker compose up` builds and starts it.

## Frontend

### Stack

Plain HTML + JS, Leaflet 1.9.4 loaded via the `unpkg` CDN (matching the existing `export/viz.html` convention in this repo). No bundler, no package.json, no build step.

### Map layers

- Base: CARTO Voyager raster tiles (`https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}{r}.png`).
- Overlay: OpenSeaMap seamarks raster tiles (`https://tiles.openseamap.org/seamark/{z}/{x}/{y}.png`) on top, for nautical chart symbols.
- Fullscreen (`100vh`/`100vw`), no other chrome besides the tool controls.

### Backend connection

Frontend reads the API base URL and API key from a small constants block at the top of the JS file (e.g. `const API_BASE = 'http://localhost:3000'; const API_KEY = 'dev-local-key';`) — no env/config plumbing for a local single-user experiment. Every request sends `X-Api-Key: <API_KEY>`.

### Route tool interaction model

- A toggle button (top-left corner, styled like the existing `.info-panel` in `viz.html`) activates "route tool" mode. Off by default.
- **When active:**
  - Click empty map → append a new waypoint at the end of the route.
  - Click an existing waypoint marker → remove that point.
  - Drag an existing marker → move it; route recalculates on drag-end (not per drag-move frame, to avoid hammering the API mid-drag).
  - Ctrl+Z → undo the last add/move/delete, popping a waypoint-array snapshot off an in-memory history stack. No redo, no visible undo button.
- **When inactive:** the current route and its labels stay visible but frozen — no click/drag handlers attached.
- A "Clear" button next to the toggle wipes all points immediately.

### Route calculation

`asw-serve`'s `/route` endpoint takes exactly one `from`/`to` pair per call (`GET /route?from=lat,lon&to=lat,lon`, confirmed from `crates/asw-serve/src/api.rs` — no native multi-waypoint support). The frontend therefore:

1. Maintains waypoints as a plain array `[{lat, lon}, ...]`.
2. On **any** mutation (add/move/delete/undo/clear), re-issues one `/route` call per consecutive pair `(waypoints[i], waypoints[i+1])`, all in parallel (`Promise.all`), then redraws the whole route from scratch. Chosen over incremental per-segment recompute for simplicity — routes are short and asw-serve responds in single-digit milliseconds, so there's no incremental-update logic to get wrong.
3. Each segment's returned `geometry.coordinates` (GeoJSON `LineString`, `[lon, lat]` order) is swapped to Leaflet's `[lat, lon]` order and drawn as that segment's polyline.
4. Uses each response's `distance_nm` (already rounded to 1 decimal by the server) directly — no unit conversion, matching the project's nautical-miles-only convention.
5. Nearest-node snapping happens entirely server-side (`AppState::nearest_node`) — the frontend sends raw click lat/lon, never computes or sends snapped coordinates.

### Distance labels

- Each waypoint marker shows a persistent text label with the **cumulative** distance from the route start: nothing on waypoint 1, then the running total of all prior segment distances (nm) for every subsequent point.
- Each segment shows a persistent text label at its arc-length midpoint (midpoint along the returned polyline, not the straight line between endpoints) with that segment's own distance (nm).
- Labels redraw whenever the full-recompute pass (above) completes.

### Error handling

If a `/route` call for a segment fails (404 "no route found", network error, or non-2xx): that segment renders as a dashed red line directly between its two waypoints (straight line, since no real geometry exists), with a small inline "no route found" label at its midpoint. Cumulative distance labels for every point *after* the break show `—` (undefined) rather than a misleading partial total. The rest of the route continues to render and function normally — a broken segment never blocks editing elsewhere.

## Testing

No automated test suite — this is a throwaway experiment, not a shipped feature. Verification is manual:

1. `docker compose up` in `examples/map-routing/`, confirm `/health` and `/ready` respond and `/info` reports node/edge counts with the API key.
2. Load `index.html`, confirm the CARTO + OpenSeaMap layers render fullscreen, and confirm the browser does NOT show a CORS error on the first `/route` call.
3. Click through the golden path: activate route tool, add 3+ points spanning open water, confirm segments and cumulative labels appear correctly.
4. Drag a middle point, confirm the whole route recomputes and updates correctly on drag-end.
5. Delete a point, confirm the route and labels recompute correctly.
6. Ctrl+Z several times, confirm state reverts correctly at each step.
7. Add a point on land (or otherwise unreachable), confirm the dashed-red-line error case renders, cumulative labels downstream show `—`, and the rest of the route keeps working.
8. Toggle the tool off, confirm the route stays visible but clicks/drags no longer do anything; toggle back on, confirm editing resumes.

## Out of scope

- No integration into the main README, CI, or build pipeline.
- No persistence (routes are lost on page reload).
- No mobile/touch support consideration.
- No redo (undo-only history, keyboard-only).
- No overall route-summary panel (per-point cumulative labels are considered sufficient).
