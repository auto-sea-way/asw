<img src="docs/logo/logo.svg" alt="auto-sea-way logo" width="120">

# auto-sea-way

Open source sea routing between any two coordinates on the planet. auto-sea-way builds a global routing graph of the water surface from OpenStreetMap coastlines, indexes it with H3 hexagons, and serves routes over a small HTTP API. Written in Rust.

![San Francisco to Mykolaiv — maritime route computed through Panama Canal, Atlantic, Mediterranean, and Black Sea](docs/route-sf-mykolaiv.png)

*San Francisco to Mykolaiv (9,768 nm): the computed route goes through the Panama Canal, across the Atlantic, through the Mediterranean and into the Black Sea. More benchmark routes in [bench-routes.geojson](benchmarks/bench-routes.geojson).*

## Why auto-sea-way?

A maritime application such as fleet tracking, voyage planning or logistics needs realistic sea routes between coordinates: around headlands, through straits and canals, into harbours.

The existing options are limited in different ways:

- Commercial routing APIs are closed, priced per request, and cannot be hosted by you.
- The open source libraries ([eurostat/searoute](https://github.com/eurostat/searoute),
  [searoute-py](https://github.com/genthalili/searoute-py),
  [searoute-ts](https://github.com/mayurrawte/searoute-ts),
  [scgraph](https://github.com/connor-makowski/scgraph)) route along MARNET, a fixed
  network of shipping lanes from Eurostat, and
  [@arcnautical/maritime-routing](https://github.com/arcnautical/maritime-routing) routes
  on a grid of about 5.5 km. They have no coastline detail, so they cannot tell a harbour
  entrance from open ocean.

auto-sea-way generates its graph instead of drawing it. About 40 million navigable cells are derived from OpenStreetMap land polygons, coarse in the open ocean and fine along coastlines and inside narrow passages such as Suez and Panama. Routes start and end at the exact coordinates you ask for, and a route that has to touch land (a pin on a quay, a headland clipped by smoothing) reports which segments do.

You run it yourself: one binary and one graph file. Routing happens on your own server, so requests cost nothing and the coordinates stay with you.

> **Not for navigation.** Routes are for planning and analysis only. See [Not for navigation](#not-for-navigation).

## Quick Start

```bash
# Start the routing server (graph file included in image)
docker run -e ASW_API_KEY=changeme -p 3000:3000 ghcr.io/auto-sea-way/asw:0.11.2-full
```

Wait for the `/ready` endpoint to return 200 (a few seconds while the graph file is mapped and read in), then query a route:

```bash
curl -H 'X-Api-Key: changeme' \
  'http://localhost:3000/route?from=36.85,28.27&to=36.39,25.46'
```

Returns a GeoJSON LineString. See [API Endpoints](#api-endpoints) for all available routes and [Deployment Guide](docs/deployment.md) for Docker Compose, Kubernetes, and bare-metal examples.

![H3 hexagonal grid at Marmaris Bay — adaptive resolution from open water to coastline](docs/hexagons-marmaris-bay.png)

*Adaptive H3 hexagonal grid at Marmaris Bay — coarse cells in open water, fine resolution along the coastline.*

## How It Works

1. **Read** OSM land polygons shapefile
2. **Generate** H3 hexagonal grid over ocean areas (adaptive cascade: res-3 deep ocean through res-10 shoreline, up to res-14 in passage corridors)
3. **Classify** cells as navigable using hierarchical elimination and polygon intersection
4. **Build** routing graph edges between adjacent navigable cells (same-resolution + cross-resolution)
5. **Refine** passage corridors (Suez, Panama, Bosphorus, etc.) to higher resolutions for accurate navigation
6. **Summarise** the graph as a coarse graph: one node per connected piece of water inside each res-3 region (about 41k nodes). A long route first finds its way on this coarse graph, and the fine search then stays inside a corridor around that path
7. **Serialize** graph to a flat memory-mapped binary file (format v5: sorted H3 ids, varint edge targets, per-node shore distance, delta-coded coastline runs with a 0.1° grid index, the coarse graph; no stored weights, no compression)

## Comparison with Alternatives

| | auto-sea-way | [scgraph](https://github.com/connor-makowski/scgraph) | [searoute-py](https://github.com/genthalili/searoute-py) | [searoute-ts](https://github.com/mayurrawte/searoute-ts) | [eurostat/searoute](https://github.com/eurostat/searoute) | [@arcnautical/maritime-routing](https://github.com/arcnautical/maritime-routing) | Commercial SaaS APIs |
|---|---|---|---|---|---|---|---|
| **Routing graph** | Generated from OSM land polygons (~39M cells) | marnet geograph (from searoute, ~11K nodes) | MARNET shipping lanes (~4K lanes, ~10K points) | MARNET, 100 km by default (20 km tested) | MARNET 5 to 100 km (up to ~72K points) | 0.05° grid from OSM water (~5.5 km) | Proprietary |
| **Coastline detail** | Adaptive res-3 to res-14 | None, routes along lane waypoints | None, fixed lanes | None, fixed lanes | None, fixed lanes | Grid cells of ~5.5 km | Varies |
| **Narrow passages** | Suez, Panama, Kiel, Corinth, Bosphorus and more | Only if in the network | Approximate | Approximate | Approximate | Forced grid cells for some | Usually yes |
| **Arbitrary coordinates** | Yes, exact start and end | Joins the nearest network node | Joins the nearest network node | Joins the nearest network node | Joins the nearest network node | Yes, straight lines where it finds no path | Varies |
| **NGA Pub 151, 22 port pairs (tested 2026-11-05 and 06)** | Within 2 % on 22, mean 0.5 % | Within 2 % on 15, mean 2.8 % | Within 2 % on 14, mean 1.7 % | Within 2 % on 12, mean 2.1 % | Within 2 % on 12, mean 2.4 % | Within 2 % on 5, mean 5.4 % | Not tested |
| **Over land on 45 test routes, away from inland end points** | 1.6 nm | 924 nm | 394 nm | 715 nm | 549 nm | 17,184 nm | Not tested |
| **Median time per route** | 34 ms (up to 0.75 s) | 0.06 ms | 0.66 ms | 147 ms | Batch only | 56 ms | Varies |
| **Self-hosted** | Yes, single binary or Docker | Yes, Python library | Yes, Python library | Yes, npm library | Yes, Java library | Yes, npm library | No |
| **API server included** | Yes (HTTP/JSON) | No | No | No | No | No | Yes |
| **Multi-modal** | Maritime only | Maritime, road, rail, custom | Maritime only | Maritime only | Maritime only | Maritime only | Varies |
| **Language** | Rust | Python (optional C++ extension) | Python | TypeScript | Java | TypeScript | |
| **License** | MIT / Apache 2.0 | MIT | Apache 2.0 | MIT | EUPL 1.2 | MIT | Proprietary |
| **Status** | Active | Active | Maintained (last commit 2026-05) | Active | Inactive (last commit 2022-01) | New (1.0.1, 2026-09) | |

## Benchmarks

On 5 and 6 November 2026 auto-sea-way was compared with the five open source routers above on 45 routes: the 23 benchmark routes of `asw bench` and 22 port pairs from NGA Pub 151, *Distances Between Ports*. All lines were measured by the same code, from the requested start to the requested end, and checked for land against the OpenStreetMap land polygons and Natural Earth. A line that goes over land was not compared. auto-sea-way had the shortest line on water on 40 of the 45 routes and on 15 of the 16 routes under 200 nm, and it was the closest to Pub 151 on 15 of the 22 pairs. scgraph and searoute-py are much faster, and all five are much smaller than the 1.43 GB graph. The [benchmarks page](https://auto-sea-way.github.io/benchmarks.html) has the method, the numbers for every route, a map to compare the routes and the speed tables of `asw bench`.

`asw bench` writes the latest local run to [benchmarks/BENCHMARKS.md](benchmarks/BENCHMARKS.md) and the routes to [bench-routes.geojson](benchmarks/bench-routes.geojson). Routes longer than 500 nm first find their way on the coarse graph and then search only a corridor around it; the three ocean crossings take 0.36 to 0.74 s. Graphs in an older format must be rebuilt: older files are rejected at load time.

## API Endpoints

| Endpoint | Auth | Purpose |
|----------|------|---------|
| `GET /route?from=lat,lon&to=lat,lon` | Required | Compute maritime route, returns GeoJSON LineString |
| `GET /info` | Required | Graph metadata: node/edge counts, version |
| `GET /health` | None | Liveness probe (always 200) |
| `GET /ready` | None | Readiness probe (503 during graph load, 200 when ready) |

Protected endpoints require an `X-Api-Key` header matching the configured `ASW_API_KEY`. Requests with a missing or invalid key receive `401 Unauthorized`.

**`/route` parameters:**

- `from`, `to` — `lat,lon` coordinates
- `shore_buffer` (optional, nautical miles, `0`–`5.0`, default `0`) — soft clearance: the router strongly prefers water at least this far from the coastline, but can still enter harbors/coves when there is no alternative; not a hard guarantee. Inside the corridor box of each man-made canal (see `canals`) the penalty is off, since a canal is narrower than the buffer and the route would otherwise go around it. For Suez and Panama this box also covers the Bitter Lakes, Gatun Lake and Limón Bay. The response echoes the requested value as `shore_buffer_nm`
- `arctic` (optional, `true`/`false`, default `false`) — opens the Northern Sea Route and the Northwest Passage. They are closed by default because they are only passable in summer and mostly for ice-class ships, so Asia–Europe routes go via Suez. The central Arctic north of 80°N is permanent pack ice and is never routed, with or without this flag
- `canals` (optional, `true`/`false`, default `true`) — `false` closes the man-made canals (Suez, Panama, Kiel, Corinth, Lefkada, Potidea, Osor, Privlaka, Vistula Spit, Cape Cod, Cape May, Chesapeake–Delaware), so the route goes around them. Use it to avoid canal fees and transit bookings. Each canal is closed in the middle, so ports at both ends stay reachable from their own side. Natural straits (Bosphorus, Dover, Malacca and others) are not affected. Long routes stay fast with canals closed (Rotterdam to Singapore around Africa: about 0.5 s). The exception is a long route that starts or ends in the same res-3 region as a closed canal, which needs a search of the whole graph (Colón to Callao around South America: about 4 s)

**`/route` response:** `geometry` is a polyline whose points are at most 20 nm apart along the great circle; the route is checked for land along the great circle between waypoints at that step, so great-circle, rhumb-line and straight-line readings of the geometry agree within 0.1 nm. `distance_nm` is the great-circle length of that polyline and counts only water segments. When a requested point sits on land, the geometry still starts/ends exactly there, and the overland connector segments are listed in `land_legs` (segment indices into `geometry.coordinates`) so clients can render them differently — they contribute nothing to `distance_nm`. Land detection is a coastline-crossing test, not point-in-polygon: a segment that lies entirely inland on one landmass, never touching a coastline, is not detected. `land_legs` covers pins near the shore, not arbitrary points deep inland.

## Packages

### Docker Images

Hosted on [GitHub Container Registry](https://ghcr.io/auto-sea-way/asw):

| Image | Tag | Description |
|-------|-----|-------------|
| `ghcr.io/auto-sea-way/asw` | `latest`, `0.11.2` | Slim image — bring your own graph file or auto-download via `ASW_GRAPH_URL` |
| `ghcr.io/auto-sea-way/asw` | `latest-full`, `0.11.2-full` | Full image — graph file included (~1.5 GB) |

Both images are available for `linux/amd64` and `linux/arm64`.

```bash
# Full image — zero config, graph included (~1.5 GB)
docker run -e ASW_API_KEY=your-secret -p 3000:3000 ghcr.io/auto-sea-way/asw:0.11.2-full

# Slim image — auto-download graph on first start (cached in volume)
docker run -e ASW_API_KEY=your-secret \
  -e ASW_GRAPH_URL=https://github.com/auto-sea-way/asw/releases/download/v0.11.2/asw.graph.zst \
  -v asw-data:/data -p 3000:3000 ghcr.io/auto-sea-way/asw:0.11.2

# Slim image — mounted graph file
docker run -e ASW_API_KEY=your-secret \
  -v /path/to/asw.graph:/data/asw.graph -p 3000:3000 ghcr.io/auto-sea-way/asw:0.11.2
```

The planet graph is memory-mapped. Measured on Linux with the 1.44 GB planet file: `/ready` in 0.2 s when the file is in the page cache (a few seconds from cold disk), 1.38 GB RSS after open, 1.43 GB after four transoceanic routes. Resident memory is the file plus the A* buffer pages a query touches, so a **4 GB instance** runs it comfortably. Wait for `/ready` to return 200 before sending route queries.

See [Deployment Guide](docs/deployment.md) for Docker Compose, Kubernetes, and bare-metal examples.

### Mobile bindings

Each release also attaches `AswMobile-<version>.zip` (iOS xcframework and Swift
source) and `asw-mobile-<version>.aar` (Android, arm64-v8a): the same graph file,
opened on the phone through `openGraph`, `version`, `isWater` and `route`. Apps
download `asw.graph.zst` (about 540 MB) and install it with `installGraph`. See
[crates/asw-mobile/README.md](crates/asw-mobile/README.md).

### Pre-built Binaries

Download from [GitHub Releases](https://github.com/auto-sea-way/asw/releases):

| Platform | Binary |
|----------|--------|
| Linux x86_64 | `asw-linux-amd64` |
| Linux ARM64 | `asw-linux-arm64` |
| macOS x86_64 | `asw-darwin-amd64` |
| macOS ARM64 (Apple Silicon) | `asw-darwin-arm64` |

Each release also includes the pre-built `asw.graph` file, its zstd-compressed `asw.graph.zst`, and `SHA256SUMS` for verification.

## Full Planet Build

Built on Hetzner ccx53 (32 dedicated vCPU, 128 GB RAM) in about 4.5 hours:

| Metric | Value |
|--------|-------|
| Nodes | 39,068,079 |
| Edges | 296,619,250 |
| Graph file size | 1,429 MB (v5, uncompressed, memory-mapped) |
| Connectivity | 100% (single connected component after build-time pruning) |
| Server memory (RSS) | 1.38 GB after open, 1.43 GB after a transoceanic route mix (measured on the v4 file, 12 MB larger) |
| Server memory (total) | plan for ~2.5 GB |
| Minimum instance | 4 GB RAM, no swap needed |

```bash
asw cloud build --output export/asw.graph
```

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

# Benchmark routing performance (23 fixed routes, 50 iterations each)
asw bench --graph export/asw.graph --output export/bench.json
asw bench --compare export/bench.json          # compare against a saved baseline
asw bench --shore-buffer 1.0                   # benchmark with the shore-clearance penalty applied
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

## Building from Source

Requires Rust (see `rust-toolchain.toml` for the pinned version):

```bash
cargo build --release -p asw-cli
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ASW_PORT` | `3000` | Server listen port |
| `ASW_HOST` | `0.0.0.0` | Bind address |
| `ASW_GRAPH` | `export/asw.graph` | Path to graph file |
| `ASW_GRAPH_URL` | — | URL to download the graph if the file is missing; a `.zst` URL is unpacked and checked while downloading |
| `ASW_API_KEY` | — | **Required.** API key for authenticating `/route` and `/info` requests |
| `HETZNER_TOKEN` | — | Hetzner API token for cloud builds |

## Not for navigation

auto-sea-way computes routes for planning and for software that needs sea distances. It is not a navigation aid and must not be used as the only source for navigation at sea. The graph has no depth, tide, current, traffic separation scheme, ice or weather data, and the OpenStreetMap coastline can be wrong or out of date. Check every route against official nautical charts and notices to mariners before you use it on the water. The software and the graph are provided "as is", without warranty of any kind, as stated in the licenses.

## Known Limitations

- **No depth data.** Routing treats all water as navigable — there is no bathymetry or draft-clearance check. This is generally fine for small craft like sailing boats but may route larger vessels through shallow areas. The `shore_buffer` parameter partially mitigates this by keeping routes off headlands and uncharted near-shore hazards, but it is not a substitute for nautical charts.
- **No sea-ice data.** The graph comes from land polygons, so ice is modelled with fixed areas: everything north of 80°N is closed, and the Northern Sea Route (Vilkitsky Strait and Severnaya Zemlya) and the Northwest Passage (across the Canadian Arctic Archipelago) are closed unless the request sets `arctic=true`. Ports inside these areas, such as Resolute, need `arctic=true`. Seasonal ice elsewhere, including around Antarctica, is not considered.
- **Only water inside the OSM coastline, plus selected passages.** Lakes, lagoons and rivers are land in the source data. A harbour behind a river mouth or inside a lagoon is reachable only when a passage entry adds its water (Šibenik is added this way). The Venice lagoon and the Great Lakes are not in the graph. The Chesapeake–Delaware Canal is reachable from the Delaware side only: about 1 nm of Back Creek on the Chesapeake side has no water area in OSM.

## Data Sources

Geographic data is derived from [OpenStreetMap](https://www.openstreetmap.org/), © OpenStreetMap contributors, available under the [Open Database License (ODbL) v1.0](https://opendatacommons.org/licenses/odbl/1-0/).

| Dataset | Size | License |
|---------|------|---------|
| [OSM land polygons](https://osmdata.openstreetmap.de/data/land-polygons.html) | ~900MB | ODbL |
| [Geofabrik regional extracts](https://download.geofabrik.de/) (canal water polygons) | varies | ODbL |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for a detailed list of changes in each release.
