# auto-sea-way

Open source maritime auto-routing. Generates a global water-surface routing graph from OpenStreetMap land polygon data using H3 hexagonal grid indexing. Pure Rust.

![Marmaris to Santorini — 160 nm routed through the Aegean islands](docs/route-marmaris-santorini.png)

*Marmaris to Santorini (160 nm) — computed route through the Aegean. More benchmark routes in [bench-routes.geojson](benchmarks/bench-routes.geojson) (GeoJSON map preview works in desktop web browser).*

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
docker run -e ASW_API_KEY=your-secret -p 3000:3000 ghcr.io/auto-sea-way/asw:0.1.0-full

# Slim image — auto-download graph on first start (cached in volume)
docker run -e ASW_API_KEY=your-secret \
  -e ASW_GRAPH_URL=https://github.com/auto-sea-way/asw/releases/download/v0.1.0/asw.graph \
  -v asw-data:/data -p 3000:3000 ghcr.io/auto-sea-way/asw:0.1.0

# Slim image — mounted graph file
docker run -e ASW_API_KEY=your-secret \
  -v /path/to/asw.graph:/data/asw.graph -p 3000:3000 ghcr.io/auto-sea-way/asw:0.1.0
```

The full planet graph requires ~4.2 GiB RAM (steady-state, no peak spike). Wait for the `/ready` endpoint to return 200 before sending route queries (~60-90s for the full graph).

```bash
# Query a route (Marmaris → Santorini)
curl -H 'X-Api-Key: your-secret' 'http://localhost:3000/route?from=36.85,28.27&to=36.39,25.46'

# Check server readiness (no auth required)
curl http://localhost:3000/ready

# Server info (node/edge counts)
curl -H 'X-Api-Key: your-secret' http://localhost:3000/info
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
2. **Generate** H3 hexagonal grid over ocean areas (adaptive cascade: res-3 deep ocean through res-10 shoreline, up to res-13 in passage corridors)
3. **Classify** cells as navigable using hierarchical elimination and polygon intersection
4. **Build** routing graph edges between adjacent navigable cells (same-resolution + cross-resolution)
5. **Refine** passage corridors (Suez, Panama, Bosphorus, etc.) to higher resolutions for accurate navigation
6. **Serialize** graph to compact binary format (bitcode + zstd-19, sorted H3 indices for O(log n) spatial lookup)

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

# Serve routing API (requires ASW_API_KEY in .env or --api-key)
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

Built on Hetzner ccx53 (32 dedicated AMD CPUs, 128 GB RAM) in ~4.5 hours:

| Metric | Value |
|--------|-------|
| Nodes | 40,398,071 |
| Edges | 305,035,106 |
| Graph file size | 712 MB |
| Connectivity | 96.9% (largest component: 39.1M nodes) |
| Server memory (steady) | ~4.2 GiB |
| Server memory (peak) | ~4.2 GiB |

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

| Endpoint | Auth | Purpose |
|----------|------|---------|
| `GET /route?from=lat,lon&to=lat,lon` | Required | Compute maritime route, returns GeoJSON LineString |
| `GET /info` | Required | Graph metadata: node/edge counts, version |
| `GET /health` | None | Liveness probe (always 200) |
| `GET /ready` | None | Readiness probe (503 during graph load, 200 when ready) |

Protected endpoints require an `X-Api-Key` header matching the configured `ASW_API_KEY`. Requests with a missing or invalid key receive `401 Unauthorized`.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ASW_PORT` | `3000` | Server listen port |
| `ASW_HOST` | `0.0.0.0` | Bind address |
| `ASW_GRAPH` | `export/asw.graph` | Path to graph file |
| `ASW_GRAPH_URL` | — | URL to download graph if file is missing |
| `ASW_API_KEY` | — | **Required.** API key for authenticating `/route` and `/info` requests |
| `HETZNER_TOKEN` | — | Hetzner API token for cloud builds |

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for a detailed list of changes in each release.

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
