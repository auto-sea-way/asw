use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use h3o::CellIndex;
use std::io::Write;
use std::path::PathBuf;
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
        #[arg(long)]
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
        #[arg(long, default_value = "export/asw.graph")]
        graph: PathBuf,

        /// Listen address
        #[arg(long, default_value = "0.0.0.0:3000")]
        listen: String,
    },
    /// Export graph as KML for visualization in Google Earth
    Kml {
        /// Path to the graph file
        #[arg(long, default_value = "export/asw.graph")]
        graph: PathBuf,

        /// Output KML file path
        #[arg(long, default_value = "export/asw.kml")]
        output: PathBuf,

        /// Include edges (can be very large)
        #[arg(long)]
        edges: bool,

        /// Include coastline segments
        #[arg(long)]
        coastline: bool,

        /// Render hex boundaries instead of dots
        #[arg(long)]
        hexes: bool,
    },
    /// Cloud build: provision server, build remotely, download result
    Cloud {
        #[command(subcommand)]
        action: CloudAction,
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
        #[arg(long)]
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
        .parent()  // crates/
        .and_then(|p| p.parent())  // workspace root
        .map(|p| p.to_path_buf())
        .unwrap_or(manifest_dir)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
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
        Commands::Serve { graph, listen } => {
            info!("Loading graph from {:?}...", graph);
            let file = std::fs::File::open(&graph).context("Failed to open graph file")?;
            let reader = std::io::BufReader::new(file);
            let routing_graph =
                asw_core::graph::RoutingGraph::load(reader).context("Failed to load graph")?;

            info!(
                "Graph loaded: {} nodes, {} edges",
                routing_graph.num_nodes, routing_graph.num_edges
            );

            let state = std::sync::Arc::new(asw_serve::state::AppState::new(routing_graph));

            info!(
                "Coastline: {} segments, Node tree ready",
                state.coastline.segment_count()
            );

            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let app = asw_serve::api::create_router(state);
                let listener = tokio::net::TcpListener::bind(&listen).await?;
                info!("Listening on {}", listen);
                axum::serve(listener, app).await?;
                Ok::<(), anyhow::Error>(())
            })?;
        }
        Commands::Kml {
            graph,
            output,
            edges,
            coastline,
            hexes,
        } => {
            export_kml(&graph, &output, edges, coastline, hexes)?;
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

fn export_kml(graph_path: &PathBuf, output: &PathBuf, include_edges: bool, include_coastline: bool, include_hexes: bool) -> Result<()> {
    info!("Loading graph from {:?}...", graph_path);
    let file = std::fs::File::open(graph_path).context("Failed to open graph file")?;
    let reader = std::io::BufReader::new(file);
    let graph = asw_core::graph::RoutingGraph::load(reader).context("Failed to load graph")?;

    info!("Graph: {} nodes, {} edges", graph.num_nodes, graph.num_edges);

    let mut out = std::io::BufWriter::new(
        std::fs::File::create(output).context("Failed to create KML file")?,
    );

    // KML header
    write!(out, r#"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2">
<Document>
<name>asw graph</name>
<description>{} nodes, {} edges</description>

<!-- Styles -->
<Style id="node-ocean">
  <IconStyle>
    <color>ffff8800</color>
    <scale>0.4</scale>
    <Icon><href>http://maps.google.com/mapfiles/kml/shapes/shaded_dot.png</href></Icon>
  </IconStyle>
  <LabelStyle><scale>0</scale></LabelStyle>
</Style>
<Style id="node-coastal">
  <IconStyle>
    <color>ff00aaff</color>
    <scale>0.3</scale>
    <Icon><href>http://maps.google.com/mapfiles/kml/shapes/shaded_dot.png</href></Icon>
  </IconStyle>
  <LabelStyle><scale>0</scale></LabelStyle>
</Style>
<Style id="edge">
  <LineStyle>
    <color>8800ff00</color>
    <width>1</width>
  </LineStyle>
</Style>
<Style id="coastline">
  <LineStyle>
    <color>ff0000ff</color>
    <width>1.5</width>
  </LineStyle>
</Style>
<Style id="hex-ocean">
  <PolyStyle>
    <color>60ff8800</color>
    <outline>1</outline>
  </PolyStyle>
  <LineStyle>
    <color>ffff8800</color>
    <width>1</width>
  </LineStyle>
</Style>
<Style id="hex-mid">
  <PolyStyle>
    <color>6000cc00</color>
    <outline>1</outline>
  </PolyStyle>
  <LineStyle>
    <color>ff00cc00</color>
    <width>1</width>
  </LineStyle>
</Style>
<Style id="hex-coastal">
  <PolyStyle>
    <color>6000aaff</color>
    <outline>1</outline>
  </PolyStyle>
  <LineStyle>
    <color>ff00aaff</color>
    <width>1</width>
  </LineStyle>
</Style>
<Style id="hex-near">
  <PolyStyle>
    <color>60ffff00</color>
    <outline>1</outline>
  </PolyStyle>
  <LineStyle>
    <color>ffffff00</color>
    <width>1</width>
  </LineStyle>
</Style>
<Style id="hex-shore">
  <PolyStyle>
    <color>60ff00ff</color>
    <outline>1</outline>
  </PolyStyle>
  <LineStyle>
    <color>ffff00ff</color>
    <width>1</width>
  </LineStyle>
</Style>
"#, graph.num_nodes, graph.num_edges)?;

    // Nodes folder
    if include_hexes {
        write!(out, "<Folder>\n<name>Hexes ({} total)</name>\n", graph.num_nodes)?;
        for i in 0..graph.num_nodes {
            let cell_u64 = graph.node_cells[i as usize];
            if cell_u64 == 0 {
                continue; // synthetic node, no hex
            }
            let cell = CellIndex::try_from(cell_u64);
            if let Ok(cell) = cell {
                let boundary = asw_core::h3::cell_boundary(cell);
                let res = cell.resolution() as u8;
                let style = match res {
                    0..=3 => "#hex-ocean",
                    4..=5 => "#hex-mid",
                    6..=7 => "#hex-coastal",
                    8 => "#hex-near",
                    _ => "#hex-shore",
                };
                write!(out, "<Placemark><styleUrl>{}</styleUrl><Polygon><outerBoundaryIs><LinearRing><coordinates>", style)?;
                for (j, &(lat, lon)) in boundary.iter().enumerate() {
                    if j > 0 {
                        write!(out, " ")?;
                    }
                    write!(out, "{},{},0", lon, lat)?;
                }
                // Close the ring
                if let Some(&(lat, lon)) = boundary.first() {
                    write!(out, " {},{},0", lon, lat)?;
                }
                write!(out, "</coordinates></LinearRing></outerBoundaryIs></Polygon></Placemark>\n")?;
            }
        }
        write!(out, "</Folder>\n")?;
    } else {
        write!(out, "<Folder>\n<name>Nodes ({} total)</name>\n", graph.num_nodes)?;
        for i in 0..graph.num_nodes {
            let lat = graph.node_lats[i as usize];
            let lon = graph.node_lngs[i as usize];
            let cell_u64 = graph.node_cells[i as usize];
            let style = if cell_u64 != 0 {
                let cell = CellIndex::try_from(cell_u64);
                if let Ok(c) = cell {
                    let r = c.resolution() as u8;
                    if r <= 3 { "#node-ocean" } else if r <= 5 { "#node-ocean" } else { "#node-coastal" }
                } else {
                    "#node-coastal"
                }
            } else {
                "#node-coastal"
            };
            write!(
                out,
                "<Placemark><styleUrl>{}</styleUrl><Point><coordinates>{},{},0</coordinates></Point></Placemark>\n",
                style, lon, lat
            )?;
        }
        write!(out, "</Folder>\n")?;
    }

    // Edges folder
    if include_edges {
        let edge_count = graph.num_edges / 2; // bidirectional, count once
        write!(out, "<Folder>\n<name>Edges (~{} unique)</name>\n", edge_count)?;

        // Deduplicate: only emit edge where source < target
        for src in 0..graph.num_nodes {
            let src_lat = graph.node_lats[src as usize];
            let src_lon = graph.node_lngs[src as usize];
            for (dst, _weight) in graph.edges_with_weights(src) {
                if src < dst {
                    let dst_lat = graph.node_lats[dst as usize];
                    let dst_lon = graph.node_lngs[dst as usize];
                    write!(
                        out,
                        "<Placemark><styleUrl>#edge</styleUrl><LineString><coordinates>{},{},0 {},{},0</coordinates></LineString></Placemark>\n",
                        src_lon, src_lat, dst_lon, dst_lat
                    )?;
                }
            }
        }
        write!(out, "</Folder>\n")?;
    }

    // Coastline folder
    if include_coastline && !graph.coastline_coords.is_empty() {
        write!(
            out,
            "<Folder>\n<name>Coastline ({} segments)</name>\n",
            graph.coastline_coords.len()
        )?;

        for seg in &graph.coastline_coords {
            if seg.len() < 2 {
                continue;
            }
            write!(out, "<Placemark><styleUrl>#coastline</styleUrl><LineString><coordinates>")?;
            for (j, &(lon, lat)) in seg.iter().enumerate() {
                if j > 0 {
                    write!(out, " ")?;
                }
                write!(out, "{},{},0", lon, lat)?;
            }
            write!(out, "</coordinates></LineString></Placemark>\n")?;
        }
        write!(out, "</Folder>\n")?;
    }

    // KML footer
    write!(out, "</Document>\n</kml>\n")?;
    out.flush()?;

    let file_size = std::fs::metadata(output)?.len();
    info!(
        "KML exported to {:?} ({:.1} MB)",
        output,
        file_size as f64 / 1_000_000.0
    );

    Ok(())
}
