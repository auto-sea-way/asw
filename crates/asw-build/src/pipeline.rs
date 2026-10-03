use anyhow::{Context, Result};
use asw_core::graph::GraphBuilder;
use asw_core::passages::PASSAGES;
use h3o::CellIndex;
use std::path::Path;
use tracing::info;

use crate::shapefile::Bbox;

/// Run the full build pipeline.
pub fn run(shp_path: &Path, bbox: Option<Bbox>, output_path: &Path) -> Result<()> {
    // Step 1: Load land polygons
    let mut land = crate::shapefile::load_land_polygons(shp_path)?;
    info!("Land index: {} polygons", land.polygon_count());

    // Step 1b: Extract canal water and subtract from land
    let work_dir = output_path.parent().unwrap_or(Path::new("."));
    let canal_water = crate::canal_water::extract_canal_water(PASSAGES, bbox, work_dir)?;
    if !canal_water.is_empty() {
        info!(
            "Subtracting {} canal water polygons from land...",
            canal_water.len()
        );
        land.subtract_water(&canal_water);
        info!(
            "Land index after subtraction: {} polygons",
            land.polygon_count()
        );
    }

    // Step 2: Extract coastline from post-subtraction land (includes canal waterway boundaries)
    info!("Extracting coastline segments...");
    let land_polygons = land.polygons();
    let mut coastline_runs = crate::coastline::extract_coastline(&land_polygons);
    let full_sections = asw_core::coast::CoastlineSections::from_runs(&coastline_runs);
    let coastline_index = full_sections.index();
    info!("Coastline: {} runs", coastline_index.run_count());

    // Clip stored coastline coords to bbox (for GeoJSON export)
    if let Some((min_lon, min_lat, max_lon, max_lat)) = bbox {
        let before = coastline_runs.len();
        coastline_runs.retain(|seg| {
            seg.iter().any(|&(lon, lat)| {
                lon >= min_lon && lon <= max_lon && lat >= min_lat && lat <= max_lat
            })
        });
        info!(
            "Clipped coastline to bbox: {} → {} segments",
            before,
            coastline_runs.len()
        );
    }

    // Step 3: Generate cells (main cascade res-3 through res-10, extended in passage zones)
    let mut cells = crate::cells::generate_cells(&land, &coastline_index, bbox, PASSAGES)?;
    info!("Generated {} navigable cells", cells.len());

    // Step 4: Drop the ice cap. The router never enters it, so its cells
    // are dead weight. Ids are renumbered densely: build_edges and the remap
    // below index by them.
    let before = cells.len();
    cells.retain(|&c, _| asw_core::h3::cell_center(c).0 <= asw_core::routing::ICE_CAP_LAT);
    for (i, id) in cells.values_mut().enumerate() {
        *id = i as u32;
    }
    info!(
        "Dropped {} cells north of {} N",
        before - cells.len(),
        asw_core::routing::ICE_CAP_LAT
    );

    // Step 5: Build edges (auto-detects max resolution from cells)
    let edges = crate::edges::build_edges(&cells, &coastline_index)?;
    info!("Built {} edges", edges.len());

    // Step 6: Build graph
    let mut builder = GraphBuilder::with_version(format!(
        "{} {}",
        env!("CARGO_PKG_VERSION"),
        time::OffsetDateTime::now_utc().date()
    ));

    // Sort cells by H3 index for spatial ordering (better compression)
    let mut sorted_cells: Vec<(CellIndex, u32)> = cells.iter().map(|(&c, &id)| (c, id)).collect();
    sorted_cells.sort_by_key(|(cell, _)| u64::from(*cell));

    // Compute per-node distance to shore (straight-line, capped at 5.1 nm)
    let shore_dist = crate::shore::compute_shore_distances(&sorted_cells, &coastline_index);
    info!("Computed shore distances for {} cells", shore_dist.len());

    // Build node ID remapping: old_id -> new_id
    let mut id_remap = vec![0u32; sorted_cells.len()];
    for (i, (cell, old_id)) in sorted_cells.iter().enumerate() {
        let new_id = builder.add_node(u64::from(*cell), shore_dist[i]);
        id_remap[*old_id as usize] = new_id;
    }

    // Add edges with remapped IDs
    for &(src, dst) in &edges {
        builder.add_edge(id_remap[src as usize], id_remap[dst as usize]);
    }

    // Store coastline
    builder.coastline_runs = coastline_runs;

    // Step 7: Prune to the largest connected component, then write the v5 image
    let builder = builder.prune_to_main_component();
    info!("Saving graph to {:?}...", output_path);
    std::fs::write(output_path, builder.build_bytes()).context("Failed to write graph")?;
    // populate = true also runs the CSR table scans: the build's self-check
    // must catch a builder regression here, not on a phone.
    let graph = asw_core::graph::RoutingGraph::open(output_path, true)
        .context("Written graph does not open")?;
    info!(
        "Final graph: {} nodes, {} edges, version {}",
        graph.num_nodes(),
        graph.num_edges(),
        graph.version()
    );

    let file_size = std::fs::metadata(output_path)?.len();
    info!("Graph saved: {} MB", file_size / 1_000_000);

    Ok(())
}
