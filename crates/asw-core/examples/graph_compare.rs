//! Structural stats for old-vs-new graph validation during releases.
//! Usage: cargo run --release -p asw-core --example graph_compare -- <graph-file>

use asw_core::graph::RoutingGraph;
use std::io::BufReader;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: graph_compare <graph-file>");
    let reader = BufReader::new(std::fs::File::open(&path).expect("open graph file"));
    let graph = RoutingGraph::load(reader).expect("failed to load graph");

    println!("file: {path}");
    println!("nodes: {}", graph.num_nodes);
    println!("edges: {}", graph.num_edges);

    // Zero-weight edge count (Fix B verification: must be 0 in the new graph).
    let mut zero_weight = 0u64;
    let mut min_nonzero = u16::MAX;
    let mut weight_hist = [0u64; 4]; // [0], [1], [2..=10], [>10] centi-nm
    for node in 0..graph.num_nodes {
        for (_, w) in graph.neighbors(node) {
            let w_centi = (w * 100.0).round() as u32;
            match w_centi {
                0 => {
                    zero_weight += 1;
                    weight_hist[0] += 1;
                }
                1 => weight_hist[1] += 1,
                2..=10 => weight_hist[2] += 1,
                _ => weight_hist[3] += 1,
            }
            if w_centi > 0 && (w_centi as u16) < min_nonzero {
                min_nonzero = w_centi as u16;
            }
        }
    }
    println!("zero_weight_edges: {zero_weight}");
    println!("min_nonzero_weight_centi_nm: {min_nonzero}");
    println!("weight_hist [0, 1, 2-10, >10 centi-nm]: {:?}", weight_hist);

    // Node counts near the antimeridian vs control regions.
    let mut seam = 0u64; // |lon| >= 175
    let mut bering = 0u64; // lon in [-180,-168]U[168,180], lat in [60,72]
    let mut fiji = 0u64; // |lon| >= 175, lat in [-25,-10]
    let mut med = 0u64; // control: Mediterranean 0..30E, 30..45N
    for node in 0..graph.num_nodes {
        let (lat, lon) = graph.node_pos(node);
        if lon.abs() >= 175.0 {
            seam += 1;
            if (-25.0..=-10.0).contains(&lat) {
                fiji += 1;
            }
        }
        if lon.abs() >= 168.0 && (60.0..=72.0).contains(&lat) {
            bering += 1;
        }
        if (0.0..=30.0).contains(&lon) && (30.0..=45.0).contains(&lat) {
            med += 1;
        }
    }
    println!("nodes_lon_abs_ge_175 (seam band): {seam}");
    println!("nodes_bering (|lon|>=168, lat 60-72): {bering}");
    println!("nodes_fiji_seam (|lon|>=175, lat -25..-10): {fiji}");
    println!("nodes_mediterranean_control (0-30E, 30-45N): {med}");
}
