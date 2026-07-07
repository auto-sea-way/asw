# Project Review — Fable 5, 2026-07-06

Whole-codebase review for serious performance and code-quality issues, run as a
multi-agent workflow on Claude Fable 5: 7 parallel reviewers (per crate and per
dimension), every finding then adversarially verified by independent agents
instructed to refute it (critical/high: 3 verifiers, 2 must uphold; medium: 1).

**Stats:** 37 raw findings -> 25 after dedup -> 16 confirmed, 8 refuted. 51 agents total.

## Confirmed findings

### 1. [CRITICAL/performance] AstarBuffers::reset is O(num_nodes) — ~360 MB memset on every request

**Location:** `crates/asw-core/src/astar_pool.rs:17`

**Evidence:** pub fn reset(&mut self) { self.g_score.fill(f32::MAX); self.came_from.fill(u32::MAX); self.closed.fill(false); } — called from AstarPool::release (line 50), which asw-serve runs after every route request (crates/asw-serve/src/state.rs:244). At planet scale (39.8M nodes) this rewrites 4+4+1 = 9 bytes per node, ~358 MB of memory traffic per request, regardless of how many nodes the search actually touched. A short Marmaris hop that visits a few thousand nodes still pays a full-graph memset — roughly 15-35 ms of pure memory bandwidth, which alone exceeds the single-digit-millisecond query budget and runs synchronously inside the async handler (with_astar_buffers does not spawn_blocking).

**Suggested fix:** Reset only what was touched. Cheapest option: add a generation counter — store `gen: Vec<u32>` (or fold it into g_score bookkeeping) plus `current_gen: u32` in AstarBuffers; a node's g_score/came_from/closed entries are valid only if gen[node] == current_gen, and reset() becomes `current_gen += 1` (with a rare full clear on wraparound). Alternative: record touched node IDs in a `dirty: Vec<u32>` during the search and reset only those entries in release().

### 2. [HIGH/correctness] u16 centi-nm weight quantization zeroes out all res-13 passage-corridor edges

**Location:** `crates/asw-core/src/graph.rs:123`

**Evidence:** `let weight_u16 = (weight_nm * 100.0).round() as u16;` stores edge weights in 0.01 nm units. Edge weights are haversine center-to-center distances (crates/asw-build/src/edges.rs:40). Adjacent H3 res-13 cell centers are ~6.1 m apart = 0.00332 nm, which rounds to 0 — verified numerically: res-13 -> u16 = 0, res-12 -> 0.01 nm (+14% error), res-11 -> 0.02 nm (-14% error). passages.rs gives Panama, Kiel, Corinth, and Welland corridors leaf_resolution 13, so every edge in those corridors has cost 0.0 in the shipped graph. Consequences: (1) A* sees canal/lock transit as free, so the g_score cost model undercounts any path touching a res-13 zone and can prefer geometrically longer routes through corridor meshes over correct alternatives; (2) the haversine heuristic becomes wildly inadmissible inside these zones (h > 0 while true remaining cost is 0 across thousands of cells), and with the closed-set A* in routing.rs (nodes never reopen, line 55-58) inadmissibility yields suboptimal paths, not just slower search. The debug_assert on line 119 only guards the >655.35 nm overflow case, and only in debug builds — the underflow-to-zero case is completely silent in the release builds used for planet graphs.

**Suggested fix:** Use a finer or wider weight encoding: e.g. varint-encoded u32 in 0.0001 nm units (res-13 edge = 33, res-3 edge ~640,000 still fits; most values stay 1-3 bytes), or a u32 fixed-point weight. At minimum, clamp quantized weights to >= 1 and replace the debug_assert with a hard error in GraphBuilder::build so a lossy weight can never be silently written.

### 3. [HIGH/correctness] crosses_land does planar lon/lat geometry with no antimeridian handling — Pacific date-line routes get false land crossings and near-global R-tree scans

**Location:** `crates/asw-core/src/geo_index.rs:209`

**Evidence:** `pub fn crosses_land(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> bool { let min_lon = lon1.min(lon2); ... let envelope = AABB::from_corners([min_lon, min_lat], [max_lon, max_lat]); let line = Line::new(...)` — a smoothing segment that crosses the antimeridian (e.g. Auckland lon 174 to Honolulu lon -157, both real nodes in the planet graph) produces (a) an envelope spanning lon [-157, 174] = 331 degrees, so the R-tree query returns nearly every coastline segment on Earth in that latitude band (hundreds of thousands of Line::intersects tests per call — a massive latency spike vs the single-digit-ms budget), and (b) a planar geo::Line that runs the long way around through lon 0, geometrically intersecting African/American coastlines, so the function returns true for a segment whose great-circle path is open Pacific water. smooth() (routing.rs:99, 113, 136) then can never merge waypoints across the date line: every direct-to-end test at line 98-99 re-pays the near-global scan on each loop iteration, and smoothing degenerates to raw-path granularity there. min_distance_deg (line 228) has the same wrap problem. haversine_nm handles the wrap correctly, so A* itself is fine — only the coastline checks are broken.

**Suggested fix:** Normalize segments before indexing/querying: split any query segment (and any stored coastline segment) at lon = ±180 into two sub-segments with consistent longitudes, then run the envelope + intersects test per sub-segment. Alternatively shift the query into a continuous frame (add 360 to negative lons when |lon1 - lon2| > 180) and store coastline geometry duplicated across the seam.

### 4. [HIGH/correctness] Edge midpoint land test computes wrong point for edges crossing the antimeridian

**Location:** `crates/asw-build/src/edges.rs:179`

**Evidence:** let mid_lat = (lat1 + lat2) / 2.0; let mid_lon = (lon1 + lon2) / 2.0; if water.is_water(mid_lon, mid_lat) { ... }  — H3 neighbors straddle the antimeridian (verified with h3o 0.9: cell 87164a4cdffffff at lat 52.4 has neighbors with centers at lon +179.99 and -179.99). For such an edge, lon1 ≈ +179.98 and lon2 ≈ -179.98 average to mid_lon ≈ 0, so the land check is performed on the opposite side of the planet. At Aleutian latitudes (~52 N) the point (0, 52.4) is in England — land — so every valid water edge crossing 180 deg there is removed, severing the graph along the antimeridian; at other latitudes the check samples open Atlantic and can validate an edge that actually crosses land near 180 deg. Planet builds get systematically wrong routing across the Pacific date line (Aleutians, Bering, Fiji/NZ routes).

**Suggested fix:** Wrap the longitude difference before averaging: if (lon1 - lon2).abs() > 180.0, shift the smaller lon by +360 before averaging and re-normalize the result into [-180, 180] (mid_lon = ((mid_lon + 540.0) % 360.0) - 180.0). Same wrap-aware midpoint should be used anywhere a chord midpoint is derived from two lon values.

### 5. [HIGH/correctness] cell_polygon produces degenerate world-spanning polygons for cells straddling the antimeridian, misclassifying them as land

**Location:** `crates/asw-core/src/h3.rs:42`

**Evidence:** cell_polygon builds a geo::Polygon directly from boundary vertices: `x: ll.lng(), y: ll.lat()` with no antimeridian normalization. Verified with h3o 0.9: a res-7 cell at (52.4, 179.999) has boundary vertices at both lon -179.97 and lon +179.98. In planar geo space that ring spans lon -179.99..+179.98, so its edges are chords sweeping across the whole map at that latitude. All classification in crates/asw-build/src/cells.rs (land.intersects_polygon / land.contains_polygon at lines 80, 124, 165, 246, 318, 425) then tests this degenerate polygon: a pure-water Pacific cell on the antimeridian 'intersects' land in Europe/Asia at the same latitude and is dropped at the leaf filter, punching a stripe of missing nodes along 180 deg in every planet build. Side effect: its AABB spans the full longitude range, so the R-tree query visits every land polygon in that latitude band (large slowdown for those cells).

**Suggested fix:** In cell_polygon, detect a lon jump > 180 between consecutive vertices and unwrap longitudes into a continuous range (e.g. add 360 to negative lons so the ring lives in ~[179, 181]); either split the polygon at 180 deg into two rings, or keep the unwrapped ring and make LandIndex queries antimeridian-aware. At minimum, transmeridian cells must not be classified with the raw wrapped ring.

### 6. [HIGH/performance] CPU-bound route computation runs directly on the async runtime, blocking tokio workers and health probes

**Location:** `crates/asw-serve/src/api.rs:90`

**Evidence:** `let result = app.with_astar_buffers(|buffers| { compute_route(&app.graph, ..., &knn, buffers) }).await` — `with_astar_buffers` (state.rs:238-246) is an async fn but executes the closure synchronously on the tokio worker thread. That closure includes: two `nearest_node` calls (worst case tens of ms, see the grid_disk finding), the full A* search (a long route, e.g. transoceanic, pops hundreds of thousands to millions of nodes), line-of-sight smoothing with R-tree queries, and the mandatory 358 MB `buf.reset()` in `release` (~6-15 ms of memset). Concrete scenario: on a small instance (2-4 vCPU, so 2-4 tokio workers), a handful of concurrent long-route or inland-snap requests occupy every worker thread for tens of ms each; `/health` and `/ready` — which the Docker HEALTHCHECK (asw-cli `Health` command) depends on — stall behind them, and an orchestrator with a tight probe timeout restarts a perfectly healthy container under load.

**Suggested fix:** Move the compute into `tokio::task::spawn_blocking`. This requires the state to be ownable across the blocking boundary: change `ServerState.inner` to `RwLock<Option<Arc<AppState>>>` (or arc-swap), clone the `Arc<AppState>` in the handler, drop the read guard, and run knn + compute_route inside spawn_blocking with the cloned Arc.

### 7. [HIGH/correctness] Step cache ignores build inputs: stale/partial local output silently skips download, stale remote graph skips build

**Location:** `crates/asw-cloud/src/pipeline.rs:112`

**Evidence:** `"download" => { return self.output_path.exists() && ... m.len() > 1024 ... }` and `"build_graph" => self.remote_file_exists(".../asw.graph")`. Neither key includes the bbox or any input hash. Scenario A: run `asw cloud build --bbox marmaris` (writes default `export/asw.graph`), then `asw cloud build --bbox dev` — the "download" step reports "cached" because the old marmaris file exists and is >1 KiB, and the pipeline prints "Build complete. Output: export/asw.graph" while the file is still the marmaris graph. With `--keep-server`, "build_graph" is also skipped because the remote `/data/asw/asw.graph` from the previous bbox exists. Scenario B: `scp_download` (ssh.rs:123) writes directly to the final `output_path` with no temp-file+rename, so an interrupted download leaves a truncated file >1 KiB that the next run accepts as "cached" and hands to `asw serve`, which then fails to deserialize (or worse, a truncated-but-parseable file).

**Suggested fix:** Include the bbox (and ideally a source revision) in the remote graph filename and in the local cache check, or drop the local "download" cache entirely. Make `scp_download` write to `<output>.tmp` and rename on success, mirroring what `download.rs::ensure_graph` already does correctly.

### 8. [MEDIUM/performance] H3-to-lat/lng decode recomputed for every edge relaxation in the A* inner loop

**Location:** `crates/asw-core/src/routing.rs:70`

**Evidence:** let (nlat, nlon) = graph.node_pos(neighbor); let h = haversine_nm(nlat, nlon, goal_lat, goal_lon) as f32; — node_pos (graph.rs:232-237) does CellIndex::try_from + h3o::LatLng::from, an icosahedral face projection with trig, and haversine adds sin/cos/asin. This runs once per improving relaxation, and with ~6 incoming edges per hex a node can be relaxed (and its position+heuristic recomputed) several times before it is closed. Across the millions of relaxations of a long planet-scale route this H3 decode + trig is the dominant per-edge cost of the search.

**Suggested fix:** Cache the heuristic per node per query: add an `h_score: Vec<f32>` to AstarBuffers guarded by the same generation counter proposed for the reset fix; compute node_pos+haversine only on first touch of a node and reuse the cached h on subsequent relaxations. This cuts the decode/trig work by the average relaxation multiplicity without adding any persistent coordinate array.

### 9. [MEDIUM/performance] min_distance_deg allocates a Vec of all coords per coastline segment per call

**Location:** `crates/asw-core/src/geo_index.rs:237`

**Evidence:** let coords: Vec<_> = seg.line.coords().collect(); for w in coords.windows(2) { ... } — a heap allocation (up to COASTLINE_SUBDIVIDE_MAX = 256 coord pointers) for every R-tree candidate segment on every call, done only to iterate consecutive pairs. This function is the inner loop of the build cascade: asw-build calls it for the center and every boundary vertex of every candidate cell (crates/asw-build/src/cells.rs:529-533), i.e. tens of millions of calls x several segments each during a planet build.

**Suggested fix:** Iterate pairs without collecting: either `for line in seg.line.lines() { point_to_segment_dist(pt, line.start, line.end) }` or window directly over the LineString's underlying coord Vec via `seg.line.0.windows(2)`. Zero allocations, same result.

### 10. [MEDIUM/correctness] download_and_extract: no HTTP status check and non-atomic extraction poisons the shapefile cache

**Location:** `crates/asw-build/src/shapefile.rs:209`

**Evidence:** `let mut resp = client.get(url).send().context(...)?;` never calls `.error_for_status()` (unlike canal_water.rs:93 which does), so a 404/503 HTML body is written to land-polygons-split-4326.zip and the build later dies with the misleading 'Failed to read zip'. Worse, extraction (lines 221-232) writes files directly into extract_dir with no tmp-dir/rename step, while the cache check at lines 193-199 treats 'extract_dir exists and contains any .shp' as fully cached. An interrupted or failed extraction (ctrl-C, disk full, bad zip entry) leaves a truncated land_polygons.shp that satisfies the cache check, so every subsequent `asw build` reuses the corrupt file — either failing with an unrelated shapefile parse error until the user manually deletes the directory, or (if truncated at a record boundary) silently building a graph with missing land.

**Suggested fix:** Call `.error_for_status()` on the response; extract into a temporary sibling directory and `fs::rename` it to extract_dir only after all entries extracted successfully (mirroring the .pbf.tmp pattern already used in canal_water.rs:95-102).

### 11. [MEDIUM/quality] Zone split assumes all passages share passages[0].zone_resolution, silently ignoring passages with a different one

**Location:** `crates/asw-build/src/cells.rs:193`

**Evidence:** `let zone_res = Resolution::try_from(passages[0].zone_resolution)...` then `cell.parent(zone_res)` is used to probe zone_lookup for every cell — but build_zone_lookup (lines 480-492) keys the map at each passage's own `zone_resolution`. Passage exposes per-passage zone_resolution as a documented field ('typically 5'), and all 14 current passages happen to use 5, so this works today only by coincidence. If any passage is added with zone_resolution 4 or 6, its lookup keys are at a different H3 resolution than the parent probe, no cell ever matches, and that passage's refinement silently never happens — the canal becomes unroutable with no error or warning.

**Suggested fix:** Either probe the lookup once per distinct zone_resolution present in `passages` (compute the set of resolutions and take parents at each), or make zone_resolution a single crate-level constant instead of a per-passage field so the type cannot express the unsupported case.

### 12. [MEDIUM/performance] nearest_node rescans the full grid_disk at every k — O(k_max³) redundant H3 lookups plus a Vec allocation per ring

**Location:** `crates/asw-serve/src/state.rs:138`

**Evidence:** `for k in 0..=k_max { ... for neighbor in cell.grid_disk::<Vec<_>>(k) { ... } if found_at_k { return; } }` — `grid_disk(k)` returns the entire filled disk of radius k, so iteration k re-tests every cell already tested at 0..k-1, and allocates a fresh Vec each iteration. Total cells scanned when nothing is found until k_max is Σ(1+3k(k+1)) = O(k_max³): for pass-2 k_max=30 that is ~29,000 lookups where a ring-based scan needs 2,791; the exhaustive fallback (state.rs:216-232, k up to 50) worst-cases at ~132,000 lookups. Each lookup is a binary search over the 40M-entry sorted node_h3 vec (~25 random probes, mostly cache/TLB misses, on the order of microseconds each). Concrete scenario: a user clicks a deep-inland point, or pass 1 finds no candidate — pass 2 runs k_max searches across res 9..3 plus the fallback, burning tens of ms of CPU per snap, twice per route request, all on the async runtime (compounding the blocking finding).

**Suggested fix:** Scan each cell once: call `cell.grid_disk_distances::<Vec<_>>(k_max)` once per resolution and process cells grouped by ascending k (or use `grid_ring_fast(k)` per iteration), returning at the first k that yields a main-component hit. Semantics are identical to the current found_at_k early return.

### 13. [MEDIUM/correctness] Compile cache never hits: probe runs `asw --version` but the CLI defines no version flag

**Location:** `crates/asw-cloud/src/pipeline.rs:130`

**Evidence:** `remote_binary_works` runs `"{} --version 2>/dev/null && echo yes || echo no"` against `/usr/local/bin/asw`. The clap definition in crates/asw-cli/src/main.rs:12 is `#[command(name = "asw", about = ...)]` with no `version` attribute, so clap does not generate `--version`; the binary exits nonzero with "unexpected argument" and the probe always prints "no". Result: the "compile" step is never reported cached — every `cloud build` against a kept server re-runs the rustup check and `cargo build --release` (minutes of wasted time per run), and the cache branch is effectively dead code.

**Suggested fix:** Add `version` to the clap command attribute (`#[command(name = "asw", version, about = ...)]`), or change the probe to something the binary actually supports, e.g. `test -x /usr/local/bin/asw` plus `asw --help >/dev/null`.

### 14. [MEDIUM/correctness] SSH-key uniqueness recovery is dead code (error body discarded) and its fallback picks an arbitrary, possibly wrong key

**Location:** `crates/asw-cloud/src/hetzner.rs:290`

**Evidence:** `create_ssh_key` uses `.error_for_status().context("Failed to create SSH key")?` (line 246-248), which drops the response body — Hetzner's `uniqueness_error` code lives only in the JSON body. So `if format!("{:?}", e).to_lowercase().contains("uniqueness")` (line 290) can never match: on a name collision (e.g. account already has a different key named "asw-key" uploaded from another machine) provisioning fails with an opaque "HTTP 409". Worse, even if the branch fired, the fallback `keys.first()` selects an arbitrary account key that need not match the local private key, so `create_server` would be provisioned with a key the user cannot authenticate with — `wait_for_ssh` then times out after creating a paid server.

**Suggested fix:** On non-2xx from the create call, read `resp.status()` and `resp.text()` before erroring (as `create_server` already does) and match on the parsed error code. In the name-collision case, retry with a uniquified name (e.g. append a short hash of the key) instead of grabbing `keys.first()` — never bind the server to a key whose material was not verified against the local public key.

### 15. [MEDIUM/correctness] Byte-index string slice of SSH key comment panics on non-ASCII comments

**Location:** `crates/asw-cloud/src/hetzner.rs:278`

**Evidence:** `format!("asw-{}", &pubkey_parts[2][..pubkey_parts[2].len().min(16)])` slices the public-key comment at byte offset 16. SSH key comments default to `user@hostname` and routinely contain multi-byte UTF-8 (non-ASCII usernames/hostnames, e.g. `ivan@büro-macbook`); if byte 16 is not a char boundary, this panics with "byte index 16 is not a char boundary", aborting `asw cloud provision`/`build` for that user.

**Suggested fix:** Slice on a char boundary, e.g. `pubkey_parts[2].chars().take(16).collect::<String>()`, or use `s.get(..16).unwrap_or(s)` / `floor_char_boundary`-style logic.

### 16. [MEDIUM/quality] cloud build embeds the compile-time workspace path, so distributed release binaries can never run it

**Location:** `crates/asw-cli/src/main.rs:193`

**Evidence:** `rust_src_dir()` uses `env!("CARGO_MANIFEST_DIR")` — a compile-time constant. CLAUDE.md says CI publishes binary releases on version tags; in those binaries this resolves to the GitHub runner's checkout path (e.g. `/home/runner/work/auto-sea-way/...`), so any user of a released binary who runs `asw cloud build` gets "Rust project not found at /home/runner/..." (pipeline.rs:180) — a confusing, unfixable error, since there is no flag to point at a real source checkout. Additionally, `step_upload_src` runs `git archive ... HEAD` (pipeline.rs:190-199), so even from a checkout it silently builds the last commit, not the working tree — a user testing an uncommitted fix gets a build without it, with no warning.

**Suggested fix:** Resolve the source dir at runtime: default to the current working directory if it contains the workspace `Cargo.toml`, and add a `--src` flag; fall back to `CARGO_MANIFEST_DIR` only for dev builds. Also warn (via `git status --porcelain`) when the working tree is dirty so users know uncommitted changes are excluded from the remote build.

## Unverified (verification cap)

- [medium] Serve startup runs union-find over 302M edges and retains a 160 MB component_labels Vec that is provably constant — the build pipeline already prunes to the main component (`crates/asw-serve/src/state.rs:69`)
  - Evidence: let component_labels = graph.component_labels(); let main_component = { let mut comp_sizes = std::collections::HashMap::new(); for &root in &component_labels { *comp_sizes.entry(root).or_insert(0usize) += 1; } ... }  — crates/asw-build/src/pipeline.rs:97-158 already prunes every graph to the single largest connected component before serialization ("Pruning {} nodes in {} small components"), so for any graph produced by the current builder every node has the same label. AppState::new nevertheless re-runs union-find over all ~302M directed edges (random-access finds over a 160 MB parent array — on the order of a minute of single-threaded startup work, delaying /ready), then iterates 40M HashMap inserts to count sizes for a map that ends up with one entry, and keeps the 160 MB Vec resident for the process lifetime purely to answer `component_labels[node] == main_component`, which is always true. This is ~5% of the documented 3.5 GiB RSS spent on a constant.
  - Suggested fix: Persist a component-pruned flag (or component count) in the graph header at build time; when the graph is single-component, skip component_labels entirely and make the nearest_node membership check a no-op. (Tracked in project memory as a pending v0.5.0 task — worth doing before the next planet deploy since it cuts both startup time and steady-state RSS.)

## Refuted findings (checked and dismissed)

These were reported by reviewers but did not survive adversarial verification —
listed so they are not re-reported by future reviews without new evidence.

- **Graph load decompresses ~2 GB into an unsized Vec via read_to_end, then double-buffers it during deserialize** (`crates/asw-core/src/graph.rs:168`)
  - Refutation: The finding quotes graph.rs:167-170 accurately, but every quantified cost claim collapses under scrutiny. (1) The doubling-growth memcpy is ~1x payload (~1.8 GB, ~0.2 s at DRAM bandwidth) against a documented 60-90 s load time dominated by zstd decompression and 40M-element validation — under 0.5%, and on the production platform (Linux glibc, Docker/Hetzner) large-allocation reallocs use mremap, […]
- **A* open-set BinaryHeap is allocated fresh on every request, escaping the buffer pool** (`crates/asw-core/src/routing.rs:34`)
  - Refutation: The finding's code reading is correct but its "high severity" claim collapses under quantification, and the proposed fix is actively risky for this deployment.

1. The pool was built to eliminate a different, three-orders-of-magnitude-larger allocation. AstarBuffers (crates/asw-core/src/astar_pool.rs:2-6) holds g_score: Vec<f32>, came_from: Vec<u32>, closed: Vec<bool> — 9 bytes per node, all […]
- **Any pipeline step failure leaks a running paid Hetzner server (no teardown on error path)** (`crates/asw-cloud/src/pipeline.rs:88`)
  - Refutation: The finding's code citations are factually accurate: pipeline.rs:88 propagates errors before the teardown step, main.rs:361 adds no handling, and hetzner.rs:340-354 can return Err after create_server succeeds. However, server persistence on failure is the deliberate design, not a leak. The pipeline's check_cache (pipeline.rs:103-123) probes the remote server for per-step completion artifacts so a […]
- **Pool overflow allocates a full ~360 MB buffer set per extra concurrent request and never shrinks** (`crates/asw-core/src/astar_pool.rs:42`)
  - Refutation: The finding correctly reads acquire()/release() in crates/asw-core/src/astar_pool.rs (overflow allocates a fresh full-size buffer set; release pushes back unconditionally, so the pool never shrinks), but its failure scenario is impossible in this codebase. Buffers are only held inside AppState::with_astar_buffers (crates/asw-serve/src/state.rs:238-246), whose body has no await point: acquire, the […]
- **Coastline extraction copies all land-polygon ring data 4-5 times (GBs of transient memory and memcpy at planet scale)** (`crates/asw-build/src/coastline.rs:19`)
  - Refutation: The cited clones all exist exactly as claimed (geo_index.rs:104-106 polygons() deep-clone, coastline.rs:19 pointless exterior().clone(), subdivide_ring's coord collect before the <=256 fast path at coastline.rs:50-52, and the segment clone at coastline.rs:37), so the code reading is accurate. However, the impact assessment is inflated. Copies (b) and (c) are per-rayon-task transients bounded by […]
- **Single-threaded sort of the full ~300M-element edge list for deduplication** (`crates/asw-build/src/edges.rs:150`)
  - Refutation: The finding is factually accurate but immaterial. Accurate parts: crates/asw-build/src/edges.rs:150 does run a single-threaded `sort_unstable_by_key` over the combined edge vector between two rayon-parallel phases; the README confirms planet scale is 302,483,040 final edges (~40M nodes), so the pre-dedup sort input is ~300M+ 12-byte tuples (~3.6 GB); rayon is already imported, so […]
- **Full CSR graph is built, then discarded and rebuilt from scratch for component pruning** (`crates/asw-build/src/pipeline.rs:98`)
  - Refutation: The finding's code reading is accurate: pipeline.rs builds the CSR (line 91), computes component_labels, re-decodes all edges via neighbors(), and builds a second CSR when pruning triggers — and pruning does trigger on planet builds (README:172). However, the severity is unjustified. (1) Scale is overstated: the planet graph has 302,483,040 directed edges (README:170; add_edge pushes both […]
- **node_positions HashMap causes ~600M+ hash lookups and >1 GiB overhead in land-crossing pass** (`crates/asw-build/src/edges.rs:164`)
  - Refutation: The finding's code description is accurate (edges.rs:164-177 builds a HashMap<u32,(f64,f64)> and does two hashed lookups per edge) and node IDs are verifiably dense 0..n (cells.rs:104, 504-510), so the proposed Vec fix would work. However, the claimed cost is negligible in context: (1) each filter iteration also calls water.is_water (geo_index.rs:63-72), an R-tree query over ~860K land polygons […]
