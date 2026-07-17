use anyhow::{Context, Result};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use tracing::info;

use crate::config::*;
use crate::hetzner;
use crate::ssh::{self, SshConfig};

/// Cloud build pipeline orchestrator.
pub struct Pipeline {
    pub host: Option<String>,
    pub ssh_key_path: PathBuf,
    pub output_path: PathBuf,
    pub keep_server: bool,
    pub hetzner_token: Option<String>,
    pub bbox: Option<(f64, f64, f64, f64)>,
    pub rust_src_dir: PathBuf,
}

/// Derive a filesystem-safe slug identifying a bbox, used both for the
/// remote graph filename and the local download-cache sidecar marker. This
/// is what makes the step cache input-aware: a run with a different bbox
/// gets a different remote filename and a different sidecar value, so it
/// can never be mistaken for "already built"/"already downloaded".
fn bbox_slug(bbox: Option<(f64, f64, f64, f64)>) -> String {
    match bbox {
        None => "full".to_string(),
        Some((min_lon, min_lat, max_lon, max_lat)) => {
            format!("{}_{}_{}_{}", min_lon, min_lat, max_lon, max_lat)
        }
    }
}

/// Path of the sidecar marker file recording which bbox produced the local
/// output file, e.g. `export/asw.graph` -> `export/asw.graph.bbox`.
fn bbox_sidecar_path(output_path: &Path) -> PathBuf {
    let mut name = output_path.file_name().unwrap_or_default().to_os_string();
    name.push(".bbox");
    output_path.with_file_name(name)
}

/// Remote path of the source-hash marker written after a successful compile.
/// `check_cache` only treats `upload_src`/`compile` as cached when this file
/// exists on the server and matches the local source hash — see
/// `remote_src_hash_matches`.
fn remote_hash_path() -> String {
    format!("{}/src.hash", REMOTE_DATA_DIR)
}

/// Hash over the workspace's Rust source: every `.rs` file under `crates/`,
/// plus the workspace `Cargo.toml` and `Cargo.lock`, hashed in a fixed
/// (sorted-path) order so the result doesn't depend on filesystem iteration
/// order and is stable across runs on unchanged content.
///
/// Used to detect a stale compiled binary on a kept build server: after the
/// v2->v3 graph format bump, a server whose binary merely still answers
/// `--version` is not proof it was compiled from the current source — only a
/// byte-for-byte source hash match is.
///
/// A cache key, not a security property. ponytail: DefaultHasher is not
/// guaranteed stable across Rust releases — worst case a toolchain bump
/// causes one spurious remote rebuild; switch back to a fixed algorithm if
/// that ever matters.
fn source_hash(workspace_root: &Path) -> Result<String> {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_rs_files(&workspace_root.join("crates"), &mut paths)?;
    paths.push(workspace_root.join("Cargo.toml"));
    paths.push(workspace_root.join("Cargo.lock"));
    paths.sort();

    let mut hasher = DefaultHasher::new();
    for path in &paths {
        if let Ok(contents) = std::fs::read(path) {
            path.to_string_lossy().as_bytes().hash(&mut hasher);
            contents.hash(&mut hasher);
        }
    }
    Ok(format!("{:016x}", hasher.finish()))
}

/// Recursively collect every `.rs` file under `dir` into `out`.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("Failed to read {:?}", dir))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    Ok(())
}

impl Pipeline {
    pub fn run(&mut self) -> Result<()> {
        // Steps run in order; each cache probe is evaluated only when its
        // step is reached (later probes need SSH to the provisioned host).
        self.run_step(
            0,
            "provision",
            "Create Hetzner server",
            self.host.is_some(),
            |p| p.step_provision(),
        )?;

        // One probe gates both upload_src and compile: a stale binary that
        // still answers --version is not proof it matches the current
        // source, so the two are only "cached" together, when the remote
        // hash marker matches the local source hash.
        let src_cached = self.remote_src_hash_matches().unwrap_or(false);
        self.run_step(
            1,
            "upload_src",
            "Upload Rust source to server",
            src_cached,
            |p| p.step_upload_src(),
        )?;
        self.run_step(
            2,
            "compile",
            "Install Rust + compile on server",
            src_cached,
            |p| p.step_compile(),
        )?;

        let shp_cached = self
            .remote_dir_exists(&format!("{}/land-polygons-split-4326", REMOTE_DATA_DIR))
            .unwrap_or(false);
        self.run_step(
            3,
            "download_shp",
            "Download land polygons on server",
            shp_cached,
            |p| p.step_download_shp(),
        )?;

        let graph_cached = self
            .remote_file_exists(&self.remote_graph_path())
            .unwrap_or(false);
        self.run_step(
            4,
            "build_graph",
            "Run asw build on server",
            graph_cached,
            |p| p.step_build_graph(),
        )?;

        let download_cached = self.output_path.exists()
            && self
                .output_path
                .metadata()
                .map(|m| m.len() > 1024)
                .unwrap_or(false)
            && self.local_download_cache_matches();
        self.run_step(
            5,
            "download",
            "Download graph to local machine",
            download_cached,
            |p| p.step_download(),
        )?;

        if self.keep_server {
            eprintln!("  [6/7] teardown: skipped (--keep-server)");
        } else {
            self.run_step(6, "teardown", "Delete Hetzner server", false, |p| {
                p.step_teardown()
            })?;
        }

        eprintln!("Build complete. Output: {:?}", self.output_path);
        Ok(())
    }

    fn run_step(
        &mut self,
        number: usize,
        name: &str,
        description: &str,
        cached: bool,
        action: fn(&mut Self) -> Result<()>,
    ) -> Result<()> {
        if cached {
            eprintln!("  [{}/7] {}: cached", number, name);
            return Ok(());
        }
        eprintln!("  [{}/7] {}: running — {}", number, name, description);
        action(self)?;
        eprintln!("  [{}/7] {}: done", number, name);
        Ok(())
    }

    fn ssh_cfg(&self) -> SshConfig {
        SshConfig::new(
            self.host.clone().unwrap_or_default(),
            self.ssh_key_path.clone(),
        )
    }

    /// Remote path of the graph file for this run's bbox. Including the
    /// bbox slug in the filename means a build for a different bbox can
    /// never be mistaken (by `check_cache`) for a previous build's output.
    fn remote_graph_path(&self) -> String {
        format!("{}/asw-{}.graph", REMOTE_DATA_DIR, bbox_slug(self.bbox))
    }

    /// True if the local output file was downloaded for the *same* bbox as
    /// this run — read from a sidecar marker written by `step_download`.
    /// A missing or mismatched marker (including files predating this check)
    /// means "not cached", so a stale local file is never silently reused.
    fn local_download_cache_matches(&self) -> bool {
        let sidecar = bbox_sidecar_path(&self.output_path);
        std::fs::read_to_string(sidecar)
            .map(|s| s.trim() == bbox_slug(self.bbox))
            .unwrap_or(false)
    }

    /// True only if the remote hash marker (written by `step_compile` after
    /// a successful build) exists and matches the local source hash. Unlike
    /// the old "does the remote binary run at all" probe, this can't be
    /// fooled by a stale binary from before a source/format change — it
    /// still answers `--version`, but that's not proof it was compiled from
    /// the current source.
    fn remote_src_hash_matches(&self) -> Result<bool> {
        let local_hash = source_hash(&self.rust_src_dir)?;
        let cfg = self.ssh_cfg();
        let remote_hash = ssh::run_ssh(
            &cfg,
            &format!("cat {} 2>/dev/null || true", remote_hash_path()),
        )?;
        Ok(remote_hash.trim() == local_hash)
    }

    fn remote_file_exists(&self, path: &str) -> Result<bool> {
        let cfg = self.ssh_cfg();
        let output = ssh::run_ssh(&cfg, &format!("test -f {} && echo yes || echo no", path))?;
        Ok(output.trim() == "yes")
    }

    fn remote_dir_exists(&self, path: &str) -> Result<bool> {
        let cfg = self.ssh_cfg();
        let output = ssh::run_ssh(&cfg, &format!("test -d {} && echo yes || echo no", path))?;
        Ok(output.trim() == "yes")
    }

    // ── Step implementations ────────────────────────────────────────────────

    fn step_provision(&mut self) -> Result<()> {
        let token = self
            .hetzner_token
            .as_ref()
            .context("No Hetzner token provided and no --host specified")?;

        let ip = hetzner::provision(token, &self.ssh_key_path)?;
        self.host = Some(ip);
        Ok(())
    }

    fn step_upload_src(&self) -> Result<()> {
        let cfg = self.ssh_cfg();
        let rust_dir = &self.rust_src_dir;

        if !rust_dir.exists() {
            anyhow::bail!("Rust project not found at {:?}", rust_dir);
        }

        ssh::run_ssh(&cfg, &format!("mkdir -p {}", REMOTE_SRC_DIR))?;

        // Create tarball in temp dir to avoid polluting source tree
        info!("Uploading Rust source...");
        let tar_path = std::env::temp_dir().join("asw-src.tar.gz");

        // Use git archive to respect .gitignore (excludes .env, .idea/, .DS_Store, etc.)
        let archive_result = std::process::Command::new("git")
            .args([
                "-C",
                &rust_dir.to_string_lossy(),
                "archive",
                "--format=tar.gz",
                "-o",
                &tar_path.to_string_lossy(),
                "HEAD",
            ])
            .output()
            .context("Failed to run git archive")?;

        if !archive_result.status.success() {
            let _ = std::fs::remove_file(&tar_path);
            anyhow::bail!(
                "git archive failed: {}",
                String::from_utf8_lossy(&archive_result.stderr)
            );
        }

        let upload_result = (|| -> Result<()> {
            ssh::scp_upload(&cfg, &tar_path, "/tmp/asw-src.tar.gz")?;
            ssh::run_ssh(
                &cfg,
                &format!(
                    "tar xzf /tmp/asw-src.tar.gz -C {} && rm /tmp/asw-src.tar.gz",
                    REMOTE_SRC_DIR
                ),
            )?;
            Ok(())
        })();

        let _ = std::fs::remove_file(&tar_path);
        upload_result
    }

    fn step_compile(&self) -> Result<()> {
        let cfg = self.ssh_cfg();

        info!("Installing Rust toolchain...");
        ssh::run_ssh_stream(
            &cfg,
            r#"command -v cargo >/dev/null 2>&1 || (curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y)"#,
        )?;

        info!("Compiling asw (release)...");
        ssh::run_ssh_stream(
            &cfg,
            &format!(
                r#"export PATH="$HOME/.cargo/bin:$PATH" && cd {} && cargo build --release -p asw-cli 2>&1"#,
                REMOTE_SRC_DIR
            ),
        )?;

        ssh::run_ssh(
            &cfg,
            &format!(
                "ln -sf {}/target/release/asw {}",
                REMOTE_SRC_DIR, REMOTE_BIN
            ),
        )?;

        // Record the source hash that produced this binary so a later run on
        // a kept server can tell a stale compile from an up-to-date one
        // (see `remote_src_hash_matches`).
        let hash = source_hash(&self.rust_src_dir)?;
        ssh::run_ssh(
            &cfg,
            &format!(
                "mkdir -p {} && echo '{}' > {}",
                REMOTE_DATA_DIR,
                hash,
                remote_hash_path()
            ),
        )?;

        Ok(())
    }

    fn step_download_shp(&self) -> Result<()> {
        let cfg = self.ssh_cfg();

        ssh::run_ssh(&cfg, &format!("mkdir -p {}", REMOTE_DATA_DIR))?;

        // Ensure unzip is available (defensive — bootstrap should have installed it)
        ssh::run_ssh(
            &cfg,
            "command -v unzip >/dev/null 2>&1 || apt-get install -y -qq unzip",
        )?;

        info!("Downloading land polygons (~900 MB)...");
        ssh::run_ssh_stream(
            &cfg,
            &format!(
                "cd {} && wget -q --show-progress -O land-polygons-split-4326.zip '{}' && \
                 unzip -o land-polygons-split-4326.zip && \
                 rm -f land-polygons-split-4326.zip",
                REMOTE_DATA_DIR, LAND_POLYGONS_URL
            ),
        )?;

        Ok(())
    }

    fn step_build_graph(&self) -> Result<()> {
        let cfg = self.ssh_cfg();
        let shp_path = format!("{}/land-polygons-split-4326", REMOTE_DATA_DIR);
        let graph_path = self.remote_graph_path();

        let mut cmd = format!(
            "{} build --shp {} --output {}",
            REMOTE_BIN, shp_path, graph_path
        );

        if let Some((min_lon, min_lat, max_lon, max_lat)) = self.bbox {
            cmd.push_str(&format!(
                " --bbox '{},{},{},{}'",
                min_lon, min_lat, max_lon, max_lat
            ));
        }

        info!("$ {}", cmd);
        ssh::run_ssh_stream(&cfg, &cmd)?;

        Ok(())
    }

    fn step_download(&self) -> Result<()> {
        let cfg = self.ssh_cfg();
        let remote_graph = self.remote_graph_path();

        ssh::scp_download(&cfg, &remote_graph, &self.output_path)?;

        // Record which bbox produced this local file, so a later run with a
        // different bbox can't mistake it for "already downloaded".
        let sidecar = bbox_sidecar_path(&self.output_path);
        std::fs::write(&sidecar, bbox_slug(self.bbox))
            .with_context(|| format!("Failed to write bbox cache marker {:?}", sidecar))?;

        Ok(())
    }

    fn step_teardown(&self) -> Result<()> {
        if let Some(token) = &self.hetzner_token {
            hetzner::teardown(token)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "asw-pipeline-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_pipeline(output_path: PathBuf, bbox: Option<(f64, f64, f64, f64)>) -> Pipeline {
        Pipeline {
            host: None,
            ssh_key_path: PathBuf::new(),
            output_path,
            keep_server: false,
            hetzner_token: None,
            bbox,
            rust_src_dir: PathBuf::new(),
        }
    }

    // ── Finding 7: bbox-aware cache key / filename derivation ──────────────

    #[test]
    fn bbox_slug_none_is_full() {
        assert_eq!(bbox_slug(None), "full");
    }

    #[test]
    fn bbox_slug_differs_by_bbox() {
        let marmaris = bbox_slug(Some((27.5, 36.0, 30.0, 37.0)));
        let dev = bbox_slug(Some((-5.0, 48.0, 10.0, 62.0)));
        assert_ne!(marmaris, dev);
        assert_ne!(marmaris, "full");
    }

    #[test]
    fn remote_graph_path_includes_bbox_slug() {
        let marmaris = test_pipeline(PathBuf::new(), Some((27.5, 36.0, 30.0, 37.0)));
        let dev = test_pipeline(PathBuf::new(), Some((-5.0, 48.0, 10.0, 62.0)));
        let full = test_pipeline(PathBuf::new(), None);

        assert_ne!(marmaris.remote_graph_path(), dev.remote_graph_path());
        assert_ne!(marmaris.remote_graph_path(), full.remote_graph_path());
        assert!(marmaris
            .remote_graph_path()
            .contains(&bbox_slug(marmaris.bbox)));
    }

    #[test]
    fn bbox_sidecar_path_is_alongside_output() {
        let out = PathBuf::from("/tmp/export/asw.graph");
        let sidecar = bbox_sidecar_path(&out);
        assert_eq!(sidecar, PathBuf::from("/tmp/export/asw.graph.bbox"));
    }

    // Scenario A from finding 7: build for "marmaris", then build for "dev"
    // with the same default output path. The stale local file (and its
    // stale sidecar) must NOT be reported as cached for the new bbox.
    #[test]
    fn download_cache_does_not_hit_for_a_different_bbox() {
        let dir = unique_tmp_dir("cache-diff-bbox");
        let output_path = dir.join("asw.graph");
        std::fs::write(&output_path, vec![0u8; 2048]).unwrap();

        let marmaris_bbox = Some((27.5, 36.0, 30.0, 37.0));
        let dev_bbox = Some((-5.0, 48.0, 10.0, 62.0));

        // Simulate step_download having completed for "marmaris".
        let marmaris_pipeline = test_pipeline(output_path.clone(), marmaris_bbox);
        std::fs::write(
            bbox_sidecar_path(&output_path),
            bbox_slug(marmaris_pipeline.bbox),
        )
        .unwrap();
        assert!(marmaris_pipeline.local_download_cache_matches());

        // Now the user asks for "dev" with the same (default) output path.
        let dev_pipeline = test_pipeline(output_path.clone(), dev_bbox);
        assert!(
            !dev_pipeline.local_download_cache_matches(),
            "a stale sidecar from a different bbox must not be treated as cached"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_cache_misses_when_sidecar_is_missing() {
        let dir = unique_tmp_dir("cache-no-sidecar");
        let output_path = dir.join("asw.graph");
        std::fs::write(&output_path, vec![0u8; 2048]).unwrap();
        // No sidecar written — e.g. a file left over from before this fix,
        // or a file from an entirely unrelated source.

        let pipeline = test_pipeline(output_path.clone(), Some((27.5, 36.0, 30.0, 37.0)));
        assert!(!pipeline.local_download_cache_matches());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_cache_hits_for_matching_bbox() {
        let dir = unique_tmp_dir("cache-match");
        let output_path = dir.join("asw.graph");
        std::fs::write(&output_path, vec![0u8; 2048]).unwrap();

        let bbox = Some((27.5, 36.0, 30.0, 37.0));
        let pipeline = test_pipeline(output_path.clone(), bbox);
        std::fs::write(bbox_sidecar_path(&output_path), bbox_slug(bbox)).unwrap();

        assert!(pipeline.local_download_cache_matches());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Finding 4: source-hash gates the upload_src/compile cache ──────────

    #[test]
    fn source_hash_is_stable_across_repeated_calls() {
        let dir = unique_tmp_dir("hash-stable");
        std::fs::create_dir_all(dir.join("crates/asw-core/src")).unwrap();
        std::fs::write(dir.join("crates/asw-core/src/lib.rs"), b"fn a() {}").unwrap();
        std::fs::write(dir.join("Cargo.toml"), b"[workspace]").unwrap();
        std::fs::write(dir.join("Cargo.lock"), b"# lock").unwrap();

        let h1 = source_hash(&dir).unwrap();
        let h2 = source_hash(&dir).unwrap();
        assert_eq!(h1, h2, "hash must be stable across repeated calls");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_hash_changes_when_source_content_changes() {
        let dir = unique_tmp_dir("hash-changes");
        std::fs::create_dir_all(dir.join("crates/asw-core/src")).unwrap();
        std::fs::write(dir.join("crates/asw-core/src/lib.rs"), b"fn a() {}").unwrap();
        std::fs::write(dir.join("Cargo.toml"), b"[workspace]").unwrap();
        std::fs::write(dir.join("Cargo.lock"), b"# lock").unwrap();

        let before = source_hash(&dir).unwrap();

        std::fs::write(dir.join("crates/asw-core/src/lib.rs"), b"fn a() { 1 }").unwrap();
        let after = source_hash(&dir).unwrap();

        assert_ne!(
            before, after,
            "hash must change when a source file's content changes"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_hash_ignores_non_rs_files_outside_cargo_toml_lock() {
        let dir = unique_tmp_dir("hash-ignores");
        std::fs::create_dir_all(dir.join("crates/asw-core/src")).unwrap();
        std::fs::write(dir.join("crates/asw-core/src/lib.rs"), b"fn a() {}").unwrap();
        std::fs::write(dir.join("Cargo.toml"), b"[workspace]").unwrap();
        std::fs::write(dir.join("Cargo.lock"), b"# lock").unwrap();

        let before = source_hash(&dir).unwrap();

        // An unrelated file (e.g. a README or asset) must not affect the hash.
        std::fs::write(dir.join("crates/asw-core/README.md"), b"docs change").unwrap();
        let after = source_hash(&dir).unwrap();

        assert_eq!(
            before, after,
            "non-.rs files (other than Cargo.toml/Cargo.lock) must not affect the hash"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
