# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- API key authentication for `/route` and `/info` endpoints via `X-Api-Key` header
- `--api-key` CLI argument with `ASW_API_KEY` environment variable fallback
- Constant-time key comparison (subtle crate) to prevent timing attacks

### Changed

- `/health` and `/ready` endpoints remain public (no auth required)
- Server refuses to start without a valid API key

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

[Unreleased]: https://github.com/auto-sea-way/asw/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/auto-sea-way/asw/releases/tag/v0.1.0
