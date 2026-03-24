mod bench;
mod download;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::info;

#[derive(Parser)]
#[command(name = "asw", about = "Maritime routing graph builder and server")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the routing graph from water polygon shapefiles
    Build {
        /// Path to land_polygons.shp or directory of split shapefiles (downloads if not provided)
        #[arg(long)]
        shp: Option<PathBuf>,

        /// Bounding box: min_lon,min_lat,max_lon,max_lat or preset name (dev, dev-small, marmaris)
        #[arg(long, allow_hyphen_values = true)]
        bbox: Option<String>,

        /// Output graph file path
        #[arg(long, default_value = "export/asw.graph")]
        output: PathBuf,

        /// Working directory for downloads
        #[arg(long, default_value = ".")]
        workdir: PathBuf,
    },
    /// Serve the routing API over HTTP
    Serve {
        /// Path to the graph file
        #[arg(long, env = "ASW_GRAPH", default_value = "export/asw.graph")]
        graph: PathBuf,

        /// Bind address
        #[arg(long, env = "ASW_HOST", default_value = "0.0.0.0")]
        host: String,

        /// Listen port
        #[arg(long, env = "ASW_PORT", default_value = "3000")]
        port: u16,

        /// URL to download graph from if file doesn't exist
        #[arg(long, env = "ASW_GRAPH_URL")]
        graph_url: Option<String>,

        /// API key for authenticating requests (required)
        #[arg(long, env = "ASW_API_KEY")]
        api_key: String,
    },
    /// Export graph as GeoJSON for visualization
    Geojson {
        /// Path to the graph file
        #[arg(long, default_value = "export/asw.graph")]
        graph: PathBuf,

        /// Output GeoJSON file path
        #[arg(long, default_value = "export/asw.geojson")]
        output: PathBuf,

        /// Include coastline segments
        #[arg(long)]
        coastline: bool,

        /// Bounding box: preset name or min_lon,min_lat,max_lon,max_lat
        #[arg(long, allow_hyphen_values = true)]
        bbox: Option<String>,
    },
    /// Cloud build: provision server, build remotely, download result
    Cloud {
        #[command(subcommand)]
        action: CloudAction,
    },
    /// Benchmark routing performance
    Bench {
        /// Path to the graph file
        #[arg(long, env = "ASW_GRAPH", default_value = "export/asw.graph")]
        graph: PathBuf,

        /// Measured iterations per route
        #[arg(long, default_value_t = 50)]
        iterations: usize,

        /// Output results as JSON to stdout
        #[arg(long)]
        json: bool,

        /// Write JSON results to file
        #[arg(long)]
        output: Option<PathBuf>,

        /// Compare against previous JSON baseline
        #[arg(long)]
        compare: Option<PathBuf>,
    },
    /// Health check: exit 0 if server is ready, 1 otherwise (for Docker HEALTHCHECK)
    Healthcheck {
        /// Server port to check
        #[arg(long, env = "ASW_PORT", default_value = "3000")]
        port: u16,

        /// Server host to check
        #[arg(long, env = "ASW_HOST", default_value = "127.0.0.1")]
        host: String,
    },
}

#[derive(Subcommand)]
enum CloudAction {
    /// Full remote build: provision → upload → compile → build → download
    Build {
        /// Hetzner API token
        #[arg(long, env = "HETZNER_TOKEN")]
        hetzner_token: String,

        /// Bounding box: preset name (dev, dev-small, marmaris) or min_lon,min_lat,max_lon,max_lat
        #[arg(long, allow_hyphen_values = true)]
        bbox: Option<String>,

        /// Output graph file path
        #[arg(short, long, default_value = "export/asw.graph")]
        output: PathBuf,

        /// Keep server alive after build
        #[arg(long)]
        keep_server: bool,

        /// SSH private key path (auto-detected if not specified)
        #[arg(long)]
        ssh_key: Option<PathBuf>,
    },
    /// Provision a Hetzner server (create + bootstrap)
    Provision {
        /// Hetzner API token
        #[arg(long, env = "HETZNER_TOKEN")]
        hetzner_token: String,

        /// SSH private key path (auto-detected if not specified)
        #[arg(long)]
        ssh_key: Option<PathBuf>,
    },
    /// Teardown: delete the Hetzner server
    Teardown {
        /// Hetzner API token
        #[arg(long, env = "HETZNER_TOKEN")]
        hetzner_token: String,
    },
    /// Check server status
    Status {
        /// Hetzner API token
        #[arg(long, env = "HETZNER_TOKEN")]
        hetzner_token: String,
    },
}

fn parse_bbox(s: &str) -> Result<(f64, f64, f64, f64)> {
    // Try preset first
    if let Some(bbox) = asw_cloud::config::resolve_bbox(s) {
        return Ok(bbox);
    }

    // Parse as comma-separated floats
    let parts: Vec<f64> = s
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<Vec<_>, _>>()
        .context("bbox must be 4 comma-separated floats: min_lon,min_lat,max_lon,max_lat")?;
    if parts.len() != 4 {
        anyhow::bail!("bbox must have exactly 4 values");
    }
    Ok((parts[0], parts[1], parts[2], parts[3]))
}

fn resolve_ssh_key(ssh_key: Option<PathBuf>) -> Result<PathBuf> {
    match ssh_key {
        Some(p) => Ok(p),
        None => asw_cloud::ssh::find_ssh_key(),
    }
}

/// Locate the workspace root relative to the CLI binary's compiled location.
fn rust_src_dir() -> PathBuf {
    // At compile time, CARGO_MANIFEST_DIR points to crates/asw-cli
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Go up to workspace root
    manifest_dir
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .map(|p| p.to_path_buf())
        .unwrap_or(manifest_dir)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load .env before clap parses env vars
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Build {
            shp,
            bbox,
            output,
            workdir,
        } => {
            let bbox = bbox.map(|b| parse_bbox(&b)).transpose()?;

            let shp_path = match shp {
                Some(p) => p,
                None => {
                    info!("No shapefile provided, downloading...");
                    asw_build::shapefile::download_and_extract(&workdir)?
                }
            };

            asw_build::pipeline::run(&shp_path, bbox, &output)?;
            info!("Build complete!");
        }
        Commands::Serve {
            graph,
            host,
            port,
            graph_url,
            api_key,
        } => {
            // Download graph if missing
            download::ensure_graph(&graph, graph_url.as_deref())?;

            let api_key = api_key.trim().to_string();
            if api_key.is_empty() {
                anyhow::bail!("ASW_API_KEY must not be empty or whitespace-only");
            }

            let listen = format!("{}:{}", host, port);
            let graph_path = graph.display().to_string();

            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                // Create server state (graph not loaded yet)
                let state =
                    std::sync::Arc::new(asw_serve::state::ServerState::new(graph_path, api_key));

                let app = asw_serve::api::create_router(state.clone());
                let listener = tokio::net::TcpListener::bind(&listen).await?;
                info!("Listening on {}", listen);

                // Load graph in background
                let graph_file = graph.clone();
                let bg_state = state.clone();
                let load_handle = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                    info!("Loading graph from {:?}...", graph_file);
                    let file =
                        std::fs::File::open(&graph_file).context("Failed to open graph file")?;
                    let reader = std::io::BufReader::new(file);
                    let routing_graph = asw_core::graph::RoutingGraph::load(reader)
                        .context("Failed to load graph")?;

                    info!(
                        "Graph loaded: {} nodes, {} edges",
                        routing_graph.num_nodes, routing_graph.num_edges
                    );

                    let app_state = asw_serve::state::AppState::new(routing_graph);
                    info!(
                        "Coastline: {} segments, Node tree ready",
                        app_state.coastline.segment_count()
                    );

                    bg_state.set_ready(app_state);
                    info!("Server ready");
                    Ok(())
                });

                // Monitor graph loading — exit on failure so orchestrator can restart
                tokio::spawn(async move {
                    match load_handle.await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::error!("Graph loading failed: {:#}", e);
                            std::process::exit(1);
                        }
                        Err(e) => {
                            tracing::error!("Graph loading task panicked: {}", e);
                            std::process::exit(1);
                        }
                    }
                });

                axum::serve(listener, app).await?;
                Ok::<(), anyhow::Error>(())
            })?;
        }
        Commands::Geojson {
            graph,
            output,
            coastline,
            bbox,
        } => {
            let bbox = bbox.map(|b| parse_bbox(&b)).transpose()?;
            export_geojson(&graph, &output, coastline, bbox)?;
        }
        Commands::Bench {
            graph,
            iterations,
            json,
            output,
            compare,
        } => {
            bench::run(
                &graph,
                iterations,
                json,
                output.as_deref(),
                compare.as_deref(),
            )?;
        }
        Commands::Healthcheck { port, host } => {
            let url = format!("http://{}:{}/ready", host, port);
            match reqwest::blocking::get(&url) {
                Ok(r) if r.status().is_success() => std::process::exit(0),
                _ => std::process::exit(1),
            }
        }
        Commands::Cloud { action } => match action {
            CloudAction::Build {
                hetzner_token,
                bbox,
                output,
                keep_server,
                ssh_key,
            } => {
                let ssh_key_path = resolve_ssh_key(ssh_key)?;
                let bbox = bbox.map(|b| parse_bbox(&b)).transpose()?;

                let mut pipeline = asw_cloud::pipeline::Pipeline {
                    host: None,
                    ssh_key_path,
                    output_path: output,
                    keep_server,
                    hetzner_token: Some(hetzner_token),
                    bbox,
                    rust_src_dir: rust_src_dir(),
                };
                pipeline.run()?;
            }
            CloudAction::Provision {
                hetzner_token,
                ssh_key,
            } => {
                let ssh_key_path = resolve_ssh_key(ssh_key)?;
                let ip = asw_cloud::hetzner::provision(&hetzner_token, &ssh_key_path)?;
                println!("Server ready: {}", ip);
            }
            CloudAction::Teardown { hetzner_token } => {
                asw_cloud::hetzner::teardown(&hetzner_token)?;
            }
            CloudAction::Status { hetzner_token } => {
                match asw_cloud::hetzner::status(&hetzner_token)? {
                    Some((id, ip, status)) => {
                        println!("Server: {} (id={}, status={})", ip, id, status);
                    }
                    None => {
                        println!("No server found.");
                    }
                }
            }
        },
    }

    Ok(())
}

/// Build a single GeoJSON feature string for a hex cell polygon.
fn hex_feature_string(boundary: &[(f64, f64)], res: u8, color: &str) -> String {
    let mut s = String::with_capacity(512);
    s.push_str(r#"{"type":"Feature","geometry":{"type":"Polygon","coordinates":[["#);
    for (j, &(lat, lon)) in boundary.iter().enumerate() {
        if j > 0 {
            s.push(',');
        }
        use std::fmt::Write as FmtWrite;
        write!(s, "[{},{}]", lon, lat).unwrap();
    }
    // Close the ring
    if let Some(&(lat, lon)) = boundary.first() {
        use std::fmt::Write as FmtWrite;
        write!(s, ",[{},{}]", lon, lat).unwrap();
    }
    use std::fmt::Write as FmtWrite;
    write!(
        s,
        r#"]]}},"properties":{{"layer":"hex-res-{}","fill":"{}","fill-opacity":0.38,"stroke":"{}","stroke-opacity":1.0,"stroke-width":1}}}}"#,
        res, color, color
    ).unwrap();
    s
}

/// Build a single GeoJSON feature string for a coastline segment.
fn coastline_feature_string(seg: &[(f32, f32)]) -> String {
    let mut s = String::with_capacity(256 + seg.len() * 24);
    s.push_str(r#"{"type":"Feature","geometry":{"type":"LineString","coordinates":["#);
    for (j, &(lon, lat)) in seg.iter().enumerate() {
        if j > 0 {
            s.push(',');
        }
        use std::fmt::Write as FmtWrite;
        write!(s, "[{},{}]", lon as f64, lat as f64).unwrap();
    }
    s.push_str(r##"]},"properties":{"layer":"coastline","stroke":"#ff0000","stroke-width":1.5}}"##);
    s
}

/// Write a GeoJSON FeatureCollection to the given path from pre-built feature strings.
fn write_feature_collection(path: &Path, features: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(path)
            .with_context(|| format!("Failed to create {}", path.display()))?,
    );
    write!(out, r#"{{"type":"FeatureCollection","features":["#)?;
    for (i, feat) in features.iter().enumerate() {
        if i > 0 {
            write!(out, ",")?;
        }
        write!(out, "{}", feat)?;
    }
    write!(out, "]}}")?;
    out.flush()?;
    Ok(())
}

fn export_geojson(
    graph_path: &Path,
    output: &Path,
    include_coastline: bool,
    bbox: Option<(f64, f64, f64, f64)>,
) -> Result<()> {
    info!("Loading graph from {:?}...", graph_path);
    let file = std::fs::File::open(graph_path).context("Failed to open graph file")?;
    let reader = std::io::BufReader::new(file);
    let graph = asw_core::graph::RoutingGraph::load(reader).context("Failed to load graph")?;

    info!(
        "Graph: {} nodes, {} edges",
        graph.num_nodes, graph.num_edges
    );

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Collect features by layer: hex-res-0 through hex-res-15, coastline
    // Index 0..=15 for hex resolutions, 16 for coastline
    const LAYER_COASTLINE: usize = 16;
    const NUM_LAYERS: usize = 17;
    let mut layers: Vec<Vec<String>> = vec![Vec::new(); NUM_LAYERS];

    // Hex polygons
    let mut hex_count: u64 = 0;
    for i in 0..graph.num_nodes as usize {
        let h3 = graph.node_h3[i];
        let cell = h3o::CellIndex::try_from(h3).expect("valid H3");
        let res = cell.resolution() as u8;

        let (lat, lng) = graph.node_pos(i as u32);

        // Bbox filter
        if let Some((min_lon, min_lat, max_lon, max_lat)) = bbox {
            if lng < min_lon || lng > max_lon || lat < min_lat || lat > max_lat {
                continue;
            }
        }

        let boundary = asw_core::h3::cell_boundary(cell);
        let color = match res {
            0..=3 => "#0088ff",
            4..=5 => "#00cc00",
            6..=7 => "#ffaa00",
            8 => "#00ffff",
            _ => "#ff00ff",
        };

        let feat = hex_feature_string(&boundary, res, color);
        layers[res as usize].push(feat);

        hex_count += 1;
        if hex_count.is_multiple_of(1_000_000) {
            info!("  processed {} hex features...", hex_count);
        }
    }

    // Coastline segments
    if include_coastline && !graph.coastline_coords.is_empty() {
        for seg in &graph.coastline_coords {
            if seg.len() < 2 {
                continue;
            }

            if let Some((min_lon, min_lat, max_lon, max_lat)) = bbox {
                let in_bbox = seg.iter().any(|&(lon, lat)| {
                    (lon as f64) >= min_lon
                        && (lon as f64) <= max_lon
                        && (lat as f64) >= min_lat
                        && (lat as f64) <= max_lat
                });
                if !in_bbox {
                    continue;
                }
            }

            let feat = coastline_feature_string(seg);
            layers[LAYER_COASTLINE].push(feat);
        }
    }

    // Derive base path: strip .geojson extension
    let stem = output
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = output.parent().unwrap_or(Path::new("."));

    // Write layer files: hexagons (all resolutions combined), passages, coastline
    let mut hex_features: Vec<String> = Vec::new();
    for layer in layers.iter_mut().take(16) {
        hex_features.append(layer);
    }
    if !hex_features.is_empty() {
        let hex_path = parent.join(format!("{}-hexagons.geojson", stem));
        write_feature_collection(&hex_path, &hex_features)?;
        let size = std::fs::metadata(&hex_path)?.len();
        info!(
            "  Layer {:?}: {} features, {:.1} MB",
            hex_path,
            hex_features.len(),
            size as f64 / 1_000_000.0
        );
    }

    if !layers[LAYER_COASTLINE].is_empty() {
        let coastline_path = parent.join(format!("{}-coastline.geojson", stem));
        write_feature_collection(&coastline_path, &layers[LAYER_COASTLINE])?;
        let size = std::fs::metadata(&coastline_path)?.len();
        info!(
            "  Layer {:?}: {} features, {:.1} MB",
            coastline_path,
            layers[LAYER_COASTLINE].len(),
            size as f64 / 1_000_000.0
        );
    }

    // Write combined file (all features in one FeatureCollection)
    let all_features: Vec<&String> = hex_features
        .iter()
        .chain(layers[LAYER_COASTLINE].iter())
        .collect();

    {
        let mut out = std::io::BufWriter::new(
            std::fs::File::create(output).context("Failed to create combined GeoJSON file")?,
        );
        write!(out, r#"{{"type":"FeatureCollection","features":["#)?;
        for (i, feat) in all_features.iter().enumerate() {
            if i > 0 {
                write!(out, ",")?;
            }
            write!(out, "{}", feat)?;
        }
        write!(out, "]}}")?;
        out.flush()?;
    }

    let total_features = all_features.len();
    let file_size = std::fs::metadata(output)?.len();
    info!(
        "Combined GeoJSON exported to {:?} ({} features, {:.1} MB)",
        output,
        total_features,
        file_size as f64 / 1_000_000.0
    );

    Ok(())
}
