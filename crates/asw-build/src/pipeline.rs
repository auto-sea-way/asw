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
    let land = crate::shapefile::load_land_polygons(shp_path, None)?;
    info!("Land index: {} polygons", land.polygon_count());

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

    // Add all nodes sorted by their assigned ID
    let mut sorted_cells: Vec<(CellIndex, u32)> = cells.iter().map(|(&c, &id)| (c, id)).collect();
    sorted_cells.sort_by_key(|&(_, id)| id);

    for (cell, _) in &sorted_cells {
        let (lat, lon) = cell_center(*cell);
        builder.add_node_with_cell(lat as f32, lon as f32, u64::from(*cell));
    }

    // Add edges
    for &(src, dst, cost) in &edges {
        builder.add_edge(src, dst, cost);
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
