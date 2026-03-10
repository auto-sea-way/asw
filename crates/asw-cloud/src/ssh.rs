use anyhow::{bail, Context, Result};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// SSH connection configuration.
#[derive(Clone, Debug)]
pub struct SshConfig {
    pub host: String,
    pub user: String,
    pub key_path: PathBuf,
}

impl SshConfig {
    pub fn new(host: String, key_path: PathBuf) -> Self {
        Self {
            host,
            user: "root".to_string(),
            key_path,
        }
    }
}

/// Common SSH options to avoid interactive prompts.
const SSH_OPTS: &[&str] = &[
    "-o", "StrictHostKeyChecking=no",
    "-o", "UserKnownHostsFile=/dev/null",
    "-o", "ServerAliveInterval=30",
    "-o", "LogLevel=ERROR",
];

fn ssh_base_args(cfg: &SshConfig) -> Vec<String> {
    let mut args: Vec<String> = SSH_OPTS.iter().map(|s| s.to_string()).collect();
    args.push("-i".to_string());
    args.push(cfg.key_path.to_string_lossy().to_string());
    args
}

fn ssh_target(cfg: &SshConfig) -> String {
    format!("{}@{}", cfg.user, cfg.host)
}

/// Run an SSH command and capture its output.
pub fn run_ssh(cfg: &SshConfig, cmd: &str) -> Result<String> {
    debug!("ssh {}@{}: {}", cfg.user, cfg.host, cmd);
    let mut args = ssh_base_args(cfg);
    args.push(ssh_target(cfg));
    args.push(cmd.to_string());

    let output = Command::new("ssh")
        .args(&args)
        .output()
        .context("Failed to execute ssh")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "SSH command failed (exit {}): {}\nstderr: {}",
            output.status.code().unwrap_or(-1),
            cmd,
            stderr
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run an SSH command with stdio inherited (streaming output).
pub fn run_ssh_stream(cfg: &SshConfig, cmd: &str) -> Result<()> {
    debug!("ssh (stream) {}@{}: {}", cfg.user, cfg.host, cmd);
    let mut args = ssh_base_args(cfg);
    args.push(ssh_target(cfg));
    args.push(cmd.to_string());

    let status = Command::new("ssh")
        .args(&args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to execute ssh")?;

    if !status.success() {
        bail!(
            "SSH command failed (exit {}): {}",
            status.code().unwrap_or(-1),
            cmd
        );
    }
    Ok(())
}

/// Upload a local file to the remote server via scp.
pub fn scp_upload(cfg: &SshConfig, local: &Path, remote: &str) -> Result<()> {
    info!("scp upload: {:?} → {}:{}", local, cfg.host, remote);
    let mut args: Vec<String> = SSH_OPTS.iter().map(|s| s.to_string()).collect();
    args.push("-i".to_string());
    args.push(cfg.key_path.to_string_lossy().to_string());
    args.push(local.to_string_lossy().to_string());
    args.push(format!("{}@{}:{}", cfg.user, cfg.host, remote));

    let status = Command::new("scp")
        .args(&args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to execute scp")?;

    if !status.success() {
        bail!("scp upload failed (exit {})", status.code().unwrap_or(-1));
    }
    Ok(())
}

/// Download a remote file to the local machine via scp.
pub fn scp_download(cfg: &SshConfig, remote: &str, local: &Path) -> Result<()> {
    info!("scp download: {}:{} → {:?}", cfg.host, remote, local);
    if let Some(parent) = local.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut args: Vec<String> = SSH_OPTS.iter().map(|s| s.to_string()).collect();
    args.push("-i".to_string());
    args.push(cfg.key_path.to_string_lossy().to_string());
    args.push(format!("{}@{}:{}", cfg.user, cfg.host, remote));
    args.push(local.to_string_lossy().to_string());

    let status = Command::new("scp")
        .args(&args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to execute scp")?;

    if !status.success() {
        bail!("scp download failed (exit {})", status.code().unwrap_or(-1));
    }
    Ok(())
}

/// Wait for SSH port 22 to become reachable via TCP connect.
pub fn wait_for_ssh(host: &str, timeout: Duration) -> Result<()> {
    info!("Waiting for SSH on {}...", host);
    let start = Instant::now();
    loop {
        if start.elapsed() > timeout {
            bail!("SSH not reachable on {} within {:?}", host, timeout);
        }
        let addr = format!("{}:22", host)
            .parse()
            .context(format!("Invalid host address: {}", host))?;
        match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
            Ok(_) => {
                info!("SSH is reachable on {}", host);
                return Ok(());
            }
            Err(_) => std::thread::sleep(Duration::from_secs(3)),
        }
    }
}

/// Auto-detect SSH key from ~/.ssh/.
pub fn find_ssh_key() -> Result<PathBuf> {
    let ssh_dir = dirs_next().context("Cannot determine home directory")?;
    for name in &["id_ed25519", "id_rsa", "id_ecdsa"] {
        let path = ssh_dir.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    bail!(
        "No SSH key found in ~/.ssh/ (looked for id_ed25519, id_rsa, id_ecdsa). \
         Pass --ssh-key explicitly."
    )
}

fn dirs_next() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".ssh"))
}
