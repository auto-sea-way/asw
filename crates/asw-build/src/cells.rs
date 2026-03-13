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

/// Check if a cell's center falls within any passage corridor bbox.
fn in_passage_corridor(cell: CellIndex, corridors: &[(f64, f64, f64, f64)]) -> bool {
    if corridors.is_empty() {
        return false;
    }
    let (lat, lon) = cell_center(cell);
    corridors
        .iter()
        .any(|&(min_lon, min_lat, max_lon, max_lat)| {
            lon >= min_lon && lon <= max_lon && lat >= min_lat && lat <= max_lat
        })
}

fn tier_name(res: u8) -> &'static str {
    match res {
        3 => "ocean",
        4 => "deep-mid",
        5 => "mid",
        6 => "near-mid",
        7 => "coastal",
        8 => "near-coast",
        9 => "near-shore",
        10 => "shoreline",
        11 => "passage-11",
        12 => "passage-12",
        13 => "passage-13",
        _ => "unknown",
    }
}

/// Generate all navigable H3 cells via adaptive multi-resolution cascade
/// (res-3 ocean through res-10 shoreline), with extended refinement into
/// passage zones at even higher resolutions (up to res-13).
///
/// Cells in passage corridors are protected from land-elimination during the
/// cascade, allowing narrow waterways (e.g. 25m-wide Corinth Canal) to survive
/// until resolutions where they become visible.
///
/// Returns a map of `CellIndex` to sequential `node_id` (starting from 0).
pub fn generate_cells(
    land: &LandIndex,
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
            if !land.contains_polygon(&cell_polygon(cell)) {
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

    // Collect passage corridors for cascade-level protection.
    // Cells inside passage corridors must not be eliminated by contains_polygon
    // during the cascade, because narrow waterways (e.g. 25m-wide Corinth Canal)
    // are smaller than intermediate H3 cells — the canal only becomes visible
    // at very high resolutions (res-12/13), so ancestors must survive until then.
    let passage_corridors: Vec<(f64, f64, f64, f64)> =
        passages.iter().map(|p| p.corridor).collect();

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
                    !land.intersects_polygon(&poly)
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

        // Refine to next-resolution children with hierarchical elimination.
        // NOTE: we skip the contains_polygon parent shortcut — H3 children can
        // protrude beyond parent boundary, so an all-land parent may still have
        // children that reach land. Each child is tested individually instead.
        let pb = make_progress(refine.len(), &format!("refining to res-{}", next_res));
        let corridors = &passage_corridors;
        current_water = refine
            .par_iter()
            .flat_map(|&parent_cell| {
                pb.inc(1);
                let parent_poly = cell_polygon(parent_cell);
                if !land.intersects_polygon(&parent_poly) {
                    return children(parent_cell, next_resolution);
                }
                children(parent_cell, next_resolution)
                    .into_iter()
                    .filter(|&child| {
                        !land.contains_polygon(&cell_polygon(child))
                            || in_passage_corridor(child, corridors)
                    })
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
            let zone_res =
                Resolution::try_from(passages[0].zone_resolution).expect("invalid zone resolution");
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
            if !land.intersects_polygon(&parent_poly) {
                pb.inc(leaf_cells.len() as u64);
                return leaf_cells;
            }
            // Test each leaf individually — skip contains_polygon parent shortcut
            // because H3 children can protrude beyond parent into land.
            leaf_cells
                .iter()
                .filter(|&&cell| {
                    pb.inc(1);
                    !land.intersects_polygon(&cell_polygon(cell))
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

        let mut leaf_res_groups: Vec<(u8, Vec<CellIndex>)> = by_leaf_res.into_iter().collect();
        leaf_res_groups.sort_by_key(|(res, _)| *res);
        for (leaf_res, mut current) in leaf_res_groups {
            info!(
                "Zone cascade: {} res-{} cells → refine to res-{}",
                current.len(),
                H3_RES_LEAF,
                leaf_res
            );

            // Same parent-grouping strategy as normal leaf filter (above), but
            // land-intersecting cells are refined further instead of dropped.
            let mut by_parent: HashMap<CellIndex, Vec<CellIndex>> =
                HashMap::with_capacity(current.len() / 7);
            for &cell in &current {
                let parent = cell.parent(leaf_res_minus_1).expect("parent");
                by_parent.entry(parent).or_default().push(cell);
            }

            let pb = make_progress(current.len(), &format!("zone filter res-{}", H3_RES_LEAF));

            // Split into pure water (keep at leaf resolution) and land-intersecting (refine further).
            // In passage zones, never drop "all land" cells — narrow waterways
            // are smaller than cells at this resolution and only become visible
            // at high resolutions (res-12/13).
            let classified: Vec<(CellIndex, bool)> = by_parent
                .into_par_iter()
                .flat_map(|(parent, leaf_cells)| {
                    let parent_poly = cell_polygon(parent);
                    if !land.intersects_polygon(&parent_poly) {
                        pb.inc(leaf_cells.len() as u64);
                        // All water — keep at this resolution
                        return leaf_cells
                            .into_iter()
                            .map(|c| (c, true))
                            .collect::<Vec<_>>();
                    }
                    // Test individually — keep pure water, refine everything else
                    // (including "all land" cells that may contain narrow waterways)
                    leaf_cells
                        .into_iter()
                        .map(|cell| {
                            pb.inc(1);
                            let poly = cell_polygon(cell);
                            let is_pure = !land.intersects_polygon(&poly);
                            (cell, is_pure)
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            pb.finish_and_clear();

            let mut pure = Vec::new();
            let mut straddle = Vec::new();
            for (cell, is_pure) in classified {
                if is_pure {
                    pure.push(cell);
                } else {
                    straddle.push(cell);
                }
            }

            // Add pure water cells at leaf resolution
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
                let next_resolution = Resolution::try_from(next_res).expect("invalid resolution");

                // Expand to children
                let pb =
                    make_progress(current.len(), &format!("zone refining to res-{}", next_res));
                // In passage zones, keep ALL children (including "all land")
                // because narrow waterways only become visible at high resolutions.
                let expanded: Vec<CellIndex> = current
                    .par_iter()
                    .flat_map(|&parent_cell| {
                        pb.inc(1);
                        children(parent_cell, next_resolution)
                    })
                    .collect();
                pb.finish_and_clear();

                if next_res < leaf_res {
                    // Intermediate resolution: keep pure water, refine straddle
                    let pb = make_progress(
                        expanded.len(),
                        &format!("zone classifying res-{}", next_res),
                    );
                    // In passage zones, never drop "all land" cells — refine them
                    let (pure, straddle): (Vec<CellIndex>, Vec<CellIndex>) = expanded
                        .par_iter()
                        .map(|&cell| {
                            pb.inc(1);
                            let poly = cell_polygon(cell);
                            let is_pure = !land.intersects_polygon(&poly);
                            (cell, is_pure)
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
                    // Leaf resolution: keep cells that are pure water
                    let pb = make_progress(
                        expanded.len(),
                        &format!("zone leaf filter res-{}", next_res),
                    );
                    let pure: Vec<CellIndex> = expanded
                        .par_iter()
                        .filter(|&&cell| {
                            pb.inc(1);
                            !land.intersects_polygon(&cell_polygon(cell))
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
fn build_zone_lookup(passages: &[Passage], bbox: Option<Bbox>) -> Result<HashMap<CellIndex, u8>> {
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

        let zone_res =
            Resolution::try_from(passage.zone_resolution).expect("invalid zone resolution");
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
    let center_dist = coastline.min_distance_deg(lon, lat, search_radius);
    cell_boundary(cell)
        .into_iter()
        .fold(center_dist, |best, (vlat, vlon)| {
            best.min(coastline.min_distance_deg(vlon, vlat, search_radius))
        })
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
