---
allowed-tools: Bash, Read, Glob, Grep
description: Build graph and export GeoJSON for a region. Usage: /build-region <bbox> where bbox is a preset name (marmaris, dev, dev-small) or min_lon,min_lat,max_lon,max_lat
---

# /build-region — Build graph + GeoJSON for a region

Build and visualize region: $ARGUMENTS

## Instructions

Build a routing graph for the specified region and export GeoJSON for visualization. Run as a subagent so it doesn't block the main conversation.

### 1. Parse arguments

The argument is a bbox — either a preset name or custom coordinates:
- Presets: `marmaris`, `dev`, `dev-small`
- Custom: `min_lon,min_lat,max_lon,max_lat` (e.g., `31.0,29.0,34.5,32.0`)
- If bbox starts with `-`, use `--bbox="value"` to avoid arg parsing issues

Optionally a second argument can be the output name (defaults to bbox preset or "region").

### 2. Build the graph

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release -p asw-cli
./target/release/asw build --shp land-polygons-split-4326 --bbox <BBOX> --output export/<NAME>.graph
```

If `cargo build` fails, report the error and stop.

### 3. Export GeoJSON

```bash
./target/release/asw geojson --graph export/<NAME>.graph --bbox <BBOX> --coastline --output export/<NAME>.geojson
```

This produces three files:
- `export/<NAME>-hexagons.geojson` — hex cells with `layer` property for per-resolution filtering
- `export/<NAME>-coastline.geojson` — coastline overlay
- `export/<NAME>.geojson` — combined

### 4. Report results

Summarize:
- Total cells and per-resolution breakdown (from build log)
- Number of edges
- Connectivity: largest component % and number of components
- GeoJSON file paths and sizes
- Any warnings (low connectivity, many land-crossing edges removed, etc.)
