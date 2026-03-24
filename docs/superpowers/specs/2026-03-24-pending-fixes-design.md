# Pending Fixes — Design Spec

**Date:** 2026-03-24
**Status:** Approved
**Scope:** Build pipeline fixes (A, B, C) + serve cleanup (D). Requires graph rebuild.

## Group A: v0.3.0 Build Bugfixes

### A1. Defer osmium check

**File:** `crates/asw-build/src/canal_water.rs:22-28`

The osmium availability check runs unconditionally before filtering passages by bbox. A `--bbox marmaris` build fails if osmium isn't installed, even though no PBF processing is needed.

**Fix:** Remove the upfront osmium check. Instead, check once lazily — set a flag on the first passage that needs PBF processing, run the check at that point.

### A2. Safe coordinate parsing

**File:** `crates/asw-build/src/canal_water.rs:186-201` (`coords_to_polygon`)

`c[0]` / `c[1]` index access panics if a GeoJSON coordinate array has fewer than 2 elements. Third-party OSM data can have malformed entries.

**Fix:** Replace `c[0]` / `c[1]` with `c.get(0)` / `c.get(1)` and skip malformed coordinates with `.filter_map()`.

### A3. Tmp file cleanup on download failure

**File:** `crates/asw-build/src/canal_water.rs:91-94` (download section)

If `std::io::copy` fails mid-download, the `.pbf.tmp` file is left on disk.

**Fix:** Wrap the download in a pattern that removes the tmp file on error:
```rust
let result = std::io::copy(&mut resp, &mut file);
if result.is_err() {
    let _ = std::fs::remove_file(&tmp_path);
}
let bytes = result?;
```

### A4. CLAUDE.md prerequisite

**File:** `CLAUDE.md`

Add `osmium-tool` to prerequisites. Local builds that process canal water need it installed.

## Group B: Coastline Subtraction Fix

**File:** `crates/asw-build/src/pipeline.rs:33-38`

Coastline is extracted from raw polygons (before `subtract_water`), so canal waterway boundaries are missing from the coastline index. The smoother's `crosses_land()` fails in canal areas.

**Fix:**
1. Add `pub fn polygons(&self) -> Vec<Polygon<f64>>` to `LandIndex` that extracts all polygons from the R-tree.
2. In `pipeline.rs`, replace `load_raw_polygons` + `extract_coastline(&raw_polygons)` with `extract_coastline(&land.polygons())` — using the post-subtraction land.
3. This removes the redundant second load of shapefiles (currently ~860K polygons loaded twice).

**Files modified:**
- `crates/asw-core/src/geo_index.rs` — add `polygons()` method to `LandIndex`
- `crates/asw-build/src/pipeline.rs` — use `land.polygons()` for coastline extraction

## Group C: Prune Small Components

**File:** `crates/asw-build/src/pipeline.rs` (after graph build)

88,746 disconnected components with 1.55M orphan nodes (3.8%) cause A* dead-ends and waste graph size.

**Fix:** After building the graph (line 91), compute component labels, identify the largest component, then rebuild with only main-component nodes/edges:

1. Build graph as normal
2. `graph.component_labels()` → identify main component root
3. Collect main-component node indices as a `HashSet`
4. Build a NEW graph with only nodes in the main component and edges between them
5. Log pruned count

This approach reuses existing `component_labels()` and `GraphBuilder`. The graph build is cheap (~seconds); the expensive work (cell generation, edge building) is already done.

## Group D: nearest_node Cleanup

**File:** `crates/asw-serve/src/state.rs`

4 minor items from PR #12 code review:

1. **`search_resolution` return type** — change `-> bool` to `-> ()`, remove `return true/false`
2. **`nearest_node` doc comment** — replace "H3 binary search" with "two-pass adaptive k-ring expansion"
3. **`H3_EDGE_NM` doc comment** — clarify: "skip this resolution if current best is already closer than the cell edge length"
4. **`found_at_k` semantics** — set `found_at_k = true` when ANY main-component node is found at this k (not just when it improves best). This stops unnecessary k expansion when pass 2 encounters non-improving nodes.

## Files Modified Summary

| File | Group | Change |
|------|-------|--------|
| `crates/asw-build/src/canal_water.rs` | A | Defer osmium, safe coords, tmp cleanup |
| `crates/asw-core/src/geo_index.rs` | B | Add `LandIndex::polygons()` |
| `crates/asw-build/src/pipeline.rs` | B, C | Post-subtraction coastline, prune components |
| `crates/asw-serve/src/state.rs` | D | Return type, doc comments, found_at_k |
| `CLAUDE.md` | A | Add osmium-tool prerequisite |
