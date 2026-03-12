use crate::geo_index::CoastlineIndex;
use crate::graph::RoutingGraph;
use crate::h3::haversine_km;
use ordered_float::OrderedFloat;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Result of a route computation.
pub struct RouteResult {
    /// Total distance in km
    pub distance_km: f64,
    /// Raw A* path node count
    pub raw_hops: usize,
    /// Smoothed waypoint count
    pub smooth_hops: usize,
    /// Smoothed coordinates as (lon, lat) for GeoJSON
    pub coordinates: Vec<[f64; 2]>,
}

/// A* pathfinding with haversine heuristic.
pub fn astar(graph: &RoutingGraph, start: u32, goal: u32) -> Option<(Vec<u32>, f64)> {
    let n = graph.num_nodes as usize;
    let mut g_score = vec![f32::MAX; n];
    let mut came_from = vec![u32::MAX; n];
    let mut closed = vec![false; n];

    g_score[start as usize] = 0.0;

    // Priority queue: (f_score, node_id)
    let mut open: BinaryHeap<Reverse<(OrderedFloat<f32>, u32)>> = BinaryHeap::new();

    let (goal_lat, goal_lon) = graph.node_pos(goal);
    let (start_lat, start_lon) = graph.node_pos(start);
    let h_start = haversine_km(start_lat, start_lon, goal_lat, goal_lon) as f32;
    open.push(Reverse((OrderedFloat(h_start), start)));

    while let Some(Reverse((_, current))) = open.pop() {
        if current == goal {
            // Reconstruct path
            let mut path = vec![goal];
            let mut node = goal;
            while node != start {
                node = came_from[node as usize];
                path.push(node);
            }
            path.reverse();
            let total_dist = g_score[goal as usize] as f64;
            return Some((path, total_dist));
        }

        if closed[current as usize] {
            continue;
        }
        closed[current as usize] = true;

        let current_g = g_score[current as usize];

        for (neighbor, weight) in graph.edges_with_weights(current) {
            if closed[neighbor as usize] {
                continue;
            }
            let tentative_g = current_g + weight;
            if tentative_g < g_score[neighbor as usize] {
                g_score[neighbor as usize] = tentative_g;
                came_from[neighbor as usize] = current;
                let (nlat, nlon) = graph.node_pos(neighbor);
                let h = haversine_km(nlat, nlon, goal_lat, goal_lon) as f32;
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
pub fn compute_route(
    graph: &RoutingGraph,
    from_lat: f64,
    from_lon: f64,
    to_lat: f64,
    to_lon: f64,
    coastline: &CoastlineIndex,
    node_knn: &dyn Fn(f64, f64) -> Option<(u32, f64)>,
) -> Option<RouteResult> {
    let (start, _) = node_knn(from_lat, from_lon)?;
    let (goal, _) = node_knn(to_lat, to_lon)?;

    let (raw_path, _distance_km) = astar(graph, start, goal)?;
    let raw_hops = raw_path.len();

    let smoothed = smooth(graph, &raw_path, coastline);
    let smooth_hops = smoothed.len();

    // Compute actual distance along smoothed path
    let mut smooth_dist = 0.0;
    for w in smoothed.windows(2) {
        let (lat1, lon1) = graph.node_pos(w[0]);
        let (lat2, lon2) = graph.node_pos(w[1]);
        smooth_dist += haversine_km(lat1, lon1, lat2, lon2);
    }

    let coordinates: Vec<[f64; 2]> = smoothed
        .iter()
        .map(|&n| {
            let (lat, lon) = graph.node_pos(n);
            [lon, lat] // GeoJSON uses [lon, lat]
        })
        .collect();

    Some(RouteResult {
        distance_km: smooth_dist,
        raw_hops,
        smooth_hops,
        coordinates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphBuilder;

    fn diamond_graph() -> RoutingGraph {
        let mut b = GraphBuilder::new();
        b.add_node(0.0, 0.0);   // A = 0
        b.add_node(0.05, 0.0);  // B = 1
        b.add_node(0.0, 0.05);  // C = 2
        b.add_node(0.05, 0.05); // D = 3
        b.add_edge(0, 1, 5.0);
        b.add_edge(0, 2, 10.0);
        b.add_edge(1, 3, 5.0);
        b.add_edge(2, 3, 10.0);
        b.build()
    }

    #[test]
    fn astar_shortest_path() {
        let g = diamond_graph();
        let result = astar(&g, 0, 3);
        assert!(result.is_some());
        let (path, cost) = result.unwrap();
        assert!((cost - 10.0).abs() < 1e-6, "cost was {cost}, expected 10.0");
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], 0);
        assert_eq!(*path.last().unwrap(), 3);
    }

    #[test]
    fn astar_same_node() {
        let g = diamond_graph();
        let result = astar(&g, 0, 0);
        assert!(result.is_some());
        let (path, cost) = result.unwrap();
        assert_eq!(path, vec![0]);
        assert!((cost - 0.0).abs() < 1e-6);
    }

    #[test]
    fn astar_unreachable() {
        let mut b = GraphBuilder::new();
        b.add_node(0.0, 0.0);
        b.add_node(1.0, 1.0);
        let g = b.build();
        let result = astar(&g, 0, 1);
        assert!(result.is_none());
    }
}
