use anyhow::{bail, Context, Result};
use std::path::Path;
use tracing::info;

/// True if the URL path (without query or fragment) ends in `.zst`.
fn is_zst(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_ascii_lowercase().ends_with(".zst")
}

/// Ensure the graph file exists at `path`, downloading from `url` if missing.
/// A `.zst` URL is unpacked while downloading; the result must open as a graph.
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

    // Unpack and check; on any failure the temp file is removed.
    let fetched = (|| -> Result<u64> {
        let mut file = std::fs::File::create(&tmp_path)
            .with_context(|| format!("Failed to create {:?}", tmp_path))?;
        // The zstd decoder verifies the content checksum at end of stream.
        let n = if is_zst(url) {
            let mut dec = zstd::stream::read::Decoder::new(resp)?;
            std::io::copy(&mut dec, &mut file)
        } else {
            std::io::copy(&mut resp, &mut file)
        }
        .context("Failed to write graph file")?;
        file.sync_all()?;
        drop(file);
        asw_core::graph::RoutingGraph::open(&tmp_path, false)?;
        Ok(n)
    })()
    .context("graph download is corrupt or incomplete");
    let downloaded = match fetched {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("Failed to rename {:?} to {:?}", tmp_path, path))?;
    info!(
        "Graph downloaded: {:.1} MB",
        downloaded as f64 / 1_000_000.0
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use asw_core::graph::{GraphBuilder, RoutingGraph};
    use std::io::{Read, Write};

    /// Serve `body` once with the given status line; returns the URL.
    fn serve_once(status: &'static str, body: Vec<u8>, name: &str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&body);
        });
        format!("http://127.0.0.1:{port}/{name}")
    }

    fn zst(data: &[u8]) -> Vec<u8> {
        let mut enc = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
        enc.include_checksum(true).unwrap();
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn graph_bytes() -> Vec<u8> {
        let p = dest();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let cell = h3o::LatLng::new(36.5, 28.3)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let mut b = GraphBuilder::with_version("test");
        b.add_node(u64::from(cell), 255);
        b.build().save(&p).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        bytes
    }

    /// A fresh destination path in a unique directory under the system temp dir.
    fn dest() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir()
            .join(format!(
                "asw-dl-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ))
            .join("asw.graph")
    }

    fn nothing_left(p: &Path) {
        assert!(!p.exists());
        assert!(!p.with_extension("graph.tmp").exists());
    }

    #[test]
    fn zst_download_unpacks_and_opens() {
        let p = dest();
        let url = serve_once("200 OK", zst(&graph_bytes()), "asw.graph.zst");
        ensure_graph(&p, Some(&url)).unwrap();
        RoutingGraph::open(&p, false).unwrap();
        assert!(!p.with_extension("graph.tmp").exists());
    }

    #[test]
    fn plain_download_still_works() {
        let p = dest();
        let bytes = graph_bytes();
        let url = serve_once("200 OK", bytes.clone(), "asw.graph");
        ensure_graph(&p, Some(&url)).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
    }

    #[test]
    fn zst_truncated_leaves_nothing() {
        let p = dest();
        let z = zst(&graph_bytes());
        let url = serve_once("200 OK", z[..z.len() / 2].to_vec(), "asw.graph.zst");
        assert!(ensure_graph(&p, Some(&url)).is_err());
        nothing_left(&p);
    }

    #[test]
    fn zst_not_a_graph_leaves_nothing() {
        let p = dest();
        let url = serve_once("200 OK", zst(b"hello"), "asw.graph.zst");
        assert!(ensure_graph(&p, Some(&url)).is_err());
        nothing_left(&p);
    }

    #[test]
    fn http_error_leaves_nothing() {
        let p = dest();
        let url = serve_once("404 Not Found", b"no".to_vec(), "asw.graph.zst");
        let err = format!("{:#}", ensure_graph(&p, Some(&url)).unwrap_err());
        assert!(err.contains("404"), "{err}");
        nothing_left(&p);
    }

    #[test]
    fn zst_detected_from_url_path() {
        assert!(is_zst("http://h/a/asw.graph.ZST?x=1"));
        assert!(!is_zst("http://h/a.zst.graph"));
    }
}
