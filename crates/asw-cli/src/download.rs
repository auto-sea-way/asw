use anyhow::{bail, Context, Result};
use std::path::Path;
use tracing::info;

/// Ensure the graph file exists at `path`, downloading from `url` if missing.
pub fn ensure_graph(path: &Path, url: Option<&str>) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    let url = match url {
        Some(u) => u,
        None => bail!(
            "Graph file not found at {:?}. Provide --graph-url or set ASW_GRAPH_URL to auto-download.",
            path
        ),
    };

    info!("Graph not found at {:?}, downloading from {}...", path, url);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {:?}", parent))?;
    }

    let tmp_path = path.with_extension("graph.tmp");
    // Remove stale temp from any prior failed download
    let _ = std::fs::remove_file(&tmp_path);

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .context("Failed to build HTTP client")?;

    let mut resp = client
        .get(url)
        .send()
        .context("Failed to start graph download")?
        .error_for_status()
        .context("Graph download returned a non-success HTTP status")?;
    if let Some(size) = resp.content_length() {
        info!("Download size: {:.0} MB", size as f64 / 1_000_000.0);
    }

    let mut file = std::fs::File::create(&tmp_path)
        .with_context(|| format!("Failed to create {:?}", tmp_path))?;
    let downloaded = std::io::copy(&mut resp, &mut file).context("Failed to write graph file")?;
    drop(file);

    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("Failed to rename {:?} to {:?}", tmp_path, path))?;
    info!(
        "Graph downloaded: {:.1} MB",
        downloaded as f64 / 1_000_000.0
    );
    Ok(())
}
