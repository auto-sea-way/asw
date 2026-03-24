use anyhow::{Context, Result};
use asw_core::geo_index::CoastlineIndex;
use asw_core::graph::GraphBuilder;
use asw_core::h3::cell_center;
use asw_core::passages::PASSAGES;
use h3o::CellIndex;
use std::path::Path;
use tracing::info;

use crate::shapefile::Bbox;

/// Run the full build pipeline.
pub fn run(shp_path: &Path, bbox: Option<Bbox>, output_path: &Path) -> Result<()> {
    // Step 1: Load land polygons
    let mut land = crate::shapefile::load_land_polygons(shp_path, None)?;
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

    // Step 2: Extract coastline (needed for coastal detection in step 3)
    info!("Extracting coastline segments...");
    let raw_polygons = crate::shapefile::load_raw_polygons(shp_path, None)?;
    let (coastline_segments, mut coastline_coords) =
        crate::coastline::extract_coastline(&raw_polygons);
    let coastline_index = CoastlineIndex::new(coastline_segments);
    info!("Coastline: {} segments", coastline_index.segment_count());

    // Clip stored coastline coords to bbox (for GeoJSON export)
    if let Some((min_lon, min_lat, max_lon, max_lat)) = bbox {
        let before = coastline_coords.len();
        coastline_coords.retain(|seg| {
            seg.iter().any(|&(lon, lat)| {
                (lon as f64) >= min_lon
                    && (lon as f64) <= max_lon
                    && (lat as f64) >= min_lat
                    && (lat as f64) <= max_lat
            })
        });
        info!(
            "Clipped coastline to bbox: {} → {} segments",
            before,
            coastline_coords.len()
        );
    }

    // Step 3: Generate cells (main cascade res-3 through res-10, extended in passage zones)
    let cells = crate::cells::generate_cells(&land, &coastline_index, bbox, PASSAGES)?;
    info!("Generated {} navigable cells", cells.len());

    // Step 5: Build edges (auto-detects max resolution from cells)
    let edges = crate::edges::build_edges(&cells, &land)?;
    info!("Built {} edges", edges.len());

    // Step 6: Build graph
    let mut builder = GraphBuilder::new();

    // Sort cells by H3 index for spatial ordering (better compression)
    let mut sorted_cells: Vec<(CellIndex, u32)> = cells.iter().map(|(&c, &id)| (c, id)).collect();
    sorted_cells.sort_by_key(|(cell, _)| u64::from(*cell));

    // Build node ID remapping: old_id -> new_id
    let mut id_remap = vec![0u32; sorted_cells.len()];
    for (cell, old_id) in &sorted_cells {
        let (lat, lng) = cell_center(*cell);
        let new_id = builder.add_node(u64::from(*cell), lat, lng);
        id_remap[*old_id as usize] = new_id;
    }

    // Add edges with remapped IDs
    for &(src, dst, cost) in &edges {
        builder.add_edge(id_remap[src as usize], id_remap[dst as usize], cost);
    }

    // Store coastline
    builder.coastline_coords = coastline_coords;

    // Step 7: Build and validate
    let graph = builder.build();
    info!(
        "Graph: {} nodes, {} edges",
        graph.num_nodes, graph.num_edges
    );

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

    // Serialize
    info!("Saving graph to {:?}...", output_path);
    let file = std::fs::File::create(output_path).context("Failed to create output file")?;
    let writer = std::io::BufWriter::new(file);
    graph.save(writer)?;

    let file_size = std::fs::metadata(output_path)?.len();
    info!("Graph saved: {} MB", file_size / 1_000_000);

    Ok(())
}
