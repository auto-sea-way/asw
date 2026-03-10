---
allowed-tools: Bash, Read, Glob, Grep
---

# /visualize — Export routing graph as KML for Google Earth

Visualize the routing graph: $ARGUMENTS

## Instructions

Export routing data as KML files for viewing in Google Earth. This works locally — no server needed.

### 1. Find graph file

Look for a `.graph` file in the `export/` directory (e.g., `export/marmaris.graph`, `export/asw.graph`).

### 2. Export KML

Use the `asw kml` command:

```bash
# Hex boundaries + edges (most useful)
cargo run -p asw-cli -- kml --graph export/marmaris.graph --hexes --edges --output export/asw.kml

# Just hexes
cargo run -p asw-cli -- kml --graph export/marmaris.graph --hexes --output export/asw-hexes.kml

# Include coastline
cargo run -p asw-cli -- kml --graph export/marmaris.graph --hexes --edges --coastline --output export/asw.kml
```

### 3. Open in Google Earth

```bash
open export/asw.kml
```

### Key details

- KML colors are AABBGGRR format (not RGB)
- Edge deduplication: only emits edge where source < target (bidirectional graph)
- Node resolution: 7 = coastal (within 50km of shore), 3 = deep ocean
- Output files go in the `export/` directory (gitignored)
- `--hexes` renders hex boundaries instead of point dots (much more useful)
