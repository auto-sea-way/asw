use anyhow::Result;
use asw_core::coast::CoastlineIndex;
use asw_core::h3::{cell_center, neighbors};
use asw_core::{H3_RES_BASE, H3_RES_LEAF};
use h3o::{CellIndex, Resolution};
use rayon::prelude::*;
use std::collections::HashMap;
use tracing::info;

/// An edge: (source_node_id, target_node_id). Length is recomputed at query time.
pub type Edge = (u32, u32);

/// Build all edges: same-resolution + cross-resolution, with land-crossing removal.
pub fn build_edges(
    cells: &HashMap<CellIndex, u32>,
    coastline: &CoastlineIndex<'_>,
) -> Result<Vec<Edge>> {
    let cell_list: Vec<(CellIndex, u32)> = cells.iter().map(|(&c, &id)| (c, id)).collect();

    // Step 1: Same-resolution edges (parallel)
    info!("Building same-resolution edges...");
    let pb = crate::cells::make_progress(cell_list.len(), "same-res edges");

    let same_res_edges: Vec<Edge> = cell_list
        .par_iter()
        .flat_map(|&(cell, src_id)| {
            pb.inc(1);
            let cell_res = cell.resolution();
            let mut edges = Vec::new();

            for neighbor in neighbors(cell) {
                if neighbor.resolution() == cell_res {
                    if let Some(&dst_id) = cells.get(&neighbor) {
                        if src_id < dst_id {
                            edges.push((src_id, dst_id));
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
        .map(|(c, _)| u8::from(c.resolution()))
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
        let fine_resolution = Resolution::try_from(*fine_res).expect("invalid fine resolution");

        let fine_cells: Vec<(CellIndex, u32)> = cell_list
            .iter()
            .filter(|(c, _)| c.resolution() == fine_resolution)
            .copied()
            .collect();

        if fine_cells.is_empty() {
            continue;
        }

        info!(
            "Building cross-resolution edges: res-{} ↔ res-{}...",
            fine_res, coarse_res
        );
        let pb = crate::cells::make_progress(
            fine_cells.len(),
            &format!("cross-res {}-{}", fine_res, coarse_res),
        );

        let cross_edges: Vec<Edge> = fine_cells
            .par_iter()
            .flat_map(|&(cell, src_id)| {
                pb.inc(1);
                let mut edges = Vec::new();

                if let Some(parent_cell) = cell.parent(coarse_resolution) {
                    // Connect to the parent itself and its neighbors, where
                    // they exist in our set at coarse resolution.
                    for target in neighbors(parent_cell)
                        .into_iter()
                        .chain(std::iter::once(parent_cell))
                    {
                        if let Some(&dst_id) = cells.get(&target) {
                            if target.resolution() == coarse_resolution {
                                let (a, b) = if src_id < dst_id {
                                    (src_id, dst_id)
                                } else {
                                    (dst_id, src_id)
                                };
                                edges.push((a, b));
                            }
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

    // Step 3: Land crossing removal (parallel). The segment between the two
    // cell centres must not cross the coastline. This is the same test the
    // router applies when it smooths a path, so a graph edge is never
    // reported as a land leg.
    info!("Removing land-crossing edges...");
    let total = all_edges.len();
    let pb = crate::cells::make_progress(total, "land check");

    let node_positions: HashMap<u32, (f64, f64)> = cells
        .iter()
        .map(|(&cell, &id)| {
            let (lat, lon) = cell_center(cell);
            (id, (lat, lon))
        })
        .collect();

    let valid_edges: Vec<Edge> = all_edges
        .par_iter()
        .filter_map(|&(src, dst)| {
            pb.inc(1);
            let (lat1, lon1) = node_positions[&src];
            let (lat2, lon2) = node_positions[&dst];
            (!coastline.crosses_land(lon1, lat1, lon2, lat2)).then_some((src, dst))
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

#[cfg(test)]
mod tests {
    use super::*;
    use asw_core::coast::CoastlineSections;

    #[test]
    fn edge_across_a_thin_wall_is_removed() {
        let a = h3o::LatLng::new(36.5, 28.0)
            .unwrap()
            .to_cell(Resolution::Ten);
        let b = neighbors(a)[0];
        let cells = HashMap::from([(a, 0), (b, 1)]);
        let ((alat, alon), (blat, blon)) = (cell_center(a), cell_center(b));
        // A wall through the midpoint, at right angles to the edge.
        let (mlat, mlon) = ((alat + blat) / 2.0, (alon + blon) / 2.0);
        let (dlat, dlon) = (blat - alat, blon - alon);
        let wall = vec![(mlon - dlat, mlat + dlon), (mlon + dlat, mlat - dlon)];
        let far = vec![(29.0, 37.0), (29.1, 37.0)];

        let open = CoastlineSections::from_runs(&[far]);
        assert_eq!(build_edges(&cells, &open.index()).unwrap().len(), 1);
        let walled = CoastlineSections::from_runs(&[wall]);
        assert!(build_edges(&cells, &walled.index()).unwrap().is_empty());
    }
}
