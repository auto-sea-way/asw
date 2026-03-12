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

    let resp = client
        .get(url)
        .send()
        .context("Failed to start graph download")?;

    if !resp.status().is_success() {
        bail!("Download failed: HTTP {}", resp.status());
    }

    let total = resp.content_length();
    if let Some(size) = total {
        info!("Download size: {:.0} MB", size as f64 / 1_000_000.0);
    }

    let mut reader = resp;
    let mut file = std::fs::File::create(&tmp_path)
        .with_context(|| format!("Failed to create {:?}", tmp_path))?;

    let mut downloaded: u64 = 0;
    let mut last_logged: u64 = 0;
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = std::io::Read::read(&mut reader, &mut buf).context("Download read error")?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).context("Failed to write graph file")?;
        downloaded += n as u64;

        if downloaded - last_logged >= 50_000_000 {
            if let Some(total) = total {
                info!(
                    "Downloaded {:.0}/{:.0} MB ({:.0}%)",
                    downloaded as f64 / 1_000_000.0,
                    total as f64 / 1_000_000.0,
                    (downloaded as f64 / total as f64) * 100.0
                );
            } else {
                info!("Downloaded {:.0} MB", downloaded as f64 / 1_000_000.0);
            }
            last_logged = downloaded;
        }
    }

    drop(file);

    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("Failed to rename {:?} to {:?}", tmp_path, path))?;

    info!(
        "Graph downloaded: {:.1} MB",
        downloaded as f64 / 1_000_000.0
    );

    Ok(())
}
