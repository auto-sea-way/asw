# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/auto-sea-way/asw/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/auto-sea-way/asw/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/auto-sea-way/asw/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/auto-sea-way/asw/releases/tag/v0.1.0
