# auto-sea-way

Open source maritime auto-routing. Generates a global water-surface routing graph from OpenStreetMap land polygon data using H3 hexagonal grid indexing. Pure Rust.

## Quick Start

```bash
# Build locally (downloads land polygons automatically, or pass --shp)
cargo build --release -p asw-cli
./target/release/asw build --bbox marmaris --output export/marmaris.graph

# Cloud build (Hetzner — provision, build, download, teardown)
asw cloud build --bbox marmaris --output export/marmaris.graph --keep-server
asw cloud teardown
```

`HETZNER_TOKEN` is read from `.env` automatically.

## How It Works

1. **Read** OSM land polygons shapefile
2. **Generate** H3 hexagonal grid over ocean areas (adaptive cascade: res-3 deep ocean through res-9 shoreline)
3. **Classify** cells as navigable using hierarchical elimination and polygon intersection
4. **Build** routing graph edges between adjacent navigable cells (same-resolution + cross-resolution)
5. **Add** manual edges for critical narrow passages (Suez, Panama, Bosphorus, Dover, etc.)
6. **Serialize** graph to a compact binary format

## CLI Reference

```bash
# Local build
asw build --shp land-polygons-split-4326 --bbox marmaris --output export/marmaris.graph

# Cloud build (full pipeline)
asw cloud build --bbox marmaris --output export/marmaris.graph --keep-server

# Server management
asw cloud provision
asw cloud status
asw cloud teardown

# Serve routing API
asw serve --graph export/asw.graph --listen 0.0.0.0:3000

# Visualize in Google Earth
asw kml --graph export/asw.graph --hexes --edges --output export/asw.kml
```

Bbox supports presets (`dev`, `dev-small`, `marmaris`) or `min_lon,min_lat,max_lon,max_lat`.

## Architecture

Rust workspace with 5 crates:

```
crates/
├── asw-core      # Graph data structures, H3 utilities, routing (A*)
├── asw-build     # Graph builder: shapefiles → H3 grid → edges
├── asw-serve     # HTTP API server (axum)
├── asw-cloud     # Hetzner provisioning + SSH/SCP + remote build pipeline
└── asw-cli       # CLI entry point
```

## Full Planet Build

Built on Hetzner cpx62 (16 vCPU, 32 GB RAM):

| Metric | Value |
|--------|-------|
| Nodes | 11,462,948 |
| Edges | 85,631,148 |
| Graph file size | 1.4 GB |
| Build time | ~33 min |
| Connectivity | 95.7% (largest component: 10.97M nodes) |
| Components | 31,139 |
| Peak RAM | ~7.5 GB |
| Graph load time | ~8.5 sec |
| Route query (NY→Marmaris, 8924 km) | 0.67 sec |

```bash
asw cloud build --output export/planet.graph
```

## Known Limitations

- **No depth data.** Routing treats all water as navigable — there is no bathymetry or draft-clearance check. This is generally fine for small craft like sailing boats but may route larger vessels through shallow areas.

## Data Sources

| Dataset | Size | License |
|---------|------|---------|
| [OSM land polygons](https://osmdata.openstreetmap.de/data/land-polygons.html) | ~900MB | ODbL |

## License

MIT
