use anyhow::{Context, Result};
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

struct Step {
    number: usize,
    name: &'static str,
    description: &'static str,
}

/// Derive a filesystem-safe slug identifying a bbox, used both for the
/// remote graph filename and the local download-cache sidecar marker. This
/// is what makes the step cache input-aware: a run with a different bbox
/// gets a different remote filename and a different sidecar value, so it
/// can never be mistaken for "already built"/"already downloaded".
pub fn bbox_slug(bbox: Option<(f64, f64, f64, f64)>) -> String {
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

const STEPS: &[Step] = &[
    Step {
        number: 0,
        name: "provision",
        description: "Create Hetzner server",
    },
    Step {
        number: 1,
        name: "upload_src",
        description: "Upload Rust source to server",
    },
    Step {
        number: 2,
        name: "compile",
        description: "Install Rust + compile on server",
    },
    Step {
        number: 3,
        name: "download_shp",
        description: "Download land polygons on server",
    },
    Step {
        number: 4,
        name: "build_graph",
        description: "Run asw build on server",
    },
    Step {
        number: 5,
        name: "download",
        description: "Download graph to local machine",
    },
    Step {
        number: 6,
        name: "teardown",
        description: "Delete Hetzner server",
    },
];

impl Pipeline {
    pub fn run(&mut self) -> Result<()> {
        let total = STEPS.len();

        for step in STEPS {
            if step.name == "teardown" && self.keep_server {
                eprintln!(
                    "  [{}/{}] {}: skipped (--keep-server)",
                    step.number, total, step.name
                );
                continue;
            }

            let cached = self.check_cache(step);

            if cached {
                eprintln!("  [{}/{}] {}: cached", step.number, total, step.name);
                continue;
            }

            eprintln!(
                "  [{}/{}] {}: running — {}",
                step.number, total, step.name, step.description
            );
            self.execute_step(step)?;
            eprintln!("  [{}/{}] {}: done", step.number, total, step.name);
        }

        eprintln!("Build complete. Output: {:?}", self.output_path);
        Ok(())
    }

    fn ssh_cfg(&self) -> SshConfig {
        SshConfig::new(
            self.host.clone().unwrap_or_default(),
            self.ssh_key_path.clone(),
        )
    }

    fn check_cache(&self, step: &Step) -> bool {
        let result = match step.name {
            "provision" => return self.host.is_some(),
            "upload_src" => self.remote_file_exists(&format!("{}/Cargo.toml", REMOTE_SRC_DIR)),
            "compile" => self.remote_binary_works(),
            "download_shp" => {
                self.remote_dir_exists(&format!("{}/land-polygons-split-4326", REMOTE_DATA_DIR))
            }
            "build_graph" => self.remote_file_exists(&self.remote_graph_path()),
            "download" => {
                return self.output_path.exists()
                    && self
                        .output_path
                        .metadata()
                        .map(|m| m.len() > 1024)
                        .unwrap_or(false)
                    && self.local_download_cache_matches();
            }
            _ => return false,
        };
        result.unwrap_or(false)
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

    /// Probe: does the remote binary run at all? Relies on the CLI defining
    /// a `--version` flag (see asw-cli's `#[command(..., version, ...)]`) —
    /// without it clap rejects the unknown argument and this always reports
    /// "no", making the compile-step cache permanently dead.
    fn remote_binary_works(&self) -> Result<bool> {
        let cfg = self.ssh_cfg();
        let output = ssh::run_ssh(
            &cfg,
            &format!(
                "{} --version 2>/dev/null && echo yes || echo no",
                REMOTE_BIN
            ),
        )?;
        Ok(output.trim().ends_with("yes"))
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

    fn execute_step(&mut self, step: &Step) -> Result<()> {
        match step.name {
            "provision" => self.step_provision(),
            "upload_src" => self.step_upload_src(),
            "compile" => self.step_compile(),
            "download_shp" => self.step_download_shp(),
            "build_graph" => self.step_build_graph(),
            "download" => self.step_download(),
            "teardown" => self.step_teardown(),
            _ => unreachable!(),
        }
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
    fn bbox_slug_is_deterministic() {
        let a = bbox_slug(Some((27.5, 36.0, 30.0, 37.0)));
        let b = bbox_slug(Some((27.5, 36.0, 30.0, 37.0)));
        assert_eq!(a, b);
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
}
