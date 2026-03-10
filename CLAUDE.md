# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

auto-sea-way — open source maritime auto-routing. Generates a global water-surface routing graph from OSM land polygon data using H3 hexagonal grid indexing. Pure Rust.

## Build & Run

```bash
# Local build (requires land_polygons.shp or directory of split shapefiles)
cargo build --release -p asw-cli
./target/release/asw build --shp path/to/land-polygons-split-4326 --output export/asw.graph

# Cloud build (Hetzner provisioning + remote build)
# HETZNER_TOKEN is auto-loaded from .env — no need to pass --hetzner-token
asw cloud build --bbox marmaris --output export/graph.bin --keep-server
asw cloud provision
asw cloud teardown
asw cloud status

# Serve routing API
asw serve --graph export/asw.graph

# Export KML for visualization
asw kml --graph export/asw.graph --hexes --edges --output export/asw.kml
```

## Architecture

Rust workspace with 5 crates:

- **asw-core** — graph data structures, H3 utilities, routing algorithms
- **asw-build** — graph builder: reads shapefiles, creates H3 grid, builds edges
- **asw-serve** — HTTP API server (axum) for route queries
- **asw-cloud** — Hetzner provisioning + SSH/SCP + remote build pipeline
- **asw-cli** — CLI entry point, ties all crates together

## Key Design Decisions

- H3 hexagonal grid: adaptive multi-resolution cascade (res-3 ocean through res-9 shoreline)
- Hierarchical cell elimination: test parent cell before expanding children
- Land polygons loaded without bbox filter — R-tree handles spatial queries efficiently
- Critical narrow passages (Suez, Panama, etc.) get manually-added edges
- Cloud builds: shell out to system `ssh`/`scp` for streaming output
- Hetzner API via reqwest (blocking), no SDK dependency
- Bbox presets: "dev", "dev-small", "marmaris" or custom min_lon,min_lat,max_lon,max_lat
- `.env` file in project root auto-loaded by CLI (dotenvy) — `HETZNER_TOKEN` picked up automatically
- `export/` directory for all output files (graphs, KML) — gitignored
