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

/// Query-time shore clearance penalty. Edges into nodes closer to shore than
/// `buffer_q` get their weight multiplied by `1 + DEFAULT_K * (1 - d/buffer_q)`.
/// The penalty strength is a fixed constant (`DEFAULT_K`) — nothing in the
/// codebase varies it per-request, so there is no configurable field for it.
#[derive(Debug, Clone, Copy)]
pub struct ShorePenalty {
    /// Requested clearance in shore_dist units (SHORE_DIST_UNIT_NM each).
    pub buffer_q: u8,
}

impl ShorePenalty {
    pub const DEFAULT_K: f32 = 15.0;

    /// Build from a buffer in nautical miles. Returns None for buffer <= 0
    /// (and for NaN). Quantizes UP so the requested clearance is never
    /// understated.
    pub fn from_nm(buffer_nm: f64) -> Option<Self> {
        if buffer_nm.is_nan() || buffer_nm <= 0.0 {
            return None;
        }
        // Subtract a small epsilon before ceiling so float error on an exact
        // multiple (e.g. 0.14 / 0.02 = 7.000000000000001) doesn't overstate
        // buffer_q by one unit. Mirrors quantize_shore_dist's epsilon guard
        // in graph.rs (added before floor there; subtracted before ceil here).
        let q = (buffer_nm / crate::graph::SHORE_DIST_UNIT_NM - 1e-9)
            .ceil()
            .clamp(1.0, 255.0) as u8;
        Some(Self { buffer_q: q })
    }

    /// Weight multiplier for an edge into a node `d` shore_dist units from shore.
    #[inline]
    pub fn factor(&self, d: u8) -> f32 {
        if d >= self.buffer_q {
            1.0
        } else {
            1.0 + Self::DEFAULT_K * (1.0 - d as f32 / self.buffer_q as f32)
        }
    }
}

/// A* pathfinding with haversine heuristic.
pub fn astar(
    graph: &RoutingGraph,
    start: u32,
    goal: u32,
    buffers: &mut crate::astar_pool::AstarBuffers,
    shore: Option<ShorePenalty>,
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
            let weight = match shore {
                Some(sp) => weight * sp.factor(graph.shore_dist[neighbor as usize]),
                None => weight,
            };
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

/// Sparse table for O(1) range-minimum queries over a fixed `u8` slice.
/// Built once (O(n log n)) and queried O(1) per anchor in `smooth()`,
/// replacing an O(n) running-min rebuild per anchor (worst case O(n^2)
/// across the whole smoothing pass for long, twisty raw paths).
struct RangeMin {
    /// `table[k][i]` = min over `values[i .. i + 2^k]`.
    table: Vec<Vec<u8>>,
}

impl RangeMin {
    fn build(values: &[u8]) -> Self {
        let n = values.len();
        let levels = if n == 0 {
            1
        } else {
            (n as u32).ilog2() as usize + 1
        };
        let mut table: Vec<Vec<u8>> = Vec::with_capacity(levels);
        table.push(values.to_vec());
        for k in 1..levels {
            let half = 1usize << (k - 1);
            let span = half * 2;
            let prev = &table[k - 1];
            let row: Vec<u8> = (0..=n - span)
                .map(|i| prev[i].min(prev[i + half]))
                .collect();
            table.push(row);
        }
        Self { table }
    }

    /// Minimum of `values[i..=j]` (inclusive), both indices in bounds.
    fn query(&self, i: usize, j: usize) -> u8 {
        let len = j - i + 1;
        let k = (len as u32).ilog2() as usize;
        let span = 1usize << k;
        self.table[k][i].min(self.table[k][j + 1 - span])
    }
}

/// Greedy line-of-sight smoothing over a node path (see `smooth_indices`,
/// which this delegates to after decoding node positions and shore
/// distances).
pub fn smooth(
    graph: &RoutingGraph,
    path: &[u32],
    coastline: &CoastlineIndex,
    shore_buffer_nm: f64,
) -> Vec<u32> {
    let coords: Vec<[f64; 2]> = path
        .iter()
        .map(|&n| {
            let (lat, lon) = graph.node_pos(n);
            [lon, lat]
        })
        .collect();
    let shore_dist: Vec<u8> = path.iter().map(|&n| graph.shore_dist[n as usize]).collect();
    smooth_indices(&coords, &shore_dist, coastline, shore_buffer_nm)
        .into_iter()
        .map(|i| path[i])
        .collect()
}

/// Greedy line-of-sight smoothing over raw `[lon, lat]` coordinates.
///
/// Removes waypoints whose direct predecessor→successor line (a) doesn't
/// cross any coastline and (b) when `shore_buffer_nm > 0`, doesn't come
/// closer to the coastline than min(buffer, the input's own minimum
/// clearance over the skipped span, per `shore_dist`). Rule (b) means
/// smoothing never brings the route closer to shore than the raw input
/// already accepted — full buffer in open water, graceful degradation near
/// endpoints/in coves.
///
/// `shore_dist[i]` is the quantized (`SHORE_DIST_UNIT_NM` units) shore
/// distance of `coords[i]`; it must have the same length as `coords` and is
/// only consulted when `shore_buffer_nm > 0`. Returns strictly increasing
/// indices into `coords`, always keeping the first and last. When even the
/// next hop is blocked (e.g. a pin on land), the blocked segment is kept and
/// smoothing continues from that point.
pub fn smooth_indices(
    coords: &[[f64; 2]],
    shore_dist: &[u8],
    coastline: &CoastlineIndex,
    shore_buffer_nm: f64,
) -> Vec<usize> {
    if coords.len() <= 2 {
        return (0..coords.len()).collect();
    }
    let use_buffer = shore_buffer_nm > 0.0;
    debug_assert!(
        !use_buffer || shore_dist.len() == coords.len(),
        "shore_dist must be parallel to coords when a buffer is requested"
    );

    // Range-min over the whole input's shore_dist, built once per call (not
    // per anchor) so smoothing is O(n log n) overall instead of O(n^2).
    let range_min = if use_buffer {
        Some(RangeMin::build(shore_dist))
    } else {
        None
    };

    let mut result = vec![0];
    let mut current_idx = 0;
    let end_idx = coords.len() - 1;

    while current_idx < end_idx {
        let [c_lon, c_lat] = coords[current_idx];

        let clear = |j: usize| -> bool {
            let [t_lon, t_lat] = coords[j];
            if coastline.crosses_land(c_lon, c_lat, t_lon, t_lat) {
                return false;
            }
            if let Some(rm) = &range_min {
                let raw_min_nm = rm.query(current_idx, j) as f64 * crate::graph::SHORE_DIST_UNIT_NM;
                let threshold = shore_buffer_nm.min(raw_min_nm);
                if threshold > 0.0
                    && coastline.segment_min_distance_nm(c_lon, c_lat, t_lon, t_lat, threshold)
                        < threshold
                {
                    return false;
                }
            }
            true
        };

        // Try direct line to destination
        if clear(end_idx) {
            result.push(end_idx);
            break;
        }

        // Exponential forward search: find boundary between clear and blocked
        let mut step = 1usize;
        let mut v_lo = current_idx + 1;
        let mut v_hi;
        loop {
            let test_idx = (current_idx + step).min(end_idx);
            if !clear(test_idx) {
                v_hi = test_idx;
                break;
            }
            v_lo = test_idx;
            if test_idx >= end_idx {
                v_lo = end_idx;
                v_hi = end_idx;
                break;
            }
            step *= 2;
        }

        if v_lo == end_idx {
            result.push(end_idx);
            break;
        }

        // Binary search between v_lo (clear) and v_hi (blocked)
        while v_hi - v_lo > 1 {
            let mid = (v_lo + v_hi) / 2;
            if !clear(mid) {
                v_hi = mid;
            } else {
                v_lo = mid;
            }
        }

        // v_lo is the farthest visible point. Ensure we make progress even
        // when the very next hop is blocked (land pin case).
        if v_lo <= current_idx {
            v_lo = current_idx + 1;
        }
        result.push(v_lo);
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
    shore_buffer_nm: f64,
) -> Option<RouteResult> {
    let (start, _) = node_knn(from_lat, from_lon)?;
    let (goal, _) = node_knn(to_lat, to_lon)?;

    let shore = ShorePenalty::from_nm(shore_buffer_nm);
    let (raw_path, _distance_nm) = astar(graph, start, goal, buffers, shore)?;
    let raw_hops = raw_path.len();

    let smoothed = smooth(graph, &raw_path, coastline, shore_buffer_nm);
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
            let id = b.add_node(*h3, *lat, *lng, 255);
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
        let result = astar(&g, node_a, node_d, &mut buffers, None);
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
        let result = astar(&g, node_a, node_a, &mut buffers, None);
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
        let first =
            astar(&g, node_a, node_d, &mut reused, None).expect("first search finds a path");

        // Simulate AstarPool::release() + acquire(): O(1) generation bump, no
        // full-graph clear.
        reused.reset();

        // Second search on the same (now stale-but-reset) buffers.
        let second =
            astar(&g, node_a, node_d, &mut reused, None).expect("second search finds a path");

        // Baseline: identical query on completely fresh buffers.
        let mut fresh = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let baseline =
            astar(&g, node_a, node_d, &mut fresh, None).expect("baseline search finds a path");

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
        let _ = astar(&g, node_a, node_d, &mut buffers, None).expect("path exists");

        buffers.reset();

        // Second: reverse search (D -> A) on the same, now-stale buffers.
        let reused_result = astar(&g, node_d, node_a, &mut buffers, None).expect("path exists");
        let mut fresh = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let fresh_result = astar(&g, node_d, node_a, &mut fresh, None).expect("path exists");

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
            b.add_node(*h3, *lat, *lng, 255);
        }
        let g = b.build();
        let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let result = astar(&g, 0, 1, &mut buffers, None);
        assert!(result.is_none());
    }

    #[test]
    fn range_min_matches_brute_force_min() {
        let values: [u8; 9] = [5, 3, 8, 1, 9, 2, 7, 6, 4];
        let rm = RangeMin::build(&values);
        for i in 0..values.len() {
            for j in i..values.len() {
                let expected = values[i..=j].iter().copied().min().unwrap();
                assert_eq!(
                    rm.query(i, j),
                    expected,
                    "range [{i}, {j}] expected {expected}"
                );
            }
        }
    }

    #[test]
    fn range_min_single_element() {
        let values: [u8; 1] = [42];
        let rm = RangeMin::build(&values);
        assert_eq!(rm.query(0, 0), 42);
    }

    #[test]
    fn shore_penalty_from_nm_and_factor() {
        assert!(ShorePenalty::from_nm(0.0).is_none());
        assert!(ShorePenalty::from_nm(-1.0).is_none());
        let p = ShorePenalty::from_nm(0.1).unwrap(); // 0.1 / 0.02 = 5
        assert_eq!(p.buffer_q, 5);
        let p2 = ShorePenalty::from_nm(0.001).unwrap(); // rounds UP to 1
        assert_eq!(p2.buffer_q, 1);
        let p3 = ShorePenalty::from_nm(99.0).unwrap(); // clamps to 255
        assert_eq!(p3.buffer_q, 255);
        // 0.14 / 0.02 == 7.000000000000001 in f64; without the epsilon guard
        // this ceils to 8 instead of the intended exact 7.
        let p4 = ShorePenalty::from_nm(0.14).unwrap();
        assert_eq!(p4.buffer_q, 7);

        assert_eq!(p.factor(5), 1.0); // at the buffer: no penalty
        assert_eq!(p.factor(255), 1.0); // far offshore: no penalty
        assert!((p.factor(0) - 16.0).abs() < 1e-6); // 1 + 15*1
        assert!((p.factor(2) - (1.0 + 15.0 * 0.6)).abs() < 1e-4);
    }

    /// Two corridors S->A->G (short, A hugs the shore) and S->B->G (long, B
    /// offshore). Without a buffer the short corridor wins; with one, the long.
    fn corridor_graph() -> (RoutingGraph, u32, u32, u32, u32) {
        let coords = [
            (0.0, 0.0, "S", 255u8),
            (1.0, 0.0, "A", 0u8), // on the shore
            (0.0, 1.0, "B", 255u8),
            (1.0, 1.0, "G", 255u8),
        ];
        let mut cells: Vec<(u64, f64, f64, u8, &str)> = coords
            .iter()
            .map(|&(lat, lng, label, q)| {
                let cell = h3o::LatLng::new(lat, lng)
                    .unwrap()
                    .to_cell(h3o::Resolution::Five);
                (u64::from(cell), lat, lng, q, label)
            })
            .collect();
        cells.sort_by_key(|(h3, _, _, _, _)| *h3);

        let mut b = GraphBuilder::new();
        let mut ids = std::collections::HashMap::new();
        for (h3, lat, lng, q, label) in &cells {
            let id = b.add_node(*h3, *lat, *lng, *q);
            ids.insert(*label, id);
        }
        b.add_edge(ids["S"], ids["A"], 5.0);
        b.add_edge(ids["A"], ids["G"], 5.0); // near-shore total: 10
        b.add_edge(ids["S"], ids["B"], 8.0);
        b.add_edge(ids["B"], ids["G"], 8.0); // offshore total: 16
        (b.build(), ids["S"], ids["A"], ids["B"], ids["G"])
    }

    #[test]
    fn penalty_diverts_route_offshore() {
        let (g, s, a, b_node, goal) = corridor_graph();
        let mut buffers = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);

        // Without penalty: short near-shore corridor via A.
        let (path, cost) = astar(&g, s, goal, &mut buffers, None).unwrap();
        assert_eq!(path, vec![s, a, goal]);
        assert!((cost - 10.0).abs() < 1e-4);

        // With a 0.1 nm buffer: edge S->A costs 5 * 16 = 80 -> offshore wins.
        buffers.reset();
        let shore = ShorePenalty::from_nm(0.1);
        let (path, cost) = astar(&g, s, goal, &mut buffers, shore).unwrap();
        assert_eq!(path, vec![s, b_node, goal]);
        assert!((cost - 16.0).abs() < 1e-4);
    }

    #[test]
    fn penalty_with_all_nodes_offshore_is_identity() {
        let (g, node_a, node_d) = diamond_graph(); // all nodes shore_dist=255
        let mut b1 = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let mut b2 = crate::astar_pool::AstarBuffers::new(g.num_nodes as usize);
        let plain = astar(&g, node_a, node_d, &mut b1, None).unwrap();
        let with = astar(&g, node_a, node_d, &mut b2, ShorePenalty::from_nm(0.2)).unwrap();
        assert_eq!(plain.0, with.0);
        assert!((plain.1 - with.1).abs() < 1e-6);
    }

    /// Coastline at lon 28.0 (lat 36.45..36.55) and a 3-node dogleg around it.
    /// Direct P0->P2 passes ~2.4 nm off the coast; the dogleg via P1 ~7 nm.
    fn dogleg() -> (RoutingGraph, CoastlineIndex, Vec<u32>) {
        dogleg_with_shore(&[255, 255, 255])
    }

    fn dogleg_with_shore(shore_q: &[u8; 3]) -> (RoutingGraph, CoastlineIndex, Vec<u32>) {
        use geo::LineString;
        let coastline = CoastlineIndex::new(vec![crate::geo_index::CoastlineSegment::new(
            LineString::from(vec![(28.0, 36.45), (28.0, 36.55)]),
        )]);

        let coords = [(36.3, 28.05), (36.5, 28.15), (36.7, 28.05)];
        let mut cells: Vec<(u64, f64, f64, u8, usize)> = coords
            .iter()
            .enumerate()
            .map(|(i, &(lat, lng))| {
                let cell = h3o::LatLng::new(lat, lng)
                    .unwrap()
                    .to_cell(h3o::Resolution::Nine);
                (u64::from(cell), lat, lng, shore_q[i], i)
            })
            .collect();
        cells.sort_by_key(|(h3, _, _, _, _)| *h3);

        let mut b = GraphBuilder::new();
        let mut id_by_orig = [0u32; 3];
        for (h3, lat, lng, q, orig) in &cells {
            id_by_orig[*orig] = b.add_node(*h3, *lat, *lng, *q);
        }
        b.add_edge(id_by_orig[0], id_by_orig[1], 1.0);
        b.add_edge(id_by_orig[1], id_by_orig[2], 1.0);
        let path = id_by_orig.to_vec();
        (b.build(), coastline, path)
    }

    #[test]
    fn smooth_without_buffer_cuts_the_corner() {
        let (g, coast, path) = dogleg();
        let smoothed = smooth(&g, &path, &coast, 0.0);
        assert_eq!(smoothed, vec![path[0], path[2]]);
    }

    #[test]
    fn smooth_respects_buffer() {
        let (g, coast, path) = dogleg();
        // Direct line is ~2.41 nm off the coast: allowed at 2.0, blocked at 3.0.
        let loose = smooth(&g, &path, &coast, 2.0);
        assert_eq!(loose, vec![path[0], path[2]]);
        let strict = smooth(&g, &path, &coast, 3.0);
        assert_eq!(strict, path, "3 nm buffer must keep the dogleg waypoint");
    }

    #[test]
    fn smooth_relaxes_near_endpoints() {
        // Path nodes themselves are close to shore (q=20 = 0.4 nm): the
        // threshold becomes min(3.0, 0.4) = 0.4 nm, so the direct line
        // (~2.4 nm off) is allowed even under a 3 nm buffer.
        let (g, coast, path) = dogleg_with_shore(&[20, 255, 20]);
        let smoothed = smooth(&g, &path, &coast, 3.0);
        assert_eq!(smoothed, vec![path[0], path[2]]);
    }

    /// Vertical coastline "wall" at `lon`, spanning `lat_min..lat_max`.
    fn wall_index(lon: f64, lat_min: f64, lat_max: f64) -> CoastlineIndex {
        let line = geo::LineString::from(vec![(lon, lat_min), (lon, lat_max)]);
        CoastlineIndex::new(vec![crate::geo_index::CoastlineSegment::new(line)])
    }

    /// Closed square ring around (0, 0), side 0.2 degrees — a tiny "island"
    /// used to simulate a pin on land.
    fn island_around_origin() -> CoastlineIndex {
        let ring = geo::LineString::from(vec![
            (-0.1, -0.1),
            (0.1, -0.1),
            (0.1, 0.1),
            (-0.1, 0.1),
            (-0.1, -0.1),
        ]);
        CoastlineIndex::new(vec![crate::geo_index::CoastlineSegment::new(ring)])
    }

    #[test]
    fn smooth_indices_collapses_clear_path() {
        let coastline = CoastlineIndex::new(vec![]);
        let coords = [[0.0, 0.0], [0.3, 0.1], [0.6, -0.1], [1.0, 0.0]];
        let out = smooth_indices(&coords, &[255; 4], &coastline, 0.0);
        assert_eq!(out, vec![0, 3]);
    }

    #[test]
    fn smooth_indices_keeps_necessary_corner() {
        // Wall at lon 0.5 (lat -1..1); the path detours over its top at lat 1.5.
        // Only the corner above the wall must survive smoothing.
        let coastline = wall_index(0.5, -1.0, 1.0);
        let coords = [[0.0, 0.0], [0.2, 0.5], [0.5, 1.5], [0.8, 0.5], [1.0, 0.0]];
        let out = smooth_indices(&coords, &[255; 5], &coastline, 0.0);
        assert_eq!(out, vec![0, 2, 4]);
    }

    #[test]
    fn smooth_indices_short_input_passthrough() {
        let coastline = wall_index(0.5, -1.0, 1.0);
        let coords = [[0.0, 0.0], [1.0, 0.0]];
        assert_eq!(
            smooth_indices(&coords, &[255; 2], &coastline, 0.0),
            vec![0, 1]
        );
    }

    #[test]
    fn smooth_indices_blocked_next_hop_still_progresses() {
        // First point sits inside a ring "island": it can see nothing, so the
        // smoother must keep the (land-clipping) segment to the next point and
        // continue — this is the approved land-pin behavior.
        let coastline = island_around_origin();
        let coords = [[0.0, 0.0], [0.5, 0.0], [1.0, 0.0]];
        let out = smooth_indices(&coords, &[255; 3], &coastline, 0.0);
        assert_eq!(*out.first().unwrap(), 0);
        assert_eq!(*out.last().unwrap(), 2);
    }

    #[test]
    fn smooth_indices_respects_buffer() {
        // Same geometry as the node-based dogleg tests: coastline at lon 28.0
        // (lat 36.45..36.55), direct P0->P2 line ~2.41 nm off the coast.
        let coastline = wall_index(28.0, 36.45, 36.55);
        let coords = [[28.05, 36.3], [28.15, 36.5], [28.05, 36.7]];
        let loose = smooth_indices(&coords, &[255; 3], &coastline, 2.0);
        assert_eq!(loose, vec![0, 2]);
        let strict = smooth_indices(&coords, &[255; 3], &coastline, 3.0);
        assert_eq!(strict, vec![0, 1, 2], "3 nm buffer must keep the dogleg");
    }

    #[test]
    fn smooth_indices_relaxes_for_near_shore_endpoints() {
        // Endpoints themselves are close to shore (q=20 = 0.4 nm): threshold
        // becomes min(3.0, 0.4) = 0.4 nm, so the ~2.4 nm direct line passes.
        let coastline = wall_index(28.0, 36.45, 36.55);
        let coords = [[28.05, 36.3], [28.15, 36.5], [28.05, 36.7]];
        let out = smooth_indices(&coords, &[20, 255, 20], &coastline, 3.0);
        assert_eq!(out, vec![0, 2]);
    }
}
