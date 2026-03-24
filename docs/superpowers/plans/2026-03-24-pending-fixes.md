# Pending Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix build pipeline bugs, coastline subtraction, graph pruning, and serve cleanup — all in one branch for a single graph rebuild.

**Architecture:** Four independent fix groups (A-D) touching 5 files. Groups A-C modify the build pipeline and require a graph rebuild. Group D is serve-only cleanup. All changes are backward-compatible with existing graph files.

**Tech Stack:** Rust, geo crate, rstar R-tree, h3o, GraphBuilder.

**Spec:** `docs/superpowers/specs/2026-03-24-pending-fixes-design.md`

---

## File Structure

- **Modify:** `crates/asw-build/src/canal_water.rs` — defer osmium, safe coords, tmp cleanup (Group A)
- **Modify:** `crates/asw-core/src/geo_index.rs` — add `LandIndex::polygons()` (Group B)
- **Modify:** `crates/asw-build/src/pipeline.rs` — post-subtraction coastline + prune components (Groups B, C)
- **Modify:** `crates/asw-serve/src/state.rs` — nearest_node cleanup (Group D)
- **Modify:** `CLAUDE.md` — add osmium-tool prerequisite (Group A)

---

### Task 1: Defer osmium check (A1)

**Files:**
- Modify: `crates/asw-build/src/canal_water.rs:17-28`

- [ ] **Step 1: Remove the upfront osmium check and add lazy check**

Replace lines 22-28 (the unconditional osmium check block) with a `let mut osmium_checked = false;` before the loop. Then add a lazy check inside the loop, after the bbox/URL filtering but before calling `extract_single_passage`:

```rust
pub fn extract_canal_water(
    passages: &[Passage],
    build_bbox: Option<Bbox>,
    work_dir: &Path,
) -> Result<Vec<Polygon<f64>>> {
    let canal_dir = work_dir.join("canal-water");
    std::fs::create_dir_all(&canal_dir)?;

    let mut osmium_checked = false;
    let mut all_water = Vec::new();

    for passage in passages {
        let url = match passage.geofabrik_url {
            Some(url) => url,
            None => continue,
        };

        if passage.water_types.is_empty() {
            continue;
        }

        // Skip if passage corridor doesn't overlap build bbox
        if let Some(bb) = build_bbox {
            let (p_min_lon, p_min_lat, p_max_lon, p_max_lat) = passage.corridor;
            let (b_min_lon, b_min_lat, b_max_lon, b_max_lat) = bb;
            if p_max_lon < b_min_lon
                || p_min_lon > b_max_lon
                || p_max_lat < b_min_lat
                || p_min_lat > b_max_lat
            {
                info!("Skipping canal '{}' — outside build bbox", passage.name);
                continue;
            }
        }

        // Lazy osmium check — only on first passage that needs PBF processing
        if !osmium_checked {
            let osmium_check = Command::new("osmium").arg("--version").output();
            if osmium_check.is_err() || !osmium_check.unwrap().status.success() {
                anyhow::bail!(
                    "osmium-tool is required for canal water extraction. \
                     Install: apt install osmium-tool (Linux) or brew install osmium-tool (macOS)"
                );
            }
            osmium_checked = true;
        }

        info!("Processing canal water for '{}'...", passage.name);
```

- [ ] **Step 2: Run tests**

Run: `cargo test --workspace`
Expected: All pass (no tests directly exercise this code path without osmium).

- [ ] **Step 3: Commit**

```bash
git add crates/asw-build/src/canal_water.rs
git commit -m "fix: defer osmium check until a canal passage is actually needed"
```

---

### Task 2: Safe coordinate parsing (A2)

**Files:**
- Modify: `crates/asw-build/src/canal_water.rs:186-201`

- [ ] **Step 1: Replace `coords_to_polygon` with safe indexing**

Replace the entire `coords_to_polygon` function:

```rust
fn coords_to_polygon(coords: &[Vec<Vec<f64>>]) -> Option<Polygon<f64>> {
    if coords.is_empty() {
        return None;
    }
    let exterior = LineString::new(
        coords[0]
            .iter()
            .filter_map(|c| {
                let x = *c.get(0)?;
                let y = *c.get(1)?;
                Some(Coord { x, y })
            })
            .collect(),
    );
    if exterior.0.len() < 3 {
        return None;
    }
    let holes: Vec<LineString<f64>> = coords[1..]
        .iter()
        .map(|ring| {
            LineString::new(
                ring.iter()
                    .filter_map(|c| {
                        let x = *c.get(0)?;
                        let y = *c.get(1)?;
                        Some(Coord { x, y })
                    })
                    .collect(),
            )
        })
        .filter(|ls| ls.0.len() >= 3)
        .collect();
    Some(Polygon::new(exterior, holes))
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/asw-build/src/canal_water.rs
git commit -m "fix: safe coordinate parsing in coords_to_polygon — skip malformed GeoJSON"
```

---

### Task 3: Tmp file cleanup on download failure (A3)

**Files:**
- Modify: `crates/asw-build/src/canal_water.rs:91-94`

- [ ] **Step 1: Add cleanup-on-error around the download**

Replace lines 91-94 in `extract_single_passage`:

```rust
        let tmp_path = pbf_path.with_extension("pbf.tmp");
        let mut file = std::fs::File::create(&tmp_path)?;
        let result = std::io::copy(&mut resp, &mut file);
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        let bytes = result?;
        std::fs::rename(&tmp_path, &pbf_path)?;
```

- [ ] **Step 2: Run tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/asw-build/src/canal_water.rs
git commit -m "fix: clean up partial .pbf.tmp file on download failure"
```

---

### Task 4: CLAUDE.md osmium prerequisite (A4)

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add osmium-tool to Build & Run section**

Find the line `# Local build (requires land_polygons.shp or directory of split shapefiles)` and update it to:

```
# Local build (requires land_polygons.shp or directory of split shapefiles)
# Canal water extraction also requires osmium-tool: apt install osmium-tool (Linux) or brew install osmium-tool (macOS)
```

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add osmium-tool prerequisite to CLAUDE.md Build & Run"
```

---

### Task 5: LandIndex::polygons() method (Group B, part 1)

**Files:**
- Modify: `crates/asw-core/src/geo_index.rs`

- [ ] **Step 1: Add `polygons()` method to `LandIndex`**

Add after the existing `polygon_count()` method (around line 100):

```rust
    /// Extract all land polygons from the R-tree.
    /// Used to get post-subtraction polygons for coastline extraction.
    pub fn polygons(&self) -> Vec<Polygon<f64>> {
        self.tree.iter().map(|lp| lp.polygon.clone()).collect()
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/asw-core/src/geo_index.rs
git commit -m "feat: add LandIndex::polygons() for post-subtraction coastline extraction"
```

---

### Task 6: Use post-subtraction coastline (Group B, part 2)

**Files:**
- Modify: `crates/asw-build/src/pipeline.rs:33-38`

- [ ] **Step 1: Replace raw polygon reload with land.polygons()**

Replace lines 33-38 in `pipeline.rs`:

```rust
    // Step 2: Extract coastline from post-subtraction land (includes canal waterway boundaries)
    info!("Extracting coastline segments...");
    let land_polygons = land.polygons();
    let (coastline_segments, mut coastline_coords) =
        crate::coastline::extract_coastline(&land_polygons);
    let coastline_index = CoastlineIndex::new(coastline_segments);
    info!("Coastline: {} segments", coastline_index.segment_count());
```

Also remove the `use crate::shapefile::Bbox;` import if `Bbox` is no longer used in this file (check — it may still be used by the bbox parameter). Actually `Bbox` is used on line 12 for the function signature, so keep it. But remove the now-unused `load_raw_polygons` call.

- [ ] **Step 2: Run tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/asw-build/src/pipeline.rs
git commit -m "fix: extract coastline from post-subtraction land — fixes canal over-smoothing"
```

---

### Task 7: Prune small components (Group C)

**Files:**
- Modify: `crates/asw-build/src/pipeline.rs` (after the graph build section)

- [ ] **Step 1: Add component pruning after graph build**

After the current connectivity check (lines 97-106), replace the existing graph variable with a pruned version. Insert before the serialize step:

```rust
    // Prune: keep only the largest connected component
    let graph = {
        let labels = graph.component_labels();
        let mut comp_sizes: std::collections::HashMap<u32, usize> =
            std::collections::HashMap::new();
        for &root in &labels {
            *comp_sizes.entry(root).or_insert(0) += 1;
        }
        let main_root = comp_sizes
            .iter()
            .max_by_key(|&(_, &mut size)| size)
            .map(|(&root, _)| root)
            .unwrap_or(0);

        let main_count = comp_sizes.get(&main_root).copied().unwrap_or(0);
        let pruned_count = graph.num_nodes as usize - main_count;

        if pruned_count > 0 {
            info!(
                "Pruning {} nodes in {} small components (keeping {} in main component)",
                pruned_count,
                comp_sizes.len() - 1,
                main_count,
            );

            // Build old→new ID mapping (only main-component nodes)
            let mut old_to_new: Vec<Option<u32>> = vec![None; graph.num_nodes as usize];
            let mut new_builder = GraphBuilder::new();
            for old_id in 0..graph.num_nodes {
                if labels[old_id as usize] == main_root {
                    let h3 = graph.node_h3[old_id as usize];
                    let (lat, lon) = graph.node_pos(old_id);
                    let new_id = new_builder.add_node(h3, lat, lon);
                    old_to_new[old_id as usize] = Some(new_id);
                }
            }

            // Re-add edges between main-component nodes
            for old_src in 0..graph.num_nodes {
                if labels[old_src as usize] != main_root {
                    continue;
                }
                let new_src = old_to_new[old_src as usize].unwrap();
                for (old_dst, weight) in graph.neighbors(old_src) {
                    // Only add each directed edge once (neighbors returns directed edges)
                    if let Some(new_dst) = old_to_new[old_dst as usize] {
                        new_builder.add_directed_edge(new_src, new_dst, weight);
                    }
                }
            }

            new_builder.coastline_coords = graph.coastline_coords;
            let pruned = new_builder.build();
            info!(
                "Pruned graph: {} nodes, {} edges",
                pruned.num_nodes, pruned.num_edges
            );
            pruned
        } else {
            graph
        }
    };
```

Note: use `add_directed_edge` (not `add_edge`) since `neighbors()` already returns each direction separately — using `add_edge` would double them.

- [ ] **Step 2: Remove the old connectivity check block**

The old connectivity check (lines 97-106) is now redundant since pruning replaces it. Remove:

```rust
    // Connectivity check
    let components = graph.connected_components();
    if let Some(&largest) = components.first() {
        let pct = largest as f64 / graph.num_nodes as f64 * 100.0;
        info!(
            "Largest connected component: {} nodes ({:.1}%)",
            largest, pct
        );
        info!("{} total components", components.len());
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add crates/asw-build/src/pipeline.rs
git commit -m "feat: prune small graph components — keep only largest connected component"
```

---

### Task 8: nearest_node cleanup (Group D)

**Files:**
- Modify: `crates/asw-serve/src/state.rs`

- [ ] **Step 1: Fix `search_resolution` — return `()`, fix `found_at_k` semantics**

Change `search_resolution`:

1. Change return type from `-> bool` to `-> ()`
2. Change `return true` to `return` and `false` at end to nothing (implicit `()`)
3. Change `Err(_) => return false` to `Err(_) => return`
4. Set `found_at_k = true` when ANY main-component node is found, not just when it improves best:

```rust
    /// Search a single resolution with k-ring up to `k_max`, updating `best`.
    fn search_resolution(
        &self,
        ll: &h3o::LatLng,
        lat: f64,
        lon: f64,
        res_u8: u8,
        k_max: u32,
        best: &mut Option<(u32, f64)>,
    ) {
        let res = match h3o::Resolution::try_from(res_u8) {
            Ok(r) => r,
            Err(_) => return,
        };
        let cell = ll.to_cell(res);
        for k in 0..=k_max {
            let mut found_at_k = false;
            for neighbor in cell.grid_disk::<Vec<_>>(k) {
                let nh3 = u64::from(neighbor);
                if let Some(node_id) = self.h3_lookup(nh3) {
                    if self.component_labels[node_id as usize] == self.main_component {
                        found_at_k = true;
                        let (nlat, nlon) = self.graph.node_pos(node_id);
                        let dist = asw_core::h3::haversine_nm(lat, lon, nlat, nlon);
                        if best.is_none_or(|(_, d)| dist < d) {
                            *best = Some((node_id, dist));
                        }
                    }
                }
            }
            if found_at_k {
                return;
            }
        }
    }
```

- [ ] **Step 2: Fix doc comments on `nearest_node` and `H3_EDGE_NM`**

Update the `nearest_node` doc comment (line 159):

```rust
    /// Find nearest node in the main connected component via two-pass adaptive k-ring expansion.
```

Update the `H3_EDGE_NM` doc comment (lines 93-95):

```rust
    /// Approximate H3 edge length in nautical miles, indexed by resolution (3..=13).
    /// Used for early-termination: skip this resolution if current best is already
    /// closer than the cell edge length.
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p asw-serve`
Expected: All 22 pass.

- [ ] **Step 4: Commit**

```bash
git add crates/asw-serve/src/state.rs
git commit -m "refactor: nearest_node cleanup — fix return type, doc comments, found_at_k semantics"
```

---

### Task 9: Kiel Canal res-13 (Group E)

**Files:**
- Modify: `crates/asw-core/src/passages.rs:28,52`

- [ ] **Step 1: Bump Kiel Canal leaf_resolution to 13**

In `crates/asw-core/src/passages.rs`, change the Kiel Canal passage (line 52):

```rust
        leaf_resolution: 13, // bumped from 11 — lock entrances need 3.5m edges
```

Also update the comment on line 28:

```rust
/// - ~15m locks (Kiel): res-13 (3.5m edge)
```

- [ ] **Step 2: Run tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/asw-core/src/passages.rs
git commit -m "fix: bump Kiel Canal to res-13 for lock entrance/exit connectivity"
```

---

### Task 10: Final verification

**Files:** None (verification only)

- [ ] **Step 1: Run full test suite**

```bash
cargo test --workspace
```

Expected: All pass.

- [ ] **Step 2: Run clippy**

```bash
cargo clippy --workspace -- -D warnings
```

Expected: No warnings.

- [ ] **Step 3: Run cargo fmt**

```bash
cargo fmt --all -- --check
```

Expected: No formatting issues. If any, run `cargo fmt --all` and commit.
