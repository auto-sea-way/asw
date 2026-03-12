use anyhow::Result;
use asw_core::geo_index::{CoastlineIndex, LandIndex};
use asw_core::h3::{cell_boundary, cell_center, cell_polygon, children};
use asw_core::passages::Passage;
use asw_core::{CASCADE, H3_RES_BASE, H3_RES_LEAF};
use geo::LineString;
use h3o::geom::{ContainmentMode, TilerBuilder};
use h3o::{CellIndex, Resolution};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::HashMap;
use tracing::info;

use crate::shapefile::Bbox;

/// Resolution tier names for logging.
const TIER_NAMES: &[&str] = &[
    "",
    "",
    "",
    "ocean",
    "deep-mid",
    "mid",
    "near-mid",
    "coastal",
    "near-coast",
    "near-shore",
    "shoreline",
    "passage-11",
    "passage-12",
    "passage-13",
];

fn tier_name(res: u8) -> &'static str {
    TIER_NAMES.get(res as usize).copied().unwrap_or("unknown")
}

/// Generate all navigable H3 cells (res-3 through res-9 adaptive cascade),
/// with extended cascade into passage zones at higher resolutions.
/// Returns a map of CellIndex → node_id.
pub fn generate_cells(
    water: &LandIndex,
    coastline: &CoastlineIndex,
    bbox: Option<Bbox>,
    passages: &[Passage],
) -> Result<HashMap<CellIndex, u32>> {
    let base_res = Resolution::try_from(H3_RES_BASE).expect("invalid base resolution");

    // Step 1: Generate base (res-3) covering cells
    let base_cells = generate_covering_cells(bbox, base_res)?;
    info!(
        "Generated {} res-{} covering cells",
        base_cells.len(),
        H3_RES_BASE
    );

    // Step 2: Filter to water-only (parallel)
    let pb = make_progress(
        base_cells.len(),
        &format!("filtering res-{} to water", H3_RES_BASE),
    );
    let mut current_water: Vec<CellIndex> = base_cells
        .par_iter()
        .filter_map(|&cell| {
            pb.inc(1);
            if !water.contains_polygon(&cell_polygon(cell)) {
                Some(cell)
            } else {
                None
            }
        })
        .collect();
    pb.finish_and_clear();
    info!(
        "{} res-{} cells are in water",
        current_water.len(),
        H3_RES_BASE
    );

    // Step 3: Cascade — for each tier, classify keep/refine, then expand refinements
    let mut cell_map = HashMap::new();
    let mut node_id = 0u32;
    let mut tier_counts: Vec<(u8, usize)> = Vec::new();

    for &(res, threshold) in CASCADE {
        let next_res = res + 1;
        let next_resolution = Resolution::try_from(next_res).expect("invalid next resolution");

        // Classify: pure water + far from coast → keep; everything else → refine
        let pb = make_progress(
            current_water.len(),
            &format!("classifying res-{} cells", res),
        );
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
            keep.len(),
            res,
            tier_name(res),
            refine.len(),
            next_res
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
                if water.contains_polygon(&parent_poly) {
                    return Vec::new();
                }
                if !water.intersects_polygon(&parent_poly) {
                    return children(parent_cell, next_resolution);
                }
                children(parent_cell, next_resolution)
                    .into_iter()
                    .filter(|&child| !water.contains_polygon(&cell_polygon(child)))
                    .collect::<Vec<_>>()
            })
            .collect();
        pb.finish_and_clear();
        info!(
            "{} res-{} water cells generated",
            current_water.len(),
            next_res
        );
    }

    // Step 4: Split leaf candidates into normal vs zone candidates
    let zone_lookup = build_zone_lookup(passages, bbox)?;

    let (zone_candidates, normal_candidates): (Vec<(CellIndex, u8)>, Vec<CellIndex>) =
        if zone_lookup.is_empty() {
            (Vec::new(), current_water)
        } else {
            let zone_res = Resolution::try_from(passages[0].zone_resolution)
                .expect("invalid zone resolution");
            let mut zone = Vec::new();
            let mut normal = Vec::new();
            for &cell in &current_water {
                let ancestor = cell.parent(zone_res);
                if let Some(ancestor) = ancestor {
                    if let Some(&leaf_res) = zone_lookup.get(&ancestor) {
                        zone.push((cell, leaf_res));
                    } else {
                        normal.push(cell);
                    }
                } else {
                    normal.push(cell);
                }
            }
            (zone, normal)
        };

    if !zone_candidates.is_empty() {
        info!(
            "{} res-{} cells in passage zones, {} normal",
            zone_candidates.len(),
            H3_RES_LEAF,
            normal_candidates.len()
        );
    }

    // Step 4a: Normal leaf cells — group by parent for hierarchical elimination
    let leaf_res_minus_1 = Resolution::try_from(H3_RES_LEAF - 1).expect("invalid resolution");
    let mut by_parent: HashMap<CellIndex, Vec<CellIndex>> =
        HashMap::with_capacity(normal_candidates.len() / 7);
    for &cell in &normal_candidates {
        let parent = cell.parent(leaf_res_minus_1).expect("parent");
        by_parent.entry(parent).or_default().push(cell);
    }
    let pb = make_progress(
        normal_candidates.len(),
        &format!("filtering res-{} leaf cells", H3_RES_LEAF),
    );
    let pure_leaves: Vec<CellIndex> = by_parent
        .into_par_iter()
        .flat_map(|(parent, leaf_cells)| {
            let parent_poly = cell_polygon(parent);
            if water.contains_polygon(&parent_poly) {
                pb.inc(leaf_cells.len() as u64);
                return Vec::new();
            }
            if !water.intersects_polygon(&parent_poly) {
                pb.inc(leaf_cells.len() as u64);
                return leaf_cells;
            }
            leaf_cells
                .iter()
                .filter(|&&cell| {
                    pb.inc(1);
                    !water.intersects_polygon(&cell_polygon(cell))
                })
                .copied()
                .collect()
        })
        .collect();
    pb.finish_and_clear();
    info!(
        "{} res-{} pure water leaf cells (from {} candidates)",
        pure_leaves.len(),
        H3_RES_LEAF,
        normal_candidates.len()
    );

    let before = cell_map.len();
    for cell in pure_leaves {
        insert_cell(&mut cell_map, &mut node_id, cell);
    }
    tier_counts.push((H3_RES_LEAF, cell_map.len() - before));

    // Step 4b: Zone cascade extension — refine zone candidates beyond leaf resolution
    if !zone_candidates.is_empty() {
        // Group zone candidates by leaf_resolution
        let mut by_leaf_res: HashMap<u8, Vec<CellIndex>> = HashMap::new();
        for (cell, leaf_res) in zone_candidates {
            by_leaf_res.entry(leaf_res).or_default().push(cell);
        }

        for (leaf_res, mut current) in by_leaf_res {
            info!(
                "Zone cascade: {} res-{} cells → refine to res-{}",
                current.len(),
                H3_RES_LEAF,
                leaf_res
            );

            // First, filter the res-9 zone candidates through the same leaf filter
            // (pure water check with parent grouping)
            let leaf_res_minus_1 =
                Resolution::try_from(H3_RES_LEAF - 1).expect("invalid resolution");
            let mut by_parent: HashMap<CellIndex, Vec<CellIndex>> =
                HashMap::with_capacity(current.len() / 7);
            for &cell in &current {
                let parent = cell.parent(leaf_res_minus_1).expect("parent");
                by_parent.entry(parent).or_default().push(cell);
            }

            let pb = make_progress(
                current.len(),
                &format!("zone filter res-{}", H3_RES_LEAF),
            );

            // Split into pure water (keep at res-9) and land-intersecting (refine further)
            let (pure, straddle): (Vec<CellIndex>, Vec<CellIndex>) = by_parent
                .into_par_iter()
                .flat_map(|(parent, leaf_cells)| {
                    let parent_poly = cell_polygon(parent);
                    if water.contains_polygon(&parent_poly) {
                        pb.inc(leaf_cells.len() as u64);
                        // All land — neither keep nor refine
                        return leaf_cells
                            .into_iter()
                            .map(|_| None)
                            .collect::<Vec<_>>();
                    }
                    if !water.intersects_polygon(&parent_poly) {
                        pb.inc(leaf_cells.len() as u64);
                        // All water — keep at this resolution
                        return leaf_cells
                            .into_iter()
                            .map(|c| Some((c, true)))
                            .collect::<Vec<_>>();
                    }
                    // Mixed — test individually
                    leaf_cells
                        .into_iter()
                        .map(|cell| {
                            pb.inc(1);
                            let poly = cell_polygon(cell);
                            if !water.intersects_polygon(&poly) {
                                Some((cell, true)) // pure water
                            } else if !water.contains_polygon(&poly) {
                                Some((cell, false)) // not fully land → refine
                            } else {
                                None // all land
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .flatten()
                .partition_map(|(cell, is_pure)| {
                    if is_pure {
                        rayon::iter::Either::Left(cell)
                    } else {
                        rayon::iter::Either::Right(cell)
                    }
                });
            pb.finish_and_clear();

            // Add pure water cells at res-9
            let before = cell_map.len();
            for cell in &pure {
                insert_cell(&mut cell_map, &mut node_id, *cell);
            }
            let added_at_9 = cell_map.len() - before;
            if added_at_9 > 0 {
                info!(
                    "{} zone cells kept at res-{} (pure water)",
                    added_at_9, H3_RES_LEAF
                );
            }

            // Refine straddle cells through resolutions 10..leaf_res
            current = straddle;

            for res in H3_RES_LEAF..leaf_res {
                let next_res = res + 1;
                let next_resolution =
                    Resolution::try_from(next_res).expect("invalid resolution");

                // Expand to children
                let pb = make_progress(
                    current.len(),
                    &format!("zone refining to res-{}", next_res),
                );
                let expanded: Vec<CellIndex> = current
                    .par_iter()
                    .flat_map(|&parent_cell| {
                        pb.inc(1);
                        let parent_poly = cell_polygon(parent_cell);
                        if water.contains_polygon(&parent_poly) {
                            return Vec::new();
                        }
                        if !water.intersects_polygon(&parent_poly) {
                            return children(parent_cell, next_resolution);
                        }
                        children(parent_cell, next_resolution)
                            .into_iter()
                            .filter(|&child| !water.contains_polygon(&cell_polygon(child)))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                pb.finish_and_clear();

                if next_res < leaf_res {
                    // Intermediate resolution: keep pure water, refine straddle
                    let pb = make_progress(
                        expanded.len(),
                        &format!("zone classifying res-{}", next_res),
                    );
                    let (pure, straddle): (Vec<CellIndex>, Vec<CellIndex>) = expanded
                        .par_iter()
                        .filter_map(|&cell| {
                            pb.inc(1);
                            let poly = cell_polygon(cell);
                            if !water.intersects_polygon(&poly) {
                                Some((cell, true))
                            } else if !water.contains_polygon(&poly) {
                                Some((cell, false))
                            } else {
                                None
                            }
                        })
                        .partition_map(|(cell, is_pure)| {
                            if is_pure {
                                rayon::iter::Either::Left(cell)
                            } else {
                                rayon::iter::Either::Right(cell)
                            }
                        });
                    pb.finish_and_clear();

                    let before = cell_map.len();
                    for cell in &pure {
                        insert_cell(&mut cell_map, &mut node_id, *cell);
                    }
                    let added = cell_map.len() - before;
                    if added > 0 {
                        tier_counts.push((next_res, added));
                        info!(
                            "{} zone cells kept at res-{} ({})",
                            added,
                            next_res,
                            tier_name(next_res)
                        );
                    }

                    current = straddle;
                    info!(
                        "{} zone cells will refine further from res-{}",
                        current.len(),
                        next_res
                    );
                } else {
                    // Leaf resolution: keep cells that are pure water or have water
                    let pb = make_progress(
                        expanded.len(),
                        &format!("zone leaf filter res-{}", next_res),
                    );
                    let pure: Vec<CellIndex> = expanded
                        .par_iter()
                        .filter(|&&cell| {
                            pb.inc(1);
                            !water.intersects_polygon(&cell_polygon(cell))
                        })
                        .copied()
                        .collect();
                    pb.finish_and_clear();

                    let before = cell_map.len();
                    for cell in &pure {
                        insert_cell(&mut cell_map, &mut node_id, *cell);
                    }
                    let added = cell_map.len() - before;
                    tier_counts.push((next_res, added));
                    info!(
                        "{} zone cells at res-{} ({}) from {} candidates",
                        added,
                        next_res,
                        tier_name(next_res),
                        expanded.len()
                    );
                }
            }
        }
    }

    let summary: Vec<String> = tier_counts
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(res, count)| format!("{} from {} tier (res-{})", count, tier_name(*res), res))
        .collect();

    info!("Total: {} nodes ({})", cell_map.len(), summary.join(", "));

    Ok(cell_map)
}

/// Build a lookup from zone cells (at zone_resolution) to leaf_resolution.
/// For each passage overlapping the build bbox, generate covering H3 cells
/// at zone_resolution and map them to the passage's leaf_resolution.
fn build_zone_lookup(
    passages: &[Passage],
    bbox: Option<Bbox>,
) -> Result<HashMap<CellIndex, u8>> {
    let mut lookup = HashMap::new();

    for passage in passages {
        let (p_min_lon, p_min_lat, p_max_lon, p_max_lat) = passage.corridor;

        // Skip if corridor doesn't overlap with build bbox
        if let Some((b_min_lon, b_min_lat, b_max_lon, b_max_lat)) = bbox {
            if p_max_lon < b_min_lon
                || p_min_lon > b_max_lon
                || p_max_lat < b_min_lat
                || p_min_lat > b_max_lat
            {
                continue;
            }
        }

        let zone_res = Resolution::try_from(passage.zone_resolution)
            .expect("invalid zone resolution");
        let zone_cells = generate_covering_cells(Some(passage.corridor), zone_res)?;

        let mut count = 0;
        for cell in zone_cells {
            let entry = lookup.entry(cell).or_insert(0u8);
            // Take max leaf_resolution on conflict
            if passage.leaf_resolution > *entry {
                *entry = passage.leaf_resolution;
            }
            count += 1;
        }

        info!(
            "Passage '{}': {} zone cells at res-{}, leaf res-{}",
            passage.name, count, passage.zone_resolution, passage.leaf_resolution
        );
    }

    Ok(lookup)
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

            let cells: Vec<CellIndex> = tiler.into_annotated_coverage().map(|ac| ac.cell).collect();
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
