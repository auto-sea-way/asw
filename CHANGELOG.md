# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.0] - 2026-07-07

### Added

- `shore_buffer` query parameter on `/route` (nautical miles, 0–5.0): keeps routes a configurable clearance from the coastline via a graded A* cost penalty and buffer-aware path smoothing (#26 — thanks to @Damiasroca for highlighting a real safety gap in near-shore routing)
- Per-node distance-to-shore stored in the graph (1 byte/node, 0.02 nm quantization, saturating at 5.1 nm)
- `--shore-buffer` flag on `asw bench`

### Fixed

- Deep-water routes: geometry now starts and ends exactly at the requested coordinates instead of at snapped node centers (on res-3 ocean cells the nearest node can be tens of nm away, leaving the polyline visibly detached from the route markers); two points inside the same cell no longer return a single-point 0.00 nm route

### Changed

- **BREAKING:** graph format v2 → v3 (adds `shore_dist`) — existing graph files must be rebuilt
- Direct-line shortcut: when the straight line between the requested points does not cross land — and keeps the requested `shore_buffer` clearance, degraded to the endpoints' own shore distance when they start closer — `/route` returns a 2-point great-circle route without a graph search (faster for open-water queries)
- A pin on land (or blocked from its snapped node) still returns a route: the first/last segment keeps the direct connection to the graph (small shoreline clip) instead of erroring
- `asw_core::routing::smooth` is now a thin wrapper over the new coordinate-based `smooth_indices` (same algorithm, same buffer semantics)
- Crate versions now track the release version — `asw --version` reports the actual release (was stuck at 0.1.0)

## [0.5.0] - 2026-07-07

### Fixed

- Edge weight quantization: clamp to >= 1 centi-nm so res-13 passage-corridor edges (Panama, Kiel, Corinth, Welland) are no longer free for A*; hard error (was debug-only assert) on u16 weight overflow. Requires a graph rebuild to take effect
- Antimeridian handling: `crosses_land` splits seam-crossing segments instead of testing a near-global planar chord; edge midpoints wrap longitudes before averaging; `cell_polygon` unwraps transmeridian H3 cells instead of producing degenerate world-spanning rings (fixes false land classification around the date line — Bering Strait, Fiji, Chukchi Sea)
- Cloud build step cache keyed by bbox: changing the bbox no longer silently reuses a stale remote graph or local download; scp downloads are atomic (`.tmp` + rename)
- Remote compile cache probe now works: `asw --version` exists (clap `version` attribute added)
- Hetzner SSH key creation: uniqueness-conflict recovery reads the error body and retries with a uniquified name instead of silently binding an arbitrary existing key; non-ASCII key comments no longer panic
- `asw cloud build` resolves the source directory at runtime (CWD, `--src` flag) instead of embedding the compile-time workspace path; warns when the working tree is dirty
- Shapefile download: HTTP status checked; extraction is atomic (temp dir + rename), so a failed download no longer poisons the cache
- Passage zone split probes every distinct `zone_resolution` instead of assuming all passages share the first one

### Changed

- A* buffer pool: O(1) generation-counter reset instead of a full-graph memset (~358 MB of writes per request at planet scale, previously hidden from benchmarks); per-node heuristic cached per query. Measured on Linux (Docker, planet graph): ~4.1 GiB RSS after load, 4.3 GiB after a globally diverse route mix, ~4.8 GiB hard ceiling as lazily-touched buffer pages accumulate (the `gen`/`h_score`/`closed` arrays start on untouched zero pages). Short-route p50 improves 1.1-2x and served-request latency no longer pays a hidden 10-35 ms reset
- `/route` computation runs on `tokio::task::spawn_blocking` (long routes no longer stall health probes); `ServerState` holds `Arc<AppState>`
- `nearest_node` exhaustive fallback uses geometrically-doubled eager disk scans (worst case ~1.56x one full-disk call, typical early exit far cheaper)
- `min_distance_deg` iterates coastline pairs without per-segment allocation
- Planet graph rebuilt (39,412,823 nodes / 299,517,836 edges, 702 MB): canal corridor edges carry real weights, and ~433K spurious fine-resolution nodes along the antimeridian are gone (previously over-refined by degenerate transmeridian cell polygons)

## [0.4.0] - 2026-03-28

### Added

- Build-time component pruning: keep only the largest connected component, removing ~1.65M disconnected nodes in ~91.5K small components
- `LandIndex::polygons()` method for accessing post-subtraction land polygons

### Fixed

- Kiel Canal routing: bumped from res-11 to res-13 for lock entrance/exit connectivity (was routing around Denmark at 409 nm, now 84 nm through canal)
- Coastline extraction now uses post-subtraction land polygons — canal waterway boundaries included in coastline index, fixing route over-smoothing near canals
- Safe coordinate parsing in `coords_to_polygon` — skip malformed GeoJSON instead of panicking on short coordinate arrays
- Deferred osmium availability check — builds without osmium-tool no longer fail when no canals are in the build region
- Partial `.pbf.tmp` cleanup on download failure
- `nearest_node` `found_at_k` semantics: stop k-ring expansion when any main-component node is found, not only when improving best distance

### Changed

- Planet graph: 39.8M nodes / 302M edges (was 41.3M / 310M — pruned nodes were disconnected fragments)
- `search_resolution` returns `()` instead of unused `bool`
- Updated doc comments for `nearest_node` (two-pass adaptive k-ring, not "H3 binary search") and `H3_EDGE_NM`

### Documentation

- Added osmium-tool prerequisite to CLAUDE.md build instructions

## [0.3.1] - 2026-03-24

### Fixed

- Nearest-node snapping regression: routes to remote islands (Grenada, Palagruza) and coastal towns (Gallipoli, Monopoli) now resolve correctly
- Coastal snap quality restored to v0.2.0 level — ports snap to nearby fine-resolution nodes instead of distant coarse ones
- Adaptive two-pass snapping: fast k=3 scan handles 99% of queries, proportional refinement only when needed

### Performance

- Short/medium routes 2-18x faster than v0.2.0 (H3 binary search + pre-allocated A* buffers)
- Panama Canal: 47x faster (51 nm through canal vs 10,340 nm around continent in v0.2.0)

## [0.3.0] - 2026-03-24

### Added

- Canal water subtraction: download Geofabrik PBFs at build time, extract inland water polygons via osmium, subtract from land index
- Panama Canal routable (49.7 nm through canal, previously 10,337 nm around South America)
- Kiel Canal, Houston Ship Channel, Cape Cod Canal, Chesapeake-Delaware Canal, Welland Canal passage definitions
- `geofabrik_url` and `water_types` fields on `Passage` struct for automated canal water extraction
- ODbL attribution for OSM-derived geographic data

### Changed

- Graph format v2: bitcode + zstd-19 serialization (replaces bincode)
- Sorted `node_h3: Vec<u64>` for O(log n) spatial lookup (replaces R-tree for nearest-node)
- Pre-allocated A* buffer pool (2 buffer sets) eliminates per-request allocation spikes
- Panama Canal passage bumped from res-11 to res-13 (lock channels need 3.5m cell edges)
- `osmium-tool` added to cloud build bootstrap packages

### Performance

- 47% server memory reduction: ~3.5 GiB RSS (was ~6.4 GiB) via H3 binary search replacing R-tree
- `subtract_water` uses per-polygon water R-tree spatial lookup + rayon parallelization (<1s for 860K land polygons)
- Panama Canal routing: 6.91s → 72.4ms (95x faster — no longer searching around the continent)

## [0.2.0] - 2026-03-16

### Added

- API key authentication for `/route` and `/info` endpoints via `X-Api-Key` header
- `--api-key` CLI argument with `ASW_API_KEY` environment variable fallback
- Constant-time key comparison (subtle crate) to prevent timing attacks

### Changed

- `/health` and `/ready` endpoints remain public (no auth required)
- Server refuses to start without a valid API key
- Linux binaries now statically linked with musl (fixes GLIBC version mismatch with distroless base image)
- Docker base image switched from `distroless/cc-debian12` to `distroless/static-debian12`
- Release binaries stripped for smaller file size

## [0.1.0] - 2026-03-16

### Added

- Maritime auto-routing using H3 hexagonal grid (adaptive cascade: res-3 ocean through res-9 shoreline)
- Compact binary graph format with varint-encoded edges and i32 coordinates
- HTTP API server (axum) with `/route`, `/health`, `/ready`, `/info` endpoints
- A* routing with Haversine heuristic and Chaikin curve smoothing
- Critical narrow passage edges (Suez, Bosphorus, Dover, Malacca, etc.)
- Cloud build pipeline (Hetzner provisioning + SSH/SCP)
- GeoJSON export for visualization
- Docker images (slim + full with graph included) on ghcr.io
- Cross-platform binary releases (Linux x86_64/ARM64, macOS x86_64/ARM64)
- CI/CD with GitHub Actions (CI checks, Docker push, binary releases)
- Readiness probe — server accepts connections immediately, returns 503 until graph loaded

### Performance

- 41% peak memory reduction during server init (6.4 GB → 3.8 GB)
- Pre-built statically-linked musl binaries in Docker images

[0.6.0]: https://github.com/auto-sea-way/asw/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/auto-sea-way/asw/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/auto-sea-way/asw/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/auto-sea-way/asw/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/auto-sea-way/asw/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/auto-sea-way/asw/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/auto-sea-way/asw/releases/tag/v0.1.0
