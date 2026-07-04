# Sea Route Demo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a standalone, throwaway demo web-app (Leaflet map, CARTO Voyager + OpenSeaMap layers) that lets a user click out a multi-point sea route, calls the real `asw-serve` `/route` API per segment, and shows live nautical-mile distance labels — plus the small real `asw-serve` CORS fix it depends on.

**Architecture:** Two independently-run pieces under `examples/map-routing/`: (1) `asw-serve`, packaged via a from-source multi-stage Dockerfile + `docker-compose.yml`, mounting the existing `export/planet.graph`; (2) a single-file `index.html` (vanilla JS, Leaflet via CDN, no build step) opened directly as a `file://` URL, calling the dockerized backend at `http://localhost:3000`. One real change lands in `crates/asw-serve`: a permissive CORS layer, since browsers block `file://` → `http://localhost:3000` fetches without one.

**Tech Stack:** Rust (axum, tower-http), Docker + docker-compose, vanilla JS + Leaflet 1.9.4 (CDN), no frontend build tooling.

## Global Constraints

- Distances are always nautical miles (`distance_nm` from the API), never converted to km or any other unit.
- No automated frontend test suite — this is a throwaway experiment (per spec). Frontend verification steps in this plan use the Playwright MCP browser tools (`mcp__plugin_playwright_playwright__*`) to drive a real browser deterministically, standing in for "manual" verification.
- No changes to the root `Dockerfile`, `README.md`, or CI — this demo is fully self-contained under `examples/map-routing/`, except for the one CORS change to `crates/asw-serve`.
- `cargo fmt --all` must be run before any commit that touches Rust code (existing project convention).
- Frontend recomputes **all** segments on every mutation (add/move/delete/undo/clear) — no incremental per-segment diffing (per approved spec, chosen for simplicity).
- Drag-to-move recalculates on `dragend` only, not per drag-move frame.
- Undo is Ctrl+Z only, no redo, no visible undo button.

---

### Task 1: CORS layer for asw-serve

**Files:**
- Modify: `crates/asw-serve/Cargo.toml`
- Modify: `crates/asw-serve/src/api.rs`

**Interfaces:**
- Consumes: nothing new — wraps the existing `create_router(state: Arc<ServerState>) -> Router` from `crates/asw-serve/src/api.rs:175`.
- Produces: `create_router` now returns a `Router` whose responses include `Access-Control-Allow-Origin`, and which answers CORS preflight `OPTIONS` requests directly (no route changes needed by later tasks — the frontend just calls `/route` and `/info` as before, now successfully from a browser).

Without this, a browser `fetch()` from `index.html` (opened as `file://`, origin `null`) to `http://localhost:3000` is blocked outright — confirmed by reading `crates/asw-serve/src/api.rs` and finding no `tower-http` dependency and no `Access-Control-*` handling anywhere in the crate. The custom `X-Api-Key` header the frontend must send also forces the browser to run a CORS **preflight** `OPTIONS` request first — the layer must explicitly allow that header or the preflight fails and the real request never goes out.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block at the bottom of `crates/asw-serve/src/api.rs` (after the existing `info_passes_with_correct_key` test, before the closing `}`):

```rust
    #[tokio::test]
    async fn health_includes_cors_header() {
        let app = create_router(test_state());
        let req = Request::get("/health")
            .header("Origin", "http://localhost:8080")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::OK);
        assert!(
            resp.headers().contains_key("access-control-allow-origin"),
            "expected Access-Control-Allow-Origin header on response"
        );
    }

    #[tokio::test]
    async fn preflight_allows_api_key_header() {
        let app = create_router(test_state());
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/route")
            .header("Origin", "http://localhost:8080")
            .header("Access-Control-Request-Method", "GET")
            .header("Access-Control-Request-Headers", "x-api-key")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::OK);
        assert!(
            resp.headers().contains_key("access-control-allow-headers"),
            "expected Access-Control-Allow-Headers on preflight response"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Volumes/2TB/Projects/auto-sea-way && export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p asw-serve health_includes_cors_header preflight_allows_api_key_header`
Expected: both FAIL — `health_includes_cors_header` fails the `assert!` (no CORS header present); `preflight_allows_api_key_header` fails because `OPTIONS /route` currently 404s (no route registered for that method), so the status assertion fails.

- [ ] **Step 3: Add the `tower-http` dependency**

In `crates/asw-serve/Cargo.toml`, add to the `[dependencies]` section (after `subtle = "2"`):

```toml
tower-http = { version = "0.6", features = ["cors"] }
```

- [ ] **Step 4: Add the CORS layer to the router**

In `crates/asw-serve/src/api.rs`, add to the `use axum::{...}` import block's neighboring imports (near the top, after the existing `use` lines at line 12):

```rust
use tower_http::cors::{Any, CorsLayer};
```

Replace the `create_router` function (`crates/asw-serve/src/api.rs:175-189`):

```rust
pub fn create_router(state: Arc<ServerState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([axum::http::Method::GET])
        .allow_headers(Any);

    let protected = Router::new()
        .route("/route", get(route_handler))
        .route("/info", get(info_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            api_key_middleware,
        ));

    Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .merge(protected)
        .layer(cors)
        .with_state(state)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p asw-serve`
Expected: PASS — all existing tests plus the two new ones (`health_includes_cors_header`, `preflight_allows_api_key_header`) succeed.

- [ ] **Step 6: Format and commit**

Run: `cargo fmt --all`

```bash
git add crates/asw-serve/Cargo.toml crates/asw-serve/src/api.rs Cargo.lock
git commit -m "feat(serve): add permissive CORS layer for local browser demos"
```

---

### Task 2: From-source Dockerfile for the demo backend

**Files:**
- Create: `examples/map-routing/Dockerfile`

**Interfaces:**
- Consumes: the CORS-patched `asw-serve` from Task 1 (via `cargo build --release -p asw-cli` against the working tree).
- Produces: a Docker image tagged `asw-demo` that runs `asw serve`, listening on port 3000, reading the graph from `/data/asw.graph` — used by Task 3's `docker-compose.yml`.

The root `Dockerfile` only `COPY`s a pre-built `asw-linux-${TARGETARCH}` binary produced by CI's cross-compilation matrix — it isn't buildable standalone with `docker build .` on a dev machine, and the published `ghcr.io/auto-sea-way/asw` images predate the Task 1 CORS patch. This Dockerfile builds `asw-cli` from source instead, inside the Linux build container, so no cross-compilation toolchain is needed on the host.

- [ ] **Step 1: Write the Dockerfile**

```dockerfile
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p asw-cli

FROM gcr.io/distroless/static-debian12
COPY --from=builder /build/target/release/asw /usr/local/bin/asw
ENV ASW_GRAPH=/data/asw.graph
ENV ASW_HOST=0.0.0.0
ENV ASW_PORT=3000
EXPOSE 3000
VOLUME /data
ENTRYPOINT ["asw"]
CMD ["serve"]
```

- [ ] **Step 2: Build the image**

Run: `cd /Volumes/2TB/Projects/auto-sea-way && docker build -t asw-demo -f examples/map-routing/Dockerfile .`
Expected: image builds successfully (this compiles the full workspace release profile — first build takes several minutes). Final output line: `Successfully tagged asw-demo:latest` (or the equivalent buildkit "naming to docker.io/library/asw-demo:latest done").

- [ ] **Step 3: Smoke-test the container manually**

Run:
```bash
docker run --rm -d --name asw-demo-smoke \
  -v "$(pwd)/export/planet.graph:/data/asw.graph:ro" \
  -e ASW_API_KEY=dev-local-key \
  -p 3000:3000 \
  asw-demo
sleep 2
curl -s http://localhost:3000/health
```
Expected: `ok`

Then wait for the graph to finish loading and check readiness (planet graph load can take 10-30s):
```bash
until curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/ready | grep -q 200; do sleep 2; done
curl -s -H "X-Api-Key: dev-local-key" http://localhost:3000/info
```
Expected: JSON like `{"nodes":...,"edges":...,"graph_path":"/data/asw.graph","version":"0.1.0"}`

Clean up:
```bash
docker stop asw-demo-smoke
```

- [ ] **Step 4: Commit**

```bash
git add examples/map-routing/Dockerfile
git commit -m "feat(demo): add from-source Dockerfile for sea-route-demo backend"
```

---

### Task 3: docker-compose wiring

**Files:**
- Create: `examples/map-routing/docker-compose.yml`

**Interfaces:**
- Consumes: `examples/map-routing/Dockerfile` (Task 2), `export/planet.graph` (already present locally, ~738 MB), repo-root `.env` (already contains `ASW_API_KEY`).
- Produces: `docker compose up` in `examples/map-routing/` builds and runs the backend on `localhost:3000` — this is what Task 4+'s frontend `API_BASE`/`API_KEY` constants must match.

- [ ] **Step 1: Write docker-compose.yml**

```yaml
services:
  asw:
    build:
      context: ../..
      dockerfile: examples/map-routing/Dockerfile
    volumes:
      - ../../export/planet.graph:/data/asw.graph:ro
    ports:
      - "3000:3000"
    env_file:
      - ../../.env
```

Note: `ASW_API_KEY` comes from `env_file` alone — do **not** also add an `environment: - ASW_API_KEY=${ASW_API_KEY}` entry. Docker Compose resolves `${ASW_API_KEY}` for variable substitution from the shell or a `.env` file in `examples/map-routing/` (the compose file's own directory), not from the root `.env` referenced by `env_file`. With no such source, compose would substitute an empty string, and an explicit `environment:` entry always overrides the same variable coming from `env_file:` — silently blanking out the real API key and breaking every request.

- [ ] **Step 2: Run it and verify end-to-end**

Run:
```bash
cd /Volumes/2TB/Projects/auto-sea-way/examples/map-routing
docker compose up -d --build
```
Expected: service builds (reuses Task 2's image layers) and starts.

```bash
until curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/ready | grep -q 200; do sleep 2; done
curl -s http://localhost:3000/health
```
Expected: `ok`

Read the API key from `.env` and confirm authenticated routing works end-to-end (Marmaris → Gocek, matching the known-good coordinates from `export/viz.html`):
```bash
API_KEY=$(grep '^ASW_API_KEY=' ../../.env | cut -d= -f2)
curl -s -H "X-Api-Key: $API_KEY" \
  "http://localhost:3000/route?from=36.85163,28.27008&to=36.6557,28.9405"
```
Expected: JSON with a `distance_nm` field and a `geometry.coordinates` LineString — confirms the whole stack (CORS-patched binary, mounted graph, compose networking) works before any frontend code exists.

Leave the stack running — Task 4 onward needs it live for Playwright verification. (If it must be stopped between sessions: `docker compose down` from `examples/map-routing/`.)

- [ ] **Step 3: Commit**

```bash
git add examples/map-routing/docker-compose.yml
git commit -m "feat(demo): add docker-compose wiring for sea-route-demo backend"
```

---

### Task 4: Frontend skeleton — map, layers, toggle/clear controls, click-to-add

**Files:**
- Create: `examples/map-routing/index.html`

**Interfaces:**
- Consumes: the running backend from Task 3 at `http://localhost:3000` (not called yet in this task — just wiring the UI shell).
- Produces: global mutable state `waypoints` (array of `{lat, lon}`), `routeToolActive` (boolean), a Leaflet `map` instance, a `layerGroup` (Leaflet `L.layerGroup`) that Task 5 renders route geometry/labels into, and a `recompute()` function (no-op stub in this task, filled in by Task 5) that every mutation calls.

This task only builds the map shell and point-adding mechanics — no backend calls yet, so `recompute()` just redraws plain markers with no distances.

- [ ] **Step 1: Write index.html**

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8" />
  <title>ASW Sea Route Demo</title>
  <link rel="stylesheet" href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css" />
  <style>
    html, body { margin: 0; padding: 0; height: 100%; }
    #map { position: absolute; top: 0; bottom: 0; left: 0; right: 0; }
    .panel {
      position: absolute; top: 10px; left: 10px; z-index: 1000;
      background: white; padding: 10px 14px; border-radius: 8px;
      box-shadow: 0 2px 8px rgba(0,0,0,0.2); font-family: sans-serif; font-size: 13px;
    }
    .panel button {
      margin-right: 6px; padding: 4px 10px; cursor: pointer;
      border: 1px solid #999; border-radius: 4px; background: #f5f5f5;
    }
    .panel button.active { background: #0066ff; color: white; border-color: #0066ff; }
    .wp-marker .wp-dot {
      width: 10px; height: 10px; border-radius: 50%;
      background: #0066ff; border: 2px solid #fff;
      box-shadow: 0 0 2px rgba(0,0,0,0.6);
    }
  </style>
</head>
<body>
  <div id="map"></div>
  <div class="panel">
    <button id="toggleBtn">Route Tool: Off</button>
    <button id="clearBtn">Clear</button>
  </div>
  <script src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"></script>
  <script>
    const API_BASE = 'http://localhost:3000';
    const API_KEY = 'dev-local-key';

    const map = L.map('map').setView([36.75, 28.7], 8);
    L.tileLayer('https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}{r}.png', {
      maxZoom: 19,
      attribution: '&copy; OpenStreetMap &copy; CARTO'
    }).addTo(map);
    L.tileLayer('https://tiles.openseamap.org/seamark/{z}/{x}/{y}.png', {
      maxZoom: 18,
      attribution: '&copy; OpenSeaMap'
    }).addTo(map);

    let waypoints = [];
    let routeToolActive = false;
    const layerGroup = L.layerGroup().addTo(map);

    const wpIcon = L.divIcon({
      className: 'wp-marker',
      html: '<div class="wp-dot"></div>',
      iconSize: [14, 14],
      iconAnchor: [7, 7]
    });

    const toggleBtn = document.getElementById('toggleBtn');
    const clearBtn = document.getElementById('clearBtn');

    toggleBtn.addEventListener('click', () => {
      routeToolActive = !routeToolActive;
      toggleBtn.textContent = 'Route Tool: ' + (routeToolActive ? 'On' : 'Off');
      toggleBtn.classList.toggle('active', routeToolActive);
      recompute();
    });

    clearBtn.addEventListener('click', () => {
      waypoints = [];
      recompute();
    });

    map.on('click', (e) => {
      if (!routeToolActive) return;
      waypoints.push({ lat: e.latlng.lat, lon: e.latlng.lng });
      recompute();
    });

    function recompute() {
      layerGroup.clearLayers();
      waypoints.forEach((wp) => {
        L.marker([wp.lat, wp.lon], { icon: wpIcon, draggable: routeToolActive }).addTo(layerGroup);
      });
    }
  </script>
</body>
</html>
```

- [ ] **Step 2: Verify with Playwright**

Use the `mcp__plugin_playwright_playwright__browser_navigate` tool to open `file:///Volumes/2TB/Projects/auto-sea-way/examples/map-routing/index.html`.

Use `mcp__plugin_playwright_playwright__browser_snapshot` to confirm the page loaded: expect to see the "Route Tool: Off" and "Clear" buttons in the accessibility tree, with no console errors about Leaflet failing to load.

Use `mcp__plugin_playwright_playwright__browser_click` on the "Route Tool: Off" button; take another snapshot and confirm its text changed to "Route Tool: On".

Use `mcp__plugin_playwright_playwright__browser_click` with coordinates over the map area (e.g. center of the viewport) three times at different positions; use `mcp__plugin_playwright_playwright__browser_evaluate` with `() => waypoints.length` and confirm it returns `3`.

Click "Clear"; re-evaluate `() => waypoints.length` and confirm it returns `0`.

- [ ] **Step 3: Commit**

```bash
git add examples/map-routing/index.html
git commit -m "feat(demo): add map skeleton, layers, and click-to-add waypoints"
```

---

### Task 5: Route calculation, polyline rendering, distance labels

**Files:**
- Modify: `examples/map-routing/index.html`

**Interfaces:**
- Consumes: `waypoints`, `routeToolActive`, `layerGroup`, `wpIcon`, `recompute()` stub from Task 4; the live backend from Task 3 at `API_BASE`/`API_KEY`.
- Produces: `fetchSegment(from, to)`, `haversineNm(lat1, lon1, lat2, lon2)`, `midpointAlongLine(coords)` helper functions and a fully-implemented `recompute()` — later tasks (editing, undo, error handling) call `recompute()` the same way and rely on these helpers being present.

Per the approved spec, `/route` only takes one `from`/`to` pair per call, so `recompute()` issues one call per consecutive waypoint pair in parallel via `Promise.all`, then redraws everything from scratch (no incremental per-segment diffing).

- [ ] **Step 1: Replace the `recompute` stub with the full implementation**

In `examples/map-routing/index.html`, replace the `function recompute() { ... }` block from Task 4 with:

```javascript
    function haversineNm(lat1, lon1, lat2, lon2) {
      const R_NM = 3440.065;
      const toRad = (d) => d * Math.PI / 180;
      const dLat = toRad(lat2 - lat1);
      const dLon = toRad(lon2 - lon1);
      const a = Math.sin(dLat / 2) ** 2 +
        Math.cos(toRad(lat1)) * Math.cos(toRad(lat2)) * Math.sin(dLon / 2) ** 2;
      return R_NM * 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
    }

    // coords: GeoJSON LineString coordinates, [[lon, lat], ...].
    // Returns [lat, lon] at the point halfway along the polyline's own
    // length (arc-length midpoint), not the straight line between its ends.
    function midpointAlongLine(coords) {
      if (coords.length === 1) return [coords[0][1], coords[0][0]];
      const segLens = [];
      let total = 0;
      for (let i = 0; i < coords.length - 1; i++) {
        const [lon1, lat1] = coords[i];
        const [lon2, lat2] = coords[i + 1];
        const d = haversineNm(lat1, lon1, lat2, lon2);
        segLens.push(d);
        total += d;
      }
      let target = total / 2;
      for (let i = 0; i < segLens.length; i++) {
        if (target <= segLens[i] || i === segLens.length - 1) {
          const t = segLens[i] === 0 ? 0 : target / segLens[i];
          const [lon1, lat1] = coords[i];
          const [lon2, lat2] = coords[i + 1];
          return [lat1 + (lat2 - lat1) * t, lon1 + (lon2 - lon1) * t];
        }
        target -= segLens[i];
      }
      const last = coords[coords.length - 1];
      return [last[1], last[0]];
    }

    async function fetchSegment(from, to) {
      const url = `${API_BASE}/route?from=${from.lat},${from.lon}&to=${to.lat},${to.lon}`;
      try {
        const resp = await fetch(url, { headers: { 'X-Api-Key': API_KEY } });
        if (!resp.ok) return { ok: false };
        const data = await resp.json();
        return { ok: true, distanceNm: data.distance_nm, coordinates: data.geometry.coordinates };
      } catch (err) {
        return { ok: false };
      }
    }

    async function recompute() {
      layerGroup.clearLayers();
      if (waypoints.length === 0) return;

      const segmentPromises = [];
      for (let i = 0; i < waypoints.length - 1; i++) {
        segmentPromises.push(fetchSegment(waypoints[i], waypoints[i + 1]));
      }
      const segments = await Promise.all(segmentPromises);

      let cumulative = 0;
      let broken = false;
      const cumulativeAtPoint = [0];
      for (let i = 0; i < segments.length; i++) {
        if (!broken && segments[i].ok) {
          cumulative += segments[i].distanceNm;
          cumulativeAtPoint.push(cumulative);
        } else {
          broken = true;
          cumulativeAtPoint.push(null);
        }
      }

      waypoints.forEach((wp, i) => {
        const marker = L.marker([wp.lat, wp.lon], { icon: wpIcon, draggable: routeToolActive })
          .addTo(layerGroup);

        if (i > 0) {
          const label = cumulativeAtPoint[i] === null ? '—' : `${cumulativeAtPoint[i].toFixed(1)} nm`;
          marker.bindTooltip(label, {
            permanent: true, direction: 'right', className: 'cum-label', offset: [8, 0]
          });
        }
      });

      segments.forEach((seg, i) => {
        if (seg.ok) {
          const latlngs = seg.coordinates.map((c) => [c[1], c[0]]);
          L.polyline(latlngs, { color: '#0066ff', weight: 4, opacity: 0.9 }).addTo(layerGroup);
          const mid = midpointAlongLine(seg.coordinates);
          L.marker(mid, {
            icon: L.divIcon({ className: 'seg-label', html: `${seg.distanceNm.toFixed(1)} nm`, iconSize: null })
          }).addTo(layerGroup);
        } else {
          const latlngs = [
            [waypoints[i].lat, waypoints[i].lon],
            [waypoints[i + 1].lat, waypoints[i + 1].lon]
          ];
          L.polyline(latlngs, { color: '#d32f2f', weight: 3, opacity: 0.9, dashArray: '6 6' }).addTo(layerGroup);
          const midLat = (waypoints[i].lat + waypoints[i + 1].lat) / 2;
          const midLon = (waypoints[i].lon + waypoints[i + 1].lon) / 2;
          L.marker([midLat, midLon], {
            icon: L.divIcon({ className: 'seg-label error', html: 'no route found', iconSize: null })
          }).addTo(layerGroup);
        }
      });
    }
```

- [ ] **Step 2: Add the distance-label CSS**

In the `<style>` block, after the existing `.wp-marker .wp-dot { ... }` rule, add:

```css
    .cum-label, .seg-label {
      background: white; border: 1px solid #333; border-radius: 4px;
      padding: 1px 5px; font-size: 11px; font-family: monospace; white-space: nowrap;
    }
    .seg-label.error { color: #d32f2f; border-color: #d32f2f; }
```

- [ ] **Step 3: Verify with Playwright against the live backend**

Confirm the Task 3 stack is still running: `curl -s http://localhost:3000/health` should print `ok`.

Use `mcp__plugin_playwright_playwright__browser_navigate` to (re)open `file:///Volumes/2TB/Projects/auto-sea-way/examples/map-routing/index.html`.

Use `mcp__plugin_playwright_playwright__browser_click` to turn the route tool on, then use `mcp__plugin_playwright_playwright__browser_evaluate` to add three known-good waypoints directly (bypassing pixel-perfect map clicking) and await the recompute:

```javascript
() => {
  waypoints.push({ lat: 36.85163, lon: 28.27008 });
  waypoints.push({ lat: 36.6557, lon: 28.9405 });
  return recompute().then(() => 'done');
}
```

Then use `mcp__plugin_playwright_playwright__browser_snapshot` and confirm: a tooltip showing a plausible nm value (e.g. `"NN.N nm"`) near the second waypoint, and a segment label with an nm value near the route's midpoint. Cross-check the number against the `curl` result from Task 3, Step 2 — they should match (same coordinates, same backend).

Use `mcp__plugin_playwright_playwright__browser_take_screenshot` to visually confirm the blue route polyline renders between the two points along water, not a straight line through land.

- [ ] **Step 4: Commit**

```bash
git add examples/map-routing/index.html
git commit -m "feat(demo): compute and render route segments with distance labels"
```

---

### Task 6: Point editing — drag to move, click to delete

**Files:**
- Modify: `examples/map-routing/index.html`

**Interfaces:**
- Consumes: `recompute()`, `waypoints`, `routeToolActive`, `wpIcon` from Task 5.
- Produces: marker `click` and `dragend` handlers wired inside `recompute()`'s waypoint-rendering loop — no new externally-visible functions, but this is the only place later tasks (undo) need to also snapshot history from.

- [ ] **Step 1: Add click-to-delete and dragend-to-move handlers**

In `examples/map-routing/index.html`, inside `recompute()`, replace this block from Task 5:

```javascript
      waypoints.forEach((wp, i) => {
        const marker = L.marker([wp.lat, wp.lon], { icon: wpIcon, draggable: routeToolActive })
          .addTo(layerGroup);

        if (i > 0) {
          const label = cumulativeAtPoint[i] === null ? '—' : `${cumulativeAtPoint[i].toFixed(1)} nm`;
          marker.bindTooltip(label, {
            permanent: true, direction: 'right', className: 'cum-label', offset: [8, 0]
          });
        }
      });
```

with:

```javascript
      waypoints.forEach((wp, i) => {
        const marker = L.marker([wp.lat, wp.lon], { icon: wpIcon, draggable: routeToolActive })
          .addTo(layerGroup);

        if (i > 0) {
          const label = cumulativeAtPoint[i] === null ? '—' : `${cumulativeAtPoint[i].toFixed(1)} nm`;
          marker.bindTooltip(label, {
            permanent: true, direction: 'right', className: 'cum-label', offset: [8, 0]
          });
        }

        marker.on('click', (e) => {
          if (!routeToolActive) return;
          L.DomEvent.stopPropagation(e);
          waypoints.splice(i, 1);
          recompute();
        });

        marker.on('dragend', (e) => {
          if (!routeToolActive) return;
          const { lat, lng } = e.target.getLatLng();
          waypoints[i] = { lat, lon: lng };
          recompute();
        });
      });
```

(`L.DomEvent.stopPropagation(e)` on the marker's own click prevents that click from also bubbling to the map's `click` handler and appending a spurious new waypoint at the same spot.)

- [ ] **Step 2: Verify with Playwright**

Reopen `file:///Volumes/2TB/Projects/auto-sea-way/examples/map-routing/index.html`, turn the route tool on, and seed three waypoints via `browser_evaluate` (same technique as Task 5, Step 3) so the middle point sits over open water, e.g. `(36.807825, 28.265023)`.

Test delete: use `mcp__plugin_playwright_playwright__browser_click` on the middle waypoint marker (locate it via `browser_snapshot`'s element refs); then `browser_evaluate(() => waypoints.length)` and confirm it returns `2`.

Re-seed three waypoints. Test move: use `mcp__plugin_playwright_playwright__browser_drag` to drag the middle marker a short distance on-screen; then `browser_evaluate(() => waypoints[1])` and confirm its `lat`/`lon` changed from the seeded value, and `browser_snapshot` to confirm both segment labels updated to new (different) nm values.

- [ ] **Step 3: Commit**

```bash
git add examples/map-routing/index.html
git commit -m "feat(demo): add drag-to-move and click-to-delete waypoint editing"
```

---

### Task 7: Undo (Ctrl+Z)

**Files:**
- Modify: `examples/map-routing/index.html`

**Interfaces:**
- Consumes: `waypoints`, `recompute()`.
- Produces: a module-level `history` array and `pushHistory()`/`undo()` functions. Every mutation site (map click-to-add, clear button, marker click-to-delete, marker dragend) must call `pushHistory()` immediately before mutating `waypoints`, capturing the **pre-mutation** state.

- [ ] **Step 1: Add the history stack and undo function**

In `examples/map-routing/index.html`, after the line `let routeToolActive = false;`, add:

```javascript
    let history = [];

    function pushHistory() {
      history.push(waypoints.map((w) => ({ ...w })));
    }

    function undo() {
      if (history.length === 0) return;
      waypoints = history.pop();
      recompute();
    }

    document.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'z') {
        e.preventDefault();
        undo();
      }
    });
```

- [ ] **Step 2: Call `pushHistory()` before every mutation**

In the `clearBtn` click handler, change:
```javascript
    clearBtn.addEventListener('click', () => {
      waypoints = [];
      recompute();
    });
```
to:
```javascript
    clearBtn.addEventListener('click', () => {
      pushHistory();
      waypoints = [];
      recompute();
    });
```

In the `map.on('click', ...)` handler, change:
```javascript
    map.on('click', (e) => {
      if (!routeToolActive) return;
      waypoints.push({ lat: e.latlng.lat, lon: e.latlng.lng });
      recompute();
    });
```
to:
```javascript
    map.on('click', (e) => {
      if (!routeToolActive) return;
      pushHistory();
      waypoints.push({ lat: e.latlng.lat, lon: e.latlng.lng });
      recompute();
    });
```

In the marker `click` handler added in Task 6, change:
```javascript
        marker.on('click', (e) => {
          if (!routeToolActive) return;
          L.DomEvent.stopPropagation(e);
          waypoints.splice(i, 1);
          recompute();
        });
```
to:
```javascript
        marker.on('click', (e) => {
          if (!routeToolActive) return;
          L.DomEvent.stopPropagation(e);
          pushHistory();
          waypoints.splice(i, 1);
          recompute();
        });
```

In the marker `dragend` handler added in Task 6, change:
```javascript
        marker.on('dragend', (e) => {
          if (!routeToolActive) return;
          const { lat, lng } = e.target.getLatLng();
          waypoints[i] = { lat, lon: lng };
          recompute();
        });
```
to:
```javascript
        marker.on('dragend', (e) => {
          if (!routeToolActive) return;
          pushHistory();
          const { lat, lng } = e.target.getLatLng();
          waypoints[i] = { lat, lon: lng };
          recompute();
        });
```

- [ ] **Step 3: Verify with Playwright**

Reopen the page, turn the route tool on, seed one waypoint via `browser_evaluate` wrapped so history is captured correctly:
```javascript
() => {
  pushHistory();
  waypoints.push({ lat: 36.85163, lon: 28.27008 });
  return recompute().then(() => waypoints.length);
}
```
Expected: returns `1`.

Add a second point the same way, confirm `waypoints.length` is `2`.

Use `mcp__plugin_playwright_playwright__browser_press_key` with `ControlOrMeta+z`; then `browser_evaluate(() => waypoints.length)` and confirm it returns `1` (back to one point). Press it again; confirm `waypoints.length` returns `0`. Press it a third time (history now empty); confirm it stays `0` and no error is thrown.

- [ ] **Step 4: Commit**

```bash
git add examples/map-routing/index.html
git commit -m "feat(demo): add Ctrl+Z undo for waypoint edits"
```

---

### Task 8: Error handling for unreachable segments

**Files:**
- Modify: `examples/map-routing/index.html`

**Interfaces:**
- Consumes: the error path already implemented in `recompute()` (Task 5's `seg.ok === false` branch already draws the dashed red line, the "no route found" label, and the downstream `—` cumulative labels).
- Produces: nothing new — this task is verification-only, confirming the existing error-rendering branch actually fires against the real backend, since it was written but never exercised in Tasks 5-7 (all prior verification used reachable coordinate pairs).

Investigation while writing this plan found that `AppState::nearest_node` (`crates/asw-serve/src/state.rs:164`) always snaps to the nearest node on the main connected component, however far away — its own test suite confirms it finds an ocean node from a query 400nm inland (`deep_inland_finds_ocean_node`). On a single fully-connected planet graph, clicking on land will simply snap to the nearest coastline node rather than failing, so a "point on land" is not a reliable way to trigger a `/route` failure. The reliable way to exercise the error branch is to make the backend unreachable mid-session and attempt an edit — a real network failure, which `fetchSegment`'s `catch` block already handles.

- [ ] **Step 1: Verify the error path by stopping the backend mid-session**

Reopen `file:///Volumes/2TB/Projects/auto-sea-way/examples/map-routing/index.html`, turn the route tool on, and seed two reachable waypoints via `browser_evaluate` (as in Task 5, Step 3) so a normal blue route renders first. Confirm via `browser_snapshot` that it renders normally with a real nm label.

Stop the backend: `cd /Volumes/2TB/Projects/auto-sea-way/examples/map-routing && docker compose stop`

Back in the browser, use `browser_evaluate` to add a third point and trigger a fresh recompute (all segments re-fetch on every mutation, so this call now fails for every segment):
```javascript
() => {
  pushHistory();
  waypoints.push({ lat: 36.6, lon: 29.0 });
  return recompute().then(() => 'done');
}
```

Use `browser_snapshot` and `browser_take_screenshot` to confirm: both segments now render as dashed red lines with a "no route found" label at each midpoint, and the waypoint tooltip on point 2 and point 3 both show `—` instead of a stale or partial nm figure.

- [ ] **Step 2: Restart the backend for any further work**

```bash
docker compose start
until curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/ready | grep -q 200; do sleep 2; done
```

- [ ] **Step 3: Commit**

No code changed in this task (verification only). Skip the commit — proceed to Task 9.

---

### Task 9: README and full golden-path pass

**Files:**
- Create: `examples/map-routing/README.md`

**Interfaces:**
- Consumes: nothing — this is documentation plus a final end-to-end manual pass tying together Tasks 1-8.
- Produces: nothing consumed by other tasks — this is the last task in the plan.

- [ ] **Step 1: Write README.md**

```markdown
# Sea Route Demo

Throwaway example app: a fullscreen Leaflet map (CARTO Voyager + OpenSeaMap
seamarks) with one tool — click to lay down a multi-point sea route, computed
segment-by-segment via the real `asw-serve` `/route` API, with live
nautical-mile distance labels.

Not integrated into the main build, README, or CI — self-contained here.

## Run it

1. From the repo root, make sure `export/planet.graph` exists and `.env`
   has `ASW_API_KEY` set (both already required by the rest of this repo).
2. Start the backend:
   ```bash
   cd examples/map-routing
   docker compose up -d --build
   ```
   Wait for it to become ready (planet graph load takes ~10-30s):
   ```bash
   until curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/ready | grep -q 200; do sleep 2; done
   ```
3. Open `index.html` directly in a browser (double-click it, or
   `open index.html` on macOS) — no local web server needed.
4. Click "Route Tool: On", then click the map to lay down waypoints.
   Drag a point to move it, click a point to delete it, Ctrl+Z to undo,
   "Clear" to reset.

`index.html` hardcodes `API_BASE = 'http://localhost:3000'` and
`API_KEY = 'dev-local-key'` — if your `.env`'s `ASW_API_KEY` differs, edit
`API_KEY` in `index.html` to match, or set `ASW_API_KEY=dev-local-key` in
your `.env` for this demo.

## Stop it

```bash
docker compose down
```
```

- [ ] **Step 2: Run the full golden-path pass**

With the backend up (`docker compose up -d` from Step 1, if not already running), use the Playwright MCP tools to walk through the entire spec's "Testing" checklist in one pass, in order, against `file:///Volumes/2TB/Projects/auto-sea-way/examples/map-routing/index.html`:

1. Confirm `/health` and `/ready` respond and `/info` reports node/edge counts with the API key (`curl` commands from Task 3, Step 2).
2. Load the page; `browser_snapshot` confirms CARTO + OpenSeaMap layers render and the two panel buttons are present; `browser_console_messages` shows no CORS errors.
3. Activate the route tool, add 3+ points spanning open water (click on the map or seed via `browser_evaluate`); confirm segments and cumulative labels appear correctly.
4. Drag a middle point; confirm the route recomputes and labels update on drag-end.
5. Delete a point; confirm the route and labels recompute correctly.
6. Ctrl+Z several times; confirm state reverts correctly at each step, including back to zero waypoints.
7. Stop the backend (`docker compose stop`) mid-session and add a point; confirm the dashed-red-line error case renders, cumulative labels downstream show `—`; restart the backend (`docker compose start`) and confirm normal editing resumes once ready.
8. Toggle the route tool off; confirm the route stays visible but clicks/drags no longer do anything; toggle back on; confirm editing resumes.

Fix anything that fails before moving on — do not commit a broken golden path.

- [ ] **Step 3: Commit**

```bash
git add examples/map-routing/README.md
git commit -m "docs(demo): add sea-route-demo README"
```
