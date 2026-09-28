# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

auto-sea-way — open source maritime auto-routing. Generates a global water-surface routing graph from OSM land polygon data using H3 hexagonal grid indexing. Pure Rust.

## Build & Run

```bash
# Local build (requires land_polygons.shp or directory of split shapefiles)
# Canal water extraction also requires osmium-tool: apt install osmium-tool (Linux) or brew install osmium-tool (macOS)
cargo build --release -p asw-cli
./target/release/asw build --shp path/to/land-polygons-split-4326 --output export/asw.graph

# Cloud build (Hetzner provisioning + remote build)
# HETZNER_TOKEN is auto-loaded from .env — no need to pass --hetzner-token
asw cloud build --bbox marmaris --output export/graph.bin --keep-server
asw cloud provision
asw cloud teardown
asw cloud status

# Serve routing API (ASW_API_KEY required, loaded from .env)
asw serve --graph export/asw.graph --port 3000

# Docker
docker build -t asw .
docker run -v asw-data:/data -p 3000:3000 asw

# Export GeoJSON for visualization
asw geojson --graph export/asw.graph --bbox marmaris --coastline --output export/asw.geojson
```

## Architecture

Rust workspace with 5 crates:

- **asw-core** — graph data structures, H3 utilities, routing algorithms
- **asw-build** — graph builder: reads shapefiles, creates H3 grid, builds edges
- **asw-serve** — HTTP API server (axum) for route queries
- **asw-cloud** — Hetzner provisioning + SSH/SCP + remote build pipeline
- **asw-cli** — CLI entry point, ties all crates together

## Key Design Decisions

- H3 hexagonal grid: adaptive multi-resolution cascade (res-3 ocean through res-10 shoreline, up to res-13 in passage corridors)
- Hierarchical cell elimination: test parent cell before expanding children
- Land polygons loaded without bbox filter — R-tree handles spatial queries efficiently
- Critical narrow passages (Suez, Panama, etc.) use resolution cascade corridors
- Graph format v4: flat little-endian sections, memory-mapped (`memmap2`), sorted `u64` node ids for O(log n) lookup, per-node `shore_dist: u8` (quantized distance-to-shore, 0.02 nm units, saturating at 5.1 nm), coastline runs as an i32 microdegree head plus i16 deltas per point, with a 0.1° grid index, edge weights recomputed as centre-to-centre haversine (nothing stored per edge but the target id). Spec: `docs/superpowers/specs/2026-09-27-graph-format-v4-design.md`
- Nearest-node snapping via H3 binary search (no R-tree) — sorted node_h3 vec is both coordinate store and spatial index
- Query-time endpoint stitching: routes start/end at the exact requested coordinates; clear line-of-sight pairs short-circuit to a direct great-circle leg without a graph search (shore-buffer aware; no graph densification needed for deep water)
- A* buffer pool (2 buffer sets, zero-filled, allocated on first use and reused) eliminates per-request allocation spikes
- Cloud builds: shell out to system `ssh`/`scp` for streaming output
- Hetzner API via reqwest (blocking), no SDK dependency
- Bbox presets: "dev", "dev-small", "marmaris" or custom min_lon,min_lat,max_lon,max_lat
- `.env` file in project root auto-loaded by CLI (dotenvy) — `HETZNER_TOKEN` and `ASW_API_KEY` picked up automatically
- API key auth: `/route` and `/info` require `X-Api-Key` header; `/health` and `/ready` are public
- `export/` directory for all output files (graphs, GeoJSON) — gitignored
- Docker: statically-linked musl binaries on distroless/static-debian12, graph auto-download via `ASW_GRAPH_URL`
- Readiness probe: server starts TCP listener immediately, `/ready` returns 503 until graph loaded
- Server memory: the graph is memory-mapped, so RSS is the file in page cache plus the A* buffer pages touched by queries (buffers are zero-filled and lazily allocated). Planet v4 measured on Linux: 1.44 GB file, `/ready` in 0.2 s from page cache, 1.38 GB RSS after open, 1.43 GB after transoceanic routes; a 4 GB instance is enough
- CI/CD: GitHub Actions for CI, Docker push to ghcr.io, and binary releases on version tags
