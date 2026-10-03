//! Coarse graph and corridor search (graph format v5).
//!
//! A coarse node is one connected piece of water inside one res-3 H3 region.
//! A long route first runs A* over these ~41k coarse nodes, then the fine A*
//! is limited to the regions along that coarse path. See the v5 design spec.

use crate::graph::RoutingGraph;
use crate::h3::haversine_nm;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

/// Resolution of the coarse regions.
pub const REGION_RES: u64 = 3;

/// Rings of coarse neighbours added around the coarse path. One ring kept
/// the final distance within ±0.2 % of the full search on the planet.
pub const CORRIDOR_RINGS: usize = 1;

/// Shorter great-circle distances skip the corridor: the full search takes a
/// few ms there, less than the coarse search costs (planet bench: no gain at
/// 534 nm, 1.5x at 1,265 nm).
// ponytail: fixed threshold; a short great circle can still hide a long
// detour (both sides of an isthmus), which then runs the full search as v4 did.
pub const CORRIDOR_MIN_NM: f64 = 500.0;

/// Region of an H3 cell: its res-3 parent, or the cell itself when it is
/// res 3 or coarser. Plain bit operations: set the resolution field to 3 and
/// the digits of resolutions 4..15 to 7 ("unused").
#[inline]
pub fn region_of(h3: u64) -> u64 {
    let res = (h3 >> 52) & 0xF;
    if res <= REGION_RES {
        return h3;
    }
    let digits_below = (1u64 << ((15 - REGION_RES) * 3)) - 1;
    (h3 & !(0xF << 52)) | (REGION_RES << 52) | digits_below
}

/// The coarse sections exactly as written to a v5 graph file.
#[derive(Default)]
pub(crate) struct CoarseSections {
    pub region: Vec<u64>,
    pub rep: Vec<u32>,
    pub size: Vec<u32>,
    /// lat, lon in microdegrees, two entries per coarse node.
    pub pos: Vec<i32>,
    pub offsets: Vec<u32>,
    pub targets: Vec<u32>,
}

impl CoarseSections {
    /// Build from the node ids and the sorted, deduplicated adjacency lists.
    pub fn build(node_h3: &[u64], adj: &[Vec<u32>]) -> Self {
        let n = node_h3.len();
        let region: Vec<u64> = node_h3.iter().map(|&h| region_of(h)).collect();

        // Pieces: union-find over edges that stay inside one region.
        let mut uf = crate::graph::UnionFind::new(n);
        for (u, list) in adj.iter().enumerate() {
            for &v in list {
                if v as usize > u && region[v as usize] == region[u] {
                    uf.union(u as u32, v);
                }
            }
        }
        // Visiting nodes in id order meets each piece first at its smallest
        // member, which becomes its rep.
        let mut pieces: Vec<(u64, u32, u32)> = Vec::new(); // (region, rep, root)
        let mut seen_root = vec![false; n];
        for i in 0..n as u32 {
            let root = uf.find(i);
            if !seen_root[root as usize] {
                seen_root[root as usize] = true;
                pieces.push((region[i as usize], i, root));
            }
        }
        drop(seen_root);
        pieces.sort_unstable();
        let nc = pieces.len();
        // root -> coarse id, reusing one node-sized table.
        let mut coarse_of = vec![u32::MAX; n];
        for (c, &(_, _, root)) in pieces.iter().enumerate() {
            coarse_of[root as usize] = c as u32;
        }
        for i in 0..n as u32 {
            coarse_of[i as usize] = coarse_of[uf.find(i) as usize];
        }
        drop(uf);
        drop(region);

        let mut size = vec![0u32; nc];
        let mut acc = vec![[0f64; 3]; nc];
        let mut out_adj: Vec<Vec<u32>> = vec![Vec::new(); nc];
        for (u, &h) in node_h3.iter().enumerate() {
            let c = coarse_of[u] as usize;
            size[c] += 1;
            let cell = h3o::CellIndex::try_from(h).expect("invalid H3 index");
            let (lat, lon) = crate::h3::cell_center(cell);
            let (lat, lon) = (lat.to_radians(), lon.to_radians());
            acc[c][0] += lat.cos() * lon.cos();
            acc[c][1] += lat.cos() * lon.sin();
            acc[c][2] += lat.sin();
            for &v in &adj[u] {
                let cv = coarse_of[v as usize];
                if cv as usize != c {
                    out_adj[c].push(cv);
                }
            }
        }
        let mut pos = Vec::with_capacity(nc * 2);
        for a in &acc {
            let lat = a[2].atan2((a[0] * a[0] + a[1] * a[1]).sqrt()).to_degrees();
            let lon = a[1].atan2(a[0]).to_degrees();
            pos.push((lat * 1e6).round() as i32);
            pos.push((lon * 1e6).round() as i32);
        }
        let mut offsets = Vec::with_capacity(nc + 1);
        let mut targets = Vec::new();
        for list in &mut out_adj {
            list.sort_unstable();
            list.dedup();
            offsets.push(targets.len() as u32);
            targets.extend_from_slice(list);
        }
        offsets.push(targets.len() as u32);

        Self {
            region: pieces.iter().map(|p| p.0).collect(),
            rep: pieces.iter().map(|p| p.1).collect(),
            size,
            pos,
            offsets,
            targets,
        }
    }
}

/// Coarse node that contains `node`.
///
/// A region with one piece needs no search. Otherwise flood-fill from the
/// node inside its region, but only until it has seen more nodes than the
/// second-largest piece: past that point the node must be in the largest
/// piece. Finishing earlier gives the piece's smallest id, its rep.
pub fn coarse_node_of(graph: &RoutingGraph, node: u32) -> Option<u32> {
    let region = region_of(graph.node_h3(node));
    let regions = graph.coarse_regions();
    let lo = regions.partition_point(|&r| r < region);
    let hi = regions.partition_point(|&r| r <= region);
    match hi - lo {
        0 => return None,
        1 => return Some(lo as u32),
        _ => {}
    }
    let sizes = &graph.coarse_sizes()[lo..hi];
    let largest = (0..sizes.len()).max_by_key(|&i| sizes[i]).unwrap();
    let limit = (0..sizes.len())
        .filter(|&i| i != largest)
        .map(|i| sizes[i])
        .max()
        .unwrap() as usize;

    let mut seen = HashSet::from([node]);
    let mut stack = vec![node];
    let mut min_id = node;
    while let Some(u) = stack.pop() {
        for v in graph.neighbor_ids(u) {
            if region_of(graph.node_h3(v)) == region && seen.insert(v) {
                if seen.len() > limit {
                    return Some((lo + largest) as u32);
                }
                min_id = min_id.min(v);
                stack.push(v);
            }
        }
    }
    graph.coarse_reps()[lo..hi]
        .iter()
        .position(|&r| r == min_id)
        .map(|i| (lo + i) as u32)
}

/// Sorted region ids the fine search may use between `start` and `goal`, or
/// None when a corridor does not apply: both ends in one coarse node (a
/// short route) or no coarse path. The caller then searches the whole graph.
pub fn corridor(
    graph: &RoutingGraph,
    start: u32,
    goal: u32,
    arctic: bool,
    canals: bool,
) -> Option<Vec<u64>> {
    let cs = coarse_node_of(graph, start)?;
    let ct = coarse_node_of(graph, goal)?;
    if cs == ct {
        return None;
    }
    let nc = graph.num_coarse() as usize;
    let (tlat, tlon) = graph.coarse_pos(ct);
    let mut g = vec![f64::INFINITY; nc];
    let mut prev = vec![u32::MAX; nc];
    let mut closed = vec![false; nc];
    let mut open = BinaryHeap::new();
    g[cs as usize] = 0.0;
    open.push(Reverse((0u64, cs)));
    while let Some(Reverse((_, u))) = open.pop() {
        if u == ct {
            break;
        }
        if std::mem::replace(&mut closed[u as usize], true) {
            continue;
        }
        let (ulat, ulon) = graph.coarse_pos(u);
        for &v in graph.coarse_neighbors(u) {
            if closed[v as usize] {
                continue;
            }
            let (vlat, vlon) = graph.coarse_pos(v);
            if v != ct && crate::routing::blocked(vlat, vlon, arctic, canals) {
                continue;
            }
            let ng = g[u as usize] + haversine_nm(ulat, ulon, vlat, vlon);
            if ng < g[v as usize] {
                g[v as usize] = ng;
                prev[v as usize] = u;
                // f >= 0, so the f64 bit pattern orders like the value.
                let f = ng + haversine_nm(vlat, vlon, tlat, tlon);
                open.push(Reverse((f.to_bits(), v)));
            }
        }
    }
    if prev[ct as usize] == u32::MAX {
        return None;
    }
    let mut members = vec![ct];
    let mut c = ct;
    while c != cs {
        c = prev[c as usize];
        members.push(c);
    }
    let mut frontier = members.clone();
    let mut in_set: HashSet<u32> = members.iter().copied().collect();
    for _ in 0..CORRIDOR_RINGS {
        let mut next = Vec::new();
        for &u in &frontier {
            for &v in graph.coarse_neighbors(u) {
                if in_set.insert(v) {
                    next.push(v);
                }
            }
        }
        members.extend_from_slice(&next);
        frontier = next;
    }
    let regions = graph.coarse_regions();
    let mut out: Vec<u64> = members.iter().map(|&c| regions[c as usize]).collect();
    out.sort_unstable();
    out.dedup();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphBuilder;
    use h3o::{LatLng, Resolution};

    fn cell(lat: f64, lon: f64, res: Resolution) -> u64 {
        u64::from(LatLng::new(lat, lon).unwrap().to_cell(res))
    }

    #[test]
    fn region_of_matches_h3o_parent() {
        for &(lat, lon) in &[(0.0, 0.0), (36.8, 28.3), (-60.0, 170.0), (79.9, -100.0)] {
            for res in [
                Resolution::Four,
                Resolution::Seven,
                Resolution::Ten,
                Resolution::Thirteen,
            ] {
                let c = LatLng::new(lat, lon).unwrap().to_cell(res);
                let want = u64::from(c.parent(Resolution::Three).unwrap());
                assert_eq!(region_of(u64::from(c)), want, "{lat},{lon} res {res:?}");
            }
            let r3 = cell(lat, lon, Resolution::Three);
            assert_eq!(region_of(r3), r3);
            let r2 = cell(lat, lon, Resolution::Two);
            assert_eq!(region_of(r2), r2);
        }
    }

    /// Graph over `cells` (any order) with the given edges between their
    /// indices. Returns the graph and each cell's node id.
    fn build(cells: &[u64], edges: &[(usize, usize)]) -> (RoutingGraph, Vec<u32>) {
        let mut order: Vec<usize> = (0..cells.len()).collect();
        order.sort_by_key(|&i| cells[i]);
        let mut b = GraphBuilder::default();
        let mut ids = vec![0u32; cells.len()];
        for &i in &order {
            ids[i] = b.add_node(cells[i], 255);
        }
        for &(a, c) in edges {
            b.add_edge(ids[a], ids[c]);
        }
        (b.build(), ids)
    }

    /// Children of one res-3 cell, in a line of res-7 neighbours.
    fn children_chain(parent: h3o::CellIndex, count: usize) -> Vec<u64> {
        let center = parent.center_child(Resolution::Seven).unwrap();
        let mut out = vec![u64::from(center)];
        let mut cur = center;
        while out.len() < count {
            let next = cur
                .grid_disk::<Vec<_>>(1)
                .into_iter()
                .find(|c| {
                    !out.contains(&u64::from(*c)) && c.parent(Resolution::Three) == Some(parent)
                })
                .unwrap();
            out.push(u64::from(next));
            cur = next;
        }
        out
    }

    #[test]
    fn pieces_split_by_connectivity_inside_a_region() {
        // One region, two separate chains (3 and 2 nodes) = two coarse nodes.
        let parent = LatLng::new(10.0, 10.0).unwrap().to_cell(Resolution::Three);
        let c = children_chain(parent, 6);
        // chain A: c0-c1-c2, chain B: c4-c5 (c3 left out so they never touch)
        let cells = vec![c[0], c[1], c[2], c[4], c[5]];
        let (g, ids) = build(&cells, &[(0, 1), (1, 2), (3, 4)]);
        assert_eq!(g.num_coarse(), 2);
        assert_eq!(g.coarse_regions(), &[u64::from(parent); 2]);
        let mut sizes = g.coarse_sizes().to_vec();
        sizes.sort();
        assert_eq!(sizes, vec![2, 3]);
        // Every node finds the coarse node whose rep is in its own chain.
        let a = coarse_node_of(&g, ids[1]).unwrap();
        let b = coarse_node_of(&g, ids[4]).unwrap();
        assert_ne!(a, b);
        assert_eq!(coarse_node_of(&g, ids[0]), Some(a));
        assert_eq!(coarse_node_of(&g, ids[2]), Some(a));
        assert_eq!(coarse_node_of(&g, ids[3]), Some(b));
        assert_eq!(g.coarse_sizes()[a as usize], 3);
        assert_eq!(g.coarse_sizes()[b as usize], 2);
    }

    #[test]
    fn coarse_edges_follow_fine_edges_across_regions() {
        // Three regions in a row: A-B-C, fine edges only A-B and B-C.
        let pa = LatLng::new(0.0, 0.0).unwrap().to_cell(Resolution::Three);
        let pb = LatLng::new(0.0, 3.0).unwrap().to_cell(Resolution::Three);
        let pc = LatLng::new(0.0, 6.0).unwrap().to_cell(Resolution::Three);
        let cells: Vec<u64> = [pa, pb, pc]
            .iter()
            .map(|p| u64::from(p.center_child(Resolution::Seven).unwrap()))
            .collect();
        let (g, ids) = build(&cells, &[(0, 1), (1, 2)]);
        assert_eq!(g.num_coarse(), 3);
        let [ca, cb, cc] = [0, 1, 2].map(|i| coarse_node_of(&g, ids[i]).unwrap());
        assert_eq!(g.coarse_neighbors(ca), &[cb]);
        assert_eq!(g.coarse_neighbors(cc), &[cb]);
        let mut nb = g.coarse_neighbors(cb).to_vec();
        nb.sort();
        let mut want = vec![ca, cc];
        want.sort();
        assert_eq!(nb, want);
        // Centroid of a one-node piece is the cell centre.
        let (lat, lon) = g.coarse_pos(ca);
        let (nlat, nlon) = g.node_pos(ids[0]);
        assert!((lat - nlat).abs() < 1e-5 && (lon - nlon).abs() < 1e-5);
        // Corridor from A to C holds all three regions (path) and nothing else.
        let corr = corridor(&g, ids[0], ids[2], false, true).unwrap();
        let mut want = vec![u64::from(pa), u64::from(pb), u64::from(pc)];
        want.sort();
        assert_eq!(corr, want);
        // Same coarse node at both ends: no corridor.
        assert!(corridor(&g, ids[0], ids[0], false, true).is_none());
    }

    #[test]
    fn flood_stops_early_inside_the_largest_piece() {
        // Big chain of 6 plus a separate single node in the same region: the
        // limit is 1, so a node of the big chain is resolved after 2 visits.
        let parent = LatLng::new(-20.0, 40.0).unwrap().to_cell(Resolution::Three);
        let c = children_chain(parent, 8);
        let cells = vec![c[0], c[1], c[2], c[3], c[4], c[5], c[7]];
        let edges: Vec<(usize, usize)> = (0..5).map(|i| (i, i + 1)).collect();
        let (g, ids) = build(&cells, &edges);
        assert_eq!(g.num_coarse(), 2);
        let big = coarse_node_of(&g, ids[3]).unwrap();
        assert_eq!(g.coarse_sizes()[big as usize], 6);
        let single = coarse_node_of(&g, ids[6]).unwrap();
        assert_eq!(g.coarse_sizes()[single as usize], 1);
        assert_eq!(g.coarse_reps()[single as usize], ids[6]);
    }
}
