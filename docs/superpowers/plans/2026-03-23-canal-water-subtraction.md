# Canal Water Subtraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Download Geofabrik regional PBFs at build time, extract inland water polygons, and subtract them from the land index so canals like Panama and Kiel get proper hexagonal coverage.

**Architecture:** Each `Passage` optionally specifies a Geofabrik PBF URL. During the build, before cell generation, the pipeline downloads each PBF, shells out to `osmium` to extract `natural=water` polygons, clips to the passage corridor, filters to navigable types, and subtracts the resulting water polygons from the `LandIndex`. The existing cascade and zone refinement logic is unchanged.

**Tech Stack:** Rust (existing workspace), `osmium-tool` (system dependency for PBF filtering/export), `geojson` crate (parsing extracted GeoJSON), `geo` crate (polygon difference operations).

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `crates/asw-core/src/passages.rs` | Add `geofabrik_url` and `water_filter` fields to `Passage`; update existing passages |
| Create | `crates/asw-build/src/canal_water.rs` | Download PBF, run osmium, parse GeoJSON, return water polygons per passage |
| Modify | `crates/asw-build/src/lib.rs` | Export new `canal_water` module |
| Modify | `crates/asw-core/src/geo_index.rs` | Add `subtract_water()` method to `LandIndex` |
| Modify | `crates/asw-build/src/pipeline.rs` | Insert canal water step between land load and cell generation |
| Modify | `crates/asw-cloud/src/config.rs` | Add `osmium-tool` to `BOOTSTRAP_PACKAGES` |
| ~~Skip~~ | ~~`crates/asw-cloud/src/pipeline.rs`~~ | Not needed — canal extraction runs inside `asw build` on the remote server |
| Modify | `crates/asw-build/Cargo.toml` | Add `geojson` dependency |

---

### Task 1: Add Geofabrik fields to Passage struct

**Files:**
- Modify: `crates/asw-core/src/passages.rs`

- [ ] **Step 1: Add fields to Passage struct**

```rust
pub struct Passage {
    pub name: &'static str,
    pub corridor: (f64, f64, f64, f64),
    pub zone_resolution: u8,
    pub leaf_resolution: u8,
    /// Geofabrik PBF URL for inland canal water extraction.
    /// None for natural straits where coastline already provides water gaps.
    pub geofabrik_url: Option<&'static str>,
    /// OSM water= tag values to keep (e.g., "lock", "reservoir", "lake", "river").
    /// Empty means skip water extraction even if geofabrik_url is set.
    pub water_types: &'static [&'static str],
}
```

- [ ] **Step 2: Update PASSAGES array with Geofabrik URLs and water types**

Update each passage entry. Passages that are natural straits (Bosphorus, Dardanelles, etc.) get `geofabrik_url: None`. Inland canals get their regional PBF URL.

```rust
pub static PASSAGES: &[Passage] = &[
    Passage {
        name: "Suez Canal",
        corridor: (32.20, 29.85, 32.65, 31.32),
        zone_resolution: 5,
        leaf_resolution: 11,
        geofabrik_url: None, // sea-level canal, coastline provides gaps
        water_types: &[],
    },
    Passage {
        name: "Panama Canal",
        corridor: (-79.95, 8.88, -79.50, 9.42),
        zone_resolution: 5,
        leaf_resolution: 13,  // bumped from 11 — lock channels need 3.5m edges
        geofabrik_url: Some("https://download.geofabrik.de/central-america/panama-latest.osm.pbf"),
        water_types: &["lock", "reservoir", "lake", "river"],
    },
    Passage {
        name: "Kiel Canal",
        corridor: (9.05, 53.85, 10.20, 54.40),
        zone_resolution: 5,
        leaf_resolution: 11,
        geofabrik_url: Some("https://download.geofabrik.de/europe/germany/schleswig-holstein-latest.osm.pbf"),
        water_types: &["lock", "reservoir", "lake", "river", "canal"],
    },
    Passage {
        name: "Corinth Canal",
        corridor: (22.94, 37.88, 23.03, 37.96),
        zone_resolution: 5,
        leaf_resolution: 13,
        geofabrik_url: None, // sea-level canal, coastline provides gaps
        water_types: &[],
    },
    // Natural straits — all get geofabrik_url: None, water_types: &[]
    Passage {
        name: "Bosphorus",
        corridor: (28.95, 40.95, 29.20, 41.28),
        zone_resolution: 5,
        leaf_resolution: 10,
        geofabrik_url: None,
        water_types: &[],
    },
    Passage {
        name: "Dardanelles",
        corridor: (26.10, 39.95, 26.75, 40.50),
        zone_resolution: 5,
        leaf_resolution: 10,
        geofabrik_url: None,
        water_types: &[],
    },
    Passage {
        name: "Malacca Strait",
        corridor: (103.35, 1.10, 103.90, 1.40),
        zone_resolution: 5,
        leaf_resolution: 10,
        geofabrik_url: None,
        water_types: &[],
    },
    Passage {
        name: "Singapore Strait",
        corridor: (103.70, 1.15, 104.35, 1.30),
        zone_resolution: 5,
        leaf_resolution: 10,
        geofabrik_url: None,
        water_types: &[],
    },
    Passage {
        name: "Messina Strait",
        corridor: (15.55, 38.05, 15.70, 38.35),
        zone_resolution: 5,
        leaf_resolution: 10,
        geofabrik_url: None,
        water_types: &[],
    },
    Passage {
        name: "Dover Strait",
        corridor: (1.15, 50.85, 1.70, 51.20),
        zone_resolution: 5,
        leaf_resolution: 10,
        geofabrik_url: None,
        water_types: &[],
    },
];
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p asw-core 2>&1`
Expected: success (other crates may fail until they're updated to use the new fields)

- [ ] **Step 4: Commit**

```bash
git add crates/asw-core/src/passages.rs
git commit -m "feat: add geofabrik_url and water_types to Passage struct"
```

---

### Task 2: Add `subtract_water()` to LandIndex

**Files:**
- Modify: `crates/asw-core/src/geo_index.rs`

- [ ] **Step 1: Add `subtract_water` method to `LandIndex`**

This method takes a list of water polygons and rebuilds the R-tree with land polygons that have water holes punched in them. Only land polygons whose bounding box intersects the water polygons are modified (for performance).

```rust
use geo::algorithm::bool_ops::BooleanOps;

impl LandIndex {
    /// Subtract water polygons from land, creating holes where canals exist.
    /// Only land polygons that intersect the water bounding box are modified.
    pub fn subtract_water(&mut self, water_polygons: &[Polygon<f64>]) {
        if water_polygons.is_empty() {
            return;
        }

        // Compute water bounding box for quick filtering
        let mut w_min_x = f64::MAX;
        let mut w_min_y = f64::MAX;
        let mut w_max_x = f64::MIN;
        let mut w_max_y = f64::MIN;
        for wp in water_polygons {
            for coord in wp.exterior().coords() {
                w_min_x = w_min_x.min(coord.x);
                w_min_y = w_min_y.min(coord.y);
                w_max_x = w_max_x.max(coord.x);
                w_max_y = w_max_y.max(coord.y);
            }
        }
        let water_envelope = AABB::from_corners([w_min_x, w_min_y], [w_max_x, w_max_y]);

        // Create MultiPolygon for subtraction
        let water_multi = geo::MultiPolygon::new(water_polygons.to_vec());

        // Clone all land polygons, modify those that intersect water, rebuild tree
        let all_polys: Vec<LandPolygon> = self.tree.iter().cloned()
            .flat_map(|lp| {
                if !lp.envelope.intersects(&water_envelope) {
                    return vec![lp];
                }
                // Subtract water from this land polygon
                // BooleanOps::difference returns MultiPolygon
                let diff = lp.polygon.difference(&water_multi);
                diff.into_iter()
                    .map(|p| LandPolygon::new(p))
                    .collect::<Vec<_>>()
            })
            .collect();

        self.tree = RTree::bulk_load(all_polys);
    }
}
```

Note: `geo 0.32` (our workspace version) has `BooleanOps` in `geo::algorithm::bool_ops`. The `difference` method on `Polygon` takes a `&MultiPolygon` and returns `MultiPolygon`. We iterate it to get individual `Polygon`s for the R-tree.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p asw-core 2>&1`
Expected: success

- [ ] **Step 4: Commit**

```bash
git add crates/asw-core/src/geo_index.rs
git commit -m "feat: add subtract_water() to LandIndex for canal gap creation"
```

---

### Task 3: Create canal_water module (PBF download + osmium + GeoJSON parsing)

**Files:**
- Create: `crates/asw-build/src/canal_water.rs`
- Modify: `crates/asw-build/src/lib.rs`
- Modify: `crates/asw-build/Cargo.toml`

- [ ] **Step 1: Add `geojson` dependency to asw-build**

In `crates/asw-build/Cargo.toml`, add (it's already in the workspace):
```toml
geojson.workspace = true
```

- [ ] **Step 2: Create `canal_water.rs`**

This module:
1. Downloads a Geofabrik PBF to a temp dir (reqwest, same pattern as `download_and_extract`)
2. Runs `osmium tags-filter` to extract `natural=water` features
3. Runs `osmium export` to convert to GeoJSON
4. Parses the GeoJSON, clips to corridor bbox, filters by water type
5. Returns `Vec<geo::Polygon<f64>>`

```rust
//! Download and extract canal water polygons from Geofabrik PBF files.
//!
//! For each passage with a `geofabrik_url`, downloads the regional PBF,
//! extracts `natural=water` polygons via osmium, clips to the passage
//! corridor, and returns geo::Polygons for land subtraction.

use anyhow::{Context, Result};
use asw_core::passages::Passage;
use geo::{Coord, LineString, Polygon};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

use crate::shapefile::Bbox;

/// Extract canal water polygons for all passages that have a `geofabrik_url`.
/// Downloads PBFs to `work_dir`, processes with osmium, returns water polygons.
/// Skips passages whose corridors don't overlap `build_bbox` (if provided).
pub fn extract_canal_water(
    passages: &[Passage],
    build_bbox: Option<Bbox>,
    work_dir: &Path,
) -> Result<Vec<Polygon<f64>>> {
    let canal_dir = work_dir.join("canal-water");
    std::fs::create_dir_all(&canal_dir)?;

    // Check osmium is available
    let osmium_check = Command::new("osmium").arg("--version").output();
    if osmium_check.is_err() || !osmium_check.unwrap().status.success() {
        anyhow::bail!(
            "osmium-tool is required for canal water extraction. \
             Install it: apt install osmium-tool (Linux) or brew install osmium-tool (macOS)"
        );
    }

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

        info!("Processing canal water for '{}'...", passage.name);
        let water = extract_single_passage(passage, url, &canal_dir)?;
        info!(
            "  {} water polygons extracted for '{}'",
            water.len(),
            passage.name
        );
        all_water.extend(water);
    }

    if !all_water.is_empty() {
        info!("Total canal water polygons: {}", all_water.len());
    }

    Ok(all_water)
}

fn extract_single_passage(
    passage: &Passage,
    url: &str,
    work_dir: &Path,
) -> Result<Vec<Polygon<f64>>> {
    // Derive filenames from passage name
    let safe_name = passage.name.to_lowercase().replace(' ', "-");
    let pbf_path = work_dir.join(format!("{}.osm.pbf", safe_name));
    let water_pbf_path = work_dir.join(format!("{}-water.osm.pbf", safe_name));
    let geojson_path = work_dir.join(format!("{}-water.geojson", safe_name));

    // Step 1: Download PBF (with caching)
    if !pbf_path.exists() {
        info!("  Downloading {}...", url);
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(600)) // 10 min
            .build()?;
        let mut resp = client.get(url).send().context("Failed to download PBF")?;
        let mut file = std::fs::File::create(&pbf_path)?;
        let bytes = std::io::copy(&mut resp, &mut file)?;
        info!("  Downloaded {} MB", bytes / 1_000_000);
    } else {
        info!("  Using cached PBF: {:?}", pbf_path);
    }

    // Step 2: osmium tags-filter for natural=water
    if !water_pbf_path.exists() {
        info!("  Filtering for natural=water...");
        let status = Command::new("osmium")
            .args([
                "tags-filter",
                pbf_path.to_str().unwrap(),
                "nwr/natural=water",
                "-o",
                water_pbf_path.to_str().unwrap(),
                "--overwrite",
            ])
            .status()
            .context("Failed to run osmium tags-filter")?;
        if !status.success() {
            anyhow::bail!("osmium tags-filter failed for {}", passage.name);
        }
    }

    // Step 3: osmium export to GeoJSON
    info!("  Exporting to GeoJSON...");
    let status = Command::new("osmium")
        .args([
            "export",
            water_pbf_path.to_str().unwrap(),
            "-o",
            geojson_path.to_str().unwrap(),
            "--overwrite",
        ])
        .status()
        .context("Failed to run osmium export")?;
    if !status.success() {
        anyhow::bail!("osmium export failed for {}", passage.name);
    }

    // Step 4: Parse GeoJSON, clip to corridor, filter by water type
    let geojson_str = std::fs::read_to_string(&geojson_path)
        .context("Failed to read GeoJSON")?;
    let geojson: geojson::GeoJson = geojson_str.parse()
        .context("Failed to parse GeoJSON")?;

    let (min_lon, min_lat, max_lon, max_lat) = passage.corridor;
    let water_types: std::collections::HashSet<&str> =
        passage.water_types.iter().copied().collect();

    let mut polygons = Vec::new();

    if let geojson::GeoJson::FeatureCollection(fc) = geojson {
        for feature in fc.features {
            // Filter by water type
            if let Some(ref props) = feature.properties {
                let water_val = props
                    .get("water")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !water_val.is_empty() && !water_types.contains(water_val) {
                    continue;
                }
                // Skip features that aren't natural=water
                let natural = props
                    .get("natural")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if natural != "water" {
                    continue;
                }
            } else {
                continue;
            }

            // Convert geometry
            if let Some(geom) = feature.geometry {
                let polys = geojson_geometry_to_polygons(&geom.value);
                for poly in polys {
                    // Clip: skip polygons entirely outside corridor
                    if polygon_intersects_bbox(&poly, (min_lon, min_lat, max_lon, max_lat)) {
                        polygons.push(poly);
                    }
                }
            }
        }
    }

    // Cleanup intermediate files (keep PBF cache)
    let _ = std::fs::remove_file(&geojson_path);
    let _ = std::fs::remove_file(&water_pbf_path);

    Ok(polygons)
}

/// Convert a GeoJSON geometry value to geo::Polygons.
fn geojson_geometry_to_polygons(value: &geojson::Value) -> Vec<Polygon<f64>> {
    match value {
        geojson::Value::Polygon(coords) => {
            if let Some(poly) = coords_to_polygon(coords) {
                vec![poly]
            } else {
                vec![]
            }
        }
        geojson::Value::MultiPolygon(multi) => multi
            .iter()
            .filter_map(|coords| coords_to_polygon(coords))
            .collect(),
        _ => vec![], // Skip points, lines, etc.
    }
}

fn coords_to_polygon(coords: &[Vec<Vec<f64>>]) -> Option<Polygon<f64>> {
    if coords.is_empty() {
        return None;
    }
    let exterior = LineString::new(
        coords[0]
            .iter()
            .map(|c| Coord { x: c[0], y: c[1] })
            .collect(),
    );
    let holes: Vec<LineString<f64>> = coords[1..]
        .iter()
        .map(|ring| {
            LineString::new(ring.iter().map(|c| Coord { x: c[0], y: c[1] }).collect())
        })
        .collect();
    Some(Polygon::new(exterior, holes))
}

fn polygon_intersects_bbox(poly: &Polygon<f64>, bbox: Bbox) -> bool {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    let mut p_min_x = f64::MAX;
    let mut p_min_y = f64::MAX;
    let mut p_max_x = f64::MIN;
    let mut p_max_y = f64::MIN;
    for coord in poly.exterior().coords() {
        p_min_x = p_min_x.min(coord.x);
        p_min_y = p_min_y.min(coord.y);
        p_max_x = p_max_x.max(coord.x);
        p_max_y = p_max_y.max(coord.y);
    }
    !(p_max_x < min_lon || p_min_x > max_lon || p_max_y < min_lat || p_min_y > max_lat)
}
```

- [ ] **Step 3: Add module to lib.rs**

In `crates/asw-build/src/lib.rs`:
```rust
pub mod canal_water;
pub mod cells;
pub mod coastline;
pub mod edges;
pub mod pipeline;
pub mod shapefile;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p asw-build 2>&1`
Expected: success

- [ ] **Step 5: Commit**

```bash
git add crates/asw-build/src/canal_water.rs crates/asw-build/src/lib.rs crates/asw-build/Cargo.toml
git commit -m "feat: add canal_water module for Geofabrik PBF water extraction"
```

---

### Task 4: Integrate canal water into build pipeline

**Files:**
- Modify: `crates/asw-build/src/pipeline.rs`

- [ ] **Step 1: Add canal water step between land load and cell generation**

In `pipeline.rs`, after loading land polygons (line 15-16) and before cell generation (line 45), insert the canal water extraction and subtraction:

```rust
pub fn run(shp_path: &Path, bbox: Option<Bbox>, output_path: &Path) -> Result<()> {
    // Step 1: Load land polygons
    let mut land = crate::shapefile::load_land_polygons(shp_path, None)?;
    info!("Land index: {} polygons", land.polygon_count());

    // Step 1b: Extract canal water and subtract from land
    let work_dir = output_path
        .parent()
        .unwrap_or(Path::new("."));
    let canal_water = crate::canal_water::extract_canal_water(PASSAGES, bbox, work_dir)?;
    if !canal_water.is_empty() {
        info!("Subtracting {} canal water polygons from land...", canal_water.len());
        land.subtract_water(&canal_water);
        info!("Land index after subtraction: {} polygons", land.polygon_count());
    }

    // Step 2: Extract coastline (unchanged)
    // ...
```

Note: `land` must be `let mut land` now (was `let land`).

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p asw-build 2>&1`
Expected: success

- [ ] **Step 3: Commit**

```bash
git add crates/asw-build/src/pipeline.rs
git commit -m "feat: integrate canal water subtraction into build pipeline"
```

---

### Task 5: Update cloud build pipeline

**Files:**
- Modify: `crates/asw-cloud/src/config.rs`
- Modify: `crates/asw-cloud/src/pipeline.rs`

- [ ] **Step 1: Add osmium-tool to bootstrap packages**

In `crates/asw-cloud/src/config.rs`:
```rust
pub const BOOTSTRAP_PACKAGES: &str = "wget unzip curl build-essential pkg-config libssl-dev osmium-tool";
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p asw-cloud 2>&1`
Expected: success

Note: No changes needed to the cloud `pipeline.rs` — the canal water extraction runs inside `asw build` (the Rust binary executed on the remote server in step 4). The remote `asw build` command already calls `asw_build::pipeline::run()` which now includes the canal water step. The PBFs are downloaded by the Rust code during the build, not as a separate SSH step.

- [ ] **Step 3: Commit**

```bash
git add crates/asw-cloud/src/config.rs
git commit -m "feat: add osmium-tool to cloud build bootstrap packages"
```

---

### Task 6: Add new canal passages

**Files:**
- Modify: `crates/asw-core/src/passages.rs`

- [ ] **Step 1: Add new canal passages to PASSAGES array**

Append these after the existing entries:

```rust
    // ── New canals ──────────────────────────────────────────────────────
    Passage {
        name: "Houston Ship Channel",
        corridor: (-95.30, 29.30, -94.70, 29.80),
        zone_resolution: 5,
        leaf_resolution: 12,
        geofabrik_url: Some("https://download.geofabrik.de/north-america/us/texas-latest.osm.pbf"),
        water_types: &["lock", "reservoir", "lake", "river", "canal"],
    },
    Passage {
        name: "Cape Cod Canal",
        corridor: (-70.65, 41.72, -70.48, 41.79),
        zone_resolution: 5,
        leaf_resolution: 12,
        geofabrik_url: Some("https://download.geofabrik.de/north-america/us/massachusetts-latest.osm.pbf"),
        water_types: &["lock", "reservoir", "lake", "river", "canal"],
    },
    Passage {
        name: "Chesapeake-Delaware Canal",
        corridor: (-75.85, 39.40, -75.55, 39.60),
        zone_resolution: 5,
        leaf_resolution: 12,
        geofabrik_url: Some("https://download.geofabrik.de/north-america/us/delaware-latest.osm.pbf"),
        water_types: &["lock", "reservoir", "lake", "river", "canal"],
    },
    Passage {
        name: "Welland Canal",
        corridor: (-79.30, 42.85, -79.15, 43.25),
        zone_resolution: 5,
        leaf_resolution: 13,
        geofabrik_url: Some("https://download.geofabrik.de/north-america/canada/ontario-latest.osm.pbf"),
        water_types: &["lock", "reservoir", "lake", "river", "canal"],
    },
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p asw-core 2>&1`
Expected: success

- [ ] **Step 3: Commit**

```bash
git add crates/asw-core/src/passages.rs
git commit -m "feat: add Houston, Cape Cod, Chesapeake-Delaware, Welland canal passages"
```

---

### Task 7: Local integration test with Panama Canal

- [ ] **Step 1: Build graph for Panama corridor**

Run: `cargo build --release -p asw-cli 2>&1`
Then: `./target/release/asw build --bbox '-79.95,8.88,-79.50,9.42' --output export/panama-test.graph 2>&1`

Expected: Build succeeds, logs show canal water extraction for Panama, cells generated in the canal area.

- [ ] **Step 2: Export GeoJSON for visualization**

Run: `./target/release/asw geojson --graph export/panama-test.graph --bbox '-79.95,8.88,-79.50,9.42' --output export/panama-test.geojson 2>&1`

Visually verify in geojson.io that hexagons cover:
- Gatun Lake
- Culebra Cut
- All lock channels (Gatun, Pedro Miguel, Miraflores, Cocoli, Agua Clara)
- Connected to ocean at both Atlantic and Pacific entrances

- [ ] **Step 3: Commit and push**

```bash
git add -A
git commit -m "test: verify Panama Canal coverage with canal water subtraction"
```

---

### Task 8: Add ODbL attribution

**Files:**
- Modify: `README.md` (add data attribution section)

- [ ] **Step 1: Add attribution notice**

Add a "Data Sources" or "Attribution" section to README.md:

```markdown
## Data Attribution

Geographic data in this project is derived from [OpenStreetMap](https://www.openstreetmap.org/), © OpenStreetMap contributors, available under the [Open Database License (ODbL) v1.0](https://opendatacommons.org/licenses/odbl/1-0/).

- Land polygons: [osmdata.openstreetmap.de](https://osmdata.openstreetmap.de/data/land-polygons.html)
- Canal water polygons: [Geofabrik downloads](https://download.geofabrik.de/) (regional OSM extracts)
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add ODbL attribution for OSM-derived geographic data"
```

---

## Implementation Notes

### osmium dependency
The build requires `osmium-tool` installed on the system:
- macOS: `brew install osmium-tool`
- Ubuntu/Debian: `apt install osmium-tool`
- Cloud build: added to `BOOTSTRAP_PACKAGES` in Task 5

### PBF caching
Downloaded PBFs are cached in `<work_dir>/canal-water/`. On repeated builds, the PBF is reused. The filtered water PBF and GeoJSON are regenerated each time (they're small and fast). To force a fresh download, delete the cached PBF.

### Download sizes
Total additional download for all canals: ~1 GB (dominated by Texas PBF at ~700 MB for Houston Ship Channel). Consider whether smaller regional extracts are available. Panama is only ~30 MB.

### geo BooleanOps
The `subtract_water()` method uses `geo::BooleanOps::difference`. Verify the workspace `geo` version supports this (>= 0.27). If not, update the geo dependency.

### Passage resolution rationale
- Panama: res-13 (lock channels ~33m wide, need 3.5m cell edges)
- Kiel: res-11 (100m+ wide, 25m edges sufficient)
- Houston/Cape Cod/Chesapeake: res-12 (90-160m wide, 9m edges)
- Welland: res-13 (24m lock width, need 3.5m edges)
