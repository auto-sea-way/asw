use anyhow::{Context, Result};
use asw_core::graph::RoutingGraph;
use asw_core::h3::haversine_km;
use asw_core::routing::compute_route;
use asw_serve::state::AppState;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Instant;
use tracing::info;

/// Number of sailing routes (first N in ROUTES array). The rest are passage transits.
const NUM_SAILING_ROUTES: usize = 10;

/// Hardcoded real-world sailing routes: (name, from_lat, from_lon, to_lat, to_lon)
const ROUTES: &[(&str, f64, f64, f64, f64)] = &[
    // Short crossings (< 100km)
    ("English Channel",      51.11,   1.32,  50.97,   1.86),
    ("Aegean Hop",           36.85,  28.28,  36.44,  28.23),
    ("Strait of Gibraltar",  36.13,  -5.35,  35.79,  -5.81),
    ("Baltic Crossing",      60.14,  24.97,  59.45,  24.76),
    // Medium passages (100-500km)
    ("Balearic Sea",         39.55,   2.64,  41.37,   2.18),
    ("Florida Strait",       24.55, -81.79,  23.15, -82.35),
    ("Malacca Route",         1.26, 103.86,   7.88,  98.38),
    // Long haul (1000km+)
    ("Tasman Sea",          -33.86, 151.28, -36.83, 174.78),
    ("South Atlantic",      -33.92,  18.43, -22.91, -43.16),
    ("North Atlantic",       40.65, -74.03,  50.89,  -1.39),
    // Passage transits (short routes forcing passage edges)
    ("Suez Canal",           29.50,  32.85,  31.50,  32.00),
    ("Panama Canal",          8.80, -79.45,   9.40, -80.05),
    ("Kiel Canal",           54.40,  10.20,  54.25,   9.10),
    ("Corinth Canal",        37.95,  22.95,  37.88,  23.05),
    ("Bosphorus",            40.85,  28.90,  41.35,  29.15),
    ("Dardanelles",          40.50,  26.72,  40.00,  26.15),
    ("Malacca Strait",        1.30, 103.90,   1.15, 103.45),
    ("Singapore Strait",      1.30, 103.80,   1.22, 104.20),
    ("Messina Strait",       38.30,  15.65,  38.05,  15.67),
    ("Dover Strait",         51.15,   1.30,  50.85,   1.75),
];

struct BenchRoute {
    name: String,
    from_lat: f64,
    from_lon: f64,
    to_lat: f64,
    to_lon: f64,
    is_passage: bool,
}

struct RouteStats {
    name: String,
    distance_km: f64,
    raw_hops: usize,
    smooth_hops: usize,
    timings_us: Vec<u64>,
    is_passage: bool,
    coordinates: Vec<[f64; 2]>,
    from_lat: f64,
    from_lon: f64,
    to_lat: f64,
    to_lon: f64,
}

impl RouteStats {
    fn sorted_timings(&self) -> Vec<u64> {
        let mut t = self.timings_us.clone();
        t.sort_unstable();
        t
    }

    fn min_us(&self) -> u64 {
        *self.timings_us.iter().min().unwrap_or(&0)
    }

    fn max_us(&self) -> u64 {
        *self.timings_us.iter().max().unwrap_or(&0)
    }

    fn percentile(&self, p: f64) -> u64 {
        let sorted = self.sorted_timings();
        if sorted.is_empty() {
            return 0;
        }
        let idx = ((sorted.len() as f64 * p / 100.0) as usize).min(sorted.len() - 1);
        sorted[idx]
    }

    fn p50_us(&self) -> u64 {
        self.percentile(50.0)
    }

    fn p95_us(&self) -> u64 {
        self.percentile(95.0)
    }
}

#[derive(Serialize, Deserialize)]
struct BenchResult {
    graph: GraphMeta,
    commit: String,
    timestamp: String,
    iterations: usize,
    routes: Vec<RouteBenchResult>,
}

#[derive(Serialize, Deserialize)]
struct GraphMeta {
    nodes: u32,
    edges: u32,
    file: String,
}

#[derive(Serialize, Deserialize)]
struct RouteBenchResult {
    name: String,
    distance_km: f64,
    raw_hops: usize,
    smooth_hops: usize,
    min_us: u64,
    p50_us: u64,
    p95_us: u64,
    max_us: u64,
}

fn format_time(us: u64) -> String {
    if us < 1_000 {
        format!("{}us", us)
    } else if us < 1_000_000 {
        format!("{:.1}ms", us as f64 / 1_000.0)
    } else {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    }
}

fn format_number(n: u32) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

fn git_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Resolve hardcoded routes against the loaded graph.
///
/// Skips routes whose endpoints can't be snapped to graph nodes or
/// aren't in the same connected component.
fn resolve_routes(app: &AppState) -> Vec<BenchRoute> {
    let mut routes = Vec::new();

    for (i, &(name, from_lat, from_lon, to_lat, to_lon)) in ROUTES.iter().enumerate() {
        let from_node = app.nearest_node(from_lat, from_lon);
        let to_node = app.nearest_node(to_lat, to_lon);

        match (from_node, to_node) {
            (Some(_), Some(_)) => {
                // Validate routability with a test computation
                let knn = |lat: f64, lon: f64| app.nearest_node(lat, lon);
                if compute_route(&app.graph, from_lat, from_lon, to_lat, to_lon, &app.coastline, &knn).is_none() {
                    info!("  SKIP {} (no route found)", name);
                    continue;
                }
                routes.push(BenchRoute {
                    name: name.to_string(),
                    from_lat,
                    from_lon,
                    to_lat,
                    to_lon,
                    is_passage: i >= NUM_SAILING_ROUTES,
                });
                info!("  OK   {}", name);
            }
            _ => {
                info!("  SKIP {} (endpoints not in graph)", name);
            }
        }
    }

    routes
}

/// Run benchmark iterations for each route.
fn run_benchmark(
    app: &AppState,
    graph: &RoutingGraph,
    routes: &[BenchRoute],
    iterations: usize,
) -> Vec<RouteStats> {
    let warmup = 3;
    let knn = |lat: f64, lon: f64| app.nearest_node(lat, lon);

    routes
        .iter()
        .map(|route| {
            // Warmup
            for _ in 0..warmup {
                let _ = compute_route(
                    graph,
                    route.from_lat,
                    route.from_lon,
                    route.to_lat,
                    route.to_lon,
                    &app.coastline,
                    &knn,
                );
            }

            // Capture metadata from first measured run
            let first = compute_route(
                graph,
                route.from_lat,
                route.from_lon,
                route.to_lat,
                route.to_lon,
                &app.coastline,
                &knn,
            );
            let (distance_km, raw_hops, smooth_hops, coordinates) = match &first {
                Some(r) => (r.distance_km, r.raw_hops, r.smooth_hops, r.coordinates.clone()),
                None => (0.0, 0, 0, Vec::new()),
            };

            // Measured iterations
            let mut timings_us = Vec::with_capacity(iterations);

            for _ in 0..iterations {
                let start = Instant::now();
                let _ = compute_route(
                    graph,
                    route.from_lat,
                    route.from_lon,
                    route.to_lat,
                    route.to_lon,
                    &app.coastline,
                    &knn,
                );
                timings_us.push(start.elapsed().as_micros() as u64);
            }

            RouteStats {
                name: route.name.clone(),
                distance_km,
                raw_hops,
                smooth_hops,
                timings_us,
                is_passage: route.is_passage,
                coordinates,
                from_lat: route.from_lat,
                from_lon: route.from_lon,
                to_lat: route.to_lat,
                to_lon: route.to_lon,
            }
        })
        .collect()
}

/// Print results as a formatted terminal table.
fn print_table(stats: &[RouteStats], graph: &RoutingGraph, iterations: usize) {
    println!(
        "\nasw bench - {} routes x {} iterations ({} nodes, {} edges)\n",
        stats.len(),
        iterations,
        graph.num_nodes,
        graph.num_edges,
    );
    println!(
        "{:<20} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "Route", "Distance", "Min", "P50", "P95", "Max", "Hops"
    );
    println!("{}", "-".repeat(90));

    for s in stats {
        println!(
            "{:<20} {:>9.1}km {:>10} {:>10} {:>10} {:>10} {:>5}>{:<4}",
            s.name,
            s.distance_km,
            format_time(s.min_us()),
            format_time(s.p50_us()),
            format_time(s.p95_us()),
            format_time(s.max_us()),
            s.raw_hops,
            s.smooth_hops,
        );
    }
    println!();
}

/// Build a JSON-serializable result.
fn build_result(
    stats: &[RouteStats],
    graph: &RoutingGraph,
    graph_path: &str,
    iterations: usize,
) -> BenchResult {
    BenchResult {
        graph: GraphMeta {
            nodes: graph.num_nodes,
            edges: graph.num_edges,
            file: graph_path.to_string(),
        },
        commit: git_commit(),
        timestamp: chrono_now(),
        iterations,
        routes: stats
            .iter()
            .map(|s| RouteBenchResult {
                name: s.name.clone(),
                distance_km: s.distance_km,
                raw_hops: s.raw_hops,
                smooth_hops: s.smooth_hops,
                min_us: s.min_us(),
                p50_us: s.p50_us(),
                p95_us: s.p95_us(),
                max_us: s.max_us(),
            })
            .collect(),
    }
}

/// ISO 8601 timestamp without pulling in chrono.
fn chrono_now() -> String {
    // Use system time
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    // Simple UTC conversion
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    // Days since epoch to Y-M-D (simplified)
    let mut y = 1970i64;
    let mut remaining_days = days as i64;

    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        y += 1;
    }

    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining_days < md as i64 {
            m = i + 1;
            break;
        }
        remaining_days -= md as i64;
    }
    let d = remaining_days + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hours, minutes, seconds
    )
}

/// Print comparison table between current and baseline results.
fn print_comparison(current: &BenchResult, baseline: &BenchResult) -> bool {
    println!(
        "\nComparing against baseline (commit {}, {})\n",
        baseline.commit, baseline.timestamp
    );
    println!(
        "{:<20} {:>12} {:>12} {:>10} {:>10}",
        "Route", "P50 before", "P50 now", "Delta", "Status"
    );
    println!("{}", "-".repeat(74));

    let mut has_regression = false;

    for current_route in &current.routes {
        let baseline_route = baseline.routes.iter().find(|r| r.name == current_route.name);
        match baseline_route {
            Some(base) => {
                let delta_pct = if base.p50_us > 0 {
                    ((current_route.p50_us as f64 - base.p50_us as f64) / base.p50_us as f64)
                        * 100.0
                } else {
                    0.0
                };
                let status = if delta_pct > 10.0 {
                    has_regression = true;
                    "REGRESSION"
                } else if delta_pct < -10.0 {
                    "IMPROVED"
                } else {
                    "OK"
                };
                println!(
                    "{:<20} {:>12} {:>12} {:>+9.1}% {:>10}",
                    current_route.name,
                    format_time(base.p50_us),
                    format_time(current_route.p50_us),
                    delta_pct,
                    status,
                );
            }
            None => {
                println!(
                    "{:<20} {:>12} {:>12} {:>10} {:>10}",
                    current_route.name, "-", format_time(current_route.p50_us), "-", "NEW"
                );
            }
        }
    }
    println!();

    has_regression
}

/// Write markdown benchmark results to `benchmarks/BENCHMARKS.md`.
fn write_markdown(
    stats: &[RouteStats],
    graph: &RoutingGraph,
    iterations: usize,
) -> Result<()> {
    std::fs::create_dir_all("benchmarks")?;

    let commit = git_commit();
    let date = &chrono_now()[..10]; // YYYY-MM-DD

    let sailing: Vec<&RouteStats> = stats.iter().filter(|s| !s.is_passage).collect();
    let passages: Vec<&RouteStats> = stats.iter().filter(|s| s.is_passage).collect();

    let mut md = String::new();
    md.push_str("# Routing Benchmarks\n\n");
    md.push_str(&format!(
        "**Graph**: {} nodes / {} edges\n",
        format_number(graph.num_nodes),
        format_number(graph.num_edges),
    ));
    md.push_str(&format!(
        "**Commit**: `{}` | **Date**: {} | **Iterations**: {}\n\n",
        commit, date, iterations
    ));

    // Sailing routes section
    if !sailing.is_empty() {
        md.push_str("## Sailing Routes\n\n");
        md.push_str("| Route | Distance | Min | P50 | P95 | Max | Hops |\n");
        md.push_str("|-------|----------|-----|-----|-----|-----|------|\n");
        for s in &sailing {
            md.push_str(&format!(
                "| {} | {:.1}km | {} | {} | {} | {} | {}>{} |\n",
                s.name,
                s.distance_km,
                format_time(s.min_us()),
                format_time(s.p50_us()),
                format_time(s.p95_us()),
                format_time(s.max_us()),
                s.raw_hops,
                s.smooth_hops,
            ));
        }
        md.push('\n');
    }

    // Passage transits section
    if !passages.is_empty() {
        md.push_str("## Passage Transits\n\n");
        md.push_str("| Route | Distance | Min | P50 | P95 | Max | Hops |\n");
        md.push_str("|-------|----------|-----|-----|-----|-----|------|\n");
        for s in &passages {
            md.push_str(&format!(
                "| {} | {:.1}km | {} | {} | {} | {} | {}>{} |\n",
                s.name,
                s.distance_km,
                format_time(s.min_us()),
                format_time(s.p50_us()),
                format_time(s.p95_us()),
                format_time(s.max_us()),
                s.raw_hops,
                s.smooth_hops,
            ));
        }
        md.push('\n');
    }

    md.push_str("*Generated by `asw bench`*\n");

    std::fs::write("benchmarks/BENCHMARKS.md", &md)
        .context("Failed to write benchmarks/BENCHMARKS.md")?;
    info!("Markdown results written to benchmarks/BENCHMARKS.md");

    Ok(())
}

/// Write GeoJSON visualization of benchmark routes to `benchmarks/bench-routes.geojson`.
fn write_geojson(stats: &[RouteStats]) -> Result<()> {
    std::fs::create_dir_all("benchmarks")?;

    let mut features = Vec::new();

    for s in stats {
        if s.coordinates.is_empty() {
            continue;
        }

        let stroke = if s.is_passage { "#0000ff" } else { "#0088ff" };
        let category = if s.is_passage { "passage" } else { "sailing" };

        // Route line
        let coords: Vec<serde_json::Value> = s
            .coordinates
            .iter()
            .map(|c| serde_json::json!([c[0], c[1]]))
            .collect();

        features.push(serde_json::json!({
            "type": "Feature",
            "properties": {
                "name": s.name,
                "distance_km": format!("{:.1}", s.distance_km),
                "category": category,
                "stroke": stroke,
                "stroke-width": 3,
                "stroke-opacity": 0.8
            },
            "geometry": {
                "type": "LineString",
                "coordinates": coords
            }
        }));

        // Start point (original input coordinate)
        features.push(serde_json::json!({
            "type": "Feature",
            "properties": {
                "name": format!("{} (start)", s.name),
                "marker-color": "#00cc00",
                "marker-size": "small",
                "marker-symbol": "circle"
            },
            "geometry": {
                "type": "Point",
                "coordinates": [s.from_lon, s.from_lat]
            }
        }));

        // End point (original input coordinate)
        features.push(serde_json::json!({
            "type": "Feature",
            "properties": {
                "name": format!("{} (end)", s.name),
                "marker-color": "#ff0000",
                "marker-size": "small",
                "marker-symbol": "circle"
            },
            "geometry": {
                "type": "Point",
                "coordinates": [s.to_lon, s.to_lat]
            }
        }));
    }

    let geojson = serde_json::json!({
        "type": "FeatureCollection",
        "features": features
    });

    let json_str = serde_json::to_string_pretty(&geojson)
        .context("Failed to serialize GeoJSON")?;
    std::fs::write("benchmarks/bench-routes.geojson", json_str)
        .context("Failed to write bench-routes.geojson")?;

    info!("GeoJSON routes written to benchmarks/bench-routes.geojson");
    Ok(())
}

/// Entry point for `asw bench`.
pub fn run(
    graph_path: &Path,
    iterations: usize,
    json: bool,
    output: Option<&Path>,
    compare: Option<&Path>,
) -> Result<()> {
    info!("Loading graph from {:?}...", graph_path);
    let file = std::fs::File::open(graph_path).context("Failed to open graph file")?;
    let reader = std::io::BufReader::new(file);
    let graph = RoutingGraph::load(reader).context("Failed to load graph")?;
    info!(
        "Graph loaded: {} nodes, {} edges",
        graph.num_nodes, graph.num_edges
    );

    info!("Building app state...");
    let app = AppState::new(graph);
    info!("App state ready");

    info!("Resolving benchmark routes...");
    let routes = resolve_routes(&app);
    if routes.is_empty() {
        anyhow::bail!("No routable benchmark routes found in graph");
    }
    info!("Resolved {} benchmark routes", routes.len());

    for route in &routes {
        let dist = haversine_km(route.from_lat, route.from_lon, route.to_lat, route.to_lon);
        info!(
            "  {} ({:.1}km): ({:.4},{:.4}) -> ({:.4},{:.4}){}",
            route.name, dist, route.from_lat, route.from_lon, route.to_lat, route.to_lon,
            if route.is_passage { " [passage]" } else { "" }
        );
    }

    info!("Running {} iterations per route...", iterations);
    let stats = run_benchmark(&app, &app.graph, &routes, iterations);

    let graph_path_str = graph_path.display().to_string();
    let result = build_result(&stats, &app.graph, &graph_path_str, iterations);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).context("Failed to serialize results")?
        );
    } else {
        print_table(&stats, &app.graph, iterations);
    }

    if let Some(out_path) = output {
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize results")?;
        std::fs::write(out_path, json_str).context("Failed to write output file")?;
        info!("Results written to {:?}", out_path);
    }

    // Always write markdown and GeoJSON
    write_markdown(&stats, &app.graph, iterations)?;
    write_geojson(&stats)?;

    if let Some(compare_path) = compare {
        let baseline_str =
            std::fs::read_to_string(compare_path).context("Failed to read baseline file")?;
        let baseline: BenchResult =
            serde_json::from_str(&baseline_str).context("Failed to parse baseline JSON")?;
        let has_regression = print_comparison(&result, &baseline);
        if has_regression {
            std::process::exit(1);
        }
    }

    Ok(())
}
