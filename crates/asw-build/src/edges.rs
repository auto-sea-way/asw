use anyhow::Result;
use asw_core::geo_index::LandIndex;
use asw_core::h3::{cell_center, haversine_km, neighbors, parent, resolution};
use asw_core::{H3_RES_BASE, H3_RES_LEAF};
use h3o::{CellIndex, Resolution};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::HashMap;
use tracing::info;

/// An edge: (source_node_id, target_node_id, cost_km)
pub type Edge = (u32, u32, f32);

/// Build all edges: same-resolution + cross-resolution, with land-crossing removal.
pub fn build_edges(cells: &HashMap<CellIndex, u32>, water: &LandIndex) -> Result<Vec<Edge>> {
    let cell_list: Vec<(CellIndex, u32)> = cells.iter().map(|(&c, &id)| (c, id)).collect();

    // Step 1: Same-resolution edges (parallel)
    info!("Building same-resolution edges...");
    let pb = ProgressBar::new(cell_list.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} same-res edges")
            .unwrap(),
    );

    let same_res_edges: Vec<Edge> = cell_list
        .par_iter()
        .flat_map(|&(cell, src_id)| {
            pb.inc(1);
            let cell_res = resolution(cell);
            let (src_lat, src_lon) = cell_center(cell);
            let mut edges = Vec::new();

            for neighbor in neighbors(cell) {
                if resolution(neighbor) == cell_res {
                    if let Some(&dst_id) = cells.get(&neighbor) {
                        if src_id < dst_id {
                            let (dst_lat, dst_lon) = cell_center(neighbor);
                            let cost = haversine_km(src_lat, src_lon, dst_lat, dst_lon) as f32;
                            edges.push((src_id, dst_id, cost));
                        }
                    }
                }
            }
            edges
        })
        .collect();
    pb.finish_and_clear();
    info!("{} same-resolution edges", same_res_edges.len());

    // Step 2: Cross-resolution edges for each adjacent pair: (fine, coarse)
    // Derive max resolution from actual cells (may exceed H3_RES_LEAF due to corridor cells)
    let max_res = cell_list
        .iter()
        .map(|(c, _)| resolution(*c))
        .max()
        .unwrap_or(H3_RES_LEAF);
    let cross_res_pairs: Vec<(u8, u8)> = (H3_RES_BASE..max_res)
        .rev()
        .map(|coarse| (coarse + 1, coarse))
        .collect();

    let mut all_cross_edges: Vec<Edge> = Vec::new();

    for (fine_res, coarse_res) in &cross_res_pairs {
        let coarse_resolution =
            Resolution::try_from(*coarse_res).expect("invalid coarse resolution");

        let fine_cells: Vec<(CellIndex, u32)> = cell_list
            .iter()
            .filter(|(c, _)| resolution(*c) == *fine_res)
            .copied()
            .collect();

        if fine_cells.is_empty() {
            continue;
        }

        info!(
            "Building cross-resolution edges: res-{} ↔ res-{}...",
            fine_res, coarse_res
        );
        let pb = ProgressBar::new(fine_cells.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(&format!(
                    "[{{elapsed_precise}}] {{bar:40}} {{pos}}/{{len}} cross-res {}-{}",
                    fine_res, coarse_res
                ))
                .unwrap(),
        );

        let cross_edges: Vec<Edge> = fine_cells
            .par_iter()
            .flat_map(|&(cell, src_id)| {
                pb.inc(1);
                let mut edges = Vec::new();
                let (src_lat, src_lon) = cell_center(cell);

                if let Some(parent_cell) = parent(cell, coarse_resolution) {
                    // Connect to parent's neighbors if they exist at coarse resolution
                    for parent_neighbor in neighbors(parent_cell) {
                        if let Some(&dst_id) = cells.get(&parent_neighbor) {
                            if resolution(parent_neighbor) == *coarse_res {
                                let (dst_lat, dst_lon) = cell_center(parent_neighbor);
                                let cost = haversine_km(src_lat, src_lon, dst_lat, dst_lon) as f32;
                                let (a, b) = if src_id < dst_id {
                                    (src_id, dst_id)
                                } else {
                                    (dst_id, src_id)
                                };
                                edges.push((a, b, cost));
                            }
                        }
                    }

                    // Connect to the parent itself if it exists in our set
                    if let Some(&dst_id) = cells.get(&parent_cell) {
                        if resolution(parent_cell) == *coarse_res {
                            let (dst_lat, dst_lon) = cell_center(parent_cell);
                            let cost = haversine_km(src_lat, src_lon, dst_lat, dst_lon) as f32;
                            let (a, b) = if src_id < dst_id {
                                (src_id, dst_id)
                            } else {
                                (dst_id, src_id)
                            };
                            edges.push((a, b, cost));
                        }
                    }
                }
                edges
            })
            .collect();
        pb.finish_and_clear();
        info!(
            "{} cross-resolution edges (res-{} ↔ res-{})",
            cross_edges.len(),
            fine_res,
            coarse_res
        );

        all_cross_edges.extend(cross_edges);
    }

    // Combine and deduplicate
    let mut all_edges = same_res_edges;
    all_edges.extend(all_cross_edges);

    all_edges.sort_unstable_by_key(|e| (e.0, e.1));
    all_edges.dedup_by_key(|e| (e.0, e.1));
    info!("{} edges after deduplication", all_edges.len());

    // Step 3: Land crossing removal (parallel)
    info!("Removing land-crossing edges...");
    let total = all_edges.len();
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40} {pos}/{len} land check")
            .unwrap(),
    );

    let node_positions: HashMap<u32, (f64, f64)> = cells
        .iter()
        .map(|(&cell, &id)| {
            let (lat, lon) = cell_center(cell);
            (id, (lat, lon))
        })
        .collect();

    let valid_edges: Vec<Edge> = all_edges
        .par_iter()
        .filter_map(|&(src, dst, cost)| {
            pb.inc(1);
            let (lat1, lon1) = node_positions[&src];
            let (lat2, lon2) = node_positions[&dst];
            let mid_lat = (lat1 + lat2) / 2.0;
            let mid_lon = (lon1 + lon2) / 2.0;
            if water.is_water(mid_lon, mid_lat) {
                Some((src, dst, cost))
            } else {
                None
            }
        })
        .collect();
    pb.finish_and_clear();

    let removed = total - valid_edges.len();
    info!(
        "{} valid edges ({} removed as land-crossing)",
        valid_edges.len(),
        removed
    );

    Ok(valid_edges)
}
