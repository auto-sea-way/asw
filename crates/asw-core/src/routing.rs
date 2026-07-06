use crate::geo_index::CoastlineIndex;
use crate::graph::RoutingGraph;
use crate::h3::haversine_nm;
use ordered_float::OrderedFloat;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Result of a route computation.
pub struct RouteResult {
    /// Total distance in nm
    pub distance_nm: f64,
    /// Raw A* path node count
    pub raw_hops: usize,
    /// Smoothed waypoint count
    pub smooth_hops: usize,
    /// Smoothed coordinates as (lon, lat) for GeoJSON
    pub coordinates: Vec<[f64; 2]>,
}

/// Ensure `node` is valid for the current generation in `buffers`, computing
/// and caching its haversine heuristic to `(goal_lat, goal_lon)` on first
/// touch this generation. `g_score`/`came_from`/`closed` are reset to their
/// defaults by `touch()` itself; the heuristic decode (H3 -> lat/lng + trig)
/// only happens once per node per search, no matter how many times the node
/// is relaxed afterwards.
#[inline]
fn touch_and_cache_h(
    buffers: &mut crate::astar_pool::AstarBuffers,
    node: u32,
    graph: &RoutingGraph,
    goal_lat: f64,
    goal_lon: f64,
) {
    if buffers.touch(node) {
        let (nlat, nlon) = graph.node_pos(node);
        buffers.h_score[node as usize] = haversine_nm(nlat, nlon, goal_lat, goal_lon) as f32;
    }
}

/// A* pathfinding with haversine heuristic.
pub fn astar(
    graph: &RoutingGraph,
    start: u32,
    goal: u32,
    buffers: &mut crate::astar_pool::AstarBuffers,
) -> Option<(Vec<u32>, f64)> {
    // Priority queue: (f_score, node_id)
    let mut open: BinaryHeap<Reverse<(OrderedFloat<f32>, u32)>> = BinaryHeap::new();

    let (goal_lat, goal_lon) = graph.node_pos(goal);

    touch_and_cache_h(buffers, start, graph, goal_lat, goal_lon);
    buffers.g_score[start as usize] = 0.0;
    let h_start = buffers.h_score[start as usize];
    open.push(Reverse((OrderedFloat(h_start), start)));

    while let Some(Reverse((_, current))) = open.pop() {
        if current == goal {
            // Reconstruct path
            let mut path = vec![goal];
            let mut node = goal;
            while node != start {
                node = buffers.came_from[node as usize];
                path.push(node);
            }
            path.reverse();
            let total_dist = buffers.g_score[goal as usize] as f64;
            return Some((path, total_dist));
        }

        touch_and_cache_h(buffers, current, graph, goal_lat, goal_lon);
        if buffers.closed[current as usize] {
            continue;
        }
        buffers.closed[current as usize] = true;

        let current_g = buffers.g_score[current as usize];

        for (neighbor, weight) in graph.neighbors(current) {
            touch_and_cache_h(buffers, neighbor, graph, goal_lat, goal_lon);
            if buffers.closed[neighbor as usize] {
                continue;
            }
            let tentative_g = current_g + weight;
            if tentative_g < buffers.g_score[neighbor as usize] {
                buffers.g_score[neighbor as usize] = tentative_g;
                buffers.came_from[neighbor as usize] = current;
                let h = buffers.h_score[neighbor as usize];
                let f = tentative_g + h;
                open.push(Reverse((OrderedFloat(f), neighbor)));
            }
        }
    }

    None // No path found
}

/// Greedy line-of-sight smoothing.
///
/// Takes the raw A* path and removes unnecessary waypoints by checking
/// if direct lines between waypoints cross any coastline.
pub fn smooth(graph: &RoutingGraph, path: &[u32], coastline: &CoastlineIndex) -> Vec<u32> {
    if path.len() <= 2 {
        return path.to_vec();
    }

    let mut result = vec![path[0]];
    let mut current_idx = 0;
    let end_idx = path.len() - 1;

    while current_idx < end_idx {
        let (c_lat, c_lon) = graph.node_pos(path[current_idx]);

        // Try direct line to destination
        let (e_lat, e_lon) = graph.node_pos(path[end_idx]);
        if !coastline.crosses_land(c_lon, c_lat, e_lon, e_lat) {
            result.push(path[end_idx]);
            break;
        }

        // Exponential forward search: find boundary between clear and blocked
        let mut step = 1usize;
        let mut v_lo = current_idx + 1; // Last known clear
        let mut v_hi;

        // First, find a clear starting point (next hop should always be clear)
        loop {
            let test_idx = (current_idx + step).min(end_idx);
            let (t_lat, t_lon) = graph.node_pos(path[test_idx]);
            if coastline.crosses_land(c_lon, c_lat, t_lon, t_lat) {
                v_hi = test_idx;
                break;
            }
            v_lo = test_idx;
            if test_idx >= end_idx {
                // Can see all the way to the end
                v_lo = end_idx;
                v_hi = end_idx;
                break;
            }
            step *= 2;
        }

        if v_lo == end_idx {
            result.push(path[end_idx]);
            break;
        }

        // Binary search between v_lo (clear) and v_hi (blocked)
        while v_hi - v_lo > 1 {
            let mid = (v_lo + v_hi) / 2;
            let (m_lat, m_lon) = graph.node_pos(path[mid]);
            if coastline.crosses_land(c_lon, c_lat, m_lon, m_lat) {
                v_hi = mid;
            } else {
                v_lo = mid;
            }
        }

        // v_lo is the farthest visible point
        // Ensure we make progress
        if v_lo <= current_idx {
            v_lo = current_idx + 1;
        }
        result.push(path[v_lo]);
        current_idx = v_lo;
    }

    result
}

/// Compute a full route: snap → A* → smooth → build result.
#[allow(clippy::too_many_arguments)]
pub fn compute_route(
    graph: &RoutingGraph,
    from_lat: f64,
    from_lon: f64,
    to_lat: f64,
    to_lon: f64,
    coastline: &CoastlineIndex,
    node_knn: &dyn Fn(f64, f64) -> Option<(u32, f64)>,
    buffers: &mut crate::astar_pool::AstarBuffers,
) -> Option<RouteResult> {
    let (start, _) = node_knn(from_lat, from_lon)?;
    let (goal, _) = node_knn(to_lat, to_lon)?;

    let (raw_path, _distance_nm) = astar(graph, start, goal, buffers)?;
    let raw_hops = raw_path.len();

    let smoothed = smooth(graph, &raw_path, coastline);
    let smooth_hops = smoothed.len();

    // Compute actual distance along smoothed path
    let mut smooth_dist = 0.0;
    for w in smoothed.windows(2) {
        let (lat1, lon1) = graph.node_pos(w[0]);
        let (lat2, lon2) = graph.node_pos(w[1]);
        smooth_dist += haversine_nm(lat1, lon1, lat2, lon2);
    }

    let coordinates: Vec<[f64; 2]> = smoothed
        .iter()
        .map(|&n| {
            let (lat, lon) = graph.node_pos(n);
            [lon, lat] // GeoJSON uses [lon, lat]
        })
        .collect();

    Some(RouteResult {
        distance_nm: smooth_dist,
        raw_hops,
        smooth_hops,
        coordinates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphBuilder;

    /// Returns (graph, node_a, node_d) where A->B->D is shortest (cost 10).
    fn diamond_graph() -> (RoutingGraph, u32, u32) {
        // Points spaced far enough apart to map to distinct H3 cells at res-5
        let c0 = h3o::LatLng::new(0.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(1.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c2 = h3o::LatLng::new(0.0, 1.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c3 = h3o::LatLng::new(1.0, 1.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);

        let mut cells: Vec<(u64, f64, f64, &str)> = vec![
            (u64::from(c0), 0.0, 0.0, "A"),
            (u64::from(c1), 1.0, 0.0, "B"),
            (u64::from(c2), 0.0, 1.0, "C"),
            (u64::from(c3), 1.0, 1.0, "D"),
        ];
        cells.sort_by_key(|(h3, _, _, _)| *h3);
        cells.dedup_by_key(|(h3, _, _, _)| *h3);
        assert_eq!(cells.len(), 4, "Need 4 distinct H3 cells for diamond graph");

        let mut b = GraphBuilder::new();
        let mut ids = std::collections::HashMap::new();
        for (h3, lat, lng, label) in &cells {
            let id = b.add_node(*h3, *lat, *lng);
            ids.insert(*label, id);
        }

        b.add_edge(ids["A"], ids["B"], 5.0);
        b.add_edge(ids["A"], ids["C"], 10.0);
        b.add_edge(ids["B"], ids["D"], 5.0);
        b.add_edge(ids["C"], ids["D"], 10.0);
        (b.build(), ids["A"], ids["D"])
    }

    #[test]
    fn astar_shortest_path() {
        let (g, node_a, node_d) = diamond_graph();
        let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let result = astar(&g, node_a, node_d, &mut buffers);
        assert!(result.is_some());
        let (path, cost) = result.unwrap();
        assert!((cost - 10.0).abs() < 1e-6, "cost was {cost}, expected 10.0");
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], node_a);
        assert_eq!(*path.last().unwrap(), node_d);
    }

    #[test]
    fn astar_same_node() {
        let (g, node_a, _) = diamond_graph();
        let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let result = astar(&g, node_a, node_a, &mut buffers);
        assert!(result.is_some());
        let (path, cost) = result.unwrap();
        assert_eq!(path, vec![node_a]);
        assert!((cost - 0.0).abs() < 1e-6);
    }

    /// THE regression test for the generation-counter reset: reusing the same
    /// buffers across two searches (with a `reset()` in between, mimicking
    /// `AstarPool::acquire`/`release`) must produce a result bit-identical to
    /// running the second search on brand-new buffers. This catches stale
    /// g_score/came_from/closed/h_score state leaking across generations.
    #[test]
    fn astar_reused_buffers_match_fresh_buffers_after_reset() {
        let (g, node_a, node_d) = diamond_graph();

        let mut reused = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);

        // First search touches (and closes) every node in the tiny diamond graph.
        let first = astar(&g, node_a, node_d, &mut reused).expect("first search finds a path");

        // Simulate AstarPool::release() + acquire(): O(1) generation bump, no
        // full-graph clear.
        reused.reset();

        // Second search on the same (now stale-but-reset) buffers.
        let second = astar(&g, node_a, node_d, &mut reused).expect("second search finds a path");

        // Baseline: identical query on completely fresh buffers.
        let mut fresh = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let baseline = astar(&g, node_a, node_d, &mut fresh).expect("baseline search finds a path");

        assert_eq!(
            second.0, baseline.0,
            "path nodes must match a fresh-buffer run"
        );
        assert!(
            (second.1 - baseline.1).abs() < 1e-6,
            "cost {} must match fresh-buffer cost {}",
            second.1,
            baseline.1
        );
        // Repeating the identical query twice should also yield identical results.
        assert_eq!(first.0, second.0);
        assert!((first.1 - second.1).abs() < 1e-6);
    }

    /// A search in one direction (closing nodes and stamping came_from/g_score
    /// pointers) followed by reset() and a *different* query in the reverse
    /// direction on the same buffers must not be corrupted by stale state
    /// (e.g. `closed` flags or `came_from` pointers left over from the first
    /// generation).
    #[test]
    fn astar_stale_state_does_not_leak_across_generations() {
        let (g, node_a, node_d) = diamond_graph();
        let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);

        // First: forward search closes/stamps every node on the A->D path.
        let _ = astar(&g, node_a, node_d, &mut buffers).expect("path exists");

        buffers.reset();

        // Second: reverse search (D -> A) on the same, now-stale buffers.
        let reused_result = astar(&g, node_d, node_a, &mut buffers).expect("path exists");
        let mut fresh = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let fresh_result = astar(&g, node_d, node_a, &mut fresh).expect("path exists");

        assert_eq!(reused_result.0, fresh_result.0);
        assert!((reused_result.1 - fresh_result.1).abs() < 1e-6);
    }

    #[test]
    fn astar_unreachable() {
        let c0 = h3o::LatLng::new(0.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(10.0, 10.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let mut cells = vec![(u64::from(c0), 0.0, 0.0), (u64::from(c1), 10.0, 10.0)];
        cells.sort_by_key(|(h3, _, _)| *h3);
        let mut b = GraphBuilder::new();
        for (h3, lat, lng) in &cells {
            b.add_node(*h3, *lat, *lng);
        }
        let g = b.build();
        let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let result = astar(&g, 0, 1, &mut buffers);
        assert!(result.is_none());
    }
}
