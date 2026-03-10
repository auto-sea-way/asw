use anyhow::{Context, Result};
use h3o::CellIndex;
use asw_core::geo_index::CoastlineIndex;
use asw_core::graph::GraphBuilder;
use asw_core::h3::{cell_center, haversine_km};
use asw_core::passages::PASSAGES;
use asw_core::PASSAGE_SNAP_KM;
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

use crate::shapefile::Bbox;

/// Run the full build pipeline.
pub fn run(shp_path: &Path, bbox: Option<Bbox>, output_path: &Path) -> Result<()> {
    // Step 1: Load land polygons (inverted: not-in-land = water)
    let water = crate::shapefile::load_land_polygons(shp_path, None)?;
    info!("Land index: {} polygons", water.polygon_count());

    // Step 2: Extract coastline (needed for coastal detection in step 3)
    info!("Extracting coastline segments...");
    let raw_polygons = crate::shapefile::load_raw_polygons(shp_path, None)?;
    let (coastline_segments, mut coastline_coords) = crate::coastline::extract_coastline(&raw_polygons);
    let coastline_index = CoastlineIndex::new(coastline_segments);
    info!("Coastline: {} segments", coastline_index.segment_count());

    // Clip stored coastline coords to bbox (for KML export)
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

    // Step 3: Generate cells (uses coastline for coastal detection)
    let cells = crate::cells::generate_cells(&water, &coastline_index, bbox)?;
    info!("Generated {} navigable cells", cells.len());

    // Step 4: Build edges
    let edges = crate::edges::build_edges(&cells, &water)?;
    info!("Built {} edges", edges.len());

    // Step 5: Build graph
    let mut builder = GraphBuilder::new();

    // Add all nodes sorted by their assigned ID
    let mut sorted_cells: Vec<(CellIndex, u32)> =
        cells.iter().map(|(&c, &id)| (c, id)).collect();
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

    // Step 6: Add critical passages
    info!("Adding critical passages...");
    add_passages(&mut builder, &cells)?;

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

fn add_passages(
    builder: &mut GraphBuilder,
    cells: &HashMap<CellIndex, u32>,
) -> Result<()> {
    for passage in PASSAGES {
        let mut prev_node: Option<u32> = None;

        for &(lat, lon) in passage.waypoints {
            // Find nearest existing node
            let mut best_id = None;
            let mut best_dist = f64::MAX;

            for (&cell, &id) in cells {
                let (clat, clon) = cell_center(cell);
                let d = haversine_km(lat, lon, clat, clon);
                if d < best_dist {
                    best_dist = d;
                    best_id = Some(id);
                }
            }

            let node_id = if best_dist > PASSAGE_SNAP_KM || best_id.is_none() {
                let id = builder.add_node(lat as f32, lon as f32);
                info!(
                    "  {} — synthetic node at ({:.4}, {:.4}), nearest was {:.1}km away",
                    passage.name, lat, lon, best_dist
                );
                id
            } else {
                best_id.unwrap()
            };

            if let Some(prev) = prev_node {
                if prev != node_id {
                    let (plat, plon) = (
                        builder.node_lats[prev as usize] as f64,
                        builder.node_lngs[prev as usize] as f64,
                    );
                    let cost = haversine_km(lat, lon, plat, plon) as f32;
                    builder.add_edge(prev, node_id, cost);
                }
            }

            prev_node = Some(node_id);
        }

        info!("  Added passage: {}", passage.name);
    }

    Ok(())
}
