use anyhow::{Context, Result};
use std::path::PathBuf;
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

const STEPS: &[Step] = &[
    Step { number: 0, name: "provision",    description: "Create Hetzner server" },
    Step { number: 1, name: "upload_src",   description: "Upload Rust source to server" },
    Step { number: 2, name: "compile",      description: "Install Rust + compile on server" },
    Step { number: 3, name: "download_shp", description: "Download land polygons on server" },
    Step { number: 4, name: "build_graph",  description: "Run asw build on server" },
    Step { number: 5, name: "download",     description: "Download graph to local machine" },
    Step { number: 6, name: "teardown",     description: "Delete Hetzner server" },
];

impl Pipeline {
    pub fn run(&mut self) -> Result<()> {
        let total = STEPS.len();

        for step in STEPS {
            if step.name == "teardown" && self.keep_server {
                eprintln!("  [{}/{}] {}: skipped (--keep-server)", step.number, total, step.name);
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
            "download_shp" => self.remote_dir_exists(&format!(
                "{}/land-polygons-split-4326",
                REMOTE_DATA_DIR
            )),
            "build_graph" => self.remote_file_exists(&format!("{}/asw.graph", REMOTE_DATA_DIR)),
            "download" => {
                return self.output_path.exists()
                    && self.output_path.metadata().map(|m| m.len() > 1024).unwrap_or(false);
            }
            _ => return false,
        };
        result.unwrap_or(false)
    }

    fn remote_binary_works(&self) -> Result<bool> {
        let cfg = self.ssh_cfg();
        let output = ssh::run_ssh(
            &cfg,
            &format!("{} --version 2>/dev/null && echo yes || echo no", REMOTE_BIN),
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
        ssh::run_ssh(&cfg, "command -v unzip >/dev/null 2>&1 || apt-get install -y -qq unzip")?;

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
        let graph_path = format!("{}/asw.graph", REMOTE_DATA_DIR);

        let mut cmd = format!("{} build --shp {} --output {}", REMOTE_BIN, shp_path, graph_path);

        if let Some((min_lon, min_lat, max_lon, max_lat)) = self.bbox {
            cmd.push_str(&format!(
                " --bbox {},{},{},{}",
                min_lon, min_lat, max_lon, max_lat
            ));
        }

        info!("$ {}", cmd);
        ssh::run_ssh_stream(&cfg, &cmd)?;

        Ok(())
    }

    fn step_download(&self) -> Result<()> {
        let cfg = self.ssh_cfg();
        let remote_graph = format!("{}/asw.graph", REMOTE_DATA_DIR);

        ssh::scp_download(&cfg, &remote_graph, &self.output_path)?;

        Ok(())
    }

    fn step_teardown(&self) -> Result<()> {
        if let Some(token) = &self.hetzner_token {
            hetzner::teardown(token)?;
        }
        Ok(())
    }
}
