# README Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign README.md with new hero screenshot, hexagon coverage image, motivation section, Docker-only quick start, and comparison table.

**Architecture:** Generate two screenshots using Leaflet HTML pages + Playwright MCP tools. Route data from local server, hex data from `asw geojson` CLI. Rewrite README.md with new section ordering per spec.

**Tech Stack:** Leaflet.js, Carto Voyager tiles, Playwright (MCP tools), existing `asw` CLI binary

**Spec:** `docs/superpowers/specs/2026-03-24-readme-redesign-design.md`

---

### Task 1: Generate route GeoJSON data (SF → Mykolaiv)

**Files:**
- Create: `docs/screenshots/route-sf-mykolaiv.geojson`

This task starts the local server, queries the SF→Mykolaiv route, and saves the GeoJSON response.

- [ ] **Step 1: Build the release binary (if not already built)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build --release -p asw-cli 2>&1 | tail -5`
Expected: `Finished` or `Compiling` messages, exit 0

- [ ] **Step 2: Create docs/screenshots directory**

Run: `mkdir -p docs/screenshots`

- [ ] **Step 3: Start the server in the background**

Run: `./target/release/asw serve --graph export/asw.graph --port 3001 --api-key screenshot &`

Note: Use port 3001 to avoid conflicts. The server PID will be needed for cleanup.

- [ ] **Step 4: Wait for server readiness**

Run: `for i in $(seq 1 120); do curl -s http://localhost:3001/ready && break; sleep 1; done`
Expected: `{"status":"ok"}` after ~60-90 seconds

- [ ] **Step 5: Query the route and save GeoJSON**

Run: `curl -s -H 'X-Api-Key: screenshot' 'http://localhost:3001/route?from=37.78,-122.42&to=46.97,31.99' > docs/screenshots/route-sf-mykolaiv.geojson`

Verify: `cat docs/screenshots/route-sf-mykolaiv.geojson | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['type'], len(d['geometry']['coordinates']), 'coords')"`
Expected: `Feature <N> coords` where N is a large number of waypoints

- [ ] **Step 6: Kill the server**

Run: `kill %1 2>/dev/null || true`

---

### Task 2: Generate hexagon GeoJSON data (Panama Canal)

**Files:**
- Create: `export/panama-hexagons.geojson` (intermediate, used by Task 4)

- [ ] **Step 1: Export Panama Canal hexagons**

Run: `./target/release/asw geojson --graph export/asw.graph --bbox -79.95,9.15,-79.75,9.40 --output export/panama.geojson 2>&1`
Expected: Log output showing hex features exported. Creates `export/panama-hexagons.geojson`.

- [ ] **Step 2: Verify the output and property format**

Run: `python3 -c "import json; d=json.load(open('export/panama-hexagons.geojson')); print(len(d['features']), 'features'); print('sample props:', d['features'][0]['properties'])"`
Expected: A number of features (likely thousands) with properties containing `layer: 'hex-res-N'` and `fill` color.

- [ ] **Step 3: Copy hexagon GeoJSON to screenshots directory**

Run: `mkdir -p docs/screenshots && cp export/panama-hexagons.geojson docs/screenshots/panama-hexagons.geojson`

---

### Task 3: Create Leaflet HTML pages for screenshots

**Files:**
- Create: `docs/screenshots/route.html`
- Create: `docs/screenshots/hexagons.html`

- [ ] **Step 1: Create the route screenshot HTML**

Create `docs/screenshots/route.html`:

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>Route Screenshot</title>
  <style>
    html, body, #map { margin: 0; padding: 0; width: 1200px; height: 500px; }
  </style>
  <link rel="stylesheet" href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css" />
  <script src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"></script>
</head>
<body>
  <div id="map"></div>
  <script>
    const map = L.map('map', { zoomControl: false, attributionControl: false });
    L.tileLayer('https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}{r}.png', {
      maxZoom: 19
    }).addTo(map);

    fetch('route-sf-mykolaiv.geojson')
      .then(r => r.json())
      .then(data => {
        // Handle both Feature and FeatureCollection responses
        const geom = data.type === 'FeatureCollection'
          ? data.features[0].geometry
          : data.geometry;
        const coords = geom.coordinates.map(c => [c[1], c[0]]);
        const line = L.polyline(coords, {
          color: '#2d8a4e',
          weight: 3,
          opacity: 0.9
        }).addTo(map);

        // Start/end markers
        const start = coords[0];
        const end = coords[coords.length - 1];
        L.circleMarker(start, { radius: 6, color: '#d32f2f', fillColor: '#d32f2f', fillOpacity: 1, weight: 2 }).addTo(map);
        L.circleMarker(end, { radius: 6, color: '#d32f2f', fillColor: '#d32f2f', fillOpacity: 1, weight: 2 }).addTo(map);

        map.fitBounds(line.getBounds(), { padding: [20, 20] });
      });
  </script>
</body>
</html>
```

- [ ] **Step 2: Create the hexagon screenshot HTML**

Create `docs/screenshots/hexagons.html`:

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>Hexagon Coverage Screenshot</title>
  <style>
    html, body, #map { margin: 0; padding: 0; width: 1200px; height: 500px; }
  </style>
  <link rel="stylesheet" href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css" />
  <script src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"></script>
</head>
<body>
  <div id="map"></div>
  <script>
    const map = L.map('map', { zoomControl: false, attributionControl: false });
    L.tileLayer('https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}{r}.png', {
      maxZoom: 19
    }).addTo(map);

    // Resolution-based color map (overrides the GeoJSON fill property for better screenshot aesthetics)
    const resColors = {
      3: '#1a5276', 4: '#2471a3', 5: '#2e86c1', 6: '#3498db',
      7: '#5dade2', 8: '#85c1e9', 9: '#aed6f1',
      10: '#f39c12', 11: '#e67e22', 12: '#d35400', 13: '#c0392b'
    };

    fetch('panama-hexagons.geojson')
      .then(r => r.json())
      .then(data => {
        const layer = L.geoJSON(data, {
          style: feature => {
            const layerName = feature.properties.layer || '';
            const res = parseInt(layerName.replace('hex-res-', ''));
            const color = resColors[res] || feature.properties.fill || '#888';
            return {
              fillColor: color,
              fillOpacity: 0.45,
              color: color,
              weight: 0.8,
              opacity: 0.7
            };
          }
        }).addTo(map);

        map.fitBounds(layer.getBounds(), { padding: [10, 10] });
      });
  </script>
</body>
</html>
```

- [ ] **Step 3: Verify both HTML files load in browser**

Open each file manually or with Playwright to confirm they render without errors. The map tiles need a network connection to load.

---

### Task 4: Capture screenshots with Playwright

**Files:**
- Create: `docs/route-sf-mykolaiv.png`
- Create: `docs/hexagons-panama.png`

Uses Playwright MCP tools to navigate to the HTML pages and capture screenshots.

- [ ] **Step 1: Capture the route screenshot**

Using Playwright MCP tools:
1. Navigate to `file:///Volumes/2TB/Projects/auto-sea-way/docs/screenshots/route.html`
2. Wait 5 seconds for tiles to load
3. Take screenshot, save to `docs/route-sf-mykolaiv.png`

Verify: Open the PNG to confirm route is visible on Carto Voyager basemap with green polyline and red markers.

- [ ] **Step 2: Capture the hexagon screenshot**

Using Playwright MCP tools:
1. Navigate to `file:///Volumes/2TB/Projects/auto-sea-way/docs/screenshots/hexagons.html`
2. Wait 5 seconds for tiles to load
3. Take screenshot, save to `docs/hexagons-panama.png`

Verify: Open the PNG to confirm hexagons are visible with color gradient from blue (ocean) to orange/red (canal).

- [ ] **Step 3: Delete old screenshot (via git)**

Run: `git rm docs/route-marmaris-santorini.png`

---

### Task 5: Rewrite README.md

**Files:**
- Modify: `README.md`

This is the main task — rewrite the entire README per the spec's section ordering. The full content is assembled from existing sections (reordered) plus new sections from the spec.

- [ ] **Step 1: Write the new README**

The new README.md follows this structure (see spec Section 2 for ordering):

**Header + hero:**
```markdown
# auto-sea-way

Open source maritime auto-routing. Generates a global water-surface routing graph from OpenStreetMap land polygon data using H3 hexagonal grid indexing. Pure Rust.

![San Francisco to Mykolaiv — maritime route computed through Panama Canal, Atlantic, Mediterranean, and Black Sea](docs/route-sf-mykolaiv.png)

*San Francisco to Mykolaiv — computed route across the Atlantic, through the Mediterranean and into the Black Sea. More benchmark routes in [bench-routes.geojson](benchmarks/bench-routes.geojson).*
```

**Motivation** — copy verbatim from spec Section 3a.

**Quick Start** — copy verbatim from spec Section 3b.

**Hexagon image:**
```markdown
![H3 hexagonal grid covering the Panama Canal north entrance — adaptive resolution from ocean to canal](docs/hexagons-panama.png)

*Adaptive H3 hexagonal grid at the Panama Canal — coarse cells in open water, fine resolution through the canal corridor.*
```

**How It Works** — copy from existing README lines 88-96.

**Comparison** — copy from spec Section 3c.

**Routing Benchmarks** — copy from existing README lines 151-183.

**API Endpoints** — copy from existing README lines 185-194.

**Packages** — merged section:
1. Docker Images subsection (from existing lines 69-78)
2. Docker run examples (from existing lines 25-37)
3. Memory note (from existing line 39)
4. Query examples (from existing lines 41-50)
5. Deployment guide link (from existing line 52)
6. Pre-built Binaries subsection (from existing lines 56-67)

**Full Planet Build** — copy from existing README lines 133-149.

**CLI Reference** — copy from existing README lines 97-118.

**Architecture** — copy from existing README lines 120-131.

**Building from Source** — copy from spec Section 3d.

**Environment Variables** — copy from existing README lines 196-205.

**Known Limitations** — copy from existing README lines 211-214.

**Data Sources** — copy from existing README lines 216-223.

**License** — copy from existing README lines 225-232.

**Changelog** — copy from existing README lines 207-209.

- [ ] **Step 2: Verify all internal links work**

Check that these anchors/links resolve:
- `[API Endpoints](#api-endpoints)`
- `[Deployment Guide](docs/deployment.md)`
- `[bench-routes.geojson](benchmarks/bench-routes.geojson)`
- `[CHANGELOG.md](CHANGELOG.md)`
- Both image paths: `docs/route-sf-mykolaiv.png`, `docs/hexagons-panama.png`

Run: `ls docs/route-sf-mykolaiv.png docs/hexagons-panama.png docs/deployment.md benchmarks/bench-routes.geojson CHANGELOG.md`
Expected: All files exist.

- [ ] **Step 3: Commit all changes**

```bash
git add docs/screenshots/ docs/route-sf-mykolaiv.png docs/hexagons-panama.png README.md
git commit -m "docs: redesign README with new hero image, motivation, and comparison table"
```

Note: `docs/route-marmaris-santorini.png` was already removed via `git rm` in Task 4 Step 3.
