# auto-sea-way

Open source maritime auto-routing. Generates a global water-surface routing graph from OpenStreetMap land polygon data using H3 hexagonal grid indexing. Pure Rust.

```geojson
{
  "type": "FeatureCollection",
  "features": [
    {
      "type": "Feature",
      "geometry": {
        "coordinates": [
          [-15.4095938, 28.1001285],
          [-15.3916962, 28.1596386],
          [-15.4057828, 28.1815523],
          [-15.4301724, 28.1813966],
          [-15.7101972, 28.1704919],
          [-16.9573766, 27.929098],
          [-19.6117422, 27.8013625],
          [-60.9582337, 14.1065049],
          [-60.9693428, 14.091205],
          [-60.9572332, 14.0729669]
        ],
        "type": "LineString"
      },
      "properties": {
        "name": "ARC Transatlantic — Las Palmas to St. Lucia (2,678 nm)",
        "stroke": "#e74c3c",
        "stroke-width": 3,
        "stroke-opacity": 0.9
      }
    },
    {
      "type": "Feature",
      "geometry": {
        "coordinates": [
          [28.2702572, 36.8503694],
          [28.2650238, 36.8078255],
          [28.2709866, 36.794881],
          [28.304985, 36.7298323],
          [28.2834538, 36.7147419],
          [28.0693861, 36.5667919],
          [28.0351225, 36.5538083],
          [27.6786431, 36.5124342],
          [26.9650288, 36.4686358],
          [25.7500744, 36.3220237],
          [25.4374058, 36.3321481],
          [25.3601343, 36.3518564],
          [25.3521023, 36.3581032],
          [25.3670747, 36.368654],
          [25.4303912, 36.3906015]
        ],
        "type": "LineString"
      },
      "properties": {
        "name": "Eastern Med — Marmaris to Santorini (160 nm)",
        "stroke": "#2ecc71",
        "stroke-width": 3,
        "stroke-opacity": 0.9
      }
    },
    {
      "type": "Feature",
      "geometry": {
        "coordinates": [
          [-1.2951259, 50.7631972],
          [-1.3011278, 50.7682042],
          [-1.3230084, 50.7676522],
          [-1.5350385, 50.7051159],
          [-1.6189288, 50.6433367],
          [-2.0620135, 49.3993873],
          [-2.0084327, 49.2179761],
          [-2.006293, 48.6584405]
        ],
        "type": "LineString"
      },
      "properties": {
        "name": "English Channel — Cowes to Saint-Malo (136 nm)",
        "stroke": "#3498db",
        "stroke-width": 3,
        "stroke-opacity": 0.9
      }
    },
    {
      "type": "Feature",
      "geometry": {
        "coordinates": [
          [98.4000619, 7.8693423],
          [99.1694373, 6.5377319],
          [99.1994907, 6.5139133],
          [100.6491636, 3.6972559],
          [101.2001833, 2.9475104],
          [101.2934109, 2.8323584],
          [101.8613072, 2.3948159],
          [102.2094286, 2.181417],
          [102.3370118, 2.1149746],
          [102.7151524, 1.8478109],
          [103.2976624, 1.4552271],
          [103.4407384, 1.324259],
          [103.5103651, 1.2628231],
          [103.631416, 1.2084035],
          [103.7284371, 1.2140332],
          [103.8409815, 1.236001],
          [103.8599217, 1.2608607]
        ],
        "type": "LineString"
      },
      "properties": {
        "name": "Malacca Strait — Phuket to Singapore (535 nm)",
        "stroke": "#f39c12",
        "stroke-width": 3,
        "stroke-opacity": 0.9
      }
    }
  ]
}
```

*Sample routes: ARC Transatlantic rally, Eastern Mediterranean charter, English Channel crossing, Malacca Strait passage*

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

## Docker

```bash
# Full image — zero config, graph included (~870 MB)
docker run -p 3000:3000 ghcr.io/auto-sea-way/asw:0.1.0-full

# Slim image — auto-download graph on first start (cached in volume)
docker run -e ASW_GRAPH_URL=https://github.com/auto-sea-way/asw/releases/download/v0.1.0/asw.graph \
  -v asw-data:/data -p 3000:3000 ghcr.io/auto-sea-way/asw:0.1.0

# Slim image — mounted graph file
docker run -v /path/to/asw.graph:/data/asw.graph -p 3000:3000 ghcr.io/auto-sea-way/asw:0.1.0
```

See [Deployment Guide](docs/deployment.md) for Docker Compose, Kubernetes, and bare-metal examples.

## Packages

### Pre-built Binaries

Download from [GitHub Releases](https://github.com/auto-sea-way/asw/releases):

| Platform | Binary |
|----------|--------|
| Linux x86_64 | `asw-linux-amd64` |
| Linux ARM64 | `asw-linux-arm64` |
| macOS x86_64 | `asw-darwin-amd64` |
| macOS ARM64 (Apple Silicon) | `asw-darwin-arm64` |

Each release also includes the pre-built `asw.graph` file and `SHA256SUMS` for verification.

### Docker Images

Hosted on [GitHub Container Registry](https://ghcr.io/auto-sea-way/asw):

| Image | Tag | Description |
|-------|-----|-------------|
| `ghcr.io/auto-sea-way/asw` | `latest`, `0.1.0` | Slim image — bring your own graph file or auto-download via `ASW_GRAPH_URL` |
| `ghcr.io/auto-sea-way/asw` | `latest-full`, `0.1.0-full` | Full image — graph file included (~870 MB) |

Both images are available for `linux/amd64` and `linux/arm64`.

### Building from Source

Requires Rust (see `rust-toolchain.toml` for the pinned version):

```bash
cargo build --release -p asw-cli
```

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
asw serve --graph export/asw.graph --host 0.0.0.0 --port 3000

# Export as GeoJSON for visualization
asw geojson --graph export/asw.graph --bbox marmaris --coastline --output export/asw.geojson
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

Built on Hetzner ccx33 (8 dedicated AMD CPUs, 32 GB RAM):

| Metric | Value |
|--------|-------|
| Nodes | 40,397,636 |
| Edges | 305,031,722 |
| Graph file size | 843 MB |
| Connectivity | 96.9% (largest component: 39.1M nodes) |

```bash
asw cloud build --output export/asw.graph
```

## Routing Benchmarks

20 routes, 50 iterations each.

### Sailing Routes

| Route | Distance | P50 | P95 | Hops |
|-------|----------|-----|-----|------|
| English Channel | 22.1nm | 8.5ms | 10.3ms | 32>3 |
| Aegean Hop | 25.2nm | 8.4ms | 12.2ms | 59>5 |
| Strait of Gibraltar | 30.4nm | 7.6ms | 8.1ms | 81>4 |
| Baltic Crossing | 41.9nm | 8.3ms | 9.1ms | 53>5 |
| Balearic Sea | 127.1nm | 8.6ms | 9.1ms | 123>7 |
| Florida Strait | 90.0nm | 7.7ms | 8.3ms | 38>3 |
| Malacca Route | 534.4nm | 35.9ms | 38.2ms | 491>19 |
| Tasman Sea | 1265.5nm | 40.1ms | 41.3ms | 412>16 |
| South Atlantic | 3272.9nm | 30.8ms | 31.8ms | 401>8 |
| North Atlantic | 3040.6nm | 629.4ms | 684.4ms | 679>17 |

### Passage Transits

| Route | Distance | P50 | P95 | Hops |
|-------|----------|-----|-----|------|
| Suez Canal | 141.5nm | 14.2ms | 14.8ms | 1155>23 |
| Corinth Canal | 6.6nm | 6.9ms | 7.7ms | 428>9 |
| Bosphorus | 32.4nm | 7.4ms | 8.3ms | 161>10 |
| Dardanelles | 45.7nm | 6.7ms | 7.5ms | 117>5 |
| Malacca Strait | 28.6nm | 6.9ms | 7.3ms | 88>5 |
| Singapore Strait | 28.1nm | 6.6ms | 7.3ms | 48>5 |
| Messina Strait | 15.7nm | 6.4ms | 7.0ms | 70>6 |
| Dover Strait | 18.8nm | 6.1ms | 6.6ms | 23>3 |

## API Endpoints

| Endpoint | Purpose |
|----------|---------|
| `GET /route?from=lat,lon&to=lat,lon` | Compute maritime route, returns GeoJSON LineString |
| `GET /health` | Liveness probe (always 200) |
| `GET /ready` | Readiness probe (503 during graph load, 200 when ready) |
| `GET /info` | Graph metadata: node/edge counts, version |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ASW_PORT` | `3000` | Server listen port |
| `ASW_HOST` | `0.0.0.0` | Bind address |
| `ASW_GRAPH` | `export/asw.graph` | Path to graph file |
| `ASW_GRAPH_URL` | — | URL to download graph if file is missing |
| `HETZNER_TOKEN` | — | Hetzner API token for cloud builds |

## Known Limitations

- **No depth data.** Routing treats all water as navigable — there is no bathymetry or draft-clearance check. This is generally fine for small craft like sailing boats but may route larger vessels through shallow areas.
- **Panama Canal routing.** The Panama Canal passage is not correctly connected, causing routes to go around South America instead. Fix planned for a future release.
- **Kiel Canal routing.** The Kiel Canal passage is not correctly connected, causing routes to go around Denmark instead. Fix planned for a future release.

## Data Sources

| Dataset | Size | License |
|---------|------|---------|
| [OSM land polygons](https://osmdata.openstreetmap.de/data/land-polygons.html) | ~900MB | ODbL |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
