# Refactor: Kilometers to Nautical Miles

**Date:** 2026-03-13
**Status:** Approved

## Summary

Replace all kilometer-based distance calculations, storage, and display with nautical miles throughout the codebase. Nautical miles are the standard unit in maritime navigation and better fit the project's domain.

## Approach

Single-source change: rename `haversine_km` to `haversine_nm` and change the Earth radius constant from 6371.0 km to 3440.065 nm. All downstream code carries the unit through mechanically — no conversion factors needed anywhere.

## Changes

### Core: `asw-core/src/h3.rs`

- Rename `haversine_km` → `haversine_nm`
- Change Earth radius: `6371.0` → `3440.065`
- Update test expected values:
  - London–Paris: ~344 km → ~186 nm
  - Antipodal: ~20015 km → ~10808 nm
- Update doc comment and assertion messages

### Core: `asw-core/src/graph.rs`

- Comment "Edge cost in km" → "Edge cost in nm"
- Comment "Edges as (source, target, weight_km)" → "weight_nm"
- Comment "iterator of (target_id, weight_km)" → "weight_nm"
- Rename parameter `weight_km` → `weight_nm` in `add_edge` and `add_directed_edge`
- Update `haversine_km` call in `nearest_node` method → `haversine_nm`

### Core: `asw-core/src/routing.rs`

- Import `haversine_nm` instead of `haversine_km`
- Rename field `distance_km` → `distance_nm` in `RouteResult`
- Update doc comment "Total distance in km" → "Total distance in nm"
- Rename local variable `_distance_km` → `_distance_nm`

### Core: `asw-core/src/lib.rs`

- Update resolution table comments from km to nm for res-3 through res-7 (sub-km entries at res-8+ already use meters, no change needed):
  - res-3: edge ~59km → ~32nm, threshold ~33km → ~18nm
  - res-4: edge ~22km → ~12nm, threshold ~17km → ~9.2nm
  - res-5: edge ~8.4km → ~4.5nm, threshold ~5.5km → ~3.0nm
  - res-6: edge ~3.2km → ~1.7nm, threshold ~2.8km → ~1.5nm
  - res-7: edge ~1.2km → ~0.65nm, threshold ~1.3km → ~0.70nm

### Build: `asw-build/src/edges.rs`

- Import `haversine_nm` instead of `haversine_km`
- Update comment "An edge: (source_node_id, target_node_id, cost_km)" → "cost_nm"

### Serve: `asw-serve/src/api.rs`

- Rename `RouteResponse.distance_km` → `distance_nm` (breaking API change, acceptable — no external consumers)

### Serve: `asw-serve/src/state.rs`

- Call `haversine_nm` instead of `haversine_km`

### CLI: `asw-cli/src/bench.rs`

- Import `haversine_nm` instead of `haversine_km`
- Rename `distance_km` fields → `distance_nm` in benchmark structs
- Update format strings from `km` → `nm`
- Update comments referencing km ranges

### CLI: `asw-cli/src/main.rs`

- Rename `weight_km` → `weight_nm` in GeoJSON passage feature output

### Benchmarks: `benchmarks/BENCHMARKS.md` and `benchmarks/bench-routes.geojson`

- Both will need regeneration after next benchmark run (values will naturally be in nm, `distance_km` keys become `distance_nm`)

### Docs: `README.md`

- Update route distance reference (e.g., "8924 km" → "~4819 nm")

## Not Changed

- **Graph binary format**: Field names don't appear in bincode serialization; values will be nm after rebuild
- **H3 grid logic**: Uses degrees/coordinates, not distance units
- **GeoJSON coordinates**: lat/lon, no distance units
- **Passage definitions**: Specified by coordinates, not distances

## Testing

- All existing unit tests updated with nm-based expected values
- `cargo test` must pass
- `cargo build --release` must succeed
