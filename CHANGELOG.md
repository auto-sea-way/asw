# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `canals` option on `/route` and on `Graph.route` in `asw-mobile` (default `true`). With `canals=false` the router closes the man-made canals (Suez, Panama, Kiel, Corinth, Lefkada, Potidea, Osor, Privlaka, Cape Cod, Cape May, Chesapeake–Delaware) and goes around them, for routes that avoid canal fees. Each canal is closed in the middle, so ports at both ends stay reachable. On the planet graph, Port Said to Suez is 101.5 nm by default and 12,171 nm with `canals=false`.
- Lefkada Canal as a passage. The channel between Lefkada and the mainland is now refined to res-13 and routable: from north of the floating bridge to Nidri is 9.2 nm through the channel, where the route went 37.1 nm around the island before. Needs a rebuilt graph.
- Four more narrow channels as passages, each needs a rebuilt graph. Evripos Strait at the Chalkis old bridge: North to South Euboean Gulf is 23.0 nm, where the route went 199.1 nm around Euboea before. Potidea Canal: 1.8 nm, was 58.7 nm around Kassandra. Osor Channel between Cres and Lošinj (about 11 m wide, refined to res-14): 2.5 nm, was 33.5 nm. Trogir Channel: 1.3 nm, was 17.6 nm around Čiovo.
- Five more passages, each needs a rebuilt graph. Privlaka Channel at Mali Lošinj (res-14): 3.3 nm, was 11.4 nm. Menai Strait: 10.8 nm, was about 50 nm around Anglesey. Swinomish Channel: 6.7 nm, was 82.8 nm. Cape May Canal: 4.5 nm, and 8.3 nm with `canals=false`. Šibenik: St. Anthony Channel, the harbour and the Krka estuary come from the Croatia OSM extract, so Šibenik is reachable for the first time.

### Changed

- The build keeps an edge only when the segment between the two cell centres does not cross the coastline. It tested only the midpoint before. This is the same test the router uses for smoothing, so a route no longer reports a land leg on a graph edge that clips a quay corner (seen at the Lefkada and Trogir bridges). On a Croatia regional build this removes 138 more edges and 32 of 186,557 nodes.
- Passages refine up to res-14 (was res-13), and nearest-node snapping searches res-14 too.
- With `canals=false`, the coarse search keeps out of the res-3 regions that hold a closed canal, so the corridor goes around and long routes keep the corridor speed. The coarse graph cannot see the small closed areas themselves. Planet: Rotterdam to Singapore around Africa (11,704 nm) takes 0.5 s, and New York to Los Angeles around South America (12,863 nm) takes 1.7 s. A long route that starts or ends in the region of a closed canal uses the full search: Colón to Callao takes about 4 s.
- A downloaded OSM extract is cached under its own file name, so passages that use the same extract share one download.

### Removed

- Welland Canal passage. The Great Lakes are not in the graph, so the entry produced no cells and only cost an Ontario extract download on every build.

## [0.9.0] - 2026-09-30

The planet graph file changes (format v5, rebuilt 2026-09-30). Download it again with the new release, or rebuild.

### Changed

- **BREAKING:** graph format v4 → v5. The file adds a coarse graph (one node per connected piece of water inside each res-3 H3 region, about 41k nodes and 2 MB on the planet). Routes longer than 500 nm first run A* on the coarse graph and then limit the fine search to a corridor around that path, falling back to the full search when the corridor has no route. Planet, same machine: Rotterdam to Singapore 4.5 s → 0.81 s, Shanghai to Rotterdam 8.5 s → 1.19 s (with the Arctic fix), North Atlantic 579 → 286 ms; routes under 500 nm are unchanged. Distances stay within 0.4 %. v4 files are rejected; the planet graph must be downloaded again or rebuilt. Spec: `docs/superpowers/specs/2026-09-30-graph-format-v5-corridor-design.md`.
- The build drops cells north of 80°N, which the router never enters (543k nodes, 12 MB). `is_water` there now returns false.
- `asw bench` has three ocean crossings around continents (Rotterdam–Singapore, Shanghai–Rotterdam, Tokyo–Seattle).

### Fixed

- Routes no longer cross the Arctic ice cap. The graph is built from land polygons only, so the Arctic Ocean counted as open water and Shanghai to Rotterdam went over the North Pole (7,124 nm). The router now never enters the sea north of 80°N, and it closes the Northern Sea Route and the Northwest Passage by default. Shanghai to Rotterdam now goes via Suez (10,380 nm).

### Added

- `arctic` option on `/route` and on `Graph.route` in `asw-mobile` (default `false`) to open the Northern Sea Route and the Northwest Passage for summer or ice-class voyages.

## [0.8.1] - 2026-09-28

The planet graph file is unchanged from 0.7.0 (format v4).

### Added

- `asw.graph.zst` release asset: the planet graph compressed with zstd level 19 (about 540 MB instead of 1,437 MB), listed in `SHA256SUMS`.
- `asw-mobile`: `installGraph(source, destination)` decompresses a downloaded `asw.graph.zst`, verifies the zstd checksum and the graph header, and atomically renames the result over the destination; on failure the old file is untouched. This is now the recommended way to install or update the graph on a phone.

## [0.8.0] - 2026-09-28

The planet graph file is unchanged from 0.7.0 (format v4); the release carries the same `asw.graph`.

### Added

- `asw-mobile`: Swift and Kotlin bindings (UniFFI) over the v4 graph file with `openGraph`, `version`, `isWater` and `route`, shipped as an iOS xcframework and an Android AAR on every release.

### Changed

- `RoutingGraph::open(path, false)` skips the monotonicity scans over the coastline and grid tables, so a phone opens the planet file with a few pages resident; the server, the build self-check and `asw geojson` open with `populate = true` and still scan.

## [0.7.0] - 2026-09-28

### Changed

- **BREAKING:** graph format v3 → v4. The file is now flat and memory-mapped: the server opens the planet in seconds instead of 60-90 s, and resident memory drops from ~4.1 GiB to the working set. Edge weights are no longer stored (recomputed from cell centres, so distances move in the second decimal); the coastline lives in the file as delta-coded microdegree runs with a 0.1° grid index instead of an R-tree built at load. Planet file: 1.44 GB. v3 files are rejected; the planet graph must be rebuilt.
- `nearest_node` moved from asw-serve into asw-core; `LandIndex` moved from asw-core into asw-build. asw-core no longer depends on `bitcode`, `zstd`, `serde`, `rstar` or `rayon`.
- `/info` reports `graph_version`, the version string stored in the graph header.
- Documentation: the shoreline leaf resolution is res-10 (it has been since the res-9 → res-10 refinement tier), not res-9.
- `asw cloud build` caches the remote compile by `git rev-parse HEAD` instead of a hash of the working tree (the upload is `git archive HEAD` anyway)
- Server readiness uses `std::sync::OnceLock` instead of an async `RwLock<Option<_>>`
- Shapefile rings convert through the `shapefile` crate's `geo-types` support; zip extraction uses `ZipArchive::extract_unwrapped_root_dir`; graph download streams with `std::io::copy` (no 50 MB progress lines)
- Cell classification, progress bars, API error responses, SSH/scp process spawning, Hetzner requests and bench result structs each share one helper instead of repeated copies
- Bench timestamps use the `time` crate instead of hand-rolled calendar math; bench timings are sorted once per route instead of on every stat call
- Hand-rolled bounding-box code replaced with geo's `BoundingRect` (load/build time only; the hot-path point-to-segment distance stays hand-rolled — geo's `hypot`-based version measured +9-30% p50 on short-route benches)

### Added

- `asw_core::routing::is_water(graph, lat, lon)`: water test by coastline crossing parity from the nearest water node, correct inside marina basins narrower than a leaf cell.

### Removed

- Dead code found by a repo-wide audit (~850 lines, no behavior change — verified against `main` with a planet-graph benchmark: identical distances and hop counts on all 20 routes):
  - `asw_core::routing::smooth` (superseded by `smooth_indices`), `RoutingGraph::connected_components`, `AstarPool::capacity`, the `graph_compare` example, and thin `h3` wrappers (`parent`, `children`, `resolution`, `lat_lng_to_cell`)
  - `Passage.zone_resolution` — all passages use resolution 5; replaced by the `ZONE_RESOLUTION` const
  - `load_raw_polygons` (no callers) and the load-time bbox filter on `load_land_polygons` (documented footgun, only ever called with `None`)
  - asw-cloud: SSH-key name-conflict retry machinery (key names now include a hash of the key material, so the conflict cannot occur), the string-keyed step-dispatch table (replaced by a linear step sequence), the `~/.ssh` pubkey fallback scan (the `.pub` must sit next to the private key), `SshConfig.user` (always root), and assorted dead constants
  - Unused dependencies: `sha2` and `serde_json` (asw-cloud), `geojson` (asw-core), `geojson` and `anyhow` (asw-serve), `geo-types` (asw-core, asw-build, and the workspace — no crate imports it directly; the types come via `geo`); `tokio` trimmed from `full` to the used features
  - `asw-serve` stub binary (the crate is a library; the binary is `asw`)

- Second audit pass (~930 lines, 1 dependency, 11k lines of historical plan/spec docs):
  - Serve-time connected-component labels (the build already prunes the graph to one component): saves ~160 MB RSS and a union-find pass on planet load
  - `RangeMin` sparse table in route smoothing (a slice min is cheaper than the R-tree query next to it), `ShorePenalty` struct (now `shore_buffer_q` + `shore_factor`), `AstarPool` capacity cap (the `/route` semaphore already bounds it), unused lat/lng on `GraphBuilder::add_node`, `Passage.water_types` (one hardcoded list; Panama now also keeps `water=canal`), `tier_name`, per-passage SSH key display names, the `asw --version` integration test
  - `asw-cli/src/srcdir.rs`: `asw cloud build --src` now simply defaults to the current directory
  - `docs/superpowers/` and `docs/reviews/` (history keeps them)
  - Dependency `ordered-float`: A* orders its heap on `f32::to_bits`, valid because f-scores are never negative

## [0.6.1] - 2026-07-08

### Added

- `land_legs` on `/route` responses: segment indices of the returned geometry that cross land (pin-on-land stitch legs and coastline-clipping smoothed segments), so clients can style them differently
- Bench GeoJSON draws only the water spans of each route — land legs appear as gaps (GitHub's geojson preview ignores styling, so color could not carry the signal); land-leg indices are kept as feature properties

### Changed

- `distance_nm` counts only water segments — overland stitch legs are excluded (previously inflated distances for pins placed on land, e.g. Kiel transit benchmark)

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
- Planet graph rebuilt in format v3: 39,412,823 nodes / 299,517,836 edges, 717 MB (+15 MB for per-node shore distances; topology identical to v0.5.0)

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

[0.9.0]: https://github.com/auto-sea-way/asw/compare/v0.8.1...v0.9.0
[0.8.1]: https://github.com/auto-sea-way/asw/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/auto-sea-way/asw/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/auto-sea-way/asw/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/auto-sea-way/asw/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/auto-sea-way/asw/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/auto-sea-way/asw/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/auto-sea-way/asw/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/auto-sea-way/asw/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/auto-sea-way/asw/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/auto-sea-way/asw/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/auto-sea-way/asw/releases/tag/v0.1.0
