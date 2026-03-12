---
allowed-tools: Bash, Read, Glob, Grep
---

# /visualize — Export routing graph as GeoJSON for visualization

Visualize the routing graph: $ARGUMENTS

## Instructions

Export routing data as GeoJSON files for viewing in geojson.io, QGIS, or any GeoJSON viewer.

### 1. Find graph file

Look for a `.graph` file in the `export/` directory (e.g., `export/marmaris.graph`, `export/asw.graph`).

### 2. Export GeoJSON

Use the `asw geojson` command:

```bash
# Hexes + coastline (most useful)
cargo run --release -p asw-cli -- geojson --graph export/marmaris.graph --bbox marmaris --coastline --output export/marmaris.geojson

# Without coastline
cargo run --release -p asw-cli -- geojson --graph export/marmaris.graph --bbox marmaris --output export/marmaris.geojson
```

### 3. Open for viewing

```bash
open export/marmaris.geojson
```

Or upload to geojson.io for browser-based viewing.

### Key details

- Produces up to three files from a single output path:
  - `export/<name>-hexagons.geojson` — hex cells with `layer` property for per-resolution filtering
  - `export/<name>-coastline.geojson` — coastline overlay (when `--coastline` is used)
  - `export/<name>.geojson` — combined
- `--bbox` accepts preset names (marmaris, dev, dev-small) or custom min_lon,min_lat,max_lon,max_lat
- If bbox starts with `-`, use `--bbox="value"` to avoid arg parsing issues
- Output files go in the `export/` directory (gitignored)
