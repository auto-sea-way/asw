use anyhow::Result;
use geo::LineString;
use h3o::geom::{ContainmentMode, TilerBuilder};
use h3o::{CellIndex, Resolution};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use asw_core::geo_index::{CoastlineIndex, LandIndex};
use asw_core::h3::{cell_boundary, cell_center, cell_polygon, children};
use asw_core::{CASCADE, H3_RES_BASE, H3_RES_LEAF};
use std::collections::HashMap;
use tracing::info;

use crate::shapefile::Bbox;

/// Resolution tier names for logging.
const TIER_NAMES: &[&str] = &[
    "", "", "", "ocean", "deep-mid", "mid", "near-mid", "coastal", "near-coast", "shoreline",
];

fn tier_name(res: u8) -> &'static str {
    TIER_NAMES.get(res as usize).copied().unwrap_or("unknown")
}

/// Generate all navigable H3 cells (res-3 through res-9 adaptive cascade).
/// Returns a map of CellIndex → node_id.
pub fn generate_cells(
    water: &LandIndex,
    coastline: &CoastlineIndex,
    bbox: Option<Bbox>,
) -> Result<HashMap<CellIndex, u32>> {
    let base_res = Resolution::try_from(H3_RES_BASE).expect("invalid base resolution");

    // Step 1: Generate base (res-3) covering cells
    let base_cells = generate_covering_cells(bbox, base_res)?;
    info!("Generated {} res-{} covering cells", base_cells.len(), H3_RES_BASE);

    // Step 2: Filter to water-only (parallel)
    let pb = make_progress(base_cells.len(), &format!("filtering res-{} to water", H3_RES_BASE));
    let mut current_water: Vec<CellIndex> = base_cells
        .par_iter()
        .filter_map(|&cell| {
            pb.inc(1);
            if cell_has_water(cell, water) { Some(cell) } else { None }
        })
        .collect();
    pb.finish_and_clear();
    info!("{} res-{} cells are in water", current_water.len(), H3_RES_BASE);

    // Step 3: Cascade — for each tier, classify keep/refine, then expand refinements
    // Uses exact polygon intersection: cells that don't intersect land are pure water
    // and kept immediately. Mixed cells (intersecting land) are always refined, even if
    // far from coast, so no separate refinement pass is needed later.
    let mut cell_map = HashMap::new();
    let mut node_id = 0u32;
    let mut tier_counts: Vec<(u8, usize)> = Vec::new();

    for &(res, threshold) in CASCADE {
        let next_res = res + 1;
        let next_resolution = Resolution::try_from(next_res).expect("invalid next resolution");

        // Classify: pure water + far from coast → keep; everything else → refine
        let pb = make_progress(current_water.len(), &format!("classifying res-{} cells", res));
        let (keep, refine): (Vec<CellIndex>, Vec<CellIndex>) = current_water
            .par_iter()
            .map(|&cell| {
                pb.inc(1);
                let is_far = cell_min_coast_dist(cell, coastline, threshold) > threshold;
                let keep = if is_far {
                    // Far from coast — only keep if no land intersects this cell
                    let poly = cell_polygon(cell);
                    !water.intersects_polygon(&poly)
                } else {
                    false // Near coast — always refine for higher resolution
                };
                (cell, keep)
            })
            .partition_map(|(cell, keep)| {
                if keep {
                    rayon::iter::Either::Left(cell)
                } else {
                    rayon::iter::Either::Right(cell)
                }
            });
        pb.finish_and_clear();
        info!(
            "{} res-{} cells kept ({}), {} will refine to res-{}",
            keep.len(), res, tier_name(res), refine.len(), next_res
        );

        // Kept cells are confirmed pure water — add directly to cell_map
        let before = cell_map.len();
        for cell in keep {
            insert_cell(&mut cell_map, &mut node_id, cell);
        }
        tier_counts.push((res, cell_map.len() - before));

        // Refine to next-resolution children with hierarchical elimination
        let pb = make_progress(refine.len(), &format!("refining to res-{}", next_res));
        current_water = refine
            .par_iter()
            .flat_map(|&parent_cell| {
                pb.inc(1);
                let parent_poly = cell_polygon(parent_cell);
                // Fast path: parent entirely on land → skip all children
                if water.contains_polygon(&parent_poly) {
                    return Vec::new();
                }
                // Fast path: parent entirely water → all children are water
                // (H3 children can protrude slightly beyond parent boundary,
                // but the overlap is negligible relative to land polygon resolution)
                if !water.intersects_polygon(&parent_poly) {
                    return children(parent_cell, next_resolution);
                }
                // Slow path: parent straddles coastline → test children individually
                children(parent_cell, next_resolution)
                    .into_iter()
                    .filter(|&child| cell_has_water(child, water))
                    .collect::<Vec<_>>()
            })
            .collect();
        pb.finish_and_clear();
        info!("{} res-{} water cells generated", current_water.len(), next_res);
    }

    // Step 4: Leaf cells — group by parent for hierarchical elimination
    let leaf_res_minus_1 = Resolution::try_from(H3_RES_LEAF - 1).expect("invalid resolution");
    let mut by_parent: HashMap<CellIndex, Vec<CellIndex>> = HashMap::with_capacity(current_water.len() / 7);
    for &cell in &current_water {
        let parent = cell.parent(leaf_res_minus_1).expect("parent");
        by_parent.entry(parent).or_default().push(cell);
    }
    let pb = make_progress(current_water.len(), &format!("filtering res-{} leaf cells", H3_RES_LEAF));
    let pure_leaves: Vec<CellIndex> = by_parent
        .into_par_iter()
        .flat_map(|(parent, leaf_cells)| {
            let parent_poly = cell_polygon(parent);
            // Fast path: parent entirely on land → all children are land
            if water.contains_polygon(&parent_poly) {
                pb.inc(leaf_cells.len() as u64);
                return Vec::new();
            }
            // Fast path: parent entirely water → all children are pure water
            // (H3 children can protrude slightly beyond parent boundary,
            // but the overlap is negligible relative to land polygon resolution)
            if !water.intersects_polygon(&parent_poly) {
                pb.inc(leaf_cells.len() as u64);
                return leaf_cells;
            }
            // Slow path: test children individually
            leaf_cells
                .iter()
                .filter(|&&cell| {
                    pb.inc(1);
                    let poly = cell_polygon(cell);
                    !water.intersects_polygon(&poly)
                })
                .copied()
                .collect()
        })
        .collect();
    pb.finish_and_clear();
    info!(
        "{} res-{} leaf cells are pure water (from {} candidates)",
        pure_leaves.len(), H3_RES_LEAF, current_water.len()
    );

    let before = cell_map.len();
    for cell in pure_leaves {
        insert_cell(&mut cell_map, &mut node_id, cell);
    }
    tier_counts.push((H3_RES_LEAF, cell_map.len() - before));

    let summary: Vec<String> = tier_counts
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(res, count)| format!("{} from {} tier (res-{})", count, tier_name(*res), res))
        .collect();

    info!("Total: {} nodes ({})", cell_map.len(), summary.join(", "));

    Ok(cell_map)
}

/// Check if any part of a cell (center or any vertex) is in water.
fn cell_has_water(cell: CellIndex, water: &LandIndex) -> bool {
    let (lat, lon) = cell_center(cell);
    if water.is_water(lon, lat) {
        return true;
    }
    for (vlat, vlon) in cell_boundary(cell) {
        if water.is_water(vlon, vlat) {
            return true;
        }
    }
    false
}

/// Insert a cell into the map, assigning the next available node ID.
fn insert_cell(cell_map: &mut HashMap<CellIndex, u32>, node_id: &mut u32, cell: CellIndex) {
    cell_map.entry(cell).or_insert_with(|| {
        let id = *node_id;
        *node_id += 1;
        id
    });
}

/// Approximate circumradius (center-to-vertex) in degrees for each H3 resolution.
fn cell_radius_deg(cell: CellIndex) -> f64 {
    match cell.resolution() as u8 {
        0..=3 => 0.6,
        4 => 0.2,
        5 => 0.08,
        6 => 0.03,
        7 => 0.012,
        8 => 0.005,
        _ => 0.002,
    }
}

/// Minimum distance (degrees) from any point of a cell (center + 6 vertices) to the coastline.
/// The search envelope is expanded by the cell's radius so we find coastline segments
/// that are within `threshold_deg` of ANY point in the cell, not just the query point.
fn cell_min_coast_dist(cell: CellIndex, coastline: &CoastlineIndex, threshold_deg: f64) -> f64 {
    let search_radius = threshold_deg + cell_radius_deg(cell);
    let (lat, lon) = cell_center(cell);
    let mut best = coastline.min_distance_deg(lon, lat, search_radius);
    for (vlat, vlon) in cell_boundary(cell) {
        let d = coastline.min_distance_deg(vlon, vlat, search_radius);
        if d < best {
            best = d;
        }
    }
    best
}

fn make_progress(len: usize, label: &str) -> ProgressBar {
    let pb = ProgressBar::new(len as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "[{{elapsed_precise}}] {{bar:40}} {{pos}}/{{len}} {}",
                label
            ))
            .unwrap(),
    );
    pb
}

/// Generate H3 cells covering a bbox (or global) at given resolution.
fn generate_covering_cells(bbox: Option<Bbox>, res: Resolution) -> Result<Vec<CellIndex>> {
    match bbox {
        Some((min_lon, min_lat, max_lon, max_lat)) => {
            let polygon = geo::Polygon::new(
                LineString::from(vec![
                    (min_lon, min_lat),
                    (max_lon, min_lat),
                    (max_lon, max_lat),
                    (min_lon, max_lat),
                    (min_lon, min_lat),
                ]),
                vec![],
            );

            let mut tiler = TilerBuilder::new(res)
                .containment_mode(ContainmentMode::Covers)
                .build();
            tiler.add(polygon)?;

            let cells: Vec<CellIndex> = tiler
                .into_annotated_coverage()
                .map(|ac| ac.cell)
                .collect();
            Ok(cells)
        }
        None => {
            info!("Generating global res-{} covering...", res as u8);
            let cells: Vec<CellIndex> = CellIndex::base_cells()
                .flat_map(|base| base.children(res))
                .collect();
            Ok(cells)
        }
    }
}
