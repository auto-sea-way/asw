# README Redesign — Design Spec

**Date:** 2026-03-24
**Goal:** Make the README more visually appealing and "selling" — better screenshots, motivation section, quick start focused on running (not building), comparison with alternatives.

---

## 1. Screenshots

Two new images generated via Playwright automation, both using Carto Voyager basemap in Leaflet.

### 1a. Hero Image — San Francisco to Mykolaiv Route

- **File:** `docs/route-sf-mykolaiv.png`
- **Data source:** Real route from local server (`export/asw.graph`)
- **Process:**
  1. Start local server: `./target/release/asw serve --graph export/asw.graph --port 3000 --api-key screenshot`
  2. Wait for `/ready` to return 200
  3. Query: `curl -H 'X-Api-Key: screenshot' 'http://localhost:3000/route?from=37.78,-122.42&to=46.97,31.99'`
  4. Save route GeoJSON to `docs/screenshots/route-sf-mykolaiv.geojson`
  5. Render in Leaflet HTML page on Carto Voyager tiles
  6. Screenshot via Playwright (~1200×500px, fitted to route bounds)
  7. Shut down server
- **Expected:** ~8,000+ nm route through Panama Canal → Atlantic → Gibraltar → Med → Dardanelles → Black Sea. May take several seconds to compute. Playwright screenshot step should allow up to 30s timeout.
- **Styling:** Green polyline, circle markers at start/end, light map background
- **Alt text:** "San Francisco to Mykolaiv — maritime route computed through Panama Canal, Atlantic, Mediterranean, and Black Sea"
- **Replaces:** `docs/route-marmaris-santorini.png`

### 1b. Hexagon Coverage — Panama Canal North Entrance

- **File:** `docs/hexagons-panama.png`
- **Data source:** Real hexagon data from `asw geojson` export
- **Process:**
  1. Run: `./target/release/asw geojson --graph export/asw.graph --bbox -79.95,9.15,-79.75,9.40 --output export/panama.geojson`
  2. Load resulting `export/panama-hexagons.geojson` into Leaflet HTML page on Carto Voyager tiles
  3. Color hexagons by H3 resolution (e.g., blue for coarse ocean, orange for fine canal cells)
  4. Screenshot via Playwright (~1200×500px)
- **Styling:** Semi-transparent fill, thin border, colored by resolution tier
- **Alt text:** "H3 hexagonal grid covering the Panama Canal north entrance — adaptive resolution from ocean to canal"

### Screenshot Tooling

- Small HTML files in `docs/screenshots/` with Leaflet + Carto Voyager tiles
- Screenshots captured using Playwright MCP tools (available in dev environment) — no separate `capture.js` script needed
  - Navigates to `file://` URLs, waits for tiles to load, captures PNG
  - Saves to `docs/` directory
- HTML files kept in repo for reproducibility; PNGs committed to `docs/`
- GeoJSON data files saved to `docs/screenshots/` for reproducibility

---

## 2. README Structure

New section order (sections marked **NEW** or **MOVED**):

```
 1. Title + one-liner tagline
 2. Hero screenshot — SF→Mykolaiv on Carto Voyager                    (NEW image)
 3. Why auto-sea-way? — motivation for devs building maritime apps     (NEW section)
 4. Quick Start — Docker one-liner + curl example only                 (REWRITTEN)
 5. Hexagon coverage screenshot — Panama Canal                         (NEW image)
 6. How It Works — 6-step pipeline                                     (existing)
 7. Comparison with Alternatives — table                               (NEW section)
 8. Routing Benchmarks                                                 (existing)
 9. API Endpoints                                                      (existing)
10. Packages — Docker images, binaries, slim image details             (MERGED: old Docker + Packages)
11. Full Planet Build                                                  (existing)
12. CLI Reference                                                      (existing)
13. Architecture                                                       (existing)
14. Building from Source                                               (MOVED from Quick Start)
15. Environment Variables                                              (existing)
16. Known Limitations                                                  (existing)
17. Data Sources                                                       (existing)
18. License                                                            (existing)
19. Changelog link                                                     (existing)
```

---

## 3. New Section Content

### 3a. Why auto-sea-way? (Motivation)

```markdown
## Why auto-sea-way?

If you're building a maritime application — fleet tracking, voyage planning, logistics
optimization — you need a way to compute realistic sea routes between coordinates.
The alternatives are:

- **Commercial SaaS APIs** — subscription pricing, closed-source,
  no self-hosting option, vendor lock-in
- **Open-source libraries** ([eurostat/searoute](https://github.com/eurostat/searoute)
  and its forks, [genthalili/searoute-py](https://github.com/genthalili/searoute-py)) —
  static hand-drawn network of ~4,000 edges, no coastline detail, can't distinguish
  a harbor entrance from open ocean

auto-sea-way takes a different approach: it **generates** a high-resolution routing graph
algorithmically from OpenStreetMap land polygons using H3 hexagonal indexing. The result is
41M+ navigable cells with adaptive resolution — coarse in open ocean (fast), fine near
coastlines and through narrow passages like Suez and Panama (accurate).

Ship it as a single binary + graph file. Self-hosted, no third-party API keys, no rate limits.
```

### 3b. Quick Start (Docker-only)

```markdown
## Quick Start

\```bash
# Start the routing server (graph file included in image)
docker run -e ASW_API_KEY=changeme -p 3000:3000 ghcr.io/auto-sea-way/asw:0.3.1-full
\```

Wait for the `/ready` endpoint to return 200 (~60-90s while the graph loads), then query a route:

\```bash
curl -H 'X-Api-Key: changeme' \
  'http://localhost:3000/route?from=36.85,28.27&to=36.39,25.46'
\```

Returns a GeoJSON LineString. See [API Endpoints](#api-endpoints) for all available routes
and [Deployment Guide](docs/deployment.md) for Docker Compose, Kubernetes, and bare-metal examples.
```

### 3c. Comparison with Alternatives

```markdown
## Comparison with Alternatives

| | auto-sea-way | [eurostat/searoute](https://github.com/eurostat/searoute) | [searoute-py](https://github.com/genthalili/searoute-py) | Commercial SaaS APIs |
|---|---|---|---|---|
| **Routing graph** | Generated from OSM data (41M+ cells) | Static hand-drawn (~4K edges) | Static hand-drawn (~4K edges) | Proprietary |
| **Coastline detail** | Adaptive res-3→res-13 | Fixed low resolution | Fixed low resolution | Varies |
| **Narrow passages** | Suez, Panama, Bosphorus, etc. | Approximate | Approximate | Usually yes |
| **Arbitrary coordinates** | Yes | Ports + coords | Ports + coords | Varies |
| **Self-hosted** | Yes — single binary | Yes — Java library | Yes — Python library | No |
| **API server included** | Yes (HTTP/JSON) | No | No | Yes |
| **Language** | Rust | Java | Python | — |
| **License** | MIT / Apache 2.0 | EUPL | MIT | Proprietary |
| **Status** | Active | Inactive (last commit 2023) | Maintained | — |
```

### 3d. Building from Source (moved to bottom)

```markdown
## Building from Source

Requires Rust (see `rust-toolchain.toml` for the pinned version):

\```bash
cargo build --release -p asw-cli
\```
```

---

## 4. Packages Section (merged)

Combines the current "Packages" and "Docker" sections into one. Order:

1. Docker images table (existing)
2. Docker run examples: full image, slim + URL, slim + mount (existing, moved from old Docker section)
3. Memory requirements note (existing, moved from old Docker section)
4. Pre-built binaries table (existing)
5. Link to Deployment Guide

---

## 5. What Stays Unchanged

These sections keep their current content:
- How It Works
- Routing Benchmarks
- API Endpoints
- Full Planet Build
- CLI Reference
- Architecture
- Environment Variables
- Known Limitations
- Data Sources
- License
- Changelog link

---

## 6. Files Changed

| Action | File | Description |
|--------|------|-------------|
| Create | `docs/screenshots/route.html` | Leaflet page for hero route screenshot |
| Create | `docs/screenshots/hexagons.html` | Leaflet page for hex coverage screenshot |
| Create | `docs/screenshots/route-sf-mykolaiv.geojson` | Route data for hero screenshot (from server query) |
| Create | `docs/route-sf-mykolaiv.png` | Hero screenshot (generated by Playwright) |
| Create | `docs/hexagons-panama.png` | Hex coverage screenshot (generated by Playwright) |
| Delete | `docs/route-marmaris-santorini.png` | Replaced by new hero image |
| Modify | `README.md` | Full restructure per section order above |
