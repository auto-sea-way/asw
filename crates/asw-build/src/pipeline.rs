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

    // Step 2: Extract coastline from post-subtraction land (includes canal waterway boundaries)
    info!("Extracting coastline segments...");
    let land_polygons = land.polygons();
    let (coastline_segments, mut coastline_coords) =
        crate::coastline::extract_coastline(&land_polygons);
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
            .max_by_key(|(_, count)| **count)
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

    // Serialize
    info!("Saving graph to {:?}...", output_path);
    let file = std::fs::File::create(output_path).context("Failed to create output file")?;
    let writer = std::io::BufWriter::new(file);
    graph.save(writer)?;

    let file_size = std::fs::metadata(output_path)?.len();
    info!("Graph saved: {} MB", file_size / 1_000_000);

    Ok(())
}
